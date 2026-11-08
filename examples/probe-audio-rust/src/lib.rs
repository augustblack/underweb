//! A native package making its own sound: a sine from an audio callback.

use underweb::{Page, Window, audio};

#[derive(Default)]
struct Probe;

impl Page for Probe {
    fn activate(&mut self, w: &Window) {
        w.set_title("audio probe (rust)");
        let rate = audio::rate() as f32;
        w.log(&format!("PROBE rate {rate}"));
        let freq: f32 = w
            .param("freq")
            .and_then(|f| f.parse().ok())
            .unwrap_or(440.0);
        let mut phase = 0f32;
        audio::set_callback(move |frames| {
            for f in frames.chunks_exact_mut(2) {
                let x = (phase * std::f32::consts::TAU).sin() * 0.5;
                f[1] += x; // right only
                phase = (phase + freq / rate).fract();
            }
        });
    }
}

underweb::export_page!(Probe);
