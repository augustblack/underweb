//! The host side: mixes every playing package's ring into one output.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::AudioRing;

pub struct Source {
    pub id: u64,
    ring: AudioRing,
    /// f32 bits; 0 mutes.
    gain: AtomicU32,
    /// Frames this source contributed, for "is it playing?" indicators.
    pub played: AtomicU64,
}

impl Source {
    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.to_bits(), Ordering::Relaxed)
    }
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
    pub fn ring(&self) -> &AudioRing {
        &self.ring
    }
}

pub struct Mixer {
    pub rate: u32,
    pub channels: usize,
    sources: Mutex<Vec<Arc<Source>>>,
    next_id: AtomicU64,
}

impl Mixer {
    pub fn new(rate: u32, channels: usize) -> Arc<Mixer> {
        Arc::new(Mixer {
            rate,
            channels,
            sources: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn add(&self, ring: AudioRing) -> Arc<Source> {
        let s = Arc::new(Source {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            ring,
            gain: AtomicU32::new(1.0f32.to_bits()),
            played: AtomicU64::new(0),
        });
        self.sources.lock().unwrap().push(s.clone());
        s
    }

    pub fn remove(&self, id: u64) {
        self.sources.lock().unwrap().retain(|s| s.id != id);
    }

    /// Fill `out` (interleaved, `self.channels`). Called from the audio
    /// callback: never blocks; if the source list is being changed right
    /// now, this block is silent.
    pub fn render(&self, out: &mut [f32]) {
        out.fill(0.0);
        let Ok(sources) = self.sources.try_lock() else {
            return;
        };
        for s in sources.iter() {
            let n = s.ring.mix_into(out, self.channels, s.gain());
            s.played.fetch_add(n as u64, Ordering::Relaxed);
        }
        for x in out.iter_mut() {
            *x = x.clamp(-1.0, 1.0);
        }
    }
}

#[cfg(feature = "device")]
pub mod device {
    //! Playing the mix on the default sound card.

    use std::sync::Arc;

    use anyhow::{Context, Result};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    use super::Mixer;

    /// Open the default output and start playing `Mixer`s output. The
    /// stream lives on its own thread for the life of the process.
    pub fn start() -> Result<Arc<Mixer>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("audio output".into())
            .spawn(move || {
                let setup = || -> Result<(Arc<Mixer>, cpal::Stream)> {
                    let device = cpal::default_host()
                        .default_output_device()
                        .context("no audio output device")?;
                    let config = device.default_output_config()?;
                    let channels = config.channels() as usize;
                    let mixer = Mixer::new(config.sample_rate().0, channels);
                    let m = mixer.clone();
                    let stream = match config.sample_format() {
                        cpal::SampleFormat::F32 => device.build_output_stream(
                            &config.into(),
                            move |out: &mut [f32], _| m.render(out),
                            |e| eprintln!("audio output: {e}"),
                            None,
                        )?,
                        other => {
                            // render as f32, convert
                            let mut buf = Vec::new();
                            let cfg: cpal::StreamConfig = config.into();
                            match other {
                                cpal::SampleFormat::I16 => device.build_output_stream(
                                    &cfg,
                                    move |out: &mut [i16], _| {
                                        buf.resize(out.len(), 0.0);
                                        m.render(&mut buf);
                                        for (o, x) in out.iter_mut().zip(&buf) {
                                            *o = (x * i16::MAX as f32) as i16;
                                        }
                                    },
                                    |e| eprintln!("audio output: {e}"),
                                    None,
                                )?,
                                f => anyhow::bail!("unsupported sample format {f:?}"),
                            }
                        }
                    };
                    stream.play()?;
                    Ok((mixer, stream))
                };
                match setup() {
                    Ok((mixer, stream)) => {
                        let _ = tx.send(Ok(mixer));
                        // keep the stream alive
                        loop {
                            std::thread::park();
                            let _ = &stream;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            })?;
        rx.recv().context("audio thread died")?
    }
}

/// No sound card: a thread pulls the mix in real time and throws it away
/// (headless machines, `UNDERWEB_AUDIO=null`).
pub fn start_null(rate: u32) -> Arc<Mixer> {
    let mixer = Mixer::new(rate, 2);
    let m = mixer.clone();
    std::thread::Builder::new()
        .name("audio null output".into())
        .spawn(move || {
            let block = 256;
            let mut buf = vec![0.0f32; block * 2];
            let period = std::time::Duration::from_secs_f64(block as f64 / rate as f64);
            let mut next = std::time::Instant::now();
            loop {
                m.render(&mut buf);
                next += period;
                if let Some(d) = next.checked_duration_since(std::time::Instant::now()) {
                    std::thread::sleep(d);
                }
            }
        })
        .expect("audio thread");
    mixer
}
