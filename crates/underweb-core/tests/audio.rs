//! Sound end to end: sandboxed packages render, the test pulls from a
//! private mixer (no sound card needed) and measures what comes out.
//! Needs `cargo xtask examples` first.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use underweb_audio::ugens::estimate_freq;
use underweb_core::{Guest, GuestEvent, Mixer, SpawnOptions};

const RATE: u32 = 48000;

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("examples")
        .join(name)
}

fn spawn(name: &str, params: &[(&str, &str)], mixer: &Arc<Mixer>) -> Guest {
    let mut g = Guest::spawn(
        example(name),
        SpawnOptions {
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            mixer: Some(mixer.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    g.configure(200, 100, 1.0).unwrap();
    g
}

/// Pull `seconds` of sound once it starts; returns (left, right).
fn record(
    mixer: &Mixer,
    guest: &Guest,
    logs: &mut Vec<String>,
    seconds: f64,
) -> (Vec<f32>, Vec<f32>) {
    let want = (seconds * RATE as f64) as usize;
    let (mut l, mut r) = (Vec::new(), Vec::new());
    let mut buf = vec![0f32; 256 * 2];
    let deadline = Instant::now() + Duration::from_secs(15);
    while l.len() < want && Instant::now() < deadline {
        while let Some(e) = guest.try_event() {
            match e {
                GuestEvent::Log(m) => logs.push(m),
                GuestEvent::Error(e) => panic!("{e}"),
                _ => {}
            }
        }
        mixer.render(&mut buf);
        let started = !l.is_empty() || buf.iter().any(|x| x.abs() > 1e-4);
        if started {
            l.extend(buf.chunks_exact(2).map(|p| p[0]));
            r.extend(buf.chunks_exact(2).map(|p| p[1]));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        l.len() >= want,
        "only {} frames of sound; logs {logs:?}",
        l.len()
    );
    (l, r)
}

fn drain_logs(guest: &Guest, logs: &mut Vec<String>, until: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !logs.iter().any(|l| l.contains(until)) {
        if let Some(GuestEvent::Log(m)) = guest.next_event(Duration::from_millis(50)) {
            logs.push(m);
        }
    }
}

fn peak(s: &[f32]) -> f32 {
    s.iter().fold(0f32, |m, x| m.max(x.abs()))
}

#[test]
fn js_oscillator_is_heard() {
    let mixer = Mixer::new(RATE, 2);
    let g = spawn("probe-audio-js", &[("osc", "330")], &mixer);
    let mut logs = Vec::new();
    let (l, r) = record(&mixer, &g, &mut logs, 1.0);
    let f = estimate_freq(&l, RATE as f64);
    assert!((f - 330.0).abs() < 3.0, "{f}");
    assert!((peak(&l) - 0.5).abs() < 0.05, "{}", peak(&l));
    assert!(peak(&r) < 0.01, "panned left");
    assert!(logs.contains(&"PROBE rate 48000".to_string()), "{logs:?}");

    // muting the tab silences it
    g.set_muted(true);
    std::thread::sleep(Duration::from_millis(50));
    let mut buf = vec![0f32; 4096];
    mixer.render(&mut buf);
    mixer.render(&mut buf);
    assert!(peak(&buf) == 0.0);
    g.shutdown();
}

#[test]
fn pure_data_patch_plays_and_answers() {
    let mixer = Mixer::new(RATE, 2);
    let g = spawn("probe-audio-js", &[("pd", "550")], &mixer);
    let mut logs = Vec::new();
    let (l, r) = record(&mixer, &g, &mut logs, 1.5);
    // only the last half second: the patch starts at 440 Hz and the switch
    // to 550 can come late on a slow machine
    let f = estimate_freq(&l[l.len() - RATE as usize / 2..], RATE as f64);
    assert!((f - 550.0).abs() < 3.0, "{f}");
    assert!((peak(&r) - 0.5).abs() < 0.05);
    drain_logs(&g, &mut logs, "PROBE pong");
    assert!(logs.contains(&"PROBE pong 42".to_string()), "{logs:?}");
    assert!(
        logs.contains(&"PROBE escape refused".to_string()),
        "{logs:?}"
    );
    g.shutdown();
}

#[test]
fn native_audio_callback_is_heard() {
    let mixer = Mixer::new(RATE, 2);
    let g = spawn("probe-audio-rust", &[("freq", "660")], &mixer);
    let mut logs = Vec::new();
    let (l, r) = record(&mixer, &g, &mut logs, 1.0);
    let f = estimate_freq(&r, RATE as f64);
    assert!((f - 660.0).abs() < 3.0, "{f}");
    assert!(peak(&l) < 0.01, "right only");
    g.shutdown();
}

#[test]
fn closing_a_package_removes_its_sound() {
    let mixer = Mixer::new(RATE, 2);
    let g = spawn("probe-audio-js", &[("osc", "440")], &mixer);
    let mut logs = Vec::new();
    record(&mixer, &g, &mut logs, 0.1);
    g.shutdown();
    let mut buf = vec![0f32; 8192];
    for _ in 0..4 {
        mixer.render(&mut buf);
    }
    assert_eq!(peak(&buf), 0.0);
}

/// The real sound card: `cargo test -p underweb-core --test audio -- --ignored`.
#[test]
#[ignore = "plays a tone on the sound card"]
fn sound_card_plays() {
    let mixer = underweb_core::default_mixer();
    let g = spawn("probe-audio-js", &[("osc", "440")], &mixer);
    std::thread::sleep(Duration::from_secs(2));
    let played = g.frames_played();
    println!(
        "sound card at {} Hz consumed {played} frames in 2 s",
        mixer.rate
    );
    g.shutdown();
    assert!(played as f64 > 1.5 * mixer.rate as f64, "{played}");
}
