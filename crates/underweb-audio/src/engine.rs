//! The package side: oscillators, Pure Data and the package's own callback,
//! rendered on a real-time thread into the [`AudioRing`] the browser plays.
//!
//! The page controls everything from its own thread: oscillator settings
//! are atomics, Pd and the callback sit behind locks the audio thread only
//! `try_lock`s. The audio thread never blocks.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::ugens::{Osc, Wave};
use crate::{AudioRing, BLOCK, CHANNELS, pd};

pub const MAX_OSCS: usize = 64;
/// How much audio to keep queued ahead of the sound card.
pub const LATENCY_FRAMES: usize = 1024;
const SCOPE_FRAMES: usize = 4096;

struct Slot {
    active: AtomicBool,
    wave: AtomicU32,
    freq: AtomicU32,
    amp: AtomicU32,
    pan: AtomicU32,
}

fn load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}
fn store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed)
}

/// A native package's own audio: fills interleaved stereo frames.
pub type Callback = Box<dyn FnMut(&mut [f32]) + Send>;

pub struct Engine {
    pub rate: u32,
    slots: Vec<Slot>,
    volume: AtomicU32,
    callback: Mutex<Option<Callback>>,
    scope: Mutex<Vec<f32>>,
    running: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// The audio thread's own state.
struct Voices {
    oscs: Vec<Osc>,
    amp: Vec<f64>,
    freq: Vec<f64>,
}

impl Engine {
    pub fn new(rate: u32) -> Arc<Engine> {
        Arc::new(Engine {
            rate,
            slots: (0..MAX_OSCS)
                .map(|_| Slot {
                    active: AtomicBool::new(false),
                    wave: AtomicU32::new(0),
                    freq: AtomicU32::new(440f32.to_bits()),
                    amp: AtomicU32::new(0),
                    pan: AtomicU32::new(0),
                })
                .collect(),
            volume: AtomicU32::new(1f32.to_bits()),
            callback: Mutex::new(None),
            scope: Mutex::new(vec![0.0; SCOPE_FRAMES]),
            running: AtomicBool::new(false),
            thread: Mutex::new(None),
        })
    }

    /// A new oscillator (id from 1), or None if all are in use.
    pub fn osc_new(&self, wave: Wave, freq: f32, amp: f32, pan: f32) -> Option<u32> {
        for (i, s) in self.slots.iter().enumerate() {
            if !s.active.load(Ordering::Acquire) {
                s.wave.store(wave as u32, Ordering::Relaxed);
                store(&s.freq, freq);
                store(&s.amp, amp);
                store(&s.pan, pan);
                s.active.store(true, Ordering::Release);
                return Some(i as u32 + 1);
            }
        }
        None
    }

    fn slot(&self, id: u32) -> Option<&Slot> {
        self.slots.get((id as usize).wrapping_sub(1))
    }

    /// NaN or out-of-range values are clamped: pages are untrusted.
    pub fn osc_set(&self, id: u32, freq: f32, amp: f32, pan: f32) {
        if let Some(s) = self.slot(id) {
            store(
                &s.freq,
                if freq.is_finite() {
                    freq.clamp(0.0, self.rate as f32 / 2.0)
                } else {
                    0.0
                },
            );
            store(
                &s.amp,
                if amp.is_finite() {
                    amp.clamp(0.0, 1.0)
                } else {
                    0.0
                },
            );
            store(
                &s.pan,
                if pan.is_finite() {
                    pan.clamp(-1.0, 1.0)
                } else {
                    0.0
                },
            );
        }
    }

    pub fn osc_free(&self, id: u32) {
        if let Some(s) = self.slot(id) {
            store(&s.amp, 0.0);
            s.active.store(false, Ordering::Release);
        }
    }

    pub fn set_volume(&self, v: f32) {
        store(
            &self.volume,
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            },
        );
    }

    pub fn set_callback(&self, cb: Option<Callback>) {
        *self.callback.lock().unwrap() = cb;
    }

    /// The last `frames` frames played, mixed to mono (for drawing).
    pub fn scope(&self, frames: usize) -> Vec<f32> {
        let s = self.scope.lock().unwrap();
        let n = frames.min(s.len());
        s[s.len() - n..].to_vec()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Start rendering into `ring` on a new thread.
    pub fn start(self: &Arc<Self>, ring: AudioRing) {
        self.stop();
        self.running.store(true, Ordering::Release);
        let me = self.clone();
        let handle = std::thread::Builder::new()
            .name("underweb audio".into())
            .spawn(move || me.run(ring))
            .expect("audio thread");
        *self.thread.lock().unwrap() = Some(handle);
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
        if let Some(h) = self.thread.lock().unwrap().take() {
            let _ = h.join();
        }
    }

    fn run(&self, ring: AudioRing) {
        let mut v = Voices {
            oscs: (0..MAX_OSCS).map(|_| Osc::new(Wave::Sine)).collect(),
            amp: vec![0.0; MAX_OSCS],
            freq: vec![440.0; MAX_OSCS],
        };
        let mut block = [0f32; BLOCK * CHANNELS];
        let target = LATENCY_FRAMES.min(ring.capacity());
        while self.running.load(Ordering::Acquire) {
            while ring.filled() + BLOCK <= target {
                self.render(&mut v, &mut block);
                ring.write(&block);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// One block: oscillators + Pd + callback, then volume.
    fn render(&self, v: &mut Voices, out: &mut [f32; BLOCK * CHANNELS]) {
        out.fill(0.0);
        let rate = self.rate as f64;
        // ~5 ms smoothing so changes don't click
        let k = 1.0 - (-1.0 / (0.005 * rate)).exp();
        for (i, s) in self.slots.iter().enumerate() {
            let active = s.active.load(Ordering::Acquire);
            let target_amp = if active { load(&s.amp) as f64 } else { 0.0 };
            if !active && v.amp[i] < 1e-5 {
                v.amp[i] = 0.0;
                continue;
            }
            let wave = Wave::from_index(s.wave.load(Ordering::Relaxed)).unwrap_or(Wave::Sine);
            if v.oscs[i].wave != wave {
                v.oscs[i] = Osc::new(wave);
            }
            let target_freq = load(&s.freq) as f64;
            let pan = load(&s.pan) as f64;
            let (gl, gr) = (
                ((1.0 - pan) * std::f64::consts::FRAC_PI_4).cos(),
                ((1.0 - pan) * std::f64::consts::FRAC_PI_4).sin(),
            );
            for f in 0..BLOCK {
                v.amp[i] += (target_amp - v.amp[i]) * k;
                v.freq[i] += (target_freq - v.freq[i]) * k;
                let x = v.oscs[i].next(v.freq[i] / rate) * v.amp[i];
                out[f * 2] += (x * gr) as f32;
                out[f * 2 + 1] += (x * gl) as f32;
            }
        }
        pd::process_into(out);
        // the package's own callback, unless it's being replaced right now
        if let Ok(mut guard) = self.callback.try_lock()
            && let Some(cb) = guard.as_mut()
        {
            let mut own = [0f32; BLOCK * CHANNELS];
            cb(&mut own);
            for (o, x) in out.iter_mut().zip(own) {
                *o += if x.is_finite() { x } else { 0.0 };
            }
        }
        let vol = load(&self.volume);
        for x in out.iter_mut() {
            *x *= vol;
        }
        if let Ok(mut scope) = self.scope.try_lock() {
            scope.drain(..BLOCK);
            scope.extend(out.as_chunks::<2>().0.iter().map(|[l, r]| 0.5 * (l + r)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ugens::estimate_freq;

    #[test]
    fn oscillators_render_and_pan() {
        let e = Engine::new(48000);
        let id = e.osc_new(Wave::Sine, 330.0, 0.5, -1.0).unwrap();
        let mut v = Voices {
            oscs: (0..MAX_OSCS).map(|_| Osc::new(Wave::Sine)).collect(),
            amp: vec![0.0; MAX_OSCS],
            freq: vec![330.0; MAX_OSCS],
        };
        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut b = [0f32; BLOCK * CHANNELS];
        for _ in 0..750 {
            e.render(&mut v, &mut b);
            left.extend(b.as_chunks::<2>().0.iter().map(|[l, _]| *l));
            right.extend(b.as_chunks::<2>().0.iter().map(|[_, r]| *r));
        }
        let f = estimate_freq(&left, 48000.0);
        assert!((f - 330.0).abs() < 2.0, "{f}");
        assert!(right.iter().all(|x| x.abs() < 1e-4), "hard left");
        e.osc_set(id, f32::NAN, 9.0, 0.0);
        e.osc_free(id);
        assert_eq!(e.scope(100).len(), 100);
    }
}
