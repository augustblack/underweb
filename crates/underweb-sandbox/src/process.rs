//! Starting runner, media and builder processes the same way on every OS.
//!
//! Unix sandboxes are applied by the child itself ([`crate::lockdown`]),
//! so here a container only matters on Windows: an AppContainer has to
//! be chosen when the process is created, with filesystem access granted
//! to it beforehand.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitStatus;

use anyhow::Result;

use crate::Policy;

/// A raw fd (Unix) or `HANDLE` (Windows) the child should inherit.
#[cfg(unix)]
pub type RawInherit = std::os::fd::RawFd;
#[cfg(windows)]
pub type RawInherit = std::os::windows::io::RawHandle;

/// What a sandboxed child may use, decided by the parent (Windows only;
/// elsewhere the child's own [`crate::lockdown`] enforces its policy).
#[derive(Debug, Clone)]
pub struct Container {
    /// Same name, same container: one per package, so packages can't read
    /// each other's data. Hashed into the AppContainer profile name.
    pub name: String,
    pub policy: Policy,
    /// Device access, by Windows capability name (`"webcam"`).
    pub capabilities: Vec<String>,
}

/// A process to start. The environment starts empty.
#[derive(Debug, Clone)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    /// Handles the child inherits (from `Channel::child_end`). On Unix
    /// they must already be inheritable; on Windows only these are
    /// inherited.
    pub inherit: Vec<RawInherit>,
    /// Give the parent the child's stderr instead of sharing its own.
    pub pipe_stderr: bool,
    pub container: Option<Container>,
}

impl Launch {
    pub fn new(program: impl Into<PathBuf>) -> Launch {
        Launch {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            inherit: Vec::new(),
            pipe_stderr: false,
            container: None,
        }
    }

    pub fn arg(&mut self, a: impl Into<OsString>) -> &mut Launch {
        self.args.push(a.into());
        self
    }

    pub fn env(&mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> &mut Launch {
        self.env.push((k.into(), v.into()));
        self
    }

    pub fn spawn(&self) -> Result<Process> {
        imp::spawn(self).map(|inner| Process { inner })
    }
}

/// A started child. Unlike `std::process::Child`, dropping it on Windows
/// kills the child (it lives in a kill-on-close job).
pub struct Process {
    inner: imp::Process,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.inner.id()
    }

    pub fn kill(&mut self) -> io::Result<()> {
        self.inner.kill()
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.inner.wait()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.inner.try_wait()
    }

    /// The child's stderr, with [`Launch::pipe_stderr`]. Once.
    pub fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.inner.take_stderr()
    }

    /// A handle to the process, for `Channel::set_peer`.
    #[cfg(windows)]
    pub fn handle(&self) -> io::Result<std::os::windows::io::OwnedHandle> {
        self.inner.handle()
    }
}

#[cfg(unix)]
mod imp {
    use std::io::{self, Read};
    use std::process::{Child, Command, ExitStatus, Stdio};

    use anyhow::{Context, Result};

    pub struct Process(Child);

    pub fn spawn(l: &super::Launch) -> Result<Process> {
        let mut cmd = Command::new(&l.program);
        cmd.args(&l.args)
            .env_clear()
            .envs(l.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null());
        if l.pipe_stderr {
            cmd.stderr(Stdio::piped());
        }
        let child = cmd
            .spawn()
            .with_context(|| format!("starting {}", l.program.display()))?;
        Ok(Process(child))
    }

    impl Process {
        pub fn id(&self) -> u32 {
            self.0.id()
        }
        pub fn kill(&mut self) -> io::Result<()> {
            self.0.kill()
        }
        pub fn wait(&mut self) -> io::Result<ExitStatus> {
            self.0.wait()
        }
        pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.0.try_wait()
        }
        pub fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
            self.0
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>)
        }
    }
}

#[cfg(windows)]
use crate::windows as imp;
