//! Embed the Underweb engine in a C program (or anything with a C FFI).
//!
//! ```c
//! char err[256];
//! UwEngine *e = uw_engine_open("uw://example.org/hello-ts", true, err, sizeof err);
//! uw_engine_configure(e, 800, 600, 1.0);
//! UwEngineEvent ev;
//! while (uw_engine_next_event(e, 1000, &ev)) {
//!     if (ev.kind == UW_EVENT_FRAME) { /* ev.rgba: ev.width * ev.height * 4 bytes */ }
//! }
//! uw_engine_close(e);
//! ```
//!
//! The runner binary is found next to the program, or at
//! `$UNDERWEB_RUNNER`. Calls on one engine must come from one thread at a
//! time.

use std::ffi::{CStr, CString, c_char};
use std::time::Duration;

use underweb_core::{Guest, GuestEvent, SpawnOptions, Store};

pub const UW_EVENT_STARTED: u32 = 1;
pub const UW_EVENT_TITLE: u32 = 2;
pub const UW_EVENT_FRAME: u32 = 3;
pub const UW_EVENT_ANIMATING: u32 = 4;
pub const UW_EVENT_LOG: u32 = 5;
pub const UW_EVENT_ERROR: u32 = 6;
pub const UW_EVENT_EXITED: u32 = 7;

/// One event. Pointers stay valid until the next call on the engine.
#[repr(C)]
pub struct UwEngineEvent {
    /// `UW_EVENT_*`.
    pub kind: u32,
    /// STARTED: sandboxed. ANIMATING: on.
    pub flag: bool,
    /// FRAME: size in pixels; `rgba` is premultiplied RGBA, rows packed.
    pub width: u32,
    pub height: u32,
    pub rgba: *const u8,
    /// TITLE, LOG, ERROR: the text.
    pub text: *const c_char,
}

/// An open package.
pub struct UwEngine {
    guest: Guest,
    frame: Vec<u8>,
    text: CString,
}

fn write_err(err: *mut c_char, len: usize, msg: &str) {
    if err.is_null() || len == 0 {
        return;
    }
    let bytes = msg.as_bytes();
    let n = bytes.len().min(len - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), err as *mut u8, n);
        *err.add(n) = 0;
    }
}

/// Resolve an address (downloading and building if needed; this can take
/// a while) and start its runner. Returns NULL on failure, with a message
/// in `err`.
///
/// # Safety
/// `address` must be a NUL-terminated string; `err` NULL or `err_len`
/// writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uw_engine_open(
    address: *const c_char,
    sandbox: bool,
    err: *mut c_char,
    err_len: usize,
) -> *mut UwEngine {
    let run = || -> Result<UwEngine, String> {
        if address.is_null() {
            return Err("address is NULL".into());
        }
        let address = unsafe { CStr::from_ptr(address) }
            .to_str()
            .map_err(|e| e.to_string())?;
        let mut store = Store::new(Store::default_root());
        store.sandbox = sandbox;
        let resolved = store.open(address, &|_| {}).map_err(|e| format!("{e:#}"))?;
        let guest = Guest::spawn(
            &resolved.dir,
            SpawnOptions {
                sandbox,
                params: resolved.params,
                ..Default::default()
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        Ok(UwEngine {
            guest,
            frame: Vec::new(),
            text: CString::default(),
        })
    };
    match run() {
        Ok(e) => Box::into_raw(Box::new(e)),
        Err(msg) => {
            write_err(err, err_len, &msg);
            std::ptr::null_mut()
        }
    }
}

/// Size the page in physical pixels. Returns 0 on success.
///
/// # Safety
/// `e` must come from `uw_engine_open` and not be closed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uw_engine_configure(
    e: *mut UwEngine,
    width: u32,
    height: u32,
    scale: f64,
) -> i32 {
    let Some(e) = (unsafe { e.as_mut() }) else {
        return -1;
    };
    if e.guest.configure(width, height, scale).is_ok() {
        0
    } else {
        -1
    }
}

/// Pointer event in logical pixels; `kind` is `UW_POINTER_*` from
/// underweb.h. Returns 0 on success.
///
/// # Safety
/// As for `uw_engine_configure`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uw_engine_pointer(
    e: *mut UwEngine,
    kind: u32,
    button: u32,
    x: f64,
    y: f64,
) -> i32 {
    let Some(e) = (unsafe { e.as_mut() }) else {
        return -1;
    };
    if e.guest.pointer(kind, button, x, y).is_ok() {
        0
    } else {
        -1
    }
}

/// Advance an animating page by `dt` seconds. Returns 0 on success.
///
/// # Safety
/// As for `uw_engine_configure`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uw_engine_tick(e: *mut UwEngine, dt: f64) -> i32 {
    let Some(e) = (unsafe { e.as_mut() }) else {
        return -1;
    };
    if e.guest.tick(dt).is_ok() { 0 } else { -1 }
}

/// Wait up to `timeout_ms` for an event. Returns false on timeout.
///
/// # Safety
/// As for `uw_engine_configure`; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uw_engine_next_event(
    e: *mut UwEngine,
    timeout_ms: u32,
    out: *mut UwEngineEvent,
) -> bool {
    let (Some(e), Some(out)) = (unsafe { e.as_mut() }, unsafe { out.as_mut() }) else {
        return false;
    };
    let Some(ev) = e.guest.next_event(Duration::from_millis(timeout_ms as u64)) else {
        return false;
    };
    *out = UwEngineEvent {
        kind: 0,
        flag: false,
        width: 0,
        height: 0,
        rgba: std::ptr::null(),
        text: std::ptr::null(),
    };
    let mut text = |s: String| {
        e.text = CString::new(s.replace('\0', "")).unwrap_or_default();
        e.text.as_ptr()
    };
    match ev {
        GuestEvent::Started { sandboxed } => {
            out.kind = UW_EVENT_STARTED;
            out.flag = sandboxed;
        }
        GuestEvent::Title(t) => {
            out.kind = UW_EVENT_TITLE;
            out.text = text(t);
        }
        GuestEvent::Log(l) => {
            out.kind = UW_EVENT_LOG;
            out.text = text(l);
        }
        GuestEvent::Error(m) => {
            out.kind = UW_EVENT_ERROR;
            out.text = text(m);
        }
        GuestEvent::Animating(on) => {
            out.kind = UW_EVENT_ANIMATING;
            out.flag = on;
        }
        GuestEvent::Exited(_) => out.kind = UW_EVENT_EXITED,
        GuestEvent::Frame(f) => {
            out.kind = UW_EVENT_FRAME;
            out.width = f.width;
            out.height = f.height;
            e.frame = f.rgba;
            out.rgba = e.frame.as_ptr();
        }
    }
    true
}

/// Stop the package and free the engine.
///
/// # Safety
/// `e` must come from `uw_engine_open`; it is invalid afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn uw_engine_close(e: *mut UwEngine) {
    if !e.is_null() {
        let e = unsafe { Box::from_raw(e) };
        e.guest.shutdown();
    }
}
