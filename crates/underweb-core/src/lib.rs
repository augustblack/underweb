//! The Underweb engine, embeddable in any app.
//!
//! [`Guest`] runs one package in its own sandboxed `underweb-runner`
//! process and turns its messages into [`GuestEvent`]s. The embedding app
//! decides how to show frames (the browser uploads them to the GPU) and
//! forwards input.
//!
//! [`Store`] turns addresses into package directories: local paths, or
//! `uw://` packages downloaded, verified, cached and if needed built from
//! source in a sandbox.
//!
//! Later phases add the permission broker and the trust engine here.

mod guest;
mod paths;
mod store;

pub use guest::{Frame, Guest, GuestEvent, SpawnOptions};
pub use paths::{data_dir, runner_path};
pub use store::{Origin, Resolved, Store};
pub use underweb_abi::{UW_POINTER_MOVE, UW_POINTER_PRESS, UW_POINTER_RELEASE};
pub use underweb_pkg::{Kind, Package};
