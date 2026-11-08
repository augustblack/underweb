//! Oscillators, after libmentiras' ugens.vala (Phasor, SineOsc, Buzz).
//! Saw and square are band-limited with PolyBLEP instead of Buzz's
//! summed harmonics: same aim (no aliasing), cheaper.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Saw,
    Square,
    Triangle,
    Noise,
}

impl Wave {
    pub fn from_index(i: u32) -> Option<Wave> {
        [
            Wave::Sine,
            Wave::Saw,
            Wave::Square,
            Wave::Triangle,
            Wave::Noise,
        ]
        .get(i as usize)
        .copied()
    }

    pub fn parse(s: &str) -> Option<Wave> {
        Some(match s {
            "sine" => Wave::Sine,
            "saw" => Wave::Saw,
            "square" => Wave::Square,
            "triangle" => Wave::Triangle,
            "noise" => Wave::Noise,
            _ => return None,
        })
    }
}

/// `phase` in [0, 1); `freq` in Hz.
#[derive(Debug, Clone)]
pub struct Osc {
    pub wave: Wave,
    phase: f64,
    noise: u32,
    tri: f64,
}

impl Osc {
    pub fn new(wave: Wave) -> Osc {
        Osc {
            wave,
            phase: 0.0,
            noise: 0x9e37_79b9,
            tri: 0.0,
        }
    }

    /// One sample, advancing by `inc = freq / rate`.
    pub fn next(&mut self, inc: f64) -> f64 {
        let t = self.phase;
        let inc = inc.clamp(0.0, 0.5);
        self.phase = (self.phase + inc).fract();
        match self.wave {
            Wave::Sine => (t * std::f64::consts::TAU).sin(),
            Wave::Saw => (2.0 * t - 1.0) - poly_blep(t, inc),
            Wave::Square => {
                let naive = if t < 0.5 { 1.0 } else { -1.0 };
                naive + poly_blep(t, inc) - poly_blep((t + 0.5).fract(), inc)
            }
            Wave::Triangle => {
                // integrated band-limited square
                let naive = if t < 0.5 { 1.0 } else { -1.0 };
                let sq = naive + poly_blep(t, inc) - poly_blep((t + 0.5).fract(), inc);
                self.tri = 4.0 * inc * sq + (1.0 - 4.0 * inc * 0.01) * self.tri;
                self.tri
            }
            Wave::Noise => {
                // xorshift32
                let mut x = self.noise;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                self.noise = x;
                x as f64 / u32::MAX as f64 * 2.0 - 1.0
            }
        }
    }
}

fn poly_blep(t: f64, dt: f64) -> f64 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// Zero crossings per second of a mono signal: a frequency estimate.
pub fn estimate_freq(samples: &[f32], rate: f64) -> f64 {
    let ups = samples
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    ups as f64 * rate / samples.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequencies_and_ranges() {
        for wave in [Wave::Sine, Wave::Saw, Wave::Square] {
            let mut o = Osc::new(wave);
            let s: Vec<f32> = (0..48000).map(|_| o.next(440.0 / 48000.0) as f32).collect();
            let f = estimate_freq(&s, 48000.0);
            assert!((f - 440.0).abs() < 3.0, "{wave:?}: {f}");
            assert!(s.iter().all(|x| x.abs() <= 1.2), "{wave:?}");
        }
        let mut n = Osc::new(Wave::Noise);
        let s: Vec<f64> = (0..1000).map(|_| n.next(0.0)).collect();
        assert!(s.iter().any(|x| *x > 0.5) && s.iter().any(|x| *x < -0.5));
    }
}
