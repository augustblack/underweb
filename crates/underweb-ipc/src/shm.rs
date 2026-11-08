use std::ptr::NonNull;

use anyhow::{Context, Result};

use crate::{OsHandle, RawOsHandle};

/// An anonymous shared-memory region that can be sent to another process.
/// A memfd or POSIX shm object on Unix, a pagefile-backed section on
/// Windows.
pub struct Shm {
    handle: OsHandle,
    ptr: NonNull<u8>,
    len: usize,
}

// The mapping is plain memory; synchronisation is by message passing.
unsafe impl Send for Shm {}
unsafe impl Sync for Shm {}

impl Shm {
    pub fn create(len: usize) -> Result<Shm> {
        let handle = imp::create(len).context("create shared memory")?;
        Shm::from_handle(handle, len)
    }

    /// Map a region received from another process.
    pub fn from_handle(handle: OsHandle, len: usize) -> Result<Shm> {
        anyhow::ensure!(len > 0, "empty shm");
        let ptr = imp::map(&handle, len)?;
        Ok(Shm { handle, ptr, len })
    }

    /// For [`crate::Channel::send`].
    pub fn raw_handle(&self) -> RawOsHandle {
        imp::raw(&self.handle)
    }

    /// A second handle to the same region.
    pub fn try_clone_handle(&self) -> Result<OsHandle> {
        Ok(self.handle.try_clone()?)
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
        imp::unmap(self.ptr, self.len);
    }
}

#[cfg(unix)]
mod imp {
    use std::num::NonZeroUsize;
    use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
    use std::ptr::NonNull;

    use anyhow::{Context, Result};
    use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap};

    pub fn create(len: usize) -> Result<OwnedFd> {
        let fd = anon_fd()?;
        nix::unistd::ftruncate(&fd, len as libc::off_t).context("ftruncate")?;
        Ok(fd)
    }

    pub fn map(fd: &OwnedFd, len: usize) -> Result<NonNull<u8>> {
        let ptr = unsafe {
            mmap(
                None,
                NonZeroUsize::new(len).unwrap(),
                ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
                MapFlags::MAP_SHARED,
                fd.as_fd(),
                0,
            )
        }
        .context("mmap")?;
        Ok(ptr.cast())
    }

    pub fn raw(fd: &OwnedFd) -> RawFd {
        fd.as_raw_fd()
    }

    pub fn unmap(ptr: NonNull<u8>, len: usize) {
        unsafe {
            let _ = munmap(ptr.cast(), len);
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
}

#[cfg(windows)]
mod imp {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use std::ptr::NonNull;

    use anyhow::{Context, Result};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READWRITE, UnmapViewOfFile,
    };

    pub fn create(len: usize) -> Result<OwnedHandle> {
        let len = len as u64;
        let h = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                (len >> 32) as u32,
                len as u32,
                std::ptr::null(),
            )
        };
        if h.is_null() {
            return Err(std::io::Error::last_os_error()).context("CreateFileMappingW");
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(h) })
    }

    pub fn map(h: &OwnedHandle, len: usize) -> Result<NonNull<u8>> {
        let view = unsafe { MapViewOfFile(h.as_raw_handle(), FILE_MAP_ALL_ACCESS, 0, 0, len) };
        NonNull::new(view.Value.cast::<u8>())
            .ok_or_else(std::io::Error::last_os_error)
            .context("MapViewOfFile")
    }

    pub fn raw(h: &OwnedHandle) -> RawHandle {
        h.as_raw_handle()
    }

    pub fn unmap(ptr: NonNull<u8>, _len: usize) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: ptr.as_ptr().cast(),
            });
        }
    }
}
