//! Original child handles for the managed creator. An observation failure
//! never kills a guessed process or turns a failed command into success.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::ProcessIdentity;

const OUTPUT_BOUND: usize = 1024 * 1024;

/// The handle and pipe receivers remain owned on every failure. Dropping this
/// value sends no signal; the caller must retain its unproved lifetime journal.
pub(crate) struct OriginalChild {
    child: Child,
    executable: PathBuf,
    birth_100ns: u64,
    stdout: Receiver<Result<Vec<u8>>>,
    stderr: Receiver<Result<Vec<u8>>>,
    identity: Option<ProcessIdentity>,
    outcome_unproved: bool,
}

fn pipe(reader: impl Read + Send + 'static) -> Receiver<Result<Vec<u8>>> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<Vec<u8>> {
            let mut bytes = Vec::new();
            reader
                .take((OUTPUT_BOUND + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > OUTPUT_BOUND {
                bail!("managed child pipe exceeds bound");
            }
            Ok(bytes)
        })();
        let _ = send.send(result);
    });
    receive
}

fn missing_pipe() -> Receiver<Result<Vec<u8>>> {
    let (send, receive) = mpsc::channel();
    let _ = send.send(Err(anyhow::anyhow!("original managed pipe handle missing")));
    receive
}

#[cfg(windows)]
fn original_birth(child: &Child) -> Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;
    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    // The std Child's retained handle, never an OpenProcess on a recycled PID.
    if unsafe {
        GetProcessTimes(
            child.as_raw_handle() as _,
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error())
            .context("original managed child creation time");
    }
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

#[cfg(not(windows))]
fn original_birth(_child: &Child) -> Result<u64> {
    bail!("managed worker creator requires Windows original-handle identity");
}

impl OriginalChild {
    pub(crate) fn spawn(executable: &Path, arguments: &[String]) -> Result<Self> {
        if !cfg!(windows) || !executable.is_absolute() {
            bail!("managed creator requires a fully qualified Windows executable");
        }
        let mut child = Command::new(executable)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("managed original child launch failed; arguments redacted")?;
        // Establish handle ownership and both readers before closing input.
        // After Start, capture failures must still return original custody.
        // An Err here would discard the only handle to a potentially live child.
        let out = child.stdout.take();
        let err = child.stderr.take();
        let pipes_missing = out.is_none() || err.is_none();
        let stdout = out.map_or_else(missing_pipe, pipe);
        let stderr = err.map_or_else(missing_pipe, pipe);
        let birth = original_birth(&child);
        drop(child.stdin.take()); // Valid pipe EOF, rather than inherited invalid stdin.
        let mut owned = Self {
            child,
            executable: executable.to_owned(),
            birth_100ns: 0,
            stdout,
            stderr,
            identity: None,
            outcome_unproved: pipes_missing,
        };
        match birth {
            Ok(value) => owned.birth_100ns = value,
            Err(_) => {
                owned.outcome_unproved = true;
            }
        }
        Ok(owned)
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn alive(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }

    /// Full CIM identity augments the original handle. CIM's microsecond birth
    /// interval must contain the handle time; printed seven digits are not
    /// proof of 100ns precision.
    pub(crate) fn bind_census(&mut self, row: &CensusIdentity) -> Result<ProcessIdentity> {
        let result = (|| -> Result<ProcessIdentity> {
            if self.birth_100ns == 0 || self.outcome_unproved {
                bail!("original spawned handle identity unproved; retain child");
            }
            let end = row
                .birth_filetime
                .checked_add(9)
                .context("CIM birth overflow")?;
            if row.pid != self.pid()
                || row.parent != std::process::id()
                || row.birth_filetime > self.birth_100ns
                || self.birth_100ns > end
                || !same_executable(&row.executable, &self.executable)
                || row.command.is_empty()
            {
                bail!("original managed child census identity unproved");
            }
            let identity = ProcessIdentity {
                pid: row.pid,
                parent: row.parent,
                birth_100ns: self.birth_100ns,
                executable: self.executable.clone(),
                command_sha256: format!("{:X}", Sha256::digest(row.command.as_bytes())),
            };
            if self.identity.as_ref().is_some_and(|old| old != &identity) {
                bail!("original managed child command identity drift");
            }
            self.identity = Some(identity.clone());
            Ok(identity)
        })();
        if result.is_err() {
            self.outcome_unproved = true;
        }
        result
    }

    pub(crate) fn completed(&mut self, timeout: Duration) -> Result<(i32, Vec<u8>, Vec<u8>)> {
        let result = (|| -> Result<_> {
            if self.outcome_unproved {
                bail!("managed child outcome already unproved");
            }
            let deadline = Instant::now()
                .checked_add(timeout)
                .context("child deadline overflow")?;
            let exit = loop {
                if let Some(status) = self.child.try_wait()? {
                    break status;
                }
                if Instant::now() >= deadline {
                    bail!("managed child deadline; no signal sent");
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            let remaining = || deadline.saturating_duration_since(Instant::now());
            let stdout = self
                .stdout
                .recv_timeout(remaining())
                .context("original stdout unproved")??;
            let stderr = self
                .stderr
                .recv_timeout(remaining())
                .context("original stderr unproved")??;
            Ok((
                exit.code().context("managed child exit code unavailable")?,
                stdout,
                stderr,
            ))
        })();
        if result.is_err() {
            self.outcome_unproved = true;
        }
        result
    }
}

fn same_executable(left: &Path, right: &Path) -> bool {
    left.is_absolute()
        && right.is_absolute()
        && left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
}

/// Query-only handle acquired for a post-barrier descendant. It intentionally
/// has no PROCESS_TERMINATE right; a turn-off is addressed to the independently
/// bound RAC process UUID, never a Windows PID signal.
pub(crate) struct KernelProcess {
    #[cfg(windows)]
    handle: std::os::windows::io::OwnedHandle,
    pub identity: ProcessIdentity,
}

impl KernelProcess {
    #[cfg(windows)]
    pub(crate) fn bind(row: &CensusIdentity) -> Result<Self> {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{
            GetProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };
        // SYNCHRONIZE permits exit observation, but no termination.
        let raw =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | 0x00100000, 0, row.pid) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error())
                .context("bind managed descendant original handle");
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
        let mut birth: FILETIME = unsafe { std::mem::zeroed() };
        let mut exit: FILETIME = unsafe { std::mem::zeroed() };
        let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
        let mut user: FILETIME = unsafe { std::mem::zeroed() };
        let mut path = vec![0u16; 32768];
        let mut count = path.len() as u32;
        if unsafe { GetProcessId(raw) } != row.pid
            || unsafe { GetProcessTimes(raw, &mut birth, &mut exit, &mut kernel, &mut user) } == 0
            || unsafe { QueryFullProcessImageNameW(raw, 0, path.as_mut_ptr(), &mut count) } == 0
        {
            bail!("managed descendant handle identity unavailable");
        }
        let birth = (u64::from(birth.dwHighDateTime) << 32) | u64::from(birth.dwLowDateTime);
        let executable = PathBuf::from(String::from_utf16(&path[..count as usize])?);
        let end = row
            .birth_filetime
            .checked_add(9)
            .context("descendant CIM interval overflow")?;
        if birth < row.birth_filetime
            || birth > end
            || row.parent == 0
            || row.command.is_empty()
            || !same_executable(&executable, &row.executable)
        {
            bail!("managed descendant PID reused or complete identity drift");
        }
        Ok(Self {
            handle,
            identity: ProcessIdentity {
                pid: row.pid,
                parent: row.parent,
                birth_100ns: birth,
                executable,
                command_sha256: format!("{:X}", Sha256::digest(row.command.as_bytes())),
            },
        })
    }

    #[cfg(not(windows))]
    pub(crate) fn bind(_row: &CensusIdentity) -> Result<Self> {
        bail!("managed descendant handle binding requires Windows");
    }

    pub(crate) fn require_current(&self, row: &CensusIdentity) -> Result<()> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            if unsafe { WaitForSingleObject(self.handle.as_raw_handle() as _, 0) } != 258 {
                bail!("original managed descendant exited or handle wait unproved");
            }
        }
        let end = row
            .birth_filetime
            .checked_add(9)
            .context("current CIM interval overflow")?;
        if row.pid != self.identity.pid
            || row.parent != self.identity.parent
            || self.identity.birth_100ns < row.birth_filetime
            || self.identity.birth_100ns > end
            || !same_executable(&row.executable, &self.identity.executable)
            || format!("{:X}", Sha256::digest(row.command.as_bytes()))
                != self.identity.command_sha256
        {
            bail!("original descendant identity changed; no process signal");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CensusIdentity {
    pub pid: u32,
    pub parent: u32,
    pub birth_filetime: u64,
    pub executable: PathBuf,
    pub command: String,
}
