//! Video end to end: sandboxed media processes decode, sandboxed runners
//! draw the frames inside shapes. Needs `cargo xtask examples` first.

use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use underweb_audio::ugens::estimate_freq;
use underweb_core::{Decision, Frame, Guest, GuestEvent, Mixer, PermissionPolicy, SpawnOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

/// Copy an example (with dist/) and replace its [permissions].
fn with_permissions(name: &str, to: &Path, permissions: &str) -> PathBuf {
    fn copy_dir(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &dst.join(e.file_name()));
            } else {
                std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
            }
        }
    }
    let dst = to.join(name);
    copy_dir(&root().join("examples").join(name), &dst);
    let m = dst.join("underweb.toml");
    let text = std::fs::read_to_string(&m).unwrap();
    let head = text.split("\n[permissions]").next().unwrap();
    std::fs::write(&m, format!("{head}\n[permissions]\n{permissions}\n")).unwrap();
    dst
}

struct Run {
    guest: Guest,
    logs: Vec<String>,
    frame: Option<Frame>,
    prompts: usize,
}

fn start(
    dir: &Path,
    params: &[(&str, &str)],
    policy: PermissionPolicy,
    mixer: Option<Arc<Mixer>>,
) -> Run {
    let mut guest = Guest::spawn(
        dir,
        SpawnOptions {
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            permissions: policy,
            remember_grants: false,
            mixer,
            ..Default::default()
        },
    )
    .unwrap();
    guest.configure(640, 360, 1.0).unwrap();
    Run {
        guest,
        logs: Vec::new(),
        frame: None,
        prompts: 0,
    }
}

impl Run {
    /// Pump events until `done` or 20 s; deny any permission prompt.
    fn until(&mut self, done: impl Fn(&Run) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && !done(self) {
            match self.guest.next_event(Duration::from_millis(50)) {
                Some(GuestEvent::Log(l)) => self.logs.push(l),
                Some(GuestEvent::Frame(f)) => self.frame = Some(f),
                Some(GuestEvent::Error(e)) => panic!("{e}\n{:?}", self.logs),
                Some(GuestEvent::PermissionRequest { id, .. }) => {
                    self.prompts += 1;
                    self.guest.answer(id, Decision::Deny);
                }
                Some(GuestEvent::Exited(_)) => panic!("runner exited: {:?}", self.logs),
                _ => {}
            }
        }
    }

    fn logged(&self, prefix: &str) -> Option<&String> {
        self.logs.iter().find(|l| l.starts_with(prefix))
    }
}

fn pixel(f: &Frame, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * f.width + x) * 4) as usize;
    f.rgba[i..i + 4].try_into().unwrap()
}

const MAGENTA: [u8; 4] = [255, 0, 255, 255];

/// Pull the mixer like a sound card until `done(left)` or `limit`; returns
/// the left channel.
fn record_until(mixer: &Mixer, limit: Duration, done: impl Fn(&[f32]) -> bool) -> Vec<f32> {
    let mut left = Vec::new();
    let mut buf = vec![0f32; 256 * 2];
    let start = Instant::now();
    let mut pulled = 0usize;
    while start.elapsed() < limit && !done(&left) {
        // keep up with the wall clock at 48 kHz
        while pulled < (start.elapsed().as_secs_f64() * 48000.0) as usize {
            mixer.render(&mut buf);
            left.extend(buf.chunks_exact(2).map(|p| p[0]));
            pulled += 256;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    left
}

#[test]
fn test_pattern_fills_the_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = with_permissions("probe-video-js", tmp.path(), "");
    let mut r = start(
        &dir,
        &[("src", "test:smpte")],
        PermissionPolicy::AllowDeclared,
        None,
    );
    r.until(|r| r.logged("PROBE ready").is_some() || r.logged("PROBE error").is_some());
    assert_eq!(
        r.logged("PROBE ready").map(String::as_str),
        Some("PROBE ready 640x360"),
        "{:?}",
        r.logs
    );
    // wait for a frame drawn with video in it
    r.until(|r| {
        r.frame
            .as_ref()
            .is_some_and(|f| pixel(f, 20, 20) != MAGENTA)
    });
    let f = r.frame.as_ref().unwrap();
    // SMPTE bars: the first is light grey
    let p = pixel(f, 20, 20);
    assert!(
        p[0] > 150 && (p[0] as i32 - p[2] as i32).abs() < 20,
        "{p:?}"
    );
    r.guest.shutdown();
}

#[test]
fn file_plays_with_moving_frames_and_sound() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = with_permissions("probe-video-js", tmp.path(), "");
    let mixer = Mixer::new(48000, 2);
    let mut r = start(
        &dir,
        &[("src", "ball.webm")],
        PermissionPolicy::AllowDeclared,
        Some(mixer.clone()),
    );
    r.until(|r| r.logged("PROBE ready").is_some() || r.logged("PROBE error").is_some());
    assert_eq!(
        r.logged("PROBE ready").map(String::as_str),
        Some("PROBE ready 320x240"),
        "{:?}",
        r.logs
    );
    // frames change as the ball moves
    r.until(|r| {
        r.frame
            .as_ref()
            .is_some_and(|f| pixel(f, 320, 180) != MAGENTA)
    });
    let first = r.frame.clone().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    r.until(|r| r.frame.as_ref().is_some_and(|f| f.rgba != first.rgba));
    assert_ne!(r.frame.as_ref().unwrap().rgba, first.rgba);
    // the clip's 440 Hz tone reaches the mixer, pulled at real-time pace
    // the way a sound card would
    // (a busy machine may start the sound late: listen until there is
    // half a second of it, for up to 10 s)
    let heard = |l: &[f32]| {
        l.iter()
            .position(|x| x.abs() >= 1e-3)
            .is_some_and(|i| l.len() - i >= 24000)
    };
    let left = record_until(&mixer, Duration::from_secs(10), heard);
    let sound: Vec<f32> = left
        .iter()
        .copied()
        .skip_while(|x| x.abs() < 1e-3)
        .take(24000)
        .collect();
    assert!(sound.len() >= 24000, "no sound from the video");
    let f = estimate_freq(&sound, 48000.0);
    assert!((f - 440.0).abs() < 10.0, "{f}");
    r.guest.shutdown();
}

#[test]
fn http_video_needs_a_fetch_permission() {
    let tmp = tempfile::tempdir().unwrap();
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let clip = std::fs::read(root().join("examples/probe-video-js/ball.webm")).unwrap();
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let _ = req.respond(tiny_http::Response::from_data(clip.clone()));
        }
    });
    let url = format!("http://127.0.0.1:{port}/ball.webm");

    let undeclared = with_permissions("probe-video-js", &tmp.path().join("a"), "");
    let mut r = start(
        &undeclared,
        &[("src", &url)],
        PermissionPolicy::AllowDeclared,
        None,
    );
    r.until(|r| r.logged("PROBE error").is_some());
    assert!(
        r.logged("PROBE error permission denied").is_some(),
        "{:?}",
        r.logs
    );
    r.guest.shutdown();

    let declared = with_permissions(
        "probe-video-js",
        &tmp.path().join("b"),
        &format!("fetch = [\"http://127.0.0.1:{port}/\"]"),
    );
    let mut r = start(
        &declared,
        &[("src", &url)],
        PermissionPolicy::AllowDeclared,
        None,
    );
    r.until(|r| r.logged("PROBE ready").is_some() || r.logged("PROBE error").is_some());
    assert_eq!(
        r.logged("PROBE ready").map(String::as_str),
        Some("PROBE ready 320x240"),
        "{:?}",
        r.logs
    );
    r.guest.shutdown();
}

#[test]
fn broadcast_reaches_an_icecast_style_server() {
    let tmp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    {
        let received = received.clone();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                received.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
    }
    let dir = with_permissions(
        "probe-video-js",
        tmp.path(),
        &format!("net.tcp.connect = [\"127.0.0.1:{port}\"]"),
    );
    let url = format!("http://127.0.0.1:{port}/live.ogg");
    let mut r = start(
        &dir,
        &[("src", "test:ball"), ("broadcast", &url)],
        PermissionPolicy::AllowDeclared,
        None,
    );
    r.until(|r| r.logged("PROBE broadcasting").is_some() || r.logged("PROBE error").is_some());
    assert!(r.logged("PROBE broadcasting").is_some(), "{:?}", r.logs);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !received.lock().unwrap().windows(4).any(|w| w == b"OggS") {
        std::thread::sleep(Duration::from_millis(100));
    }
    r.guest.shutdown();
    let data = received.lock().unwrap();
    let head = String::from_utf8_lossy(&data[..data.len().min(400)]);
    assert!(head.starts_with("PUT /live.ogg HTTP/1.1\r\n"), "{head}");
    assert!(
        head.contains("Authorization: Basic c291cmNlOmhhY2ttZQ==\r\n"),
        "{head}"
    ); // source:hackme
    assert!(data.windows(4).any(|w| w == b"OggS"), "no Ogg data");
}

#[test]
fn camera_needs_permission_and_files_stay_inside_the_package() {
    let tmp = tempfile::tempdir().unwrap();
    let undeclared = with_permissions("probe-video-js", &tmp.path().join("a"), "");
    let mut r = start(
        &undeclared,
        &[("src", "camera")],
        PermissionPolicy::Ask,
        None,
    );
    r.until(|r| r.logged("PROBE error").is_some());
    assert!(
        r.logged("PROBE error permission denied: camera is not declared")
            .is_some(),
        "{:?}",
        r.logs
    );
    r.guest.shutdown();

    let declared = with_permissions("probe-video-js", &tmp.path().join("b"), "camera = true");
    let mut r = start(&declared, &[("src", "camera")], PermissionPolicy::Ask, None);
    r.until(|r| r.logged("PROBE error").is_some());
    assert_eq!(r.prompts, 1);
    assert!(
        r.logged("PROBE error permission denied: use your camera")
            .is_some(),
        "{:?}",
        r.logs
    );
    r.guest.shutdown();

    let mut r = start(
        &declared,
        &[("src", "../../secret.webm")],
        PermissionPolicy::AllowDeclared,
        None,
    );
    r.until(|r| r.logged("PROBE error").is_some());
    assert!(
        r.logged("PROBE error")
            .unwrap()
            .contains("relative to the package"),
        "{:?}",
        r.logs
    );
    r.guest.shutdown();
}

#[test]
fn native_package_shows_video() {
    let mut guest = Guest::spawn(
        root().join("examples/probe-video-rust"),
        SpawnOptions {
            params: vec![("src".into(), "test:smpte".into())],
            ..Default::default()
        },
    )
    .unwrap();
    guest.configure(640, 360, 1.0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut logs = Vec::new();
    while Instant::now() < deadline && !logs.iter().any(|l: &String| l.starts_with("PROBE")) {
        if let Some(GuestEvent::Log(l)) = guest.next_event(Duration::from_millis(50)) {
            logs.push(l);
        }
    }
    guest.shutdown();
    assert!(
        logs.contains(&"PROBE ready 640x360 info=true".to_string()),
        "{logs:?}"
    );
}

// ---- the camera examples, with a test pattern for the camera ------------

#[test]
fn camera_effects_show_every_effect() {
    let dir = root().join("examples/camera-effects");
    for effect in [
        "normal",
        "mirror",
        "tiles",
        "kaleidoscope",
        "grayscale",
        "invert",
        "pixelate",
        "posterize",
    ] {
        let mut r = start(
            &dir,
            &[("source", "test:smpte"), ("effect", effect)],
            PermissionPolicy::AllowDeclared,
            None,
        );
        r.until(|r| r.logged("ready").is_some() && r.frame.is_some());
        assert!(
            r.logged("ready 640x360").is_some(),
            "{effect}: {:?}",
            r.logs
        );
        assert!(r.logged("error").is_none(), "{effect}: {:?}", r.logs);
        r.guest.shutdown();
    }
}

#[test]
fn camera_effects_buttons_switch_and_filter_the_picture() {
    let dir = root().join("examples/camera-effects");
    let mut r = start(
        &dir,
        &[("source", "test:smpte")],
        PermissionPolicy::AllowDeclared,
        None,
    );
    r.until(|r| r.logged("ready").is_some());
    // at 640x360 the picture is 397x298 at (16, 46): this point shows the
    // test pattern's yellow bar
    let yellow = |r: &mut Run| {
        r.frame = None;
        r.until(|r| r.frame.is_some());
        pixel(r.frame.as_ref().unwrap(), 36, 66)
    };
    let p = yellow(&mut r);
    assert!(p[0] > 150 && p[1] > 150 && p[2] < 80, "not yellow: {p:?}");

    // the buttons are a column at x 429..559, 40 apart from y 46
    let press = |r: &Run, i: f64| {
        r.guest
            .pointer(
                underweb_core::UW_POINTER_PRESS,
                1,
                480.0,
                46.0 + i * 40.0 + 17.0,
            )
            .unwrap()
    };
    press(&r, 5.0); // invert
    r.until(|r| r.logged("effect invert").is_some());
    assert!(r.logged("effect invert").is_some(), "{:?}", r.logs);
    std::thread::sleep(Duration::from_millis(200));
    let p = yellow(&mut r);
    assert!(
        p[0] < 100 && p[1] < 100 && p[2] > 150,
        "not inverted: {p:?}"
    );

    press(&r, 1.0); // mirror
    r.until(|r| r.logged("effect mirror").is_some());
    assert!(r.logged("effect mirror").is_some(), "{:?}", r.logs);
    r.guest.shutdown();
}

#[test]
fn camera_broadcast_goes_live() {
    let tmp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    {
        let received = received.clone();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                received.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
    }
    let dir = with_permissions(
        "camera-broadcast",
        tmp.path(),
        &format!("net.tcp.connect = [\"127.0.0.1:{port}\"]"),
    );
    let url = format!("http://127.0.0.1:{port}/camera.ogg");
    let mut r = start(
        &dir,
        &[("source", "test:ball"), ("url", &url), ("live", "1")],
        PermissionPolicy::AllowDeclared,
        None,
    );
    r.until(|r| r.logged("broadcasting").is_some() || r.logged("error").is_some());
    assert!(r.logged("broadcasting").is_some(), "{:?}", r.logs);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !received.lock().unwrap().windows(4).any(|w| w == b"OggS") {
        std::thread::sleep(Duration::from_millis(100));
    }
    r.guest.shutdown();
    let data = received.lock().unwrap();
    let head = String::from_utf8_lossy(&data[..data.len().min(400)]);
    assert!(head.starts_with("PUT /camera.ogg HTTP/1.1\r\n"), "{head}");
    assert!(data.windows(4).any(|w| w == b"OggS"), "no Ogg data");
}
