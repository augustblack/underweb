//! Remote packages: publish to a local static server, open by `uw://`.
//! Needs `cargo xtask examples` first (runner + prebuilt example libs).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use underweb_core::{Guest, GuestEvent, Origin, SpawnOptions, Store};
use underweb_pkg::{Package, PublishOptions, publish};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

/// Serve `dir` over HTTP on 127.0.0.1; returns the `host:port`.
fn serve(dir: &Path) -> String {
    let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
    let addr = server.server_addr().to_ip().unwrap().to_string();
    let dir = dir.to_path_buf();
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let path = dir.join(req.url().trim_start_matches('/'));
            let resp = match std::fs::read(&path) {
                Ok(b) if !req.url().contains("..") => tiny_http::Response::from_data(b).boxed(),
                _ => tiny_http::Response::empty(404).boxed(),
            };
            let _ = req.respond(resp);
        }
    });
    addr
}

/// Copy an example into a temp dir (without dist/ unless `with_dist`),
/// optionally rewriting its manifest.
fn copy_example(
    name: &str,
    to: &Path,
    with_dist: bool,
    edit: impl Fn(String) -> String,
) -> PathBuf {
    let src = root().join("examples").join(name);
    let dst = to.join(name);
    copy_dir(&src, &dst, with_dist);
    let m = dst.join("underweb.toml");
    std::fs::write(&m, edit(std::fs::read_to_string(&m).unwrap())).unwrap();
    dst
}

fn copy_dir(src: &Path, dst: &Path, with_dist: bool) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let name = e.file_name();
        if name == "target" || (name == "dist" && !with_dist) {
            continue;
        }
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst.join(&name), with_dist);
        } else {
            std::fs::copy(e.path(), dst.join(&name)).unwrap();
        }
    }
}

struct Env {
    tmp: tempfile::TempDir,
    repo: PathBuf,
    host: String,
    store: Store,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let host = serve(&repo);
    let store = Store::new(tmp.path().join("cache"));
    Env {
        tmp,
        repo,
        host,
        store,
    }
}

fn quiet(_: &str) {}

/// Open the resolved package and return its title once it has drawn.
fn first_title(dir: &Path) -> String {
    let mut g = Guest::spawn(dir, SpawnOptions::default()).unwrap();
    g.configure(400, 300, 1.0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let (mut title, mut framed) = (String::new(), false);
    while Instant::now() < deadline && (!framed || title.is_empty()) {
        match g.next_event(Duration::from_millis(100)) {
            Some(GuestEvent::Title(t)) => title = t,
            Some(GuestEvent::Frame(_)) => framed = true,
            Some(GuestEvent::Error(e)) => panic!("{e}"),
            _ => {}
        }
    }
    g.shutdown();
    assert!(framed, "no frame");
    title
}

#[test]
fn opens_published_js_and_follows_new_versions() {
    let e = env();
    let v1 = copy_example("hello-js", &e.tmp.path().join("v1"), false, |m| m);
    publish(
        &Package::open(&v1).unwrap(),
        &e.repo,
        PublishOptions::default(),
    )
    .unwrap();

    let addr = format!("uw://{}/hello-js", e.host);
    let r = e.store.open(&addr, &quiet).unwrap();
    assert!(matches!(&r.origin, Origin::Remote { version, .. } if version == "0.1.0"));
    assert_eq!(first_title(&r.dir), "JS shape");

    // publish 0.2.0 with a different title: the next open picks it up
    let v2 = copy_example("hello-js", &e.tmp.path().join("v2"), false, |m| {
        m.replace("0.1.0", "0.2.0")
    });
    let main = v2.join("main.js");
    std::fs::write(
        &main,
        std::fs::read_to_string(&main)
            .unwrap()
            .replace("JS shape", "JS shape v2"),
    )
    .unwrap();
    publish(
        &Package::open(&v2).unwrap(),
        &e.repo,
        PublishOptions::default(),
    )
    .unwrap();

    let r2 = e.store.open(&addr, &quiet).unwrap();
    assert_ne!(r.dir, r2.dir);
    assert_eq!(first_title(&r2.dir), "JS shape v2");
    // pinned
    let r1 = e.store.open(&format!("{addr}@0.1.0?x=1"), &quiet).unwrap();
    assert_eq!(r1.dir, r.dir);
    assert_eq!(r1.params, vec![("x".to_string(), "1".to_string())]);
}

#[test]
fn refuses_tampered_archive() {
    let e = env();
    let p = copy_example("hello-ts", e.tmp.path(), false, |m| m);
    let written = publish(
        &Package::open(&p).unwrap(),
        &e.repo,
        PublishOptions::default(),
    )
    .unwrap();
    let archive = written
        .iter()
        .find(|f| f.extension().is_some_and(|x| x == "zst"))
        .unwrap();
    let mut bytes = std::fs::read(archive).unwrap();
    let n = bytes.len();
    bytes[n / 2] ^= 0xff;
    std::fs::write(archive, bytes).unwrap();
    let err = e
        .store
        .open(&format!("uw://{}/hello-ts", e.host), &quiet)
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("does not match its hash"),
        "{err:#}"
    );
}

#[test]
fn works_offline_from_cache() {
    let e = env();
    let p = copy_example("hello-ts", e.tmp.path(), false, |m| m);
    publish(
        &Package::open(&p).unwrap(),
        &e.repo,
        PublishOptions::default(),
    )
    .unwrap();
    let addr = format!("uw://{}/hello-ts", e.host);
    let online = e.store.open(&addr, &quiet).unwrap();
    std::fs::remove_dir_all(&e.repo).unwrap(); // publisher gone (404s)
    let offline = e.store.open(&addr, &quiet).unwrap();
    assert_eq!(online.dir, offline.dir);
}

#[test]
fn prebuilt_native_package() {
    let e = env();
    let p = copy_example("hello-c", e.tmp.path(), true, |m| m);
    publish(
        &Package::open(&p).unwrap(),
        &e.repo,
        PublishOptions {
            source: false,
            target: true,
        },
    )
    .unwrap();
    let r = e
        .store
        .open(&format!("uw://{}/hello-c", e.host), &quiet)
        .unwrap();
    assert!(matches!(
        r.origin,
        Origin::Remote {
            built_from_source: false,
            ..
        }
    ));
    assert_eq!(first_title(&r.dir), "C circles");
}

#[test]
fn c_package_built_from_source_in_sandbox() {
    let e = env();
    let p = copy_example("hello-c", e.tmp.path(), false, |m| m);
    publish(
        &Package::open(&p).unwrap(),
        &e.repo,
        PublishOptions {
            source: true,
            target: false,
        },
    )
    .unwrap();
    let log = std::sync::Mutex::new(Vec::new());
    let r = e.store.open(&format!("uw://{}/hello-c", e.host), &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    let log = log.into_inner().unwrap().join("\n");
    let r = r.unwrap_or_else(|err| panic!("{err:#}\n{log}"));
    assert!(log.contains("sandboxed build of hello-c"), "{log}");
    assert!(matches!(
        r.origin,
        Origin::Remote {
            built_from_source: true,
            ..
        }
    ));
    assert_eq!(first_title(&r.dir), "C circles");
}

#[test]
fn rust_package_built_from_source_in_sandbox() {
    let e = env();
    let p = copy_example("hello-rust", e.tmp.path(), false, |m| m);
    publish(
        &Package::open(&p).unwrap(),
        &e.repo,
        PublishOptions {
            source: true,
            target: false,
        },
    )
    .unwrap();
    let log = std::sync::Mutex::new(Vec::new());
    let r = e.store.open(&format!("uw://{}/hello-rust", e.host), &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    let log = log.into_inner().unwrap().join("\n");
    let r = r.unwrap_or_else(|err| panic!("{err:#}\n{log}"));
    assert!(log.contains("sandboxed build of hello-rust"), "{log}");
    assert_eq!(first_title(&r.dir), "Rust shape");
}

/// A Rust source package whose build script tries to escape.
/// Outside everything the builder may write.
fn escape_path() -> PathBuf {
    std::env::temp_dir().join("underweb-evil-build")
}

fn hostile_build_package(dir: &Path, secret: &Path, port: u16) -> PathBuf {
    let outside = escape_path();
    let p = dir.join("evil-build");
    std::fs::create_dir_all(p.join("src")).unwrap();
    std::fs::write(
        p.join("underweb.toml"),
        "[package]\nname = \"evil-build\"\nversion = \"0.1.0\"\nkind = \"rust\"\n",
    )
    .unwrap();
    std::fs::write(
        p.join("Cargo.toml"),
        "[package]\nname = \"evil-build\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .unwrap();
    std::fs::write(
        p.join("build.rs"),
        format!(
            r#"fn main() {{
    let mut escaped = Vec::new();
    if std::fs::read({secret:?}).is_ok() {{ escaped.push("read secret"); }}
    if std::net::TcpStream::connect("127.0.0.1:{port}").is_ok() {{ escaped.push("network"); }}
    if std::fs::write({outside:?}, "x").is_ok() {{ escaped.push("write outside"); }}
    if !escaped.is_empty() {{ panic!("ESCAPED: {{escaped:?}}"); }}
    println!("cargo:warning=build script stayed inside the sandbox");
}}
"#
        ),
    )
    .unwrap();
    std::fs::write(
        p.join("src/lib.rs"),
        "#[unsafe(no_mangle)] pub extern \"C\" fn uw_abi_version() -> u32 { 1 }\n\
         #[unsafe(no_mangle)] pub extern \"C\" fn uw_activate(_h: *const u8) {}\n",
    )
    .unwrap();
    p
}

fn build_hostile(sandbox: bool) -> Result<(), String> {
    let e = env();
    let secret = e.tmp.path().join("secret.txt");
    std::fs::write(&secret, "top secret").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let _ = std::fs::remove_file(escape_path());
    let p = hostile_build_package(e.tmp.path(), &secret, port);
    let mut store = Store::new(e.tmp.path().join("cache"));
    store.sandbox = sandbox;
    let log = std::sync::Mutex::new(Vec::new());
    let result = store.build(&Package::open(&p).unwrap(), &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    let _ = std::fs::remove_file(escape_path());
    let log = log.into_inner().unwrap().join("\n");
    result
        .map(|_| log.clone())
        .map_err(|err| format!("{err:#}\n{log}"))
        .map(|_| ())
}

#[test]
fn build_scripts_are_sandboxed() {
    // inside the sandbox every escape attempt fails, so the build succeeds
    build_hostile(true).unwrap();
    // the control: without it, the build script gets out and fails the build
    let err = build_hostile(false).unwrap_err();
    assert!(err.contains("ESCAPED"), "{err}");
}
