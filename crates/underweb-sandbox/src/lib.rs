//! Locks the calling process down so package code can only:
//! read its own package directory and system libraries/fonts, read and
//! write its private data directory, and talk over file descriptors it
//! already holds (the IPC socket). No network, no process creation, no
//! access to the user's files.
//!
//! The runner calls [`lockdown`] after it has set up IPC, V8 and fonts and
//! before it loads any package code. Lockdown cannot be undone.
//!
//! | OS | Mechanism |
//! |---|---|
//! | macOS | `sandbox_init` with an SBPL profile (what Chromium uses) |
//! | Linux | Landlock (filesystem) + seccomp-bpf (sockets, exec, fork) |
//! | Windows | AppContainer + job object, chosen by the parent at creation ([`Launch`]); `lockdown` checks it and adds mitigations |
//!
//! Start children with [`Launch`], which applies the Windows container
//! (on Unix the child's own `lockdown` does the work).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod process;
#[cfg(windows)]
mod windows;

pub use process::{Container, Launch, Process, RawInherit};

/// `PATH` for a child on Windows, where DLLs are found through it.
#[cfg(windows)]
pub use windows::child_path;

#[derive(Debug, Clone, Default)]
pub struct Policy {
    /// Directories readable by the package (its own files).
    pub read_only: Vec<PathBuf>,
    /// Directories readable and writable (its private data dir).
    pub read_write: Vec<PathBuf>,
    /// Allow starting programs (the source builder runs compilers). They
    /// inherit the same restrictions. Package runners never get this.
    pub allow_exec: bool,
}

impl Policy {
    /// Resolve symlinks (/tmp -> /private/tmp on macOS); the kernel-side
    /// rules match real paths only.
    pub(crate) fn canonical(&self) -> Result<Policy> {
        let canon = |paths: &[PathBuf]| -> Result<Vec<PathBuf>> {
            paths
                .iter()
                .map(|p| {
                    p.canonicalize()
                        .with_context(|| format!("sandbox path {}", p.display()))
                })
                .collect()
        };
        Ok(Policy {
            read_only: canon(&self.read_only)?,
            read_write: canon(&self.read_write)?,
            allow_exec: self.allow_exec,
        })
    }
}

/// Whether [`lockdown`] actually restricts anything on this OS.
pub fn is_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux", windows))
}

/// Restrict the current process. Irreversible.
pub fn lockdown(policy: &Policy) -> Result<()> {
    // Windows applied the policy when the process was created (and a
    // container can't resolve paths: canonicalize asks the mount manager)
    #[cfg(windows)]
    return imp(policy);
    #[cfg(not(windows))]
    {
        let policy = policy.canonical()?;
        imp(&policy)
    }
}

#[cfg(target_os = "macos")]
fn imp(policy: &Policy) -> Result<()> {
    macos::lockdown(policy)
}

#[cfg(target_os = "linux")]
fn imp(policy: &Policy) -> Result<()> {
    linux::lockdown(policy)
}

#[cfg(windows)]
fn imp(policy: &Policy) -> Result<()> {
    windows::lockdown(policy)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn imp(_policy: &Policy) -> Result<()> {
    anyhow::bail!("no sandbox on this platform yet")
}

/// Directories that hold shared libraries and fonts on this system, which
/// a runner may read after lockdown. With `exec`, also compilers and their
/// headers, for the builder.
pub fn system_read_paths(exec: bool) -> Vec<PathBuf> {
    let mut paths = base_read_paths();
    if exec {
        let tools: &[&str] = if cfg!(windows) {
            // compilers in Program Files are readable already
            &[]
        } else if cfg!(target_os = "macos") {
            &[
                "/usr/bin",
                "/bin",
                "/usr/include",
                "/Library/Developer",
                "/Applications/Xcode.app",
                "/private/var/db/xcode_select_link",
                // where xcrun looks up whether the Xcode license was accepted
                "/Library/Preferences/com.apple.dt.Xcode.plist",
                "/private/var/select",
                "/private/etc",
            ]
        } else {
            &["/bin", "/etc"]
        };
        paths.extend(
            tools
                .iter()
                .map(Path::new)
                .filter(|p| p.exists())
                .map(Path::to_path_buf),
        );
        #[cfg(target_os = "macos")]
        paths.extend(selected_xcode());
    }
    paths
}

/// The Xcode that `xcode-select` (or `DEVELOPER_DIR`) chose, which need
/// not be /Applications/Xcode.app (CI machines have `Xcode_26.6.app`).
#[cfg(target_os = "macos")]
fn selected_xcode() -> Option<PathBuf> {
    let dev = std::env::var_os("DEVELOPER_DIR")
        .map(PathBuf::from)
        .or_else(|| std::fs::read_link("/private/var/db/xcode_select_link").ok())?
        .canonicalize()
        .ok()?;
    // the whole .app: xcrun loads libraries from beside the Developer dir
    let app = dev
        .ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf);
    Some(app.unwrap_or(dev))
}

#[cfg(windows)]
fn base_read_paths() -> Vec<PathBuf> {
    // Windows, Program Files and the fonts are readable by every
    // AppContainer already; only the GTK/GStreamer DLLs need a grant
    windows::library_dirs()
}

#[cfg(not(windows))]
fn base_read_paths() -> Vec<PathBuf> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/System",
            "/usr/lib",
            "/usr/share",
            "/Library/Fonts",
            "/opt/homebrew",
            "/usr/local/lib",
            "/usr/local/share",
            "/usr/local/etc/fonts",
            "/private/var/db/timezone",
        ]
    } else {
        &[
            "/usr",
            "/lib",
            "/lib64",
            "/etc/fonts",
            "/etc/ld.so.cache",
            "/var/cache/fontconfig",
        ]
    };
    candidates
        .iter()
        .map(Path::new)
        .filter(|p| p.exists())
        .map(Path::to_path_buf)
        .collect()
}
