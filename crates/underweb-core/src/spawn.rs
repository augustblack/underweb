//! Starting a child that talks to us over a [`Channel`].

use std::path::Path;

use anyhow::Result;
use underweb_ipc::Channel;
use underweb_sandbox::{Launch, Process};

/// A launch of `program` that inherits `theirs` as `--ipc`. On Windows
/// the child also gets a `PATH` to find its DLLs (the sandbox decides
/// what it may actually read).
pub(crate) fn with_channel(program: &Path, theirs: &Channel) -> Result<Launch> {
    let end = theirs.child_end()?;
    let mut l = Launch::new(program);
    l.arg("--ipc").arg(end.arg);
    l.inherit = end.handles;
    if let Some(v) = std::env::var_os("UNDERWEB_TRACE") {
        l.env("UNDERWEB_TRACE", v);
    }
    #[cfg(windows)]
    l.env("PATH", underweb_sandbox::child_path(program));
    Ok(l)
}

/// Start it, and tell our end who is at the other one (Windows passes
/// handles by duplicating them into that process).
pub(crate) fn start(l: &Launch, ours: &Channel) -> Result<Process> {
    let p = l.spawn()?;
    #[cfg(windows)]
    ours.set_peer(p.handle()?);
    #[cfg(not(windows))]
    let _ = ours;
    Ok(p)
}
