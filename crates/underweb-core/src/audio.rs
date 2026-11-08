//! The browser's one audio output, shared by every package.

use std::sync::{Arc, OnceLock};

use underweb_audio::mixer::{self, Mixer};

static MIXER: OnceLock<Arc<Mixer>> = OnceLock::new();

/// The default sound card, opened on first use. With
/// `UNDERWEB_AUDIO=null`, or when there is no sound card, sound is
/// produced at the right pace and thrown away.
pub fn default_mixer() -> Arc<Mixer> {
    MIXER
        .get_or_init(|| {
            if std::env::var("UNDERWEB_AUDIO").as_deref() == Ok("null") {
                return mixer::start_null(48000);
            }
            match mixer::device::start() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("no audio output ({e:#}); sound is discarded");
                    mixer::start_null(48000)
                }
            }
        })
        .clone()
}
