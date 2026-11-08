//! A single-producer single-consumer ring of interleaved stereo `f32`
//! frames in shared memory. The package's audio thread writes, the host's
//! audio callback reads; neither ever blocks or allocates.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use anyhow::{Result, bail};
use underweb_ipc::Shm;

use crate::CHANNELS;

#[repr(C)]
struct Header {
    /// Frames written so far (only the writer stores).
    write: AtomicU64,
    /// Frames read so far (only the reader stores).
    read: AtomicU64,
    capacity: AtomicU32,
    /// Underruns seen by the reader, for diagnostics.
    underruns: AtomicU32,
}

const HEADER: usize = 64;

pub struct AudioRing {
    shm: Shm,
    capacity: usize,
}

impl AudioRing {
    pub fn bytes_for(capacity: usize) -> usize {
        HEADER + capacity * CHANNELS * 4
    }

    /// A new ring for `capacity` frames (host side).
    pub fn create(capacity: usize) -> Result<AudioRing> {
        let shm = Shm::create(AudioRing::bytes_for(capacity))?;
        let r = AudioRing { shm, capacity };
        r.header()
            .capacity
            .store(capacity as u32, Ordering::Release);
        Ok(r)
    }

    /// Map a ring received from the host (package side).
    pub fn open(shm: Shm) -> Result<AudioRing> {
        if shm.len() < HEADER {
            bail!("audio ring too small");
        }
        let capacity = unsafe { &*(shm.as_ptr() as *const Header) }
            .capacity
            .load(Ordering::Acquire) as usize;
        if AudioRing::bytes_for(capacity) > shm.len() || capacity == 0 {
            bail!("audio ring header is inconsistent");
        }
        Ok(AudioRing { shm, capacity })
    }

    pub fn shm(&self) -> &Shm {
        &self.shm
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    fn header(&self) -> &Header {
        unsafe { &*(self.shm.as_ptr() as *const Header) }
    }

    fn samples(&self) -> *mut f32 {
        unsafe { self.shm.as_ptr().add(HEADER) as *mut f32 }
    }

    /// Frames waiting to be played.
    pub fn filled(&self) -> usize {
        let h = self.header();
        let (w, r) = (
            h.write.load(Ordering::Acquire),
            h.read.load(Ordering::Acquire),
        );
        // a hostile writer could set anything; clamp
        w.saturating_sub(r).min(self.capacity as u64) as usize
    }

    pub fn underruns(&self) -> u32 {
        self.header().underruns.load(Ordering::Relaxed)
    }

    /// Writer: append interleaved stereo frames; returns how many fit.
    pub fn write(&self, frames: &[f32]) -> usize {
        let n = (frames.len() / CHANNELS).min(self.capacity - self.filled());
        let h = self.header();
        let w = h.write.load(Ordering::Relaxed);
        let base = self.samples();
        for i in 0..n {
            let slot = ((w + i as u64) % self.capacity as u64) as usize * CHANNELS;
            for c in 0..CHANNELS {
                unsafe { *base.add(slot + c) = frames[i * CHANNELS + c] };
            }
        }
        h.write.store(w + n as u64, Ordering::Release);
        n
    }

    /// Reader: add up to `out.len() / out_channels` frames, scaled by
    /// `gain`, into `out` (interleaved with `out_channels` channels; the
    /// ring's two channels go to the first two). Returns frames read;
    /// fewer than asked is an underrun.
    pub fn mix_into(&self, out: &mut [f32], out_channels: usize, gain: f32) -> usize {
        let want = out.len() / out_channels;
        let n = want.min(self.filled());
        let h = self.header();
        let r = h.read.load(Ordering::Relaxed);
        let base = self.samples();
        for i in 0..n {
            let slot = ((r + i as u64) % self.capacity as u64) as usize * CHANNELS;
            let (l, rr) = unsafe { (*base.add(slot), *base.add(slot + 1)) };
            let (l, rr) = (sanitize(l), sanitize(rr));
            if out_channels == 1 {
                out[i] += 0.5 * (l + rr) * gain;
            } else {
                out[i * out_channels] += l * gain;
                out[i * out_channels + 1] += rr * gain;
            }
        }
        h.read.store(r + n as u64, Ordering::Release);
        if n < want {
            h.underruns.fetch_add(1, Ordering::Relaxed);
        }
        n
    }
}

/// Packages are untrusted: no NaNs, no infinities, no ear-splitting values.
fn sanitize(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_wrap() {
        let host = AudioRing::create(8).unwrap();
        let pkg = AudioRing::open(
            Shm::from_handle(
                host.shm().try_clone_handle().unwrap(),
                AudioRing::bytes_for(8),
            )
            .unwrap(),
        )
        .unwrap();
        let frames: Vec<f32> = (0..12).map(|i| i as f32 / 100.0).collect(); // 6 frames
        assert_eq!(pkg.write(&frames), 6);
        assert_eq!(pkg.write(&frames), 2); // full
        let mut out = vec![0.0; 10]; // 5 frames
        assert_eq!(host.mix_into(&mut out, 2, 1.0), 5);
        assert_eq!(&out[..4], &[0.0, 0.01, 0.02, 0.03]);
        assert_eq!(pkg.write(&frames), 5);
        let mut out = vec![0.0; 40];
        assert_eq!(host.mix_into(&mut out, 2, 0.5), 8);
        assert_eq!(host.underruns(), 1);
        // sanitised
        pkg.write(&[f32::NAN, 9.0]);
        let mut out = vec![0.0; 2];
        host.mix_into(&mut out, 2, 1.0);
        assert_eq!(out, vec![0.0, 1.0]);
    }
}
