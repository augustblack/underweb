//! The C ABI between the runner and native packages.
//!
//! A native package is a shared library that exports:
//!
//! ```c
//! uint32_t uw_abi_version(void);                         // must return UW_ABI_VERSION
//! void uw_activate(const UwHost *host);                  // build the page
//! void uw_update(const UwHost *host, double dt);         // every frame while animating
//! void uw_pointer(const UwHost *host, const UwPointerEvent *ev);  // optional
//! void uw_draw(const UwHost *host, cairo_t *cr, double w, double h); // optional, raw cairo
//! void uw_deactivate(const UwHost *host);                // optional
//! void uw_net(const UwHost *host, const UwNetEvent *ev); // optional, ABI 2
//! ```
//!
//! Everything the package can do goes through the function table in
//! [`UwHost`]. Objects are referred to by `u32` handles; 0 is "none".
//! `include/underweb.h` is generated from this file (`cargo xtask header`).
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_void};

/// The ABI this header describes. Version 2 appended network and storage
/// functions to [`UwHost`]; packages built for version 1 still load.
pub const UW_ABI_VERSION: u32 = 2;
/// The oldest ABI the runner still accepts.
pub const UW_ABI_MIN_VERSION: u32 = 1;

pub const UW_POINTER_MOVE: u32 = 0;
pub const UW_POINTER_PRESS: u32 = 1;
pub const UW_POINTER_RELEASE: u32 = 2;

/// [`UwNetEvent::kind`]: a TCP connection or listener is up, or a UDP
/// socket is open; `text` is the address.
pub const UW_NET_CONNECTED: u32 = 0;
/// Bytes arrived in `data`/`len`. For UDP, `text` is the sender.
pub const UW_NET_DATA: u32 = 1;
pub const UW_NET_CLOSED: u32 = 2;
/// The request failed or was refused; `text` says why.
pub const UW_NET_ERROR: u32 = 3;
/// A listener accepted a connection, `new_handle`; `text` is the peer.
pub const UW_NET_ACCEPTED: u32 = 4;
/// A fetch finished: `status`, the body in `data`/`len`, and the headers
/// in `text` as "name: value" lines.
pub const UW_NET_RESPONSE: u32 = 5;

/// A network event for a handle returned by one of the `net_*` or `fetch`
/// functions. Pointers are valid only during the `uw_net` call.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UwNetEvent {
    pub kind: u32,
    pub handle: u32,
    pub new_handle: u32,
    pub status: u32,
    pub data: *const u8,
    pub len: usize,
    pub text: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UwPointerEvent {
    /// `UW_POINTER_MOVE`, `UW_POINTER_PRESS` or `UW_POINTER_RELEASE`.
    pub kind: u32,
    /// Mouse button, 1 = primary. 0 for moves.
    pub button: u32,
    /// Position in logical pixels.
    pub x: f64,
    pub y: f64,
    /// Topmost shape under the pointer, 0 if none.
    pub shape: u32,
}

/// The function table the runner hands to a native package.
/// `ctx` must be passed back as the first argument of every call.
#[repr(C)]
pub struct UwHost {
    pub abi_version: u32,
    pub ctx: *mut c_void,

    // window
    pub set_title: extern "C" fn(ctx: *mut c_void, title: *const c_char),
    pub add_layout: extern "C" fn(ctx: *mut c_void, layout: u32),
    /// Ask for `uw_update` every frame (animation) or only on events.
    pub set_animating: extern "C" fn(ctx: *mut c_void, on: bool),
    pub request_redraw: extern "C" fn(ctx: *mut c_void),
    pub width: extern "C" fn(ctx: *mut c_void) -> f64,
    pub height: extern "C" fn(ctx: *mut c_void) -> f64,
    /// Value of a `?key=value` parameter from the package URL, or NULL.
    /// The string stays valid until the next call to `param`.
    pub param: extern "C" fn(ctx: *mut c_void, key: *const c_char) -> *const c_char,
    pub log: extern "C" fn(ctx: *mut c_void, message: *const c_char),

    // layouts
    pub layout_new: extern "C" fn(ctx: *mut c_void) -> u32,
    pub layout_append: extern "C" fn(ctx: *mut c_void, layout: u32, shape: u32),

    // shapes
    pub shape_new: extern "C" fn(ctx: *mut c_void) -> u32,
    pub shape_clear_path: extern "C" fn(ctx: *mut c_void, shape: u32),
    pub shape_rectangle:
        extern "C" fn(ctx: *mut c_void, shape: u32, x: f64, y: f64, w: f64, h: f64),
    pub shape_move_to: extern "C" fn(ctx: *mut c_void, shape: u32, x: f64, y: f64),
    pub shape_line_to: extern "C" fn(ctx: *mut c_void, shape: u32, x: f64, y: f64),
    pub shape_curve_to: extern "C" fn(
        ctx: *mut c_void,
        shape: u32,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x3: f64,
        y3: f64,
    ),
    pub shape_close_path: extern "C" fn(ctx: *mut c_void, shape: u32),
    pub shape_set_fill_color:
        extern "C" fn(ctx: *mut c_void, shape: u32, r: f64, g: f64, b: f64, a: f64),
    pub shape_set_stroke_color:
        extern "C" fn(ctx: *mut c_void, shape: u32, r: f64, g: f64, b: f64, a: f64),
    pub shape_set_stroke_width: extern "C" fn(ctx: *mut c_void, shape: u32, width: f64),
    pub shape_set_text: extern "C" fn(ctx: *mut c_void, shape: u32, text: *const c_char),
    pub shape_set_text_color:
        extern "C" fn(ctx: *mut c_void, shape: u32, r: f64, g: f64, b: f64, a: f64),
    /// Pango font description, e.g. "Sans Bold 14".
    pub shape_set_font: extern "C" fn(ctx: *mut c_void, shape: u32, font: *const c_char),

    // ---- ABI 2: storage and network ----
    /// The package's private directory, the only place it may write.
    pub data_dir: extern "C" fn(ctx: *mut c_void) -> *const c_char,
    /// Network requests return a handle at once; the outcome arrives later
    /// as `uw_net` events. Each needs a permission declared in
    /// underweb.toml and granted by the user.
    pub net_tcp_connect: extern "C" fn(ctx: *mut c_void, host: *const c_char, port: u16) -> u32,
    pub net_tcp_listen: extern "C" fn(ctx: *mut c_void, port: u16) -> u32,
    /// Port 0 opens an ephemeral port for sending.
    pub net_udp_open: extern "C" fn(ctx: *mut c_void, port: u16) -> u32,
    pub net_send: extern "C" fn(ctx: *mut c_void, handle: u32, data: *const u8, len: usize),
    pub net_udp_send: extern "C" fn(
        ctx: *mut c_void,
        handle: u32,
        host: *const c_char,
        port: u16,
        data: *const u8,
        len: usize,
    ),
    pub net_close: extern "C" fn(ctx: *mut c_void, handle: u32),
    /// `headers`: "Name: value" lines separated by '\n', or NULL.
    pub fetch: extern "C" fn(
        ctx: *mut c_void,
        method: *const c_char,
        url: *const c_char,
        headers: *const c_char,
        body: *const u8,
        len: usize,
    ) -> u32,
}

pub type UwAbiVersionFn = unsafe extern "C" fn() -> u32;
pub type UwActivateFn = unsafe extern "C" fn(host: *const UwHost);
pub type UwUpdateFn = unsafe extern "C" fn(host: *const UwHost, dt: f64);
pub type UwPointerFn = unsafe extern "C" fn(host: *const UwHost, ev: *const UwPointerEvent);
/// `cr` is a `cairo_t*`; the package may draw anything on it.
pub type UwDrawFn = unsafe extern "C" fn(host: *const UwHost, cr: *mut c_void, w: f64, h: f64);
pub type UwDeactivateFn = unsafe extern "C" fn(host: *const UwHost);
pub type UwNetFn = unsafe extern "C" fn(host: *const UwHost, ev: *const UwNetEvent);
