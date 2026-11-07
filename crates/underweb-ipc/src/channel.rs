use std::io::{IoSlice, IoSliceMut};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use nix::sys::socket::{
    AddressFamily, ControlMessage, ControlMessageOwned, MsgFlags, SockFlag, SockType, recvmsg,
    sendmsg, socketpair,
};
use serde::{Serialize, de::DeserializeOwned};

const MAX_MSG: usize = 1 << 20;

/// One end of a host/runner socket. `send` and `recv` may be used from
/// different threads.
pub struct Channel {
    fd: OwnedFd,
    send_lock: Mutex<()>,
}

impl Channel {
    /// A connected pair. Both ends are close-on-exec; use
    /// [`Channel::inheritable_fd`] on the end passed to a child.
    pub fn pair() -> Result<(Channel, Channel)> {
        let (a, b) = socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::empty(),
        )
        .context("socketpair")?;
        for fd in [&a, &b] {
            set_cloexec(fd.as_raw_fd(), true)?;
        }
        Ok((Channel::from_fd(a), Channel::from_fd(b)))
    }

    pub fn from_fd(fd: OwnedFd) -> Channel {
        Channel {
            fd,
            send_lock: Mutex::new(()),
        }
    }

    /// Take over an fd number inherited from the parent.
    ///
    /// # Safety
    /// `fd` must be an open socket owned by nobody else in this process.
    pub unsafe fn from_raw_fd(fd: RawFd) -> Result<Channel> {
        set_cloexec(fd, true)?;
        Ok(Channel::from_fd(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Clear close-on-exec so a spawned child inherits this end.
    pub fn inheritable_fd(&self) -> Result<RawFd> {
        set_cloexec(self.fd.as_raw_fd(), false)?;
        Ok(self.fd.as_raw_fd())
    }

    pub fn send<T: Serialize>(&self, msg: &T, fd: Option<RawFd>) -> Result<()> {
        let body = postcard::to_stdvec(msg)?;
        let len = (body.len() as u32).to_le_bytes();
        let _guard = self.send_lock.lock().unwrap();
        let iov = [IoSlice::new(&len), IoSlice::new(&body)];
        let fds;
        let cmsg: Vec<ControlMessage> = match fd {
            Some(f) => {
                fds = [f];
                vec![ControlMessage::ScmRights(&fds)]
            }
            None => vec![],
        };
        let mut sent = sendmsg::<()>(self.fd.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None)
            .context("sendmsg")?;
        // finish a short write (fd, if any, went with the first chunk)
        let total = len.len() + body.len();
        let all: Vec<u8> = [&len[..], &body[..]].concat();
        while sent < total {
            let n = nix::unistd::write(&self.fd, &all[sent..]).context("write")?;
            sent += n;
        }
        Ok(())
    }

    /// Blocks. Returns `Ok(None)` when the peer closed the socket.
    pub fn recv<T: DeserializeOwned>(&self) -> Result<Option<(T, Option<OwnedFd>)>> {
        let mut len = [0u8; 4];
        let mut got_fd = None;
        let mut filled = 0;
        while filled < len.len() {
            let mut cmsg_buf = nix::cmsg_space!([RawFd; 4]);
            let mut iov = [IoSliceMut::new(&mut len[filled..])];
            let msg = recvmsg::<()>(
                self.fd.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_buf),
                MsgFlags::empty(),
            )
            .context("recvmsg")?;
            if msg.bytes == 0 {
                if filled == 0 {
                    return Ok(None);
                }
                bail!("peer closed mid-message");
            }
            for c in msg.cmsgs()? {
                if let ControlMessageOwned::ScmRights(fds) = c {
                    for f in fds {
                        let owned = unsafe { OwnedFd::from_raw_fd(f) };
                        set_cloexec(f, true)?;
                        if got_fd.is_none() {
                            got_fd = Some(owned);
                        }
                    }
                }
            }
            filled += msg.bytes;
        }
        let n = u32::from_le_bytes(len) as usize;
        if n > MAX_MSG {
            bail!("message too large ({n} bytes)");
        }
        let mut body = vec![0u8; n];
        let mut filled = 0;
        while filled < n {
            let r = nix::unistd::read(&self.fd, &mut body[filled..]).context("read")?;
            if r == 0 {
                bail!("peer closed mid-message");
            }
            filled += r;
        }
        Ok(Some((postcard::from_bytes(&body)?, got_fd)))
    }
}

fn set_cloexec(fd: RawFd, on: bool) -> Result<()> {
    use nix::fcntl::{FcntlArg, FdFlag, fcntl};
    let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
    let flags = FdFlag::from_bits_truncate(fcntl(borrowed, FcntlArg::F_GETFD)?);
    let flags = if on {
        flags | FdFlag::FD_CLOEXEC
    } else {
        flags - FdFlag::FD_CLOEXEC
    };
    fcntl(borrowed, FcntlArg::F_SETFD(flags))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GuestMsg, HostMsg, Shm};

    #[test]
    fn round_trip_with_fd() {
        let (a, b) = Channel::pair().unwrap();
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
            Some(shm.raw_fd()),
        )
        .unwrap();
        let (msg, fd): (HostMsg, _) = b.recv().unwrap().unwrap();
        assert!(matches!(msg, HostMsg::Configure { width: 16, scale, .. } if scale == 2.0));
        let mapped = Shm::from_fd(fd.unwrap(), 4096).unwrap();
        assert_eq!(mapped.as_mut_slice()[10], 42);

        b.send(&GuestMsg::Title("hi".into()), None).unwrap();
        let (msg, fd): (GuestMsg, _) = a.recv().unwrap().unwrap();
        assert!(fd.is_none());
        assert!(matches!(msg, GuestMsg::Title(t) if t == "hi"));
    }

    #[test]
    fn eof_is_none() {
        let (a, b) = Channel::pair().unwrap();
        drop(a);
        assert!(b.recv::<HostMsg>().unwrap().is_none());
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
