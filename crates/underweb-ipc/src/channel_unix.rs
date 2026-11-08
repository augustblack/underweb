use std::io::{IoSlice, IoSliceMut};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use nix::sys::socket::{
    AddressFamily, ControlMessage, ControlMessageOwned, MsgFlags, SockFlag, SockType, recvmsg,
    sendmsg, socketpair,
};
use serde::{Serialize, de::DeserializeOwned};

/// Fetched bodies travel in one message.
const MAX_MSG: usize = 64 << 20;

/// One end of a host/runner socket. `send` and `recv` may be used from
/// different threads.
pub struct Channel {
    fd: OwnedFd,
    send_lock: Mutex<()>,
}

impl Channel {
    /// A connected pair. Both ends are close-on-exec; use
    /// [`Channel::child_end`] on the end passed to a child.
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

    /// Take over the end a parent passed with [`Channel::child_end`].
    ///
    /// # Safety
    /// `arg` must name an open socket owned by nobody else in this process.
    pub unsafe fn from_arg(arg: &str) -> Result<Channel> {
        let fd: RawFd = arg.parse().context("bad --ipc value")?;
        set_cloexec(fd, true)?;
        Ok(Channel::from_fd(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Make this end inheritable by the next child started, and describe
    /// it for the child's `--ipc` argument.
    pub fn child_end(&self) -> Result<crate::ChildEnd> {
        let fd = self.fd.as_raw_fd();
        set_cloexec(fd, false)?;
        Ok(crate::ChildEnd {
            arg: fd.to_string(),
            handles: vec![fd],
        })
    }

    /// Wait up to `timeout_ms` for something to read.
    pub fn readable(&self, timeout_ms: u16) -> Result<bool> {
        use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
        use std::os::fd::AsFd;
        let mut fds = [PollFd::new(self.fd.as_fd(), PollFlags::POLLIN)];
        let n = poll(&mut fds, PollTimeout::from(timeout_ms)).context("poll")?;
        Ok(n > 0)
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
