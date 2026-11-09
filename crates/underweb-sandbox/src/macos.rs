use std::ffi::{CStr, CString, c_char, c_int};
use std::fmt::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::{Policy, system_read_paths};

unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

fn quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn profile(policy: &Policy) -> String {
    let mut p = String::from(
        r#"(version 1)
(deny default)
(allow process-info* (target self))
(allow signal (target self))
(allow sysctl-read)
(allow file-read* file-write-data (literal "/dev/null"))
(allow file-read* (literal "/dev/urandom") (literal "/dev/random"))
(allow file-read-metadata (literal "/") (literal "/private") (literal "/var") (literal "/etc")
       (literal "/tmp") (literal "/Users"))
; fonts: CoreText talks to these services
(allow mach-lookup
       (global-name "com.apple.fonts")
       (global-name "com.apple.FontObjectsServer")
       (global-name "com.apple.system.opendirectoryd.libinfo")
       (global-name "com.apple.logd")
       (global-name "com.apple.system.logger"))
(allow user-preference-read)
; anonymous shared memory for frames (named /uw.*, unlinked at once)
(allow ipc-posix-shm-read* ipc-posix-shm-write-create ipc-posix-shm-write-data ipc-posix-shm-write-unlink
       (ipc-posix-name-prefix "/uw."))
"#,
    );
    if policy.allow_exec {
        // freshly started programs need the system's baseline (dyld,
        // libSystem init, notifyd...): Apple's own system.sb
        p.push_str("(import \"system.sb\")\n");
        p.push_str("(allow process-exec* process-fork)\n(allow file-read-metadata)\n");
        p.push_str(
            "(allow file-read* file-write* (literal \"/dev/tty\") (regex #\"^/dev/fd/\"))\n",
        );
    }
    // policy.camera needs nothing here: the media service opens the
    // camera before it locks down (macOS loads camera drivers only into
    // unsandboxed or camera-entitled processes), and capture then runs on
    // the connections it already has
    let sys = system_read_paths(policy.allow_exec);
    let _ = writeln!(p, "(allow file-read* file-map-executable");
    for d in sys
        .iter()
        .chain(&policy.read_only)
        .chain(&policy.read_write)
    {
        let _ = writeln!(p, "  (subpath {})", quote(d));
    }
    let _ = writeln!(p, ")");
    if !policy.read_write.is_empty() {
        let _ = writeln!(p, "(allow file-write*");
        for d in &policy.read_write {
            let _ = writeln!(p, "  (subpath {})", quote(d));
        }
        let _ = writeln!(p, ")");
    }
    p
}

pub fn lockdown(policy: &Policy) -> Result<()> {
    let profile = CString::new(profile(policy))?;
    let mut err: *mut c_char = std::ptr::null_mut();
    let rc = unsafe { sandbox_init(profile.as_ptr(), 0, &mut err) };
    if rc != 0 {
        let msg = if err.is_null() {
            "unknown error".to_string()
        } else {
            let m = unsafe { CStr::from_ptr(err) }
                .to_string_lossy()
                .into_owned();
            unsafe { sandbox_free_error(err) };
            m
        };
        bail!("sandbox_init failed: {msg}");
    }
    Ok(())
}
