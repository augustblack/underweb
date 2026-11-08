//! libpd for real (one test: Pd is global to the process).
#![cfg(feature = "engine")]

use underweb_audio::pd::{self, PdValue};
use underweb_audio::ugens::estimate_freq;
use underweb_audio::{BLOCK, CHANNELS};

const PATCH: &str = "#N canvas 0 0 450 300 10;
#X obj 10 10 r freq;
#X obj 10 40 osc~ 440;
#X obj 10 70 *~ 0.5;
#X obj 10 100 dac~;
#X obj 200 10 r ping;
#X obj 200 40 + 1;
#X obj 200 70 s pong;
#X obj 300 10 r hello;
#X obj 300 40 print greeting;
#X connect 0 0 1 0;
#X connect 1 0 2 0;
#X connect 2 0 3 0;
#X connect 2 0 3 1;
#X connect 4 0 5 0;
#X connect 5 0 6 0;
#X connect 7 0 8 0;
";

fn left(blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..blocks {
        let mut b = [0f32; BLOCK * CHANNELS];
        assert!(pd::process_into(&mut b));
        out.extend(b.chunks_exact(2).map(|p| p[0]));
    }
    out
}

#[test]
fn patch_plays_and_talks() {
    let dir = std::env::temp_dir().join(format!("uw-pd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("t.pd"), PATCH).unwrap();

    pd::init(48000).unwrap();
    assert!(pd::init(44100).is_err());
    pd::open(&dir.join("t.pd")).unwrap();
    assert!(pd::open(&dir.join("missing.pd")).is_err());

    let s = left(750); // 1 s
    let f = estimate_freq(&s, 48000.0);
    assert!((f - 440.0).abs() < 2.0, "{f}");
    let peak = s.iter().fold(0f32, |m, x| m.max(x.abs()));
    assert!((peak - 0.5).abs() < 0.02, "{peak}");

    pd::send("freq", &PdValue::Float(220.0)).unwrap();
    let f = estimate_freq(&left(750), 48000.0);
    assert!((f - 220.0).abs() < 2.0, "{f}");

    pd::subscribe("pong").unwrap();
    pd::send("ping", &PdValue::Float(41.0)).unwrap();
    pd::send("hello", &PdValue::Symbol("world".into())).unwrap();
    left(1);
    let msgs = pd::take_messages();
    assert!(
        msgs.iter()
            .any(|m| m.receiver == "pong" && m.value == PdValue::Float(42.0)),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.receiver == "print"
            && matches!(&m.value, PdValue::Symbol(s) if s.contains("greeting: symbol world"))),
        "{msgs:?}"
    );
}
