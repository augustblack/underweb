use std::num::NonZeroUsize;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::ptr::NonNull;

use anyhow::{Context, Result};
use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap};

/// An anonymous shared-memory region that can be sent to another process.
pub struct Shm {
    fd: OwnedFd,
    ptr: NonNull<u8>,
    len: usize,
}

// The mapping is plain memory; synchronisation is by message passing.
unsafe impl Send for Shm {}
unsafe impl Sync for Shm {}

impl Shm {
    pub fn create(len: usize) -> Result<Shm> {
        let fd = anon_fd().context("create shared memory")?;
        nix::unistd::ftruncate(&fd, len as libc::off_t).context("ftruncate")?;
        Shm::from_fd(fd, len)
    }

    pub fn from_fd(fd: OwnedFd, len: usize) -> Result<Shm> {
        let ptr = unsafe {
            mmap(
                None,
                NonZeroUsize::new(len).context("empty shm")?,
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                fd.as_fd(),
                0,
            )
        }
        .context("mmap")?;
        Ok(Shm {
            fd,
            ptr: ptr.cast(),
            len,
        })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    /// Callers coordinate access with the frame protocol: a buffer is
    /// written only by the runner until it sends `Frame`, and read only by
    /// the host until it sends `Release`.
    #[allow(clippy::mut_from_ref)]
    pub fn as_mut_slice(&self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe {
            let _ = munmap(self.ptr.cast(), self.len);
        }
    }
}

#[cfg(target_os = "linux")]
fn anon_fd() -> Result<OwnedFd> {
    use nix::sys::memfd::{MFdFlags, memfd_create};
    Ok(memfd_create(c"underweb-frame", MFdFlags::MFD_CLOEXEC)?)
}

#[cfg(not(target_os = "linux"))]
fn anon_fd() -> Result<OwnedFd> {
    use nix::fcntl::OFlag;
    use nix::sys::mman::{shm_open, shm_unlink};
    use nix::sys::stat::Mode;
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    // macOS limits shm names to 31 chars
    let name = format!(
        "/uw.{}.{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let fd = shm_open(
        name.as_str(),
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )?;
    // unlink at once: the region lives only as long as its fds/mappings
    shm_unlink(name.as_str())?;
    Ok(fd)
}
