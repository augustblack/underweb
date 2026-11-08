//! Windows transport: two anonymous pipes, one per direction.
//!
//! Handles cannot ride along with pipe data the way `SCM_RIGHTS` fds do
//! on Unix, so each message header says where its handle (if any) is:
//!
//! - `IN_RECEIVER`: the sender knew the receiving process (the host, which
//!   started it) and already duplicated the handle into it. The value is
//!   valid in the receiver.
//! - `IN_SENDER`: a child sending to the host. The value is valid in the
//!   sender, and the host pulls it across with `DuplicateHandle`. The
//!   sender keeps its handle open (it still uses the region).
//!
//! A reader thread per channel turns the blocking pipe into something
//! [`Channel::readable`] can wait on with a timeout. It starts on the
//! first read, so the parent's copy of a child's end (never read, only
//! dropped after the spawn) really closes.

use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Fetched bodies travel in one message.
const MAX_MSG: usize = 64 << 20;

const NO_HANDLE: u8 = 0;
const IN_RECEIVER: u8 = 1;
const IN_SENDER: u8 = 2;

/// One raw message off the pipe: handle placement, handle value, body.
/// `None` means the peer closed its end.
type Raw = Option<(u8, u64, Vec<u8>)>;

/// One end of a host/runner channel. `send` and `recv` may be used from
/// different threads.
pub struct Channel {
    tx: Mutex<File>,
    /// Kept so [`Channel::child_end`] can name it.
    tx_raw: RawHandle,
    rx_raw: RawHandle,
    incoming: Mutex<Incoming>,
    /// The process at the other end, when this side started it.
    peer: OnceLock<OwnedHandle>,
}

struct Incoming {
    /// The pipe, until the reader thread takes it.
    file: Option<File>,
    rx: Option<Receiver<Result<Raw>>>,
    /// A message [`Channel::readable`] took off the queue early.
    peeked: Option<Result<Raw>>,
}

// RawHandle is a pointer; these are only kept for naming the handles.
unsafe impl Send for Channel {}
unsafe impl Sync for Channel {}

fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let (mut r, mut w): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
    if unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), 0) } == 0 {
        return Err(std::io::Error::last_os_error()).context("CreatePipe");
    }
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(r),
            OwnedHandle::from_raw_handle(w),
        )
    })
}

impl Channel {
    /// A connected pair. Neither end is inheritable; use
    /// [`Channel::child_end`] on the end passed to a child.
    pub fn pair() -> Result<(Channel, Channel)> {
        let (r1, w1) = pipe()?;
        let (r2, w2) = pipe()?;
        Ok((Channel::new(r1, w2), Channel::new(r2, w1)))
    }

    fn new(rx: OwnedHandle, tx: OwnedHandle) -> Channel {
        let (rx_raw, tx_raw) = (rx.as_raw_handle(), tx.as_raw_handle());
        Channel {
            tx: Mutex::new(File::from(tx)),
            tx_raw,
            rx_raw,
            incoming: Mutex::new(Incoming {
                file: Some(File::from(rx)),
                rx: None,
                peeked: None,
            }),
            peer: OnceLock::new(),
        }
    }

    /// Take over the end a parent passed with [`Channel::child_end`].
    ///
    /// # Safety
    /// `arg` must name two open pipe handles owned by nobody else in this
    /// process.
    pub unsafe fn from_arg(arg: &str) -> Result<Channel> {
        let (r, w) = arg.split_once(',').context("bad --ipc value")?;
        let r: usize = r.parse().context("bad --ipc value")?;
        let w: usize = w.parse().context("bad --ipc value")?;
        let (r, w) = unsafe {
            (
                OwnedHandle::from_raw_handle(r as RawHandle),
                OwnedHandle::from_raw_handle(w as RawHandle),
            )
        };
        for h in [&r, &w] {
            set_inherit(h.as_raw_handle(), false)?;
        }
        Ok(Channel::new(r, w))
    }

    /// Make this end inheritable and describe it for the child's `--ipc`
    /// argument. The spawner must list `handles` explicitly
    /// (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`) so nothing else leaks.
    pub fn child_end(&self) -> Result<crate::ChildEnd> {
        for h in [self.rx_raw, self.tx_raw] {
            set_inherit(h, true)?;
        }
        Ok(crate::ChildEnd {
            arg: format!("{},{}", self.rx_raw as usize, self.tx_raw as usize),
            handles: vec![self.rx_raw, self.tx_raw],
        })
    }

    /// Name the process at the other end (the host started it). Handles
    /// sent to it are duplicated straight into it; handles it sends are
    /// pulled out of it.
    pub fn set_peer(&self, process: OwnedHandle) {
        let _ = self.peer.set(process);
    }

    /// Wait up to `timeout_ms` for something to read.
    pub fn readable(&self, timeout_ms: u16) -> Result<bool> {
        let mut inc = self.incoming.lock().unwrap();
        if inc.peeked.is_some() {
            return Ok(true);
        }
        match inc
            .reader()?
            .recv_timeout(Duration::from_millis(timeout_ms as u64))
        {
            Ok(m) => {
                inc.peeked = Some(m);
                Ok(true)
            }
            Err(RecvTimeoutError::Timeout) => Ok(false),
            // reader gone: let recv report it
            Err(RecvTimeoutError::Disconnected) => Ok(true),
        }
    }

    pub fn send<T: Serialize>(&self, msg: &T, handle: Option<RawHandle>) -> Result<()> {
        let body = postcard::to_stdvec(msg)?;
        let (place, value) = match handle {
            None => (NO_HANDLE, 0u64),
            Some(h) => match self.peer.get() {
                Some(peer) => (
                    IN_RECEIVER,
                    duplicate(unsafe { GetCurrentProcess() }, h, peer.as_raw_handle())? as u64,
                ),
                None => (IN_SENDER, h as u64),
            },
        };
        let mut frame = Vec::with_capacity(13 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.push(place);
        frame.extend_from_slice(&value.to_le_bytes());
        frame.extend_from_slice(&body);
        self.tx
            .lock()
            .unwrap()
            .write_all(&frame)
            .context("writing to the pipe")
    }

    /// Blocks. Returns `Ok(None)` when the peer closed its end.
    pub fn recv<T: DeserializeOwned>(&self) -> Result<Option<(T, Option<OwnedHandle>)>> {
        let raw = {
            let mut inc = self.incoming.lock().unwrap();
            match inc.peeked.take() {
                Some(m) => m,
                None => inc.reader()?.recv().unwrap_or(Ok(None)),
            }
        }?;
        let Some((place, value, body)) = raw else {
            return Ok(None);
        };
        let handle = match place {
            NO_HANDLE => None,
            IN_RECEIVER => Some(unsafe { OwnedHandle::from_raw_handle(value as RawHandle) }),
            IN_SENDER => {
                let peer = self
                    .peer
                    .get()
                    .context("a handle came from a process this side did not start")?;
                let h = duplicate(peer.as_raw_handle(), value as RawHandle, unsafe {
                    GetCurrentProcess()
                })?;
                Some(unsafe { OwnedHandle::from_raw_handle(h) })
            }
            other => bail!("bad handle placement {other}"),
        };
        Ok(Some((postcard::from_bytes(&body)?, handle)))
    }
}

impl Incoming {
    fn reader(&mut self) -> Result<&Receiver<Result<Raw>>> {
        if self.rx.is_none() {
            let file = self.file.take().context("channel reader is gone")?;
            let (send, recv) = channel();
            std::thread::Builder::new()
                .name("ipc reader".into())
                .spawn(move || read_thread(file, send))
                .context("starting the ipc reader")?;
            self.rx = Some(recv);
        }
        Ok(self.rx.as_ref().unwrap())
    }
}

/// A real handle to this process (for tests that talk to themselves).
#[cfg(test)]
pub(crate) fn current_process() -> Result<OwnedHandle> {
    let me = unsafe { GetCurrentProcess() };
    let h = duplicate(me, me, me)?;
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

fn read_thread(mut file: File, out: Sender<Result<Raw>>) {
    loop {
        let m = read_one(&mut file);
        let stop = !matches!(m, Ok(Some(_)));
        if out.send(m).is_err() || stop {
            return;
        }
    }
}

fn read_one(file: &mut File) -> Result<Raw> {
    let mut head = [0u8; 13];
    // a broken pipe reads as 0 bytes: the peer is gone
    let mut filled = 0;
    while filled < head.len() {
        let n = file.read(&mut head[filled..]).context("reading the pipe")?;
        if n == 0 {
            if filled == 0 {
                return Ok(None);
            }
            bail!("peer closed mid-message");
        }
        filled += n;
    }
    let len = u32::from_le_bytes(head[..4].try_into().unwrap()) as usize;
    if len > MAX_MSG {
        bail!("message too large ({len} bytes)");
    }
    let place = head[4];
    let value = u64::from_le_bytes(head[5..].try_into().unwrap());
    let mut body = vec![0u8; len];
    file.read_exact(&mut body)
        .context("peer closed mid-message")?;
    Ok(Some((place, value, body)))
}

fn duplicate(from: HANDLE, h: RawHandle, to: HANDLE) -> Result<RawHandle> {
    let mut out: HANDLE = std::ptr::null_mut();
    let ok = unsafe { DuplicateHandle(from, h, to, &mut out, 0, 0, DUPLICATE_SAME_ACCESS) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error()).context("DuplicateHandle");
    }
    Ok(out)
}

fn set_inherit(h: RawHandle, on: bool) -> Result<()> {
    let flags = if on { HANDLE_FLAG_INHERIT } else { 0 };
    if unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, flags) } == 0 {
        return Err(std::io::Error::last_os_error()).context("SetHandleInformation");
    }
    Ok(())
}
