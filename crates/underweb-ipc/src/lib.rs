//! IPC between the host (browser) and a runner (one sandboxed process per
//! package).
//!
//! Transport: created by the host and inherited by the runner, so the
//! runner never needs to look anything up after it is sandboxed. Messages
//! are length-prefixed postcard, and each can carry one OS handle (a
//! shared-memory region):
//!
//! - Unix: a stream socket from `socketpair`; handles are fds sent as
//!   `SCM_RIGHTS`.
//! - Windows: two anonymous pipes; handles are duplicated across with
//!   `DuplicateHandle` (see `channel_windows.rs`).
//!
//! Frames: the host allocates one shared-memory region holding two
//! cairo ARGB32 buffers. The runner draws into a free buffer, sends
//! [`GuestMsg::Frame`], and the host copies it out and sends
//! [`HostMsg::Release`].

mod channel;
#[cfg(unix)]
mod channel_unix;
#[cfg(windows)]
mod channel_windows;
mod media;
pub mod scene;
mod shm;

pub use channel::{Channel, ChildEnd};
pub use media::{Broadcast, FromMedia, MAX_PIXELS, MediaCmd, MediaEvent, ToMedia, VideoMailbox};
pub use scene::{EditOp, LayoutDoc, Matrix, PathOp, Rgba, SceneDoc, ShapeDoc};
pub use shm::Shm;

/// An owned OS handle that can travel with a message: an fd on Unix, a
/// `HANDLE` on Windows.
#[cfg(unix)]
pub type OsHandle = std::os::fd::OwnedFd;
#[cfg(unix)]
pub type RawOsHandle = std::os::fd::RawFd;
#[cfg(windows)]
pub type OsHandle = std::os::windows::io::OwnedHandle;
#[cfg(windows)]
pub type RawOsHandle = std::os::windows::io::RawHandle;

use serde::{Deserialize, Serialize};

/// Bumped on any incompatible protocol change.
pub const PROTOCOL_VERSION: u32 = 2;

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
    /// Network: a reply or incoming data for a handle.
    Net(NetEvent),
    /// About media stream `id`; `Ready` carries the frame mailbox fd.
    Media {
        id: u32,
        event: MediaEvent,
    },
    /// Answer to `GuestMsg::AudioStart`: carries the shared-memory audio
    /// ring (`capacity` stereo frames) the browser plays from.
    AudioReady {
        capacity: u32,
    },
    /// The layout editor opened or closed. While it is open the page gets
    /// no pointer events and the runner sends [`GuestMsg::Scene`] whenever
    /// the scene changes.
    EditMode(bool),
    /// A change made in the layout editor.
    Edit(Box<EditOp>),
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
    /// Network request on `handle`. Handles are chosen by the runner
    /// (below 2^31) except for accepted connections, which the host
    /// numbers from 2^31 up.
    Net {
        handle: u32,
        op: NetOp,
    },
    /// The package wants to make sound.
    AudioStart,
    AudioStop,
    /// Open media stream `id`: a package file, an http(s) URL, `camera`,
    /// or `test:<pattern>`; optionally broadcast it.
    MediaOpen {
        id: u32,
        source: String,
        looping: bool,
        broadcast: Option<Broadcast>,
    },
    Media {
        id: u32,
        cmd: MediaCmd,
    },
    /// The scene, for the layout editor (edit mode only).
    Scene(SceneDoc),
}

/// What a package asks the host to do. The host checks each against the
/// package's declared and granted permissions; the runner itself can't
/// open sockets at all.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetOp {
    TcpConnect {
        host: String,
        port: u16,
    },
    TcpListen {
        port: u16,
    },
    /// Port 0: an ephemeral port, for sending.
    UdpOpen {
        port: u16,
    },
    /// Write to a TCP connection.
    Send {
        data: Vec<u8>,
    },
    UdpSend {
        host: String,
        port: u16,
        data: Vec<u8>,
    },
    Close,
    Fetch {
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetEventKind {
    /// TCP connected / listening / UDP socket open. `text`: address.
    Connected,
    /// Bytes arrived. For UDP, `text` is the sender.
    Data,
    Closed,
    /// `text` says why, including "permission denied".
    Error,
    /// A listener accepted `new_handle`; `text` is the peer.
    Accepted,
    /// A fetch finished: `status`, headers in `text` ("name: value" lines),
    /// body in `data`.
    Response,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetEvent {
    pub handle: u32,
    pub kind: NetEventKind,
    pub data: Vec<u8>,
    pub text: String,
    pub status: u32,
    pub new_handle: u32,
}

impl NetEvent {
    pub fn new(handle: u32, kind: NetEventKind) -> NetEvent {
        NetEvent {
            handle,
            kind,
            data: Vec::new(),
            text: String::new(),
            status: 0,
            new_handle: 0,
        }
    }

    pub fn error(handle: u32, text: impl Into<String>) -> NetEvent {
        NetEvent {
            text: text.into(),
            ..NetEvent::new(handle, NetEventKind::Error)
        }
    }
}

/// First handle number the host uses for accepted connections.
pub const HOST_HANDLE_BASE: u32 = 1 << 31;

/// Bytes per row for a cairo ARGB32 surface of `width` pixels.
pub fn argb32_stride(width: u32) -> u32 {
    // cairo requires 4-byte alignment; ARGB32 rows are already 4*width.
    width * 4
}
