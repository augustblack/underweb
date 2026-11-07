//! Network requests from the package, sent to the host's broker. The
//! runner can't open sockets (the sandbox forbids it); it only forwards
//! requests and delivers the replies to the page.

use std::cell::Cell;

use underweb_ipc::{Channel, GuestMsg, NetOp};

pub struct Net {
    chan: *const Channel,
    next: Cell<u32>,
}

impl Net {
    /// `chan` must outlive this value (it lives for the whole run).
    pub fn new(chan: &Channel) -> Net {
        Net {
            chan,
            next: Cell::new(1),
        }
    }

    /// Start a request; its events will carry the returned handle.
    pub fn open(&self, op: NetOp) -> u32 {
        let handle = self.next.get();
        self.next.set((handle + 1) % underweb_ipc::HOST_HANDLE_BASE);
        self.on(handle, op);
        handle
    }

    /// An operation on an existing handle.
    pub fn on(&self, handle: u32, op: NetOp) {
        let chan = unsafe { &*self.chan };
        let _ = chan.send(&GuestMsg::Net { handle, op }, None);
    }
}

/// Parse "Name: value" lines.
pub fn parse_headers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}
