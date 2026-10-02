//! Product creator: fresh private registry, retained original process handles
//! and ephemeral password-only administration. There is no endpoint-adoption
//! constructor and no authority-file reader.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::child::{CensusIdentity, KernelProcess, OriginalChild};
use super::{Journal, LifetimeBinding, ManagedWorker, Observation, OwnedRuntime, ProcessIdentity};
use crate::mssql_platform_profile::ManagedRacReadAuth;

pub(crate) struct CreatorOptions {
    pub parent: PathBuf,
    pub platform_bin: PathBuf,
    pub powershell: PathBuf,
    pub agent_port: u16,
    pub cluster_port: u16,
    pub ras_port: u16,
    pub worker_first: u16,
    pub worker_last: u16,
    pub database_server: String,
    pub database: String,
    pub database_user: String,
    pub database_password: String,
    pub infobase_user: String,
    pub infobase_password: String,
    pub timeout: Duration,
}

#[derive(Clone)]
struct Tool {
    path: PathBuf,
    digest: String,
    // Windows denies executable writes/deletion while this lifetime is held.
    _original: Arc<fs::File>,
}

impl Tool {
    fn pin(path: PathBuf) -> Result<Self> {
        ordinary(&path)?;
        let mut open = fs::OpenOptions::new();
        open.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            open.share_mode(1); // FILE_SHARE_READ only
        }
        let original = Arc::new(open.open(&path)?);
        let digest = tool_digest(&path)?;
        Ok(Self {
            path,
            digest,
            _original: original,
        })
    }
    fn check(&self) -> Result<()> {
        ordinary(&self.path)?;
        if tool_digest(&self.path)? != self.digest {
            bail!("managed executable drift");
        }
        Ok(())
    }
}

fn tool_digest(path: &Path) -> Result<String> {
    let file = fs::File::open(path)?;
    let expected = file.metadata()?.len();
    if expected == 0 || expected > 256 * 1024 * 1024 {
        bail!("managed executable byte bound");
    }
    let mut bytes = Vec::new();
    file.take(expected + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected {
        bail!("managed executable opened size drift");
    }
    Ok(format!("{:X}", Sha256::digest(&bytes)))
}

fn ordinary(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("managed canonical absolute path required");
    }
    for p in path.ancestors() {
        let m = fs::symlink_metadata(p)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                bail!("managed reparse ancestry refused");
            }
        }
        if m.file_type().is_symlink() {
            bail!("managed symlink ancestry refused");
        }
    }
    Ok(())
}

/// Even failed creation returns its original children/journal. The caller
/// cannot silently discard uncertain ownership or restart a second attempt.
pub(crate) enum Creation {
    Ready(ManagedWorker<NativeRuntime>),
    Retained {
        runtime: NativeRuntime,
        journal: Journal,
        diagnostic: String,
    },
}

pub(crate) struct NativeRuntime {
    options: CreatorOptions,
    rac: Tool,
    pwsh: Tool,
    agent: Option<OriginalChild>,
    ras: Option<OriginalChild>,
    collectors: Vec<OriginalChild>,
    binding: LifetimeBinding,
    administrator: String,
    password: String,
    barrier_filetime: u64,
    admitted_load: BTreeSet<Uuid>,
    workers: BTreeMap<Uuid, KernelProcess>,
    failed: bool,
    authenticated_boundary: bool,
    startup_deadline: Option<Instant>,
    anchor_tools: Vec<Tool>,
}

pub(crate) fn create(options: CreatorOptions) -> Result<Creation> {
    if !cfg!(windows)
        || options.timeout.is_zero()
        || options.timeout > Duration::from_secs(120)
        || options.worker_first == 0
        || options.worker_first > options.worker_last
        || options.worker_last - options.worker_first > 127
        || options.database.is_empty()
        || options.database_server.is_empty()
    {
        bail!("managed creator platform, deadline or target invalid");
    }
    let ports = [options.agent_port, options.cluster_port, options.ras_port];
    if ports.contains(&0)
        || BTreeSet::from(ports).len() != 3
        || ports
            .iter()
            .any(|p| (options.worker_first..=options.worker_last).contains(p))
    {
        bail!("managed private port families must be disjoint");
    }
    ordinary(&options.parent)?;
    let nonce = Uuid::new_v4();
    let root = options.parent.join(format!("managed-{nonce}"));
    // No caller may provide a previously existing lifetime root.
    fs::create_dir(&root)?;
    ordinary(&root)?;
    fs::create_dir(root.join("srvinfo"))?;
    let journal = Journal::create(&root.join("lifetime.jsonl"), nonce)?;
    let rac = Tool::pin(options.platform_bin.join("rac.exe"))?;
    let pwsh = Tool::pin(options.powershell.clone())?;
    let anchor_tools = [
        "ragent.exe",
        "ras.exe",
        "rmngr.exe",
        "rphost.exe",
        "dbda.exe",
    ]
    .into_iter()
    .map(|name| Tool::pin(options.platform_bin.join(name)))
    .collect::<Result<Vec<_>>>()?;
    let empty_identity = ProcessIdentity {
        pid: 0,
        parent: 0,
        birth_100ns: 0,
        executable: PathBuf::new(),
        command_sha256: String::new(),
    };
    let binding = LifetimeBinding {
        nonce,
        root,
        cluster: Uuid::nil(),
        infobase: Uuid::nil(),
        database: options.database.clone(),
        agent: empty_identity.clone(),
        ras: empty_identity,
    };
    let mut runtime = NativeRuntime {
        options,
        rac,
        pwsh,
        agent: None,
        ras: None,
        collectors: Vec::new(),
        binding,
        administrator: format!("ibcmd_{nonce}"),
        password: format!("{}{}", Uuid::new_v4(), Uuid::new_v4()),
        barrier_filetime: 0,
        admitted_load: BTreeSet::new(),
        workers: BTreeMap::new(),
        failed: false,
        authenticated_boundary: false,
        startup_deadline: None,
        anchor_tools,
    };
    let mut journal = journal;
    let result = (|| -> Result<()> {
        runtime.bootstrap(&mut journal)?;
        let o = runtime.observe()?;
        if o.binding != runtime.binding
            || !o.registrations.is_empty()
            || !o.loaded.is_empty()
            || o.worker.is_some()
            || !o.password_only_admins_exact
            || !o.authenticated_inventory_exact
            || !o.lease_original_handle_exact
            || o.unknown_administration
        {
            bail!("fresh managed authority barrier drift");
        }
        journal.append("creator_barrier", &runtime.binding.nonce.to_string())?;
        runtime.startup_remaining()?;
        runtime.startup_deadline = None;
        Ok(())
    })();
    match result {
        Ok(()) => {
            let binding = runtime.binding.clone();
            Ok(Creation::Ready(ManagedWorker {
                runtime,
                binding,
                journal,
                ever_loaded: BTreeSet::new(),
                registered: false,
                admitted_worker: None,
                tainted: false,
            }))
        }
        Err(error) => {
            runtime.failed = true;
            Ok(Creation::Retained {
                runtime,
                journal,
                diagnostic: format!("managed creation unproved: {}", error),
            })
        }
    }
}

impl OwnedRuntime for NativeRuntime {
    fn observe(&mut self) -> Result<Observation> {
        self.rac.check()?;
        self.pwsh.check()?;
        self.require_admins()?;
        if !self.authenticated_boundary {
            bail!("managed administration boundary is not independently proved");
        }
        let first = self.census()?;
        let second = self.census()?;
        for (anchor, expected) in [
            (&mut self.agent, &self.binding.agent),
            (&mut self.ras, &self.binding.ras),
        ] {
            let anchor = anchor.as_mut().context("original managed anchor absent")?;
            if !anchor.alive()? {
                bail!("original managed anchor exited");
            }
            for rows in [&first, &second] {
                let row = rows
                    .iter()
                    .find(|p| p.pid == anchor.pid())
                    .context("original anchor census missing")?;
                if &anchor.bind_census(row)? != expected {
                    bail!("managed anchor identity drift");
                }
            }
        }
        let owned_first = self.descendants(&first)?;
        let owned = self.descendants(&second)?;
        if owned_first.len() != owned.len() {
            bail!("managed ancestry changed during observation");
        }
        for p in &owned {
            let old = owned_first
                .iter()
                .find(|old| old.pid == p.pid)
                .context("managed ancestry process drift")?;
            if old.parent != p.parent
                || old.birth_filetime != p.birth_filetime
                || old.executable != p.executable
                || old.command != p.command
            {
                bail!("managed ancestry complete identity drift");
            }
        }
        let cluster = self.binding.cluster;
        let args = |what: &str| {
            vec![
                what.to_owned(),
                "list".into(),
                format!("--cluster={cluster}"),
            ]
        };
        let registrations = blocks(&self.rac(
            vec![
                "infobase".into(),
                "summary".into(),
                "list".into(),
                format!("--cluster={}", self.binding.cluster),
            ],
            false,
            true,
        )?)?
        .iter()
        .map(|r| {
            Uuid::parse_str(r.get("infobase").context("registration UUID missing")?)
                .map_err(Into::into)
        })
        .collect::<Result<BTreeSet<_>>>()?;
        let mut loaded = self.admitted_load.clone();
        for family in ["session", "connection"] {
            for r in blocks(&self.rac(args(family), false, true)?)? {
                loaded.insert(Uuid::parse_str(
                    r.get("infobase").context("loaded inventory UUID missing")?,
                )?);
            }
        }
        let processes = blocks(&self.rac(args("process"), false, true)?)?;
        if processes.len() > 1 {
            bail!("managed single working process required");
        }
        let worker = processes
            .first()
            .map(|r| -> Result<_> {
                let id = Uuid::parse_str(r.get("process").context("working UUID missing")?)?;
                let pid: u32 = r.get("pid").context("working PID missing")?.parse()?;
                let p = owned
                    .iter()
                    .find(|p| p.pid == pid)
                    .context("working process is not owned descendant")?;
                let image = self
                    .anchor_tools
                    .iter()
                    .find(|tool| tool.path == self.options.platform_bin.join("rphost.exe"))
                    .context("original pinned working executable absent")?;
                image.check()?;
                require_working_image(p, &image.path, self.barrier_filetime)?;
                if let Some(handle) = self.workers.get(&id) {
                    handle.require_current(p)?;
                } else {
                    self.workers.insert(id, KernelProcess::bind(p)?);
                }
                Ok((
                    id,
                    self.workers
                        .get(&id)
                        .context("working handle binding missing")?
                        .identity
                        .clone(),
                ))
            })
            .transpose()?;
        Ok(Observation {
            binding: self.binding.clone(),
            registrations,
            loaded,
            worker,
            password_only_admins_exact: true,
            authenticated_inventory_exact: true,
            lease_original_handle_exact: true,
            unknown_administration: false,
        })
    }

    fn register(&mut self) -> Result<Uuid> {
        let raw = self.rac(
            vec![
                "infobase".into(),
                "create".into(),
                format!("--cluster={}", self.binding.cluster),
                format!("--name={}", self.binding.database),
                "--dbms=MSSQLServer".into(),
                format!("--db-server={}", self.options.database_server),
                format!("--db-name={}", self.binding.database),
                format!("--db-user={}", self.options.database_user),
                format!("--db-pwd={}", self.options.database_password),
                "--locale=ru_RU".into(),
                "--scheduled-jobs-deny=on".into(),
            ],
            false,
            true,
        )?;
        let rows = blocks(&raw)?;
        if rows.len() != 1 {
            bail!("new managed registration response shape");
        }
        let id = Uuid::parse_str(
            rows[0]
                .get("infobase")
                .context("new registration UUID missing")?,
        )?;
        if id.is_nil() {
            bail!("new registration nil UUID");
        }
        self.binding.infobase = id;
        Ok(id)
    }

    fn load(&mut self, infobase: Uuid) -> Result<()> {
        if infobase != self.binding.infobase {
            bail!("managed load target drift");
        }
        // The intent is retained even if the native response is lost.
        self.admitted_load.insert(infobase);
        self.rac(
            vec![
                "infobase".into(),
                "info".into(),
                format!("--cluster={}", self.binding.cluster),
                format!("--infobase={infobase}"),
                format!("--infobase-user={}", self.options.infobase_user),
                format!("--infobase-pwd={}", self.options.infobase_password),
            ],
            false,
            true,
        )?;
        Ok(())
    }

    fn turn_off(&mut self, worker_id: Uuid, identity: &ProcessIdentity) -> Result<()> {
        let current = self.observe()?;
        if current.worker != Some((worker_id, identity.clone())) {
            bail!("managed handoff identity drift");
        }
        self.rac(
            vec![
                "process".into(),
                "turn-off".into(),
                format!("--cluster={}", self.binding.cluster),
                format!("--process={worker_id}"),
            ],
            false,
            true,
        )?;
        Ok(())
    }
}

impl NativeRuntime {
    pub(super) fn require_endpoint(&self, rac: &Path, endpoint: &str, server: &str) -> Result<()> {
        if rac != self.rac.path
            || endpoint != format!("localhost:{}", self.options.ras_port)
            || server != self.options.database_server
        {
            bail!("managed runtime endpoint does not match original creator");
        }
        Ok(())
    }

    pub(super) fn verify_target_profile(
        &mut self,
        claimed: crate::mssql_platform_profile::MssqlNativePlatformProfile,
        options: crate::mssql_platform_profile::MssqlNativeProfileVerificationOptions<'_>,
    ) -> Result<crate::mssql_platform_profile::MssqlNativeProfileVerification> {
        self.require_endpoint(options.rac, options.ras_endpoint, options.server)?;
        if options.database != self.binding.database
            || options.cluster_id != Some(self.binding.cluster)
            || options.infobase_id != Some(self.binding.infobase)
        {
            bail!("managed profile target identity drift");
        }
        let agent_version = self.rac(vec!["agent".into(), "version".into()], true, true)?;
        let mut args = vec![
            "infobase".into(),
            "info".into(),
            format!("--cluster={}", self.binding.cluster),
            format!("--infobase={}", self.binding.infobase),
        ];
        if let Some(user) = options.infobase_user {
            args.extend([
                format!("--infobase-user={user}"),
                format!(
                    "--infobase-pwd={}",
                    options.infobase_pwd.unwrap_or_default()
                ),
            ]);
        } else if options.infobase_pwd.is_some() {
            bail!("infobase password requires user");
        }
        let registration = self.rac(args, false, true)?;
        crate::mssql_platform_profile::verify_mssql_native_profile_managed_observation(
            claimed,
            options,
            &self.credentials(),
            &agent_version,
            &registration,
        )
    }

    fn credentials(&self) -> ManagedRacReadAuth<'_> {
        ManagedRacReadAuth {
            agent_user: &self.administrator,
            agent_password: &self.password,
            cluster_user: &self.administrator,
            cluster_password: &self.password,
        }
    }

    fn execute(&mut self, tool: &Tool, argv: &[String]) -> Result<(i32, Vec<u8>, Vec<u8>)> {
        tool.check()?;
        if self.failed {
            bail!("managed native lifetime already unproved");
        }
        let timeout = self.startup_remaining()?;
        if self.collectors.len() >= 1024 {
            bail!("managed original command custody bound; retain lifetime");
        }
        self.collectors
            .push(OriginalChild::spawn(&tool.path, argv)?);
        let index = self.collectors.len() - 1;
        let result = self.collectors[index].completed(timeout);
        if result.is_err() {
            self.failed = true;
        }
        let value = result?;
        self.startup_remaining()?;
        Ok(value)
    }

    fn startup_remaining(&self) -> Result<Duration> {
        remaining_budget(self.startup_deadline, Instant::now(), self.options.timeout)
    }

    fn shell(&mut self, script: String) -> Result<String> {
        let tool = self.pwsh.clone();
        let argv = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            script,
        ];
        let (exit, out, err) = self.execute(&tool, &argv)?;
        if exit != 0 || !err.is_empty() {
            bail!("managed observation utility failed; output redacted");
        }
        String::from_utf8(out).context("managed observation must be strict UTF-8")
    }

    fn census(&mut self) -> Result<Vec<CensusIdentity>> {
        let raw = self.shell("$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $p=@(Get-CimInstance Win32_Process -OperationTimeoutSec 10 | Where-Object { $_.Name -in @('ragent.exe','ras.exe','rmngr.exe','rphost.exe','dbda.exe') } | Select-Object -First 257); if($p.Count -gt 256){throw 'census bound'}; $r=@($p | ForEach-Object { if(!$_.ExecutablePath -or !$_.CommandLine -or !$_.CreationDate){throw 'incomplete identity'}; @{pid=[uint32]$_.ProcessId;parent=[uint32]$_.ParentProcessId;birth_filetime=[uint64]$_.CreationDate.ToFileTimeUtc();executable=$_.ExecutablePath;command=$_.CommandLine} }); ConvertTo-Json -InputObject $r -Depth 4 -Compress".into())?;
        serde_json::from_str(&raw).context("managed census shape")
    }

    /// No command is addressed to a port merely because it was vacant at
    /// creation. Bind every current listener between two complete censuses.
    fn require_private_endpoint(&mut self) -> Result<()> {
        for tool in &self.anchor_tools {
            tool.check()?;
        }
        let first = self.census()?;
        let ports: Vec<_> = [
            self.options.agent_port,
            self.options.cluster_port,
            self.options.ras_port,
        ]
        .into_iter()
        .chain(self.options.worker_first..=self.options.worker_last)
        .collect();
        let selected = ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let raw = self.shell(format!("$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $p=@(Get-NetTCPConnection -State Listen -ErrorAction Stop | Where-Object {{ $_.LocalPort -in @({selected}) }} | Select-Object -First 513); if($p.Count -gt 512){{throw 'listener bound'}}; $r=@($p | ForEach-Object {{ @{{port=[uint16]$_.LocalPort;pid=[uint32]$_.OwningProcess}} }}); ConvertTo-Json -InputObject $r -Compress"))?;
        let listeners: Vec<Listener> =
            serde_json::from_str(&raw).context("managed listener shape")?;
        let second = self.census()?;
        let result = (|| -> Result<bool> {
            for (anchor, expected) in [
                (&mut self.agent, &self.binding.agent),
                (&mut self.ras, &self.binding.ras),
            ] {
                let anchor = anchor
                    .as_mut()
                    .context("managed original endpoint handle absent")?;
                if !anchor.alive()? {
                    bail!("original endpoint anchor exited");
                }
                for rows in [&first, &second] {
                    let row = rows
                        .iter()
                        .find(|r| r.pid == anchor.pid())
                        .context("endpoint census anchor absent")?;
                    if anchor.bind_census(row)? != *expected {
                        bail!("endpoint anchor drift");
                    }
                }
            }
            let a = self.descendants(&first)?;
            let b = self.descendants(&second)?;
            listener_authority(
                &listeners,
                &a,
                &b,
                &self.options.platform_bin,
                &self.binding.root,
                &first,
                &second,
                (self.options.agent_port, self.binding.agent.pid),
                (self.options.ras_port, self.binding.ras.pid),
            )
        })();
        match result {
            Ok(true) => Ok(()),
            Ok(false) => bail!("managed private endpoint is not ready"),
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    fn rac(
        &mut self,
        mut argv: Vec<String>,
        agent_auth: bool,
        authenticated: bool,
    ) -> Result<String> {
        self.require_private_endpoint()?;
        if authenticated {
            let prefix = if agent_auth { "agent" } else { "cluster" };
            argv.extend([
                format!("--{prefix}-user={}", self.administrator),
                format!("--{prefix}-pwd={}", self.password),
            ]);
        }
        argv.push(format!("localhost:{}", self.options.ras_port));
        let tool = self.rac.clone();
        let (exit, out, err) = self.execute(&tool, &argv)?;
        // Never propagate native text containing credentials or command lines.
        if exit != 0 || !err.is_empty() {
            bail!("managed RAC command failed; arguments/output redacted");
        }
        String::from_utf8(out).context("managed RAC response must be strict UTF-8")
    }

    fn bootstrap(&mut self, journal: &mut Journal) -> Result<()> {
        let deadline = Instant::now()
            .checked_add(self.options.timeout)
            .context("startup deadline overflow")?;
        self.startup_deadline = Some(deadline);
        let ports: Vec<_> = [
            self.options.agent_port,
            self.options.cluster_port,
            self.options.ras_port,
        ]
        .into_iter()
        .chain(self.options.worker_first..=self.options.worker_last)
        .collect();
        let port_list = ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let raw = self.shell(format!("$ErrorActionPreference='Stop'; if(@(Get-NetTCPConnection -State Listen -ErrorAction Stop | Where-Object {{ $_.LocalPort -in @({port_list}) }}).Count) {{throw 'private ports occupied'}}; 'EMPTY'"))?;
        if raw.trim() != "EMPTY" {
            bail!("fresh private ports not proved");
        }
        journal.append("agent_spawn_intent", self.binding.root.to_string_lossy())?;
        let agent = Tool::pin(self.options.platform_bin.join("ragent.exe"))?;
        self.startup_remaining()?;
        self.agent = Some(OriginalChild::spawn(
            &agent.path,
            &[
                "-agent".into(),
                "-port".into(),
                self.options.agent_port.to_string(),
                "-regport".into(),
                self.options.cluster_port.to_string(),
                "-range".into(),
                format!("{}:{}", self.options.worker_first, self.options.worker_last),
                "-d".into(),
                self.binding
                    .root
                    .join("srvinfo")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )?);
        self.anchor_tools.push(agent);
        let rows = self.census()?;
        let anchor = self
            .agent
            .as_mut()
            .context("agent original handle absent")?;
        let row = rows
            .iter()
            .find(|p| p.pid == anchor.pid())
            .context("spawned agent census absent")?;
        self.binding.agent = anchor.bind_census(row)?;
        journal.append("agent_spawn_confirmed", &self.binding.agent)?;
        journal.append("ras_spawn_intent", self.binding.root.to_string_lossy())?;
        let ras = Tool::pin(self.options.platform_bin.join("ras.exe"))?;
        self.startup_remaining()?;
        self.ras = Some(OriginalChild::spawn(
            &ras.path,
            &[
                "cluster".into(),
                format!("--port={}", self.options.ras_port),
                format!("localhost:{}", self.options.agent_port),
            ],
        )?);
        self.anchor_tools.push(ras);
        let rows = self.census()?;
        let anchor = self.ras.as_mut().context("RAS original handle absent")?;
        let row = rows
            .iter()
            .find(|p| p.pid == anchor.pid())
            .context("spawned RAS census absent")?;
        self.binding.ras = anchor.bind_census(row)?;
        journal.append("ras_spawn_confirmed", &self.binding.ras)?;
        // Bounded startup only retries read-only version probes, never writes.
        loop {
            match self.rac(vec!["agent".into(), "version".into()], false, false) {
                Ok(build)
                    if crate::mssql_platform_profile::parse_rac_agent_build(&build)
                        .is_ok_and(|build| build == "8.3.27.2214") =>
                {
                    self.startup_remaining()?;
                    break;
                }
                _ if self.failed || Instant::now() >= deadline => {
                    bail!("managed agent startup/profile unproved")
                }
                _ => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        let clusters = blocks(&self.rac(vec!["cluster".into(), "list".into()], false, false)?)?;
        if clusters.len() != 1 {
            bail!("fresh managed cluster count must be exactly one");
        }
        self.binding.cluster =
            Uuid::parse_str(clusters[0].get("cluster").context("cluster UUID absent")?)?;
        if self.binding.cluster.is_nil() {
            bail!("nil managed cluster");
        }
        journal.append("cluster_confirmed", self.binding.cluster.to_string())?;
        journal.append("agent_admin_intent", &self.administrator)?;
        self.rac(
            vec![
                "agent".into(),
                "admin".into(),
                "register".into(),
                format!("--name={}", self.administrator),
                format!("--pwd={}", self.password),
                "--auth=pwd".into(),
            ],
            true,
            false,
        )?;
        journal.append("agent_admin_confirmed", &self.administrator)?;
        journal.append("cluster_admin_intent", &self.administrator)?;
        self.rac(
            vec![
                "cluster".into(),
                "admin".into(),
                "register".into(),
                format!("--cluster={}", self.binding.cluster),
                format!("--name={}", self.administrator),
                format!("--pwd={}", self.password),
                "--auth=pwd".into(),
            ],
            true,
            true,
        )?;
        journal.append("cluster_admin_confirmed", &self.administrator)?;
        self.require_admins()?;
        self.challenge_authentication(journal)?;
        // Pre-barrier workers cannot acquire positive lifetime authority.
        let rows = self.census()?;
        if self.descendants(&rows)?.iter().any(|p| {
            p.executable
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("rphost.exe"))
        }) {
            bail!("pre-authentication working process exists; history cannot be inferred");
        }
        self.barrier_filetime = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .checked_div(100)
            .context("barrier clock")? as u64
            + 116444736000000000;
        Ok(())
    }

    fn require_admins(&mut self) -> Result<()> {
        for agent in [true, false] {
            let mut args = vec![
                if agent {
                    "agent".into()
                } else {
                    "cluster".into()
                },
                "admin".into(),
                "list".into(),
            ];
            if !agent {
                args.push(format!("--cluster={}", self.binding.cluster));
            }
            let admins = blocks(&self.rac(args, agent, true)?)?;
            if admins.len() != 1
                || admins[0].get("name") != Some(&self.administrator)
                || admins[0].get("auth").map(String::as_str) != Some("pwd")
                || admins[0].get("os-user").is_some_and(|v| !v.is_empty())
            {
                bail!("exact password-only managed administration not proved");
            }
        }
        Ok(())
    }

    fn challenge_authentication(&mut self, journal: &mut Journal) -> Result<()> {
        for agent in [true, false] {
            let family = if agent { "agent" } else { "cluster" };
            let mut base = vec![family.to_owned(), "admin".into(), "list".into()];
            if !agent {
                base.push(format!("--cluster={}", self.binding.cluster));
            }
            let before = self.rac(base.clone(), agent, true)?;
            for wrong_password in [false, true] {
                let kind = if wrong_password {
                    "wrong-password"
                } else {
                    "implicit-OS"
                };
                journal.append("authentication_challenge_intent", (family, kind))?;
                let mut argv = base.clone();
                if wrong_password {
                    argv.push(format!("--{family}-user={}", self.administrator));
                    argv.push(format!("--{family}-pwd={}", Uuid::new_v4()));
                }
                argv.push(format!("localhost:{}", self.options.ras_port));
                self.require_private_endpoint()?;
                let tool = self.rac.clone();
                let (exit, out, err) = self.execute(&tool, &argv)?;
                let fingerprint = DenialFingerprint {
                    exit,
                    stdout: format!("{:X}", Sha256::digest(&out)),
                    stderr: format!("{:X}", Sha256::digest(&err)),
                };
                journal.append(
                    "authentication_challenge_observed",
                    serde_json::json!({
                        "family": family, "kind": kind, "exit": exit,
                        "stdout_bytes": out.len(), "stderr_bytes": err.len(),
                        "stdout_sha256": fingerprint.stdout, "stderr_sha256": fingerprint.stderr,
                        "classified_denial": denial_admitted(&fingerprint, measured_denials()),
                    }),
                )?;
                if !denial_admitted(&fingerprint, measured_denials()) {
                    bail!(
                        "managed authentication challenge unclassified or OS bypass admitted; no ownership authority issued"
                    );
                }
                if self.rac(base.clone(), agent, true)? != before {
                    bail!("authenticated administration inventory changed across challenge");
                }
            }
        }
        self.authenticated_boundary = true;
        Ok(())
    }

    fn descendants<'a>(&self, rows: &'a [CensusIdentity]) -> Result<Vec<&'a CensusIdentity>> {
        let mut admitted = BTreeSet::from([self.binding.agent.pid, self.binding.ras.pid]);
        loop {
            let old = admitted.len();
            for p in rows {
                if admitted.contains(&p.parent) {
                    admitted.insert(p.pid);
                }
            }
            if old == admitted.len() {
                break;
            }
        }
        let result: Vec<_> = rows.iter().filter(|p| admitted.contains(&p.pid)).collect();
        if result.len() > 64 {
            bail!("managed descendant bound");
        }
        Ok(result)
    }
}

fn remaining_budget(
    deadline: Option<Instant>,
    now: Instant,
    ordinary: Duration,
) -> Result<Duration> {
    match deadline {
        Some(deadline) => deadline
            .checked_duration_since(now)
            .filter(|value| !value.is_zero())
            .context("managed startup deadline expired; no admission or next command"),
        None => Ok(ordinary),
    }
}

fn require_working_image(row: &CensusIdentity, expected: &Path, barrier: u64) -> Result<()> {
    // RAC can expose a worker before it has a selected TCP listener. Listener
    // ownership is therefore not an executable check for this PID.
    if row.birth_filetime <= barrier
        || !row.executable.is_absolute()
        || !expected.is_absolute()
        || !row
            .executable
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.to_string_lossy())
    {
        bail!(
            "working executable differs from original pinned platform or predates authentication barrier"
        );
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Listener {
    port: u16,
    pid: u32,
}

#[allow(clippy::too_many_arguments)]
fn listener_authority(
    listeners: &[Listener],
    first: &[&CensusIdentity],
    second: &[&CensusIdentity],
    platform: &Path,
    root: &Path,
    census_a: &[CensusIdentity],
    census_b: &[CensusIdentity],
    agent: (u16, u32),
    ras: (u16, u32),
) -> Result<bool> {
    if listeners.len() > 512 {
        bail!("managed listener inventory bound");
    }
    for row in census_a.iter().chain(census_b) {
        if row
            .command
            .to_ascii_lowercase()
            .contains(&root.to_string_lossy().to_ascii_lowercase())
            && !first.iter().any(|p| p.pid == row.pid)
        {
            bail!("foreign process names managed root");
        }
    }
    for listener in listeners {
        let a = first
            .iter()
            .find(|r| r.pid == listener.pid)
            .context("foreign or missing listener in first census")?;
        let b = second
            .iter()
            .find(|r| r.pid == listener.pid)
            .context("foreign or missing listener in second census")?;
        if a.parent != b.parent
            || a.birth_filetime != b.birth_filetime
            || a.executable != b.executable
            || a.command != b.command
        {
            bail!("listener identity changed between censuses");
        }
        let name = a
            .executable
            .file_name()
            .context("listener executable absent")?
            .to_string_lossy();
        if ![
            "ragent.exe",
            "ras.exe",
            "rmngr.exe",
            "rphost.exe",
            "dbda.exe",
        ]
        .iter()
        .any(|known| name.eq_ignore_ascii_case(known))
            || !a
                .executable
                .to_string_lossy()
                .eq_ignore_ascii_case(&platform.join(name.as_ref()).to_string_lossy())
        {
            bail!("listener executable is outside exact owned platform");
        }
        for (port, pid) in [agent, ras] {
            if listener.port == port && listener.pid != pid {
                bail!("private endpoint listener belongs to another process");
            }
        }
    }
    Ok([agent, ras]
        .iter()
        .all(|(port, pid)| listeners.iter().any(|l| l.port == *port && l.pid == *pid)))
}

#[derive(Debug, PartialEq, Eq)]
struct DenialFingerprint {
    exit: i32,
    stdout: String,
    stderr: String,
}

fn denial_admitted(observed: &DenialFingerprint, measured: &[DenialFingerprint]) -> bool {
    observed.exit != 0 && measured.iter().any(|known| known == observed)
}

fn measured_denials() -> &'static [DenialFingerprint] {
    // No denial/OS-bypass measurement exists in the accepted native corpus.
    // Populate only from a reviewed exact native creator experiment. Neither
    // user JSON nor a current admin list can extend this closed admission set.
    &[]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_image_without_any_listener_still_requires_exact_pinned_executable() {
        let expected = std::env::temp_dir()
            .join("owned-platform")
            .join("rphost.exe");
        let mut row = CensusIdentity {
            pid: 44,
            parent: 40,
            birth_filetime: 101,
            executable: expected.clone(),
            command: "working command".into(),
        };
        assert!(require_working_image(&row, &expected, 100).is_ok());
        row.executable = std::env::temp_dir()
            .join("foreign-platform")
            .join("rphost.exe");
        assert!(require_working_image(&row, &expected, 100).is_err());
        row.executable = expected;
        assert!(require_working_image(&row, &row.executable, 101).is_err());
        assert!(require_working_image(&row, &PathBuf::from("rphost.exe"), 100).is_err());
    }

    #[test]
    fn startup_deadline_never_admits_a_late_success_or_next_command() {
        let deadline = Instant::now();
        assert!(
            remaining_budget(
                Some(deadline),
                deadline - Duration::from_nanos(1),
                Duration::from_secs(120)
            )
            .is_ok()
        );
        assert!(remaining_budget(Some(deadline), deadline, Duration::from_secs(120)).is_err());
        assert!(
            remaining_budget(
                Some(deadline),
                deadline + Duration::from_nanos(1),
                Duration::from_secs(120)
            )
            .is_err()
        );
        assert_eq!(
            remaining_budget(None, deadline, Duration::from_secs(5)).unwrap(),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn every_endpoint_listener_needs_both_complete_owned_censuses() {
        let platform = std::env::temp_dir().join("native-platform");
        let root = std::env::temp_dir().join("managed-fresh-nonce");
        let make = |pid, name: &str| CensusIdentity {
            pid,
            parent: 1,
            birth_filetime: 100,
            executable: platform.join(name),
            command: format!("{name} private"),
        };
        let agent = make(2, "ragent.exe");
        let ras = make(3, "ras.exe");
        let worker = make(4, "rphost.exe");
        let first = vec![&agent, &ras, &worker];
        let listeners = vec![
            Listener { port: 2540, pid: 2 },
            Listener { port: 2545, pid: 3 },
            Listener { port: 2560, pid: 4 },
        ];
        let check = |ls: &[Listener], second: &[&CensusIdentity], ca: &[CensusIdentity]| {
            listener_authority(
                ls,
                &first,
                second,
                &platform,
                &root,
                ca,
                &[],
                (2540, 2),
                (2545, 3),
            )
        };
        assert!(check(&listeners, &first, &[]).unwrap());
        assert!(!check(&[], &first, &[]).unwrap()); // Read-only startup can wait; cannot issue RAC.
        assert!(check(&listeners, &[&agent, &ras], &[]).is_err()); // exited/missing listener
        let mut changed = make(4, "rphost.exe");
        changed.birth_filetime += 1;
        assert!(check(&listeners, &[&agent, &ras, &changed], &[]).is_err());
        for ls in [
            vec![Listener { port: 2540, pid: 4 }],
            vec![Listener {
                port: 2545,
                pid: 99,
            }],
        ] {
            assert!(check(&ls, &first, &[]).is_err());
        }
        let foreign = CensusIdentity {
            pid: 99,
            parent: 1,
            birth_filetime: 200,
            executable: platform.join("ragent.exe"),
            command: root.to_string_lossy().into_owned(),
        };
        assert!(check(&listeners, &first, &[foreign]).is_err());
        let alien = make(4, "powershell.exe");
        assert!(
            listener_authority(
                &listeners,
                &[&agent, &ras, &alien],
                &[&agent, &ras, &alien],
                &platform,
                &root,
                &[],
                &[],
                (2540, 2),
                (2545, 3)
            )
            .is_err()
        );
    }

    #[test]
    fn authentication_opaque_failure_and_forced_os_success_are_not_authority() {
        let measured = DenialFingerprint {
            exit: 1,
            stdout: "A".repeat(64),
            stderr: "B".repeat(64),
        };
        assert!(denial_admitted(
            &measured,
            &[DenialFingerprint {
                exit: 1,
                stdout: "A".repeat(64),
                stderr: "B".repeat(64)
            }]
        ));
        for changed in [
            DenialFingerprint {
                exit: 0,
                stdout: "A".repeat(64),
                stderr: "B".repeat(64),
            },
            DenialFingerprint {
                exit: 1,
                stdout: "C".repeat(64),
                stderr: "B".repeat(64),
            },
            DenialFingerprint {
                exit: 1,
                stdout: "A".repeat(64),
                stderr: "C".repeat(64),
            },
        ] {
            assert!(!denial_admitted(&changed, std::slice::from_ref(&measured)));
        }
        assert!(!denial_admitted(&measured, measured_denials()));
    }

    #[test]
    fn unknown_and_duplicate_inventory_lines_refuse_instead_of_losing_history() {
        assert!(blocks("infobase: a\ninfobase: b").is_err());
        assert!(blocks("unexpected output").is_err());
        assert!(blocks(&"a: b\n\n".repeat(129)).is_err());
        assert!(blocks("").unwrap().is_empty());
    }
}

fn blocks(text: &str) -> Result<Vec<BTreeMap<String, String>>> {
    let mut result = Vec::new();
    let mut record = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            if !record.is_empty() {
                result.push(std::mem::take(&mut record));
            }
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .context("managed RAC inventory line unparsed")?;
        let key = key.trim();
        if key.is_empty()
            || record
                .insert(key.to_owned(), value.trim().to_owned())
                .is_some()
        {
            bail!("managed RAC duplicate field");
        }
    }
    if !record.is_empty() {
        result.push(record);
    }
    if result.len() > 128 {
        bail!("managed RAC inventory count bound");
    }
    Ok(result)
}
