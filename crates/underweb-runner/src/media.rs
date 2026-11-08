//! Video streams, opened by the page and run by the browser (one
//! sandboxed media process each). Frames arrive in a mailbox the scene
//! draws from; events go to the page.

use std::cell::Cell;

use underweb_ipc::{Broadcast, Channel, GuestMsg, MediaCmd};

pub struct MediaCtl {
    chan: *const Channel,
    next: Cell<u32>,
}

impl MediaCtl {
    pub fn new(chan: &Channel) -> MediaCtl {
        MediaCtl {
            chan,
            next: Cell::new(1),
        }
    }

    fn send(&self, m: GuestMsg) {
        let _ = unsafe { &*self.chan }.send(&m, None);
    }

    pub fn open(&self, source: &str, looping: bool, broadcast: Option<Broadcast>) -> u32 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.send(GuestMsg::MediaOpen {
            id,
            source: source.to_string(),
            looping,
            broadcast,
        });
        id
    }

    pub fn cmd(&self, id: u32, cmd: MediaCmd) {
        self.send(GuestMsg::Media { id, cmd });
    }
}
