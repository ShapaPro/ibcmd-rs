//! The TDS connection: tiberius over tokio's TCP stream, driven by a
//! current-thread tokio runtime of its own, so that the synchronous code of
//! ibcmd-rs (and its rayon workers) calls it like a blocking client.

use std::borrow::Cow;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::TryStreamExt;
use tiberius::{AuthMethod, Client, Config, EncryptionLevel, Query, QueryItem, Row, SqlBrowser};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::address::ServerAddress;
use super::script::{ScriptBatch, ScriptVariables, split_batches};
use super::value::SqlRow;
use super::{SqlLogin, SqlTarget};

type TdsClient = Client<Compat<TcpStream>>;

/// Attempts at opening a connection before a transient failure (a refused or
/// timed-out TCP connect, a stalled login handshake on a busy machine) is
/// reported. sqlcmd runs were retried the same way.
const CONNECT_ATTEMPTS: u32 = 6;
/// A TCP connect that neither succeeds nor fails is abandoned after this.
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The largest TDS packet a client may ask for; the server may grant less.
/// Large packets carry the staged rows (tens of MB) in fewer round trips.
const PACKET_SIZE: u32 = 32767;

/// One parameter of a parameterized statement (`@P1`, `@P2`, ...).
#[derive(Clone, Copy, Debug)]
pub enum SqlParam<'a> {
    Text(&'a str),
    U8(u8),
    I32(i32),
    I64(i64),
    Binary(&'a [u8]),
}

impl SqlParam<'_> {
    /// Bytes the parameter adds to a request, for sizing batches.
    pub fn wire_bytes(&self) -> usize {
        match self {
            Self::Text(value) => value.len() * 2,
            Self::U8(_) => 1,
            Self::I32(_) => 4,
            Self::I64(_) => 8,
            Self::Binary(value) => value.len(),
        }
    }
}

fn bind<'q>(query: &mut Query<'q>, param: SqlParam<'q>) {
    match param {
        SqlParam::Text(value) => query.bind(value),
        SqlParam::U8(value) => query.bind(value),
        SqlParam::I32(value) => query.bind(value),
        SqlParam::I64(value) => query.bind(value),
        SqlParam::Binary(value) => query.bind(value),
    }
}

/// One open connection to SQL Server.
pub struct TdsConnection {
    runtime: tokio::runtime::Runtime,
    client: TdsClient,
}

impl TdsConnection {
    /// Opens a connection, retrying transient connect failures.
    pub fn open(target: &SqlTarget, address: &ServerAddress) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .context("failed to start the SQL client runtime")?;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let result = runtime.block_on(connect(target, address));
            match result {
                Ok(client) => {
                    let mut connection = Self { runtime, client };
                    connection
                        .initialize_session()
                        .with_context(|| format!("SQL Server {}", target.server))?;
                    return Ok(connection);
                }
                Err(error) if attempt < CONNECT_ATTEMPTS && connect_error_is_transient(&error) => {
                    std::thread::sleep(Duration::from_secs(u64::from(attempt) * 2));
                }
                Err(error) => {
                    return Err(anyhow!(error)).with_context(|| {
                        format!(
                            "failed to connect to SQL Server {} as {}",
                            target.server,
                            target.login.describe()
                        )
                    });
                }
            }
        }
    }

    /// The session options a `sqlcmd` session starts with, so that scripts
    /// written for sqlcmd behave the same: it switches QUOTED_IDENTIFIER off
    /// (the only option in which the two sessions differ; checked with
    /// `@@OPTIONS`: 5688 under sqlcmd, 5944 under a bare TDS login).
    fn initialize_session(&mut self) -> Result<()> {
        self.run_batch("SET QUOTED_IDENTIFIER OFF;")
    }

    /// Runs one batch as sqlcmd sends it (`simple_query`): statements that
    /// change the session (`USE`, `SET`, temporary tables, an open
    /// transaction) last until the connection closes. Results are drained and
    /// dropped; the first error of the batch is returned once the server has
    /// finished it, as `sqlcmd -b` would report it.
    pub fn run_batch(&mut self, sql: &str) -> Result<()> {
        let Self { runtime, client } = self;
        runtime.block_on(async {
            let mut stream = client.simple_query(sql).await?;
            while stream.try_next().await?.is_some() {}
            Ok::<_, tiberius::error::Error>(())
        })?;
        Ok(())
    }

    /// Runs a query through `sp_executesql` and hands every row to `row`,
    /// in order, as it arrives. Whatever the text changes in the session
    /// (`USE`, `SET`, `#temp` tables) ends with the call, so a pooled
    /// connection comes back as it went out.
    pub fn query_each<'q>(
        &mut self,
        sql: &'q str,
        params: &[SqlParam<'q>],
        mut row: impl FnMut(Row) -> Result<()>,
    ) -> Result<()> {
        let Self { runtime, client } = self;
        runtime.block_on(async {
            let mut query = Query::new(Cow::Borrowed(sql));
            for param in params {
                bind(&mut query, *param);
            }
            let mut stream = query.query(client).await?;
            while let Some(item) = stream.try_next().await? {
                if let QueryItem::Row(value) = item {
                    row(value)?;
                }
            }
            Ok(())
        })
    }

    /// Runs a statement through `sp_executesql` and returns the rows it
    /// affected (all statements of the text summed).
    pub fn execute<'q>(&mut self, sql: &'q str, params: &[SqlParam<'q>]) -> Result<u64> {
        let Self { runtime, client } = self;
        let result = runtime.block_on(async {
            let mut query = Query::new(Cow::Borrowed(sql));
            for param in params {
                bind(&mut query, *param);
            }
            query.execute(client).await
        })?;
        Ok(result.total())
    }
}

async fn connect(target: &SqlTarget, address: &ServerAddress) -> tiberius::Result<TdsClient> {
    let mut config = tiberius_config(target, address)?;
    let mut named = address.browser_instance().is_some();
    let mut routed = false;
    loop {
        let tcp = tcp_connect(&config, named).await?;
        match Client::connect(config.clone(), tcp.compat_write()).await {
            // An Azure gateway or an availability group listener may send the
            // client on to the instance that serves the database.
            Err(tiberius::error::Error::Routing { host, port }) if !routed => {
                routed = true;
                named = false;
                config = tiberius_config(target, address)?;
                config.host(host);
                config.port(port);
            }
            result => return result,
        }
    }
}

/// Opens the TCP stream; `named` asks the SQL Browser for the instance's port
/// first.
async fn tcp_connect(config: &Config, named: bool) -> tiberius::Result<TcpStream> {
    let connect = async {
        if named {
            TcpStream::connect_named(config).await
        } else {
            let stream = TcpStream::connect(config.get_addr()).await?;
            Ok(stream)
        }
    };
    let stream = tokio::time::timeout(TCP_CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| {
            tiberius::error::Error::from(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "no TCP connection to {} within {} s",
                    config.get_addr(),
                    TCP_CONNECT_TIMEOUT.as_secs()
                ),
            ))
        })??;
    stream.set_nodelay(true)?;
    Ok(stream)
}

fn tiberius_config(target: &SqlTarget, address: &ServerAddress) -> tiberius::Result<Config> {
    let mut config = Config::new();
    config.host(&address.host);
    match address.browser_instance() {
        Some(instance) => config.instance_name(instance),
        None => config.port(address.direct_port()),
    }
    if let Some(database) = &target.database {
        config.database(database);
    }
    config.application_name("ibcmd-rs");
    config.authentication(authentication(&target.login)?);
    // ODBC 18's sqlcmd encrypts every connection; so does the built-in client.
    config.encryption(EncryptionLevel::Required);
    if target.trust_server_certificate {
        config.trust_cert();
    }
    // A staged apply or a bulk read may keep one request busy for minutes;
    // sqlcmd waits without limit as well (`-t 0`).
    config.command_timeout(None);
    config.packet_size(PACKET_SIZE);
    Ok(config)
}

fn authentication(login: &SqlLogin) -> tiberius::Result<AuthMethod> {
    match login {
        SqlLogin::Sql {
            user,
            password: Some(password),
        } => Ok(AuthMethod::sql_server(user, password)),
        SqlLogin::Sql {
            user,
            password: None,
        } => Err(tiberius::error::Error::Conversion(
            format!("SQL login {user:?} has no password").into(),
        )),
        #[cfg(windows)]
        SqlLogin::Integrated => Ok(AuthMethod::Integrated),
        #[cfg(not(windows))]
        SqlLogin::Integrated => Err(tiberius::error::Error::Conversion(
            "Windows (integrated) authentication is available on Windows only; pass --sql-user"
                .into(),
        )),
    }
}

/// A failure worth another attempt: the network or the handshake, not a
/// refused login, a missing database or a protocol mismatch.
fn connect_error_is_transient(error: &tiberius::error::Error) -> bool {
    match error {
        tiberius::error::Error::Io { .. } | tiberius::error::Error::Tls(_) => true,
        tiberius::error::Error::Server(token) => !matches!(
            token.code(),
            // Login failed, password expired or must change, database
            // missing or not accessible.
            18456 | 18470 | 18486 | 18487 | 18488 | 4060 | 4064
        ),
        _ => false,
    }
}

/// A small set of open connections shared by the threads of one command.
///
/// A connection goes back to the pool after a call that succeeded; after an
/// error it is closed instead (its stream or its session may be half-way
/// through something). At most `capacity` connections are open at once; a
/// caller beyond that waits for one to come back.
pub struct TdsPool {
    target: SqlTarget,
    address: ServerAddress,
    capacity: usize,
    state: Mutex<PoolState>,
    returned: Condvar,
}

struct PoolState {
    idle: Vec<TdsConnection>,
    open: usize,
}

impl TdsPool {
    /// A pool for `target`; nothing connects until the first call.
    pub fn new(target: SqlTarget, capacity: usize) -> Result<Self> {
        let address = ServerAddress::parse(&target.server)?;
        Ok(Self {
            target,
            address,
            capacity: capacity.max(1),
            state: Mutex::new(PoolState {
                idle: Vec::new(),
                open: 0,
            }),
            returned: Condvar::new(),
        })
    }

    pub fn target(&self) -> &SqlTarget {
        &self.target
    }

    pub fn address(&self) -> &ServerAddress {
        &self.address
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Runs `work` on a pooled connection. `work` must not ask the same pool
    /// for a second connection: with every connection taken it would wait
    /// for itself.
    pub fn with<R>(&self, work: impl FnOnce(&mut TdsConnection) -> Result<R>) -> Result<R> {
        let mut lease = self.checkout()?;
        let connection = lease
            .connection
            .as_mut()
            .expect("a lease holds its connection until it ends");
        let result = work(connection);
        if result.is_ok() {
            lease.keep = true;
        }
        result
    }

    /// A connection of its own for work whose session state must not reach a
    /// pooled connection: a script's batches share `USE`, `SET` and temporary
    /// tables, exactly as they do in one sqlcmd run.
    pub fn dedicated(&self) -> Result<TdsConnection> {
        TdsConnection::open(&self.target, &self.address)
    }

    /// Runs every row the query returns through `row`.
    pub fn query_each<'q>(
        &self,
        sql: &'q str,
        params: &[SqlParam<'q>],
        row: impl FnMut(Row) -> Result<()>,
    ) -> Result<()> {
        self.with(|connection| connection.query_each(sql, params, row))
    }

    /// Every row of every result set the query returns.
    pub fn query_rows(&self, sql: &str) -> Result<Vec<SqlRow>> {
        let mut rows = Vec::new();
        self.query_each(sql, &[], |row| {
            rows.push(SqlRow::from_row(row));
            Ok(())
        })?;
        Ok(rows)
    }

    /// The document a `FOR JSON` query returns: SQL Server splits a long one
    /// into rows of about 2 000 characters, joined here. `None` when the
    /// query returned no row at all (a top-level `FOR JSON` over no rows).
    pub fn query_json(&self, sql: &str) -> Result<Option<String>> {
        let mut document: Option<String> = None;
        let mut result_set = None;
        self.query_each(sql, &[], |row| {
            let index = row.result_index();
            if *result_set.get_or_insert(index) != index {
                return Ok(());
            }
            let part = row
                .try_get::<&str, _>(0)
                .context("a FOR JSON query returned a non-text column")?
                .unwrap_or_default();
            document.get_or_insert_with(String::new).push_str(part);
            Ok(())
        })?;
        Ok(document)
    }

    /// Runs statements that return nothing the caller reads.
    pub fn execute(&self, sql: &str) -> Result<u64> {
        self.with(|connection| connection.execute(sql, &[]))
    }

    /// Runs a script written for `sqlcmd -i` on a connection of its own, one
    /// batch at a time, and stops at the first batch that fails.
    pub fn run_script(&self, script: &str, variables: ScriptVariables) -> Result<()> {
        let batches = split_batches(script, variables)?;
        if batches.is_empty() {
            return Ok(());
        }
        let mut connection = self.dedicated()?;
        let count = batches.len();
        for (index, ScriptBatch { first_line, text }) in batches.into_iter().enumerate() {
            connection.run_batch(&text).with_context(|| {
                if count == 1 {
                    "the script failed".to_owned()
                } else {
                    format!(
                        "batch {} of {count} (from script line {first_line}) failed",
                        index + 1
                    )
                }
            })?;
        }
        Ok(())
    }

    fn checkout(&self) -> Result<Lease<'_>> {
        let mut state = self.lock();
        loop {
            if let Some(connection) = state.idle.pop() {
                return Ok(Lease {
                    pool: self,
                    connection: Some(connection),
                    keep: false,
                });
            }
            if state.open < self.capacity {
                state.open += 1;
                break;
            }
            state = self
                .returned
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(state);
        match TdsConnection::open(&self.target, &self.address) {
            Ok(connection) => Ok(Lease {
                pool: self,
                connection: Some(connection),
                keep: false,
            }),
            Err(error) => {
                self.lock().open -= 1;
                self.returned.notify_one();
                Err(error)
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A pooled connection on loan; it returns (or, after a failure or a panic,
/// closes) itself when dropped.
struct Lease<'a> {
    pool: &'a TdsPool,
    connection: Option<TdsConnection>,
    keep: bool,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let connection = self.connection.take();
        if self.keep
            && let Some(connection) = connection
        {
            self.pool.lock().idle.push(connection);
        } else {
            // Closed outside the lock: dropping a connection closes its socket.
            drop(connection);
            self.pool.lock().open -= 1;
        }
        self.pool.returned.notify_one();
    }
}
