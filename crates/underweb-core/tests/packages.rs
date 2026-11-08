//! End-to-end: real runner processes, real sandbox.
//!
//! Needs the runner and the native example libraries:
//! `cargo xtask examples` (or `cargo xtask test`, which runs everything).
//! Snapshots of each package's first frame go to `target/snapshots/`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use underweb_core::{Frame, Guest, GuestEvent, SpawnOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn example(name: &str) -> PathBuf {
    root().join("examples").join(name)
}

#[derive(Default, Debug)]
struct Run {
    sandboxed: Option<bool>,
    title: String,
    logs: Vec<String>,
    errors: Vec<String>,
    frame: Option<Frame>,
    exited: bool,
}

/// Start a package at 700x450, wait until `done` says so (or 20 s).
fn run(name: &str, sandbox: bool, params: &[(&str, &str)], done: impl Fn(&Run) -> bool) -> Run {
    assert!(
        underweb_core::runner_path().exists(),
        "runner not built: run `cargo xtask examples` first"
    );
    let mut guest = Guest::spawn(
        example(name),
        SpawnOptions {
            sandbox,
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            data_dir: Some(std::env::temp_dir().join("underweb-test-data").join(name)),
            ..Default::default()
        },
    )
    .unwrap();
    guest.configure(700, 450, 1.0).unwrap();
    let mut r = Run::default();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !done(&r) && !r.exited {
        match guest.next_event(Duration::from_millis(100)) {
            Some(GuestEvent::Started { sandboxed }) => r.sandboxed = Some(sandboxed),
            Some(GuestEvent::Title(t)) => r.title = t,
            Some(GuestEvent::Log(l)) => r.logs.push(l),
            Some(GuestEvent::Error(e)) => r.errors.push(e),
            Some(GuestEvent::Frame(f)) => {
                if r.frame.is_none() {
                    snapshot(name, &f);
                }
                r.frame = Some(f);
            }
            Some(GuestEvent::Animating(_)) => {}
            Some(GuestEvent::Exited(_)) => r.exited = true,
            Some(GuestEvent::PermissionRequest { .. } | GuestEvent::Scene(_)) => {}
            None => {}
        }
    }
    if r.sandboxed.is_none() {
        // say why, for platforms we can only test in CI
        eprintln!(
            "{name}: the runner never started; errors {:?}, exit status {:?}",
            r.errors,
            guest.exit_status()
        );
    }
    guest.shutdown();
    r
}

fn snapshot(name: &str, f: &Frame) {
    let dir = root().join("target/snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    // back to cairo's BGRA for PNG output
    let mut data: Vec<u8> = f
        .rgba
        .chunks_exact(4)
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    let surface = cairo::ImageSurface::create_for_data(
        std::mem::take(&mut data),
        cairo::Format::ARgb32,
        f.width as i32,
        f.height as i32,
        f.width as i32 * 4,
    )
    .unwrap();
    let mut file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    surface.write_to_png(&mut file).unwrap();
}

fn pixel(f: &Frame, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * f.width + x) * 4) as usize;
    f.rgba[i..i + 4].try_into().unwrap()
}

/// cairo's colour rounding differs by one here and there.
#[track_caller]
fn assert_color(f: &Frame, x: u32, y: u32, want: [u8; 4]) {
    let got = pixel(f, x, y);
    let close = got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2);
    assert!(close, "pixel ({x}, {y}) is {got:?}, want {want:?}");
}

fn has_frame(r: &Run) -> bool {
    r.frame.is_some() && !r.title.is_empty()
}

fn probe_done(r: &Run) -> bool {
    r.logs.iter().any(|l| l == "PROBE done")
}

fn probe(r: &Run, name: &str) -> String {
    let prefix = format!("PROBE {name} ");
    r.logs
        .iter()
        .find_map(|l| l.strip_prefix(&prefix))
        .unwrap_or_else(|| {
            panic!(
                "no result for {name} in {:?} (errors {:?})",
                r.logs, r.errors
            )
        })
        .to_string()
}

#[test]
fn hello_js() {
    let r = run("hello-js", true, &[], has_frame);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.sandboxed, Some(true));
    assert_eq!(r.title, "JS shape");
    // inside the red rectangle, below the text
    assert_color(r.frame.as_ref().unwrap(), 250, 285, [255, 0, 0, 255]);
}

#[test]
fn hello_ts() {
    let r = run("hello-ts", true, &[], has_frame);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.title, "TS animation");
    // the label's pale fill
    assert_color(r.frame.as_ref().unwrap(), 640, 420, [242, 242, 204, 255]);
}

#[test]
fn hello_rust() {
    let r = run("hello-rust", true, &[], has_frame);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.title, "Rust shape");
    let f = r.frame.as_ref().unwrap();
    assert_color(f, 300, 290, [51, 153, 255, 255]); // the blue shape
    assert_color(f, 560, 200, [255, 204, 26, 255]); // centre of the cairo star
}

#[test]
fn hello_c() {
    let r = run("hello-c", true, &[], has_frame);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.title, "C circles");
    assert_color(r.frame.as_ref().unwrap(), 110, 230, [51, 51, 230, 255]);
}

struct ProbeTargets {
    _dir: PathBuf,
    secret: String,
    home: String,
    write_to: String,
    port: String,
    _listener: std::net::TcpListener,
}

fn probe_targets(tag: &str) -> ProbeTargets {
    let dir = std::env::temp_dir().join(format!("underweb-probe-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret.txt");
    std::fs::write(&secret, "top secret").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    ProbeTargets {
        secret: secret.to_string_lossy().into(),
        home: std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap(),
        write_to: dir.join("escaped.txt").to_string_lossy().into(),
        port: listener.local_addr().unwrap().port().to_string(),
        _dir: dir,
        _listener: listener,
    }
}

fn run_probe_rust(sandbox: bool) -> Run {
    let t = probe_targets(if sandbox { "on" } else { "off" });
    let pkg = example("probe-rust").to_string_lossy().into_owned();
    run(
        "probe-rust",
        sandbox,
        &[
            ("pkg", &pkg),
            ("secret", &t.secret),
            ("home", &t.home),
            ("write_to", &t.write_to),
            ("port", &t.port),
        ],
        probe_done,
    )
}

#[test]
fn sandbox_blocks_native_package() {
    let r = run_probe_rust(true);
    eprintln!("{}", r.logs.join("\n"));
    assert_eq!(r.sandboxed, Some(true));
    assert!(
        probe(&r, "read_own_package").starts_with("allowed"),
        "{:?}",
        r.logs
    );
    for name in [
        "read_outside",
        "list_home",
        "write_outside",
        "tcp_connect",
        "exec",
    ] {
        assert!(
            probe(&r, name).starts_with("denied"),
            "{name}: {:?}",
            r.logs
        );
    }
}

#[test]
fn without_sandbox_the_probe_gets_through() {
    // the control: the same probe succeeds when the sandbox is off, so the
    // test above fails for the right reason
    let r = run_probe_rust(false);
    assert_eq!(r.sandboxed, Some(false));
    for name in [
        "read_own_package",
        "read_outside",
        "list_home",
        "write_outside",
        "tcp_connect",
        "exec",
    ] {
        assert!(
            probe(&r, name).starts_with("allowed"),
            "{name}: {:?}",
            r.logs
        );
    }
}

#[test]
fn js_package_has_no_io_apis() {
    let t = probe_targets("js");
    let r = run("probe-js", true, &[("secret", &t.secret)], probe_done);
    eprintln!("{}", r.logs.join("\n"));
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    for name in [
        "Deno",
        "process",
        "require",
        "fetch",
        "XMLHttpRequest",
        "WebSocket",
        "Bun",
    ] {
        assert!(
            probe(&r, &format!("global_{name}")).starts_with("denied"),
            "{name}: {:?}",
            r.logs
        );
    }
    assert!(
        probe(&r, "import_outside").starts_with("denied"),
        "{:?}",
        r.logs
    );
    assert!(
        probe(&r, "import_remote").starts_with("denied"),
        "{:?}",
        r.logs
    );
}

#[test]
fn crashing_package_only_ends_its_own_process() {
    let start = Instant::now();
    let r = run("probe-rust", true, &[("crash", "1")], |_| false);
    assert!(r.exited, "runner should have exited");
    assert!(start.elapsed() < Duration::from_secs(10));
    // and the host can start another package right away
    let ok = run("hello-c", true, &[], has_frame);
    assert!(ok.frame.is_some());
}

#[test]
fn resize_reconfigures_frames() {
    let mut guest = Guest::spawn(example("hello-c"), SpawnOptions::default()).unwrap();
    guest.configure(300, 200, 1.0).unwrap();
    guest.configure(640, 480, 2.0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = None;
    while Instant::now() < deadline {
        if let Some(GuestEvent::Frame(f)) = guest.next_event(Duration::from_millis(100)) {
            last = Some((f.width, f.height));
            if last == Some((640, 480)) {
                break;
            }
        }
    }
    assert_eq!(last, Some((640, 480)));
    guest.shutdown();
}

#[test]
fn clicks_reach_the_shape_under_the_pointer() {
    for (pkg, x, y, want) in [
        ("hello-c", 110.0, 230.0, "circle 0 clicked"),
        ("hello-js", 250.0, 285.0, "clicked at 250 285"),
        ("hello-rust", 300.0, 290.0, "clicked 1 times"),
    ] {
        let mut guest = Guest::spawn(example(pkg), SpawnOptions::default()).unwrap();
        guest.configure(700, 450, 1.0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut clicked = false;
        let mut got = None;
        while Instant::now() < deadline && got.is_none() {
            match guest.next_event(Duration::from_millis(100)) {
                Some(GuestEvent::Frame(_)) if !clicked => {
                    guest
                        .pointer(underweb_core::UW_POINTER_PRESS, 1, x, y)
                        .unwrap();
                    guest
                        .pointer(underweb_core::UW_POINTER_RELEASE, 1, x, y)
                        .unwrap();
                    clicked = true;
                }
                Some(GuestEvent::Log(l)) if l == want => got = Some(l),
                _ => {}
            }
        }
        guest.shutdown();
        assert_eq!(got.as_deref(), Some(want), "{pkg}");
    }
}
