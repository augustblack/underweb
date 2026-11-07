//! The Underweb engine, embeddable in any app.
//!
//! [`Guest`] runs one package in its own sandboxed `underweb-runner`
//! process and turns its messages into [`GuestEvent`]s. The embedding app
//! decides how to show frames (the browser uploads them to the GPU) and
//! forwards input.
//!
//! Later phases add the package manager, the permission broker and the
//! trust engine here.

mod guest;
mod paths;

pub use guest::{Frame, Guest, GuestEvent, SpawnOptions};
pub use paths::{data_dir, runner_path};
pub use underweb_abi::{UW_POINTER_MOVE, UW_POINTER_PRESS, UW_POINTER_RELEASE};
pub use underweb_pkg::{Kind, Package};
