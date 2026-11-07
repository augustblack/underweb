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
//! ```
//!
//! Everything the package can do goes through the function table in
//! [`UwHost`]. Objects are referred to by `u32` handles; 0 is "none".
//! `include/underweb.h` is generated from this file (`cargo xtask header`).
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_void};

/// Bumped on any incompatible change to [`UwHost`] or the exports.
pub const UW_ABI_VERSION: u32 = 1;

pub const UW_POINTER_MOVE: u32 = 0;
pub const UW_POINTER_PRESS: u32 = 1;
pub const UW_POINTER_RELEASE: u32 = 2;

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
}

pub type UwAbiVersionFn = unsafe extern "C" fn() -> u32;
pub type UwActivateFn = unsafe extern "C" fn(host: *const UwHost);
pub type UwUpdateFn = unsafe extern "C" fn(host: *const UwHost, dt: f64);
pub type UwPointerFn = unsafe extern "C" fn(host: *const UwHost, ev: *const UwPointerEvent);
/// `cr` is a `cairo_t*`; the package may draw anything on it.
pub type UwDrawFn = unsafe extern "C" fn(host: *const UwHost, cr: *mut c_void, w: f64, h: f64);
pub type UwDeactivateFn = unsafe extern "C" fn(host: *const UwHost);
