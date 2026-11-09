//! Video streams for a package: one sandboxed `underweb-media` process
//! each. The browser checks permissions, downloads `http(s)` sources into
//! the process, gives it an audio ring in the mixer, passes its frame
//! mailbox to the package, and sends broadcasts on to the server.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use underweb_audio::AudioRing;
use underweb_ipc::{Broadcast, Channel, FromMedia, HostMsg, MediaCmd, MediaEvent, ToMedia};
use underweb_pkg::Permission;

use crate::broker::Broker;
use crate::guest::Sound;

/// Largest http(s) video the browser will download for a page.
const MAX_DOWNLOAD: u64 = 2 << 30;

struct Proc {
    chan: Arc<Channel>,
    child: Mutex<underweb_sandbox::Process>,
    closed: AtomicBool,
    /// Where broadcast data goes.
    uplink: Mutex<Option<TcpStream>>,
    sound_ids: Mutex<Vec<u64>>,
}

pub(crate) struct Media {
    to_runner: Arc<Channel>,
    broker: Arc<Broker>,
    sound: Arc<Sound>,
    package_dir: PathBuf,
    sandbox: bool,
    procs: Mutex<HashMap<u32, Arc<Proc>>>,
}

pub fn media_path() -> PathBuf {
    if let Some(p) = std::env::var_os("UNDERWEB_MEDIA") {
        return p.into();
    }
    crate::runner_path().with_file_name(if cfg!(windows) {
        "underweb-media.exe"
    } else {
        "underweb-media"
    })
}

/// GStreamer plugins shipped next to the media service by `cargo xtask
/// dist` (`Contents/PlugIns` on macOS,
/// `lib/gstreamer-1.0` elsewhere).
fn bundled_plugins() -> Option<PathBuf> {
    let bin = media_path().parent()?.to_path_buf();
    [bin.join("../PlugIns"), bin.join("../lib/gstreamer-1.0")]
        .into_iter()
        .find(|d| d.is_dir())
        .and_then(|d| d.canonicalize().ok())
}

enum Source {
    File(String),
    Url(String),
    Camera,
    Test(String),
}

impl Media {
    pub(crate) fn new(
        to_runner: Arc<Channel>,
        broker: Arc<Broker>,
        sound: Arc<Sound>,
        package_dir: &Path,
        sandbox: bool,
    ) -> Arc<Media> {
        Arc::new(Media {
            to_runner,
            broker,
            sound,
            package_dir: package_dir.to_path_buf(),
            sandbox,
            procs: Mutex::new(HashMap::new()),
        })
    }

    fn event(&self, id: u32, event: MediaEvent) {
        let _ = self.to_runner.send(&HostMsg::Media { id, event }, None);
    }

    fn fail(&self, id: u32, msg: impl Into<String>) {
        self.event(id, MediaEvent::Error(msg.into()))
    }

    /// The package asks for a stream.
    pub(crate) fn open(
        self: &Arc<Self>,
        id: u32,
        source: String,
        looping: bool,
        broadcast: Option<Broadcast>,
    ) {
        let perms = self.broker.declared();
        let (src, mut needed) =
            if source == "camera" {
                match perms.camera() {
                    Some(p) => (Source::Camera, vec![p]),
                    None => {
                        return self.fail(
                            id,
                            "permission denied: camera is not declared in underweb.toml",
                        );
                    }
                }
            } else if let Some(pattern) = source.strip_prefix("test:") {
                if !pattern
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    return self.fail(id, format!("bad test pattern {pattern:?}"));
                }
                (Source::Test(pattern.into()), vec![])
            } else if source.starts_with("https://") || source.starts_with("http://") {
                match perms.fetch(&source) {
                    Some(p) => (Source::Url(source.clone()), vec![p]),
                    None => return self.fail(
                        id,
                        format!(
                            "permission denied: fetching {source} is not declared in underweb.toml"
                        ),
                    ),
                }
            } else {
                let p = Path::new(&source);
                if p.is_absolute() || p.components().any(|c| !matches!(c, Component::Normal(_))) {
                    return self.fail(
                        id,
                        format!("{source:?}: video files are given relative to the package"),
                    );
                }
                if !self.package_dir.join(p).is_file() {
                    return self.fail(id, format!("no file {source:?} in the package"));
                }
                (Source::File(source.clone()), vec![])
            };
        if let Some(b) = &broadcast {
            match parse_http_url(&b.url) {
                Ok((host, port, _)) => match perms.tcp_connect(&host, port) {
                    Some(p) => needed.push(p),
                    None => {
                        return self.fail(id, format!("permission denied: connecting to {host}:{port} is not declared in underweb.toml"));
                    }
                },
                Err(e) => return self.fail(id, format!("{e:#}")),
            }
        }
        self.gate_all(needed, id, src, looping, broadcast);
    }

    /// Ask for each permission in turn, then start.
    fn gate_all(
        self: &Arc<Self>,
        mut needed: Vec<Permission>,
        id: u32,
        src: Source,
        looping: bool,
        b: Option<Broadcast>,
    ) {
        let Some(p) = needed.pop() else {
            if let Err(e) = self.start(id, src, looping, b) {
                self.fail(id, format!("{e:#}"));
            }
            return;
        };
        let me = self.clone();
        self.broker.gate(
            p,
            Box::new(move |r| match r {
                Ok(()) => me.gate_all(needed, id, src, looping, b),
                Err(e) => me.fail(id, e),
            }),
        );
    }

    fn start(
        self: &Arc<Self>,
        id: u32,
        src: Source,
        looping: bool,
        broadcast: Option<Broadcast>,
    ) -> Result<()> {
        let uplink = match &broadcast {
            Some(b) => Some(connect_icecast(b)?),
            None => None,
        };
        let (mine, theirs) = Channel::pair()?;
        let spec = match &src {
            Source::File(f) => format!("file:{f}"),
            Source::Url(_) => "stream".into(),
            Source::Camera => "camera".into(),
            Source::Test(p) => format!("test:{p}"),
        };
        // plugins are scanned in-process; the result is cached where the
        // media service may write
        let registry = crate::Store::default_root().join("gstreamer");
        std::fs::create_dir_all(&registry)?;
        let mut cmd = crate::spawn::with_channel(&media_path(), &theirs)?;
        cmd.arg("--source")
            .arg(&spec)
            .arg("--package")
            .arg(&self.package_dir)
            .arg("--audio-rate")
            .arg(self.sound.rate().to_string())
            .env("GST_REGISTRY", registry.join("registry.bin"))
            .env("GST_REGISTRY_FORK", "no");
        let plugins = bundled_plugins();
        if let Some(dir) = &plugins {
            // an installed copy brings its own plugins; use only those
            cmd.env("GST_PLUGIN_SYSTEM_PATH_1_0", dir)
                .env("GST_PLUGIN_PATH_1_0", "");
        }
        if looping {
            cmd.arg("--loop");
        }
        if broadcast.is_some() {
            cmd.arg("--broadcast");
        }
        if self.sandbox {
            cmd.container = Some(underweb_sandbox::Container {
                name: format!("media:{}", self.package_dir.display()),
                policy: underweb_sandbox::Policy {
                    read_only: std::iter::once(self.package_dir.clone())
                        .chain(plugins)
                        .collect(),
                    read_write: vec![registry],
                    allow_exec: false,
                    camera: matches!(src, Source::Camera),
                },
                capabilities: if matches!(src, Source::Camera) {
                    vec!["webcam".into()]
                } else {
                    vec![]
                },
            });
        } else {
            cmd.arg("--no-sandbox");
        }
        let child = crate::spawn::start(&cmd, &mine)
            .with_context(|| format!("starting {}", media_path().display()))?;
        drop(theirs);
        let proc = Arc::new(Proc {
            chan: Arc::new(mine),
            child: Mutex::new(child),
            closed: AtomicBool::new(false),
            uplink: Mutex::new(uplink),
            sound_ids: Mutex::new(Vec::new()),
        });
        self.procs.lock().unwrap().insert(id, proc.clone());

        if let Source::Url(url) = src {
            let p = proc.clone();
            std::thread::spawn(move || {
                if let Err(e) = download(&url, &p.chan) {
                    eprintln!("video download {url}: {e:#}");
                }
                let _ = p.chan.send(&ToMedia::InputEnd, None);
            });
        }
        let me = self.clone();
        std::thread::spawn(move || me.read_loop(id, &proc));
        Ok(())
    }

    fn read_loop(&self, id: u32, p: &Proc) {
        loop {
            match p.chan.recv::<FromMedia>() {
                Ok(Some((FromMedia::Event(ev), fd))) => {
                    let _ = self
                        .to_runner
                        .send(&HostMsg::Media { id, event: ev }, fd.as_ref().map(raw));
                }
                Ok(Some((FromMedia::WantsAudio, _))) => {
                    let result = (|| -> Result<()> {
                        let ring = AudioRing::create(8192)?;
                        let fd = ring.shm().raw_handle();
                        let sid = self.sound.add_extra(ring);
                        p.sound_ids.lock().unwrap().push(sid);
                        p.chan.send(
                            &ToMedia::AudioReady {
                                capacity: 8192,
                                rate: self.sound.rate(),
                            },
                            Some(fd),
                        )
                    })();
                    if let Err(e) = result {
                        eprintln!("video sound: {e:#}");
                    }
                }
                Ok(Some((FromMedia::Encoded(bytes), _))) => {
                    let mut up = p.uplink.lock().unwrap();
                    if let Some(s) = up.as_mut()
                        && let Err(e) = s.write_all(&bytes)
                    {
                        *up = None;
                        self.fail(id, format!("broadcast stopped: {e}"));
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        for sid in p.sound_ids.lock().unwrap().drain(..) {
            self.sound.remove_extra(sid);
        }
        let _ = p.child.lock().unwrap().wait();
        if !p.closed.load(Ordering::Relaxed) {
            self.fail(id, "the video stopped");
        }
        self.procs.lock().unwrap().remove(&id);
    }

    pub(crate) fn cmd(&self, id: u32, cmd: MediaCmd) {
        let Some(p) = self.procs.lock().unwrap().get(&id).cloned() else {
            return;
        };
        if cmd == MediaCmd::Close {
            close(&p);
        } else {
            let _ = p.chan.send(&ToMedia::Cmd(cmd), None);
        }
    }

    pub(crate) fn close_all(&self) {
        for p in self.procs.lock().unwrap().values() {
            close(p);
        }
    }
}

fn close(p: &Proc) {
    p.closed.store(true, Ordering::Relaxed);
    let _ = p.chan.send(&ToMedia::Cmd(MediaCmd::Close), None);
    *p.uplink.lock().unwrap() = None;
    let mut child = p.child.lock().unwrap();
    for _ in 0..20 {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
}

fn raw(h: &underweb_ipc::OsHandle) -> underweb_ipc::RawOsHandle {
    #[cfg(unix)]
    use std::os::fd::AsRawFd as _;
    #[cfg(windows)]
    use std::os::windows::io::AsRawHandle as _;
    #[cfg(unix)]
    return h.as_raw_fd();
    #[cfg(windows)]
    return h.as_raw_handle();
}

/// Stream an http(s) source into the media process.
fn download(url: &str, to: &Channel) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut resp = agent.get(url).call()?;
    let mut body = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = body.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        to.send(&ToMedia::Input(buf[..n].to_vec()), None)?;
    }
}

/// `http://host:port/mount` -> (host, port, mount)
fn parse_http_url(url: &str) -> Result<(String, u16, String)> {
    let rest = url
        .strip_prefix("http://")
        .context("broadcast servers are http://host:port/mount")?;
    let (hostport, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().context("bad port")?),
        None => (hostport.to_string(), 80),
    };
    if host.is_empty() || path.is_empty() {
        bail!("broadcast servers are http://host:port/mount");
    }
    Ok((host, port, format!("/{path}")))
}

/// Icecast 2.4+ takes a source as an HTTP PUT with Basic auth.
fn connect_icecast(b: &Broadcast) -> Result<TcpStream> {
    let (host, port, mount) = parse_http_url(&b.url)?;
    let mut s = TcpStream::connect((host.as_str(), port))
        .with_context(|| format!("connecting to {host}:{port}"))?;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("source:{}", b.password));
    write!(
        s,
        "PUT {mount} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Basic {auth}\r\n\
         Content-Type: application/ogg\r\nIce-Public: 0\r\nIce-Name: underweb\r\n\
         User-Agent: underweb/{}\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    )?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            parse_http_url("http://h:8000/live.ogg").unwrap(),
            ("h".into(), 8000, "/live.ogg".into())
        );
        assert!(parse_http_url("https://h/x").is_err());
        assert!(parse_http_url("http://h:8000").is_err());
    }
}
