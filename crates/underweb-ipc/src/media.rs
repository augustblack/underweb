//! Video: the frame mailbox and the messages about media streams.
//!
//! A media process (one per stream, sandboxed) decodes and writes the
//! newest frame into a [`VideoMailbox`]; the package's runner reads it when
//! drawing. A sequence counter makes torn reads detectable (a seqlock): the
//! writer makes it odd while writing; a reader retries if it changed.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::Shm;

const MAGIC: u32 = 0x5557_5644; // "UWVD"
const HEADER: usize = 64;
/// Larger frames are refused (4K is plenty for a page).
pub const MAX_PIXELS: u64 = 3840 * 2160;

#[repr(C)]
struct Header {
    magic: AtomicU32,
    width: AtomicU32,
    height: AtomicU32,
    stride: AtomicU32,
    seq: AtomicU64,
    frames: AtomicU64,
    position_ns: AtomicU64,
    duration_ns: AtomicU64,
}

/// One BGRA (cairo ARGB32) frame in shared memory.
pub struct VideoMailbox {
    shm: Shm,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
}

impl VideoMailbox {
    pub fn bytes_for(width: u32, height: u32) -> usize {
        HEADER + width as usize * 4 * height as usize
    }

    /// Writer side.
    pub fn create(width: u32, height: u32) -> Result<VideoMailbox> {
        if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
            bail!("video size {width}x{height} not supported");
        }
        let shm = Shm::create(VideoMailbox::bytes_for(width, height))?;
        let m = VideoMailbox {
            shm,
            width,
            height,
            stride: width * 4,
        };
        let h = m.header();
        h.width.store(width, Ordering::Relaxed);
        h.height.store(height, Ordering::Relaxed);
        h.stride.store(width * 4, Ordering::Relaxed);
        h.magic.store(MAGIC, Ordering::Release);
        Ok(m)
    }

    /// Reader side: map a mailbox received with its size.
    pub fn open(handle: crate::OsHandle, width: u32, height: u32) -> Result<VideoMailbox> {
        if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
            bail!("video size {width}x{height} not supported");
        }
        let shm = Shm::from_handle(handle, VideoMailbox::bytes_for(width, height))?;
        let m = VideoMailbox {
            shm,
            width,
            height,
            stride: width * 4,
        };
        let h = m.header();
        if h.magic.load(Ordering::Acquire) != MAGIC
            || h.width.load(Ordering::Relaxed) != width
            || h.height.load(Ordering::Relaxed) != height
        {
            bail!("not a video mailbox of {width}x{height}");
        }
        Ok(m)
    }

    pub fn raw_handle(&self) -> crate::RawOsHandle {
        self.shm.raw_handle()
    }

    fn header(&self) -> &Header {
        unsafe { &*(self.shm.as_ptr() as *const Header) }
    }

    fn pixels(&self) -> *mut u8 {
        unsafe { self.shm.as_ptr().add(HEADER) }
    }

    /// Copy a frame in. `src` rows are `src_stride` bytes apart.
    pub fn write_frame(&self, src: &[u8], src_stride: usize) {
        let h = self.header();
        let row = self.width as usize * 4;
        if src_stride < row || src.len() < src_stride * (self.height as usize - 1) + row {
            return;
        }
        h.seq.fetch_add(1, Ordering::AcqRel); // odd: writing
        for y in 0..self.height as usize {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    src.as_ptr().add(y * src_stride),
                    self.pixels().add(y * self.stride as usize),
                    row,
                )
            };
        }
        h.seq.fetch_add(1, Ordering::AcqRel); // even: done
        h.frames.fetch_add(1, Ordering::AcqRel);
    }

    /// Frames written so far; changes mean there's something new to draw.
    pub fn frames(&self) -> u64 {
        self.header().frames.load(Ordering::Acquire)
    }

    /// Copy the newest frame out (`stride * height` bytes). None if
    /// nothing was written yet or the writer kept interrupting.
    pub fn read_frame(&self, out: &mut Vec<u8>) -> Option<u64> {
        let h = self.header();
        let len = self.stride as usize * self.height as usize;
        for _ in 0..4 {
            let before = h.seq.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::thread::yield_now();
                continue;
            }
            let frames = h.frames.load(Ordering::Acquire);
            if frames == 0 {
                return None;
            }
            out.resize(len, 0);
            unsafe { std::ptr::copy_nonoverlapping(self.pixels(), out.as_mut_ptr(), len) };
            if h.seq.load(Ordering::Acquire) == before {
                return Some(frames);
            }
        }
        None
    }

    pub fn set_position(&self, position: f64, duration: f64) {
        let h = self.header();
        h.position_ns
            .store((position.max(0.0) * 1e9) as u64, Ordering::Relaxed);
        h.duration_ns
            .store((duration.max(0.0) * 1e9) as u64, Ordering::Relaxed);
    }

    /// Seconds.
    pub fn position(&self) -> f64 {
        self.header().position_ns.load(Ordering::Relaxed) as f64 / 1e9
    }

    /// Seconds; 0 for live sources.
    pub fn duration(&self) -> f64 {
        self.header().duration_ns.load(Ordering::Relaxed) as f64 / 1e9
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MediaCmd {
    Play,
    Pause,
    /// Seconds from the start.
    Seek(f64),
    Close,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MediaEvent {
    /// Frames are coming; the message carries the mailbox fd.
    Ready {
        width: u32,
        height: u32,
        duration: f64,
        has_audio: bool,
    },
    Ended,
    /// Encoding and sending to the broadcast server has begun.
    Broadcasting,
    Error(String),
}

/// Where to send an encoded stream: an Icecast-style HTTP PUT.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Broadcast {
    /// `http://host:port/mount.ogg`
    pub url: String,
    /// Source password (user "source").
    pub password: String,
}

/// Browser → media process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToMedia {
    Cmd(MediaCmd),
    /// Bytes of a stream the browser downloads (http sources).
    Input(Vec<u8>),
    InputEnd,
    /// Where to play the stream's sound (with the ring's fd).
    AudioReady {
        capacity: u32,
        rate: u32,
    },
}

/// Media process → browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FromMedia {
    Event(MediaEvent),
    /// Encoded broadcast data to send on.
    Encoded(Vec<u8>),
    /// The stream has sound: please send an audio ring.
    WantsAudio,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_round_trip() {
        let w = VideoMailbox::create(4, 2).unwrap();
        let mut out = Vec::new();
        assert!(w.read_frame(&mut out).is_none());
        let src: Vec<u8> = (0..40u8).collect(); // stride 20, 2 rows
        w.write_frame(&src, 20);
        let dup = w.shm.try_clone_handle().unwrap();
        let r = VideoMailbox::open(dup, 4, 2).unwrap();
        assert_eq!(r.read_frame(&mut out), Some(1));
        assert_eq!(&out[..16], &src[..16]);
        assert_eq!(&out[16..32], &src[20..36]);
        w.set_position(1.5, 3.0);
        assert_eq!((r.position(), r.duration()), (1.5, 3.0));
        assert!(VideoMailbox::create(100_000, 100_000).is_err());
    }
}
