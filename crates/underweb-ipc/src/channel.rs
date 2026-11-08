//! A message channel between two processes that can also carry one OS
//! handle (a shared-memory region) per message.

#[cfg(unix)]
pub use crate::channel_unix::Channel;
#[cfg(windows)]
pub use crate::channel_windows::Channel;

/// How a child process finds its end of a [`Channel`]: pass `arg` as its
/// `--ipc` value and let it inherit `handles`.
#[derive(Debug)]
pub struct ChildEnd {
    pub arg: String,
    pub handles: Vec<crate::RawOsHandle>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GuestMsg, HostMsg, Shm};

    #[test]
    fn round_trip_with_handle() {
        let (a, b) = Channel::pair().unwrap();
        // the host side knows its peer process (here: itself)
        #[cfg(windows)]
        a.set_peer(crate::channel_windows::current_process().unwrap());
        let shm = Shm::create(4096).unwrap();
        shm.as_mut_slice()[10] = 42;
        a.send(
            &HostMsg::Configure {
                generation: 1,
                width: 16,
                height: 16,
                stride: 64,
                scale: 2.0,
            },
            Some(shm.raw_handle()),
        )
        .unwrap();
        let (msg, h): (HostMsg, _) = b.recv().unwrap().unwrap();
        assert!(matches!(msg, HostMsg::Configure { width: 16, scale, .. } if scale == 2.0));
        let mapped = Shm::from_handle(h.unwrap(), 4096).unwrap();
        assert_eq!(mapped.as_mut_slice()[10], 42);

        b.send(&GuestMsg::Title("hi".into()), None).unwrap();
        let (msg, h): (GuestMsg, _) = a.recv().unwrap().unwrap();
        assert!(h.is_none());
        assert!(matches!(msg, GuestMsg::Title(t) if t == "hi"));
    }

    #[test]
    fn eof_is_none() {
        let (a, b) = Channel::pair().unwrap();
        drop(a);
        assert!(b.recv::<HostMsg>().unwrap().is_none());
    }

    #[test]
    fn readable_times_out_then_sees_data() {
        let (a, b) = Channel::pair().unwrap();
        assert!(!b.readable(20).unwrap());
        a.send(&GuestMsg::Title("x".into()), None).unwrap();
        assert!(b.readable(1000).unwrap());
        assert!(b.recv::<GuestMsg>().unwrap().is_some());
    }

    #[test]
    fn large_message() {
        let (a, b) = Channel::pair().unwrap();
        let big = "x".repeat(300_000);
        let t = std::thread::spawn(move || a.send(&GuestMsg::Log(big), None).unwrap());
        let (msg, _): (GuestMsg, _) = b.recv().unwrap().unwrap();
        t.join().unwrap();
        assert!(matches!(msg, GuestMsg::Log(s) if s.len() == 300_000));
    }
}
