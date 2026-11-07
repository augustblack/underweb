//! IPC between the host (browser) and a runner (one sandboxed process per
//! package).
//!
//! Transport: a Unix stream socket created by the host with `socketpair` and
//! inherited by the runner, so the runner never needs to look anything up
//! after it is sandboxed. Messages are length-prefixed postcard. File
//! descriptors (shared-memory frames now, sockets later) travel with a
//! message as `SCM_RIGHTS`.
//!
//! Frames: the host allocates one shared-memory region holding two
//! cairo ARGB32 buffers. The runner draws into a free buffer, sends
//! [`GuestMsg::Frame`], and the host copies it out and sends
//! [`HostMsg::Release`].

mod channel;
mod shm;

pub use channel::Channel;
pub use shm::Shm;

use serde::{Deserialize, Serialize};

/// Bumped on any incompatible protocol change.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HostMsg {
    /// (Re)size the page. Carries the shared-memory fd for the two frame
    /// buffers, each `stride * height` bytes, in physical pixels.
    /// `generation` increases with every Configure; frames carry it so the
    /// host can drop frames drawn into an old buffer.
    Configure {
        generation: u32,
        width: u32,
        height: u32,
        stride: u32,
        scale: f64,
    },
    /// One display frame passed; `dt` in seconds. Sent only while animating.
    Tick {
        dt: f64,
    },
    /// Pointer in logical pixels. kind/button as in `underweb_abi`.
    Pointer {
        kind: u32,
        button: u32,
        x: f64,
        y: f64,
    },
    /// The host is done with a frame buffer.
    Release {
        generation: u32,
        index: u32,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GuestMsg {
    /// First message: the runner is up and sandboxed (or not).
    Hello {
        protocol: u32,
        sandboxed: bool,
    },
    Title(String),
    /// Buffer `index` holds a finished frame.
    Frame {
        generation: u32,
        index: u32,
    },
    Animating(bool),
    Log(String),
    /// Fatal: the package failed to load or crashed in a callback.
    Error(String),
}

/// Bytes per row for a cairo ARGB32 surface of `width` pixels.
pub fn argb32_stride(width: u32) -> u32 {
    // cairo requires 4-byte alignment; ARGB32 rows are already 4*width.
    width * 4
}
