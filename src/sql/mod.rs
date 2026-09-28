//! SQL Server access for every command that reads or writes an infobase.
//!
//! A command talks to SQL Server itself, over TDS -- the protocol the 1C
//! platform's own driver (MSOLEDBSQL, loaded by `sqlsrvr.dll`) speaks: the
//! `tiberius` client runs inside the process, TLS goes through rustls, Windows
//! logins through SSPI. Neither sqlcmd.exe, bcp.exe nor an ODBC/OLE DB driver
//! has to be installed.
//!
//! A command given `--sqlcmd <path>` keeps the behaviour of ibcmd-rs 0.2
//! instead: statements and scripts go through that sqlcmd.exe and bulk reads
//! and writes through bcp.exe (`--bcp-executable`, else the bcp.exe beside
//! sqlcmd). The lab bundle commands (`mssql-storage-*`, `mssql-delta-*`) keep
//! bcp.exe for their native-format files either way.

mod address;
mod script;
mod tds;
mod value;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};

pub use address::{DEFAULT_PORT, ServerAddress};
pub use script::{ScriptBatch, ScriptVariables, split_batches};
pub use tds::{SqlParam, TdsConnection, TdsPool};
pub use value::{SqlRow, SqlValue};

/// The environment variable that sets how many connections a command may
/// hold open at once for parallel reads (default 4).
pub const CONNECTIONS_ENV: &str = "IBCMD_RS_SQL_CONNECTIONS";
const DEFAULT_CONNECTIONS: usize = 4;
const MAX_CONNECTIONS: usize = 64;

/// How a command logs in.
#[derive(Clone)]
pub enum SqlLogin {
    /// The Windows account the process runs as (SSPI; sqlcmd `-E`).
    Integrated,
    /// A SQL Server login (sqlcmd `-U`, the password from `--sql-pwd` or the
    /// environment).
    Sql {
        user: String,
        password: Option<String>,
    },
}

impl SqlLogin {
    pub fn from_user(user: Option<&str>, password: Option<&str>) -> Self {
        match user {
            Some(user) => Self::Sql {
                user: user.to_owned(),
                password: password.map(ToOwned::to_owned),
            },
            None => Self::Integrated,
        }
    }

    pub fn user(&self) -> Option<&str> {
        match self {
            Self::Integrated => None,
            Self::Sql { user, .. } => Some(user),
        }
    }

    pub fn password(&self) -> Option<&str> {
        match self {
            Self::Integrated => None,
            Self::Sql { password, .. } => password.as_deref(),
        }
    }

    /// For messages; never the password.
    pub fn describe(&self) -> String {
        match self {
            Self::Integrated => "the Windows login".to_owned(),
            Self::Sql { user, .. } => format!("SQL login {user:?}"),
        }
    }
}

impl std::fmt::Debug for SqlLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Integrated => formatter.write_str("Integrated"),
            Self::Sql { user, password } => formatter
                .debug_struct("Sql")
                .field("user", user)
                .field("password", &password.as_ref().map(|_| "<redacted>"))
                .finish(),
        }
    }
}

/// The SQL Server a command works against.
#[derive(Clone, Debug)]
pub struct SqlTarget {
    /// The server as given (`host`, `host\instance`, `host,port`).
    pub server: String,
    /// The database a connection starts in; the login's default otherwise.
    /// Queries name their database themselves, so this is rarely set.
    pub database: Option<String>,
    pub login: SqlLogin,
    /// Accept the server's certificate without validating it (sqlcmd `-C`):
    /// what a default SQL Server install with its self-signed certificate
    /// needs.
    pub trust_server_certificate: bool,
}

/// The external tools of the `--sqlcmd` path.
#[derive(Clone, Debug)]
pub struct SqlTools {
    pub sqlcmd: PathBuf,
    pub bcp: PathBuf,
}

impl SqlTools {
    /// `bcp` as given, else the bcp.exe beside `sqlcmd`, else `bcp` on PATH.
    pub fn new(sqlcmd: &Path, bcp: Option<&Path>) -> Self {
        Self {
            sqlcmd: sqlcmd.to_path_buf(),
            bcp: bcp.map_or_else(|| bcp_beside(sqlcmd), Path::to_path_buf),
        }
    }
}

/// The bcp.exe installed with `sqlcmd` (the ODBC client tools ship both).
pub fn bcp_beside(sqlcmd: &Path) -> PathBuf {
    if let Some(parent) = sqlcmd.parent() {
        for name in ["bcp.exe", "bcp"] {
            let candidate = parent.join(name);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("bcp")
}

/// Which way a command reaches SQL Server.
#[derive(Clone, Copy)]
pub enum SqlBackend<'a> {
    /// The built-in TDS client.
    Tds(&'a TdsPool),
    /// sqlcmd.exe and bcp.exe (`--sqlcmd`).
    Tools(&'a SqlTools),
}

/// A command's handle on SQL Server: the target and the way to reach it.
/// Cheap to clone; clones share the connections.
#[derive(Clone)]
pub struct SqlExec {
    inner: Arc<SqlExecInner>,
}

struct SqlExecInner {
    target: SqlTarget,
    backend: Backend,
}

enum Backend {
    Tds(TdsPool),
    Tools(SqlTools),
}

/// A command's SQL arguments, as the CLI spells them.
#[derive(Clone, Copy, Debug)]
pub struct SqlOptions<'a> {
    /// `--sqlcmd`: run the external tools instead of the built-in client.
    pub sqlcmd: Option<&'a Path>,
    /// `--bcp-executable`, used only with `--sqlcmd`.
    pub bcp: Option<&'a Path>,
    pub server: &'a str,
    pub user: Option<&'a str>,
    /// The resolved password (`--sql-pwd` or the environment variable).
    pub password: Option<&'a str>,
    /// Where the password was expected, for the message when it is missing.
    pub password_env: &'a str,
    pub trust_server_certificate: bool,
}

impl<'a> SqlOptions<'a> {
    /// Windows login, certificate trusted, the built-in client unless
    /// `sqlcmd` is given.
    pub fn integrated(server: &'a str, sqlcmd: Option<&'a Path>) -> Self {
        Self {
            sqlcmd,
            bcp: None,
            server,
            user: None,
            password: None,
            password_env: "IBCMD_DB_PSW",
            trust_server_certificate: true,
        }
    }
}

impl SqlExec {
    pub fn from_options(options: SqlOptions<'_>) -> Result<Self> {
        let target = SqlTarget {
            server: options.server.to_owned(),
            database: None,
            login: SqlLogin::from_user(options.user, options.password),
            trust_server_certificate: options.trust_server_certificate,
        };
        match options.sqlcmd {
            Some(sqlcmd) => Ok(Self::with_tools(target, SqlTools::new(sqlcmd, options.bcp))),
            None => {
                if let SqlLogin::Sql {
                    user,
                    password: None,
                } = &target.login
                {
                    bail!(
                        "SQL login {user:?} needs a password: pass --sql-pwd or set {}",
                        options.password_env
                    );
                }
                Self::tds(target)
            }
        }
    }

    /// The built-in client; no connection is opened until the first query.
    pub fn tds(target: SqlTarget) -> Result<Self> {
        let pool = TdsPool::new(target.clone(), connections_from_env())?;
        Ok(Self {
            inner: Arc::new(SqlExecInner {
                target,
                backend: Backend::Tds(pool),
            }),
        })
    }

    pub fn with_tools(target: SqlTarget, tools: SqlTools) -> Self {
        Self {
            inner: Arc::new(SqlExecInner {
                target,
                backend: Backend::Tools(tools),
            }),
        }
    }

    pub fn backend(&self) -> SqlBackend<'_> {
        match &self.inner.backend {
            Backend::Tds(pool) => SqlBackend::Tds(pool),
            Backend::Tools(tools) => SqlBackend::Tools(tools),
        }
    }

    pub fn pool(&self) -> Option<&TdsPool> {
        match &self.inner.backend {
            Backend::Tds(pool) => Some(pool),
            Backend::Tools(_) => None,
        }
    }

    pub fn tools(&self) -> Option<&SqlTools> {
        match &self.inner.backend {
            Backend::Tds(_) => None,
            Backend::Tools(tools) => Some(tools),
        }
    }

    pub fn target(&self) -> &SqlTarget {
        &self.inner.target
    }

    pub fn server(&self) -> &str {
        &self.inner.target.server
    }

    pub fn user(&self) -> Option<&str> {
        self.inner.target.login.user()
    }

    pub fn password(&self) -> Option<&str> {
        self.inner.target.login.password()
    }

    pub fn trust_server_certificate(&self) -> bool {
        self.inner.target.trust_server_certificate
    }
}

impl std::fmt::Debug for SqlExec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let backend = match &self.inner.backend {
            Backend::Tds(pool) => format!("tds ({} connections)", pool.capacity()),
            Backend::Tools(tools) => {
                format!("{} + {}", tools.sqlcmd.display(), tools.bcp.display())
            }
        };
        formatter
            .debug_struct("SqlExec")
            .field("target", &self.inner.target)
            .field("backend", &backend)
            .finish()
    }
}

fn connections_from_env() -> usize {
    std::env::var(CONNECTIONS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map_or(DEFAULT_CONNECTIONS, |value| value.min(MAX_CONNECTIONS))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{SqlBackend, SqlExec, SqlLogin, SqlOptions};

    fn options<'a>(sqlcmd: Option<&'a Path>, user: Option<&'a str>) -> SqlOptions<'a> {
        SqlOptions {
            sqlcmd,
            bcp: None,
            server: "sql01\\ERP,1500",
            user,
            password: None,
            password_env: "TEST_SQL_PASSWORD",
            trust_server_certificate: true,
        }
    }

    #[test]
    fn the_built_in_client_is_the_default_and_opens_nothing_up_front() {
        let sql = SqlExec::from_options(options(None, None)).unwrap();
        assert!(matches!(sql.backend(), SqlBackend::Tds(_)));
        assert!(sql.tools().is_none());
        assert_eq!(sql.server(), "sql01\\ERP,1500");
        assert_eq!(sql.user(), None);
    }

    #[test]
    fn sqlcmd_selects_the_external_tools_with_bcp_beside_it() {
        let sql =
            SqlExec::from_options(options(Some(Path::new("no/such/dir/sqlcmd")), None)).unwrap();
        let tools = sql.tools().expect("--sqlcmd keeps the external tools");
        assert_eq!(tools.sqlcmd, Path::new("no/such/dir/sqlcmd"));
        assert_eq!(tools.bcp, Path::new("bcp"));
        let explicit = SqlOptions {
            bcp: Some(Path::new("C:/tools/bcp.exe")),
            ..options(Some(Path::new("sqlcmd")), None)
        };
        let sql = SqlExec::from_options(explicit).unwrap();
        assert_eq!(sql.tools().unwrap().bcp, Path::new("C:/tools/bcp.exe"));
    }

    #[test]
    fn a_sql_login_without_password_is_refused_before_connecting() {
        let error = SqlExec::from_options(options(None, Some("ibcmd")))
            .unwrap_err()
            .to_string();
        assert!(error.contains("TEST_SQL_PASSWORD"), "{error}");
        // sqlcmd asked for the password itself, so --sqlcmd keeps accepting it.
        assert!(SqlExec::from_options(options(Some(Path::new("sqlcmd")), Some("ibcmd"))).is_ok());
    }

    #[test]
    fn an_unparsable_server_fails_only_the_built_in_client() {
        let bad = SqlOptions {
            server: "np:\\\\.\\pipe\\sql\\query",
            ..options(None, None)
        };
        assert!(SqlExec::from_options(bad).is_err());
        let with_sqlcmd = SqlOptions {
            sqlcmd: Some(Path::new("sqlcmd")),
            ..bad
        };
        assert!(SqlExec::from_options(with_sqlcmd).is_ok());
    }

    #[test]
    fn passwords_never_reach_debug_output() {
        let login = SqlLogin::from_user(Some("ibcmd"), Some("s3cret-value"));
        let rendered = format!("{login:?} {}", login.describe());
        assert!(!rendered.contains("s3cret-value"), "{rendered}");
        let sql = SqlExec::from_options(SqlOptions {
            password: Some("s3cret-value"),
            ..options(None, Some("ibcmd"))
        })
        .unwrap();
        assert!(!format!("{sql:?}").contains("s3cret-value"));
    }
}
