//! The package's audio: the engine (oscillators, Pd, native callback) and
//! the connection to the browser, which plays what the engine writes into
//! a shared ring. Audio starts on first use.

use std::cell::Cell;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use underweb_audio::engine::Engine;
use underweb_audio::pd;
use underweb_ipc::{Channel, GuestMsg};

pub struct Audio {
    pub engine: Arc<Engine>,
    chan: *const Channel,
    requested: Cell<bool>,
    pkg_dir: PathBuf,
}

impl Audio {
    /// `rate` 0: the browser has no audio.
    pub fn new(chan: &Channel, rate: u32, pkg_dir: &Path) -> Audio {
        Audio {
            engine: Engine::new(rate.max(1)),
            chan,
            requested: Cell::new(false),
            pkg_dir: pkg_dir.to_path_buf(),
        }
    }

    pub fn rate(&self) -> u32 {
        if self.engine.rate > 1 {
            self.engine.rate
        } else {
            0
        }
    }

    /// Ask the browser for an audio ring (once).
    pub fn start(&self) -> Result<()> {
        if self.rate() == 0 {
            bail!("this browser has no audio output");
        }
        if !self.requested.replace(true) {
            unsafe { &*self.chan }.send(&GuestMsg::AudioStart, None)?;
        }
        Ok(())
    }

    pub fn stop(&self) {
        self.engine.stop();
        if self.requested.replace(false) {
            let _ = unsafe { &*self.chan }.send(&GuestMsg::AudioStop, None);
        }
    }

    pub fn active(&self) -> bool {
        self.requested.get()
    }

    /// The browser's ring arrived: start rendering.
    pub fn attach(&self, ring: underweb_audio::AudioRing) {
        if self.requested.get() {
            self.engine.start(ring);
        }
    }

    /// Open a Pd patch from the package (path relative to it) and start
    /// audio.
    pub fn pd_open(&self, rel: &str) -> Result<()> {
        let p = Path::new(rel);
        if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
            bail!("patch paths are relative to the package: {rel:?}");
        }
        self.start()?;
        pd::init(self.rate()).context("starting Pd")?;
        pd::open(&self.pkg_dir.join(p))
    }
}
