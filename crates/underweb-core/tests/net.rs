//! Networking through the broker: real sockets on 127.0.0.1, the JS and
//! Rust probe packages, permission prompts and remembered grants.
//! Needs `cargo xtask examples` first.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use underweb_core::{Decision, GrantState, Guest, GuestEvent, PermissionPolicy, SpawnOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

/// Copy an example (with dist/) and replace its [permissions].
fn with_permissions(name: &str, to: &Path, permissions: &str) -> PathBuf {
    let src = root().join("examples").join(name);
    let dst = to.join(name);
    copy_dir(&src, &dst);
    let m = dst.join("underweb.toml");
    let text = std::fs::read_to_string(&m).unwrap();
    let head = text.split("\n[permissions]").next().unwrap();
    std::fs::write(&m, format!("{head}\n[permissions]\n{permissions}\n")).unwrap();
    dst
}

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

struct Servers {
    tcp: u16,
    udp: u16,
    http: u16,
}

/// TCP and UDP echo servers and an HTTP server answering "hi".
fn servers() -> Servers {
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_port = tcp.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in tcp.incoming().flatten() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 || s.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_port = udp.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok((n, from)) = udp.recv_from(&mut buf) {
            let mut reply = b"pong:".to_vec();
            reply.extend_from_slice(&buf[..n]);
            let _ = udp.send_to(&reply, from);
        }
    });
    let http = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let http_port = http.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for req in http.incoming_requests() {
            let _ = req.respond(tiny_http::Response::from_string("hi"));
        }
    });
    Servers {
        tcp: tcp_port,
        udp: udp_port,
        http: http_port,
    }
}

struct Run {
    logs: Vec<String>,
    prompts: Vec<String>,
}

/// Run until `done(logs)`; answer prompts with `answer`.
fn run(
    dir: &Path,
    params: Vec<(String, String)>,
    opts: SpawnOptions,
    answer: Option<Decision>,
    done: impl Fn(&[String]) -> bool,
) -> (Run, Guest) {
    let mut g = Guest::spawn(dir, SpawnOptions { params, ..opts }).unwrap();
    g.configure(300, 200, 1.0).unwrap();
    let mut r = Run {
        logs: Vec::new(),
        prompts: Vec::new(),
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !done(&r.logs) {
        match g.next_event(Duration::from_millis(100)) {
            Some(GuestEvent::Log(l)) => r.logs.push(l),
            Some(GuestEvent::Error(e)) => panic!("{e}\n{:?}", r.logs),
            Some(GuestEvent::PermissionRequest {
                id, description, ..
            }) => {
                r.prompts.push(description);
                if let Some(d) = answer {
                    g.answer(id, d);
                }
            }
            Some(GuestEvent::Exited(_)) => break,
            _ => {}
        }
    }
    (r, g)
}

fn find<'a>(logs: &'a [String], prefix: &str) -> &'a str {
    logs.iter()
        .find_map(|l| l.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} in {logs:?}"))
}

fn js_probe(tmp: &Path, s: &Servers) -> (PathBuf, Vec<(String, String)>) {
    let dir = with_permissions(
        "probe-net-js",
        tmp,
        &format!(
            "net.tcp.connect = [\"127.0.0.1:{}\"]\nnet.udp.send = [\"127.0.0.1:{}\"]\nfetch = [\"http://127.0.0.1:{}/\"]",
            s.tcp, s.udp, s.http
        ),
    );
    let params = vec![
        ("tcp".to_string(), s.tcp.to_string()),
        ("udp".to_string(), s.udp.to_string()),
        ("http".to_string(), s.http.to_string()),
    ];
    (dir, params)
}

fn done(logs: &[String]) -> bool {
    logs.iter().any(|l| l == "PROBE done")
}

#[test]
fn js_package_uses_network_and_storage_through_the_broker() {
    let tmp = tempfile::tempdir().unwrap();
    let s = servers();
    let (dir, params) = js_probe(tmp.path(), &s);
    let opts = || SpawnOptions {
        permissions: PermissionPolicy::AllowDeclared,
        data_dir: Some(tmp.path().join("data")),
        remember_grants: false,
        ..Default::default()
    };
    let (r, g) = run(&dir, params.clone(), opts(), None, done);
    g.shutdown();
    assert_eq!(find(&r.logs, "PROBE storage "), "1 count");
    assert_eq!(find(&r.logs, "PROBE tcp "), "hello");
    assert_eq!(find(&r.logs, "PROBE udp "), "pong:ping");
    assert_eq!(find(&r.logs, "PROBE fetch "), "200 hi");
    let undeclared = find(&r.logs, "PROBE undeclared ");
    assert!(undeclared.contains("not declared"), "{undeclared}");
    assert!(r.prompts.is_empty());

    // storage survives a restart
    let (r, g) = run(&dir, params, opts(), None, done);
    g.shutdown();
    assert_eq!(find(&r.logs, "PROBE storage "), "2 count");
}

#[test]
fn undecided_permissions_prompt_and_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let s = servers();
    let (dir, params) = js_probe(tmp.path(), &s);
    let opts = SpawnOptions {
        remember_grants: false,
        data_dir: Some(tmp.path().join("data")),
        ..Default::default()
    };
    let (r, g) = run(&dir, params, opts, Some(Decision::Deny), done);
    g.shutdown();
    // one prompt per declared permission used
    assert_eq!(r.prompts.len(), 3, "{:?}", r.prompts);
    assert!(
        r.prompts
            .iter()
            .any(|p| p == &format!("connect to 127.0.0.1 on port {}", s.tcp))
    );
    for step in ["tcp", "udp", "fetch"] {
        let msg = find(&r.logs, &format!("PROBE {step} "));
        assert!(msg.starts_with("error permission denied"), "{step}: {msg}");
    }
}

#[test]
fn always_allow_is_remembered_per_package() {
    let tmp = tempfile::tempdir().unwrap();
    let s = servers();
    let (dir, params) = js_probe(tmp.path(), &s);
    let opts = || SpawnOptions {
        grants_dir: Some(tmp.path().join("grants")),
        data_dir: Some(tmp.path().join("data")),
        identity: Some("uw://example.org/probe-net-js".into()),
        ..Default::default()
    };
    let (r, g) = run(
        &dir,
        params.clone(),
        opts(),
        Some(Decision::AllowAlways),
        done,
    );
    assert_eq!(r.prompts.len(), 3, "{:?} {:?}", r.prompts, r.logs);
    assert_eq!(find(&r.logs, "PROBE tcp "), "hello");
    assert!(
        g.permissions()
            .iter()
            .all(|(_, s)| *s == GrantState::AllowedAlways)
    );
    g.shutdown();

    // next time: no prompts
    let (r, g) = run(&dir, params.clone(), opts(), None, done);
    assert!(r.prompts.is_empty(), "{:?}", r.prompts);
    assert_eq!(find(&r.logs, "PROBE fetch "), "200 hi");
    // forgetting one brings its prompt back
    let tcp = g
        .permissions()
        .into_iter()
        .map(|(p, _)| p)
        .find(|p| p.key().starts_with("net.tcp"))
        .unwrap();
    g.forget(&tcp);
    g.shutdown();
    let (r, g) = run(
        &dir,
        params.clone(),
        opts(),
        Some(Decision::AllowOnce),
        done,
    );
    assert_eq!(r.prompts.len(), 1, "{:?}", r.prompts);
    g.shutdown();

    // the same package name from somewhere else starts from scratch
    let other = SpawnOptions {
        identity: Some("uw://evil.example/probe-net-js".into()),
        ..opts()
    };
    let (r, g) = run(&dir, params, other, Some(Decision::Deny), done);
    assert_eq!(r.prompts.len(), 3);
    g.shutdown();
}

#[test]
fn native_package_connects_listens_and_writes_its_data_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let s = servers();
    let listen_port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let dir = with_permissions(
        "probe-net-rust",
        tmp.path(),
        &format!(
            "net.tcp.connect = [\"127.0.0.1:{}\"]\nnet.tcp.listen = [{listen_port}]",
            s.tcp
        ),
    );
    let params = vec![
        ("tcp".into(), s.tcp.to_string()),
        ("listen".into(), listen_port.to_string()),
    ];
    let opts = SpawnOptions {
        permissions: PermissionPolicy::AllowDeclared,
        data_dir: Some(tmp.path().join("data")),
        remember_grants: false,
        ..Default::default()
    };
    let mut g = Guest::spawn(&dir, SpawnOptions { params, ..opts }).unwrap();
    g.configure(300, 200, 1.0).unwrap();
    let mut logs: Vec<String> = Vec::new();
    let mut client = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !logs.iter().any(|l| l == "PROBE accepted") {
        if let Some(GuestEvent::Log(l)) = g.next_event(Duration::from_millis(100)) {
            if l == "PROBE listening" {
                client = Some(TcpStream::connect(("127.0.0.1", listen_port)).unwrap());
            }
            logs.push(l);
        }
    }
    let mut buf = [0u8; 7];
    let mut client = client.expect("never listened");
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"welcome");
    while Instant::now() < deadline && !logs.iter().any(|l| l.starts_with("PROBE tcp")) {
        if let Some(GuestEvent::Log(l)) = g.next_event(Duration::from_millis(100)) {
            logs.push(l);
        }
    }
    g.shutdown();
    assert_eq!(find(&logs, "PROBE storage "), "1 ok");
    assert_eq!(find(&logs, "PROBE tcp "), "hello");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("data/visits")).unwrap(),
        "1"
    );
}
