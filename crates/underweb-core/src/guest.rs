use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use underweb_audio::AudioRing;
use underweb_audio::mixer::{Mixer, Source};
use underweb_ipc::{Channel, EditOp, GuestMsg, HostMsg, SceneDoc, Shm, argb32_stride};
use underweb_pkg::{Package, Permission};

use crate::broker::{Broker, Decision, GrantState, PermissionPolicy};
use crate::paths::{data_dir, grants_file, runner_path};

pub struct SpawnOptions {
    /// Turn the OS sandbox off (development only; the UI should say so).
    pub sandbox: bool,
    /// `?key=value` parameters passed to the page.
    pub params: Vec<(String, String)>,
    /// Defaults to [`data_dir`] for the package name.
    pub data_dir: Option<PathBuf>,
    /// Called from the reader thread whenever an event is queued, e.g. to
    /// wake the UI.
    pub wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Undecided permissions: ask (the default), grant or refuse.
    pub permissions: PermissionPolicy,
    /// Who the package is, for remembering grants: its origin (the index
    /// URL for remote packages). Defaults to the package directory.
    pub identity: Option<String>,
    /// Where "always allow" grants are stored. `None` with
    /// `remember_grants: false` keeps them in memory.
    pub grants_dir: Option<PathBuf>,
    pub remember_grants: bool,
    /// Where the package's sound goes; defaults to the sound card
    /// ([`crate::default_mixer`]).
    pub mixer: Option<Arc<Mixer>>,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        SpawnOptions {
            sandbox: true,
            params: Vec::new(),
            data_dir: None,
            wake: None,
            permissions: PermissionPolicy::Ask,
            identity: None,
            grants_dir: None,
            remember_grants: true,
            mixer: None,
        }
    }
}

/// A finished frame, RGBA with premultiplied alpha, physical pixels.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

#[derive(Debug, Clone)]
pub enum GuestEvent {
    Started {
        sandboxed: bool,
    },
    Title(String),
    Frame(Frame),
    Animating(bool),
    Log(String),
    Error(String),
    /// The runner process is gone. `None` if killed by a signal.
    Exited(Option<i32>),
    /// The package used a declared permission nobody has decided on yet.
    /// Answer with [`Guest::answer`]; its request waits until then.
    PermissionRequest {
        id: u64,
        permission: Permission,
        /// "connect to example.org on port 7000"
        description: String,
    },
    /// The page's scene, while the layout editor is open
    /// ([`Guest::set_edit_mode`]).
    Scene(SceneDoc),
}

struct Buffers {
    generation: u32,
    width: u32,
    height: u32,
    stride: u32,
    shm: Shm,
}

pub struct Guest {
    pub package: Package,
    chan: Arc<Channel>,
    child: underweb_sandbox::Process,
    buffers: Arc<Mutex<Option<Buffers>>>,
    generation: u32,
    events: Receiver<GuestEvent>,
    /// Only the newest frame is kept: a slow consumer skips frames
    /// instead of queueing them (each can be tens of megabytes).
    latest: Arc<Mutex<Option<Frame>>>,
    broker: Arc<Broker>,
    sound: Arc<Sound>,
    media: Arc<crate::media::Media>,
}

/// The package's place in the mixer: its own sound, and its videos'.
pub(crate) struct Sound {
    mixer: Arc<Mixer>,
    source: Mutex<Option<Arc<Source>>>,
    extra: Mutex<Vec<Arc<Source>>>,
    muted: std::sync::atomic::AtomicBool,
}

/// Frames queued between the package and the sound card, at most.
const RING_FRAMES: usize = 8192;

impl Sound {
    fn start(&self, chan: &Channel) -> Result<()> {
        let mut src = self.source.lock().unwrap();
        if src.is_none() {
            let ring = AudioRing::create(RING_FRAMES)?;
            let fd = ring.shm().raw_handle();
            let s = self.mixer.add(ring);
            s.set_gain(if self.muted.load(std::sync::atomic::Ordering::Relaxed) {
                0.0
            } else {
                1.0
            });
            chan.send(
                &HostMsg::AudioReady {
                    capacity: RING_FRAMES as u32,
                },
                Some(fd),
            )?;
            *src = Some(s);
        }
        Ok(())
    }

    fn stop(&self) {
        if let Some(s) = self.source.lock().unwrap().take() {
            self.mixer.remove(s.id);
        }
    }

    fn gain(&self) -> f32 {
        if self.muted.load(std::sync::atomic::Ordering::Relaxed) {
            0.0
        } else {
            1.0
        }
    }

    pub(crate) fn rate(&self) -> u32 {
        self.mixer.rate
    }

    /// Another source for this package (a video's sound); muted with it.
    pub(crate) fn add_extra(&self, ring: AudioRing) -> u64 {
        let s = self.mixer.add(ring);
        s.set_gain(self.gain());
        let id = s.id;
        self.extra.lock().unwrap().push(s);
        id
    }

    pub(crate) fn remove_extra(&self, id: u64) {
        self.mixer.remove(id);
        self.extra.lock().unwrap().retain(|s| s.id != id);
    }
}

impl Guest {
    pub fn spawn(package_dir: impl AsRef<Path>, opts: SpawnOptions) -> Result<Guest> {
        let package = Package::open(package_dir)?;
        let data = opts
            .data_dir
            .clone()
            .unwrap_or_else(|| data_dir(package.name()));
        std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;

        let (host_end, guest_end) = Channel::pair()?;
        let runner = runner_path();
        let mut cmd = crate::spawn::with_channel(&runner, &guest_end)?;
        cmd.arg("--package")
            .arg(&package.dir)
            .arg("--data")
            .arg(&data);
        if opts.sandbox {
            // Windows applies this at creation; elsewhere the runner
            // locks itself down with the same policy
            cmd.container = Some(underweb_sandbox::Container {
                name: package.name().to_string(),
                policy: underweb_sandbox::Policy {
                    read_only: vec![package.dir.clone()],
                    read_write: vec![data.clone()],
                    allow_exec: false,
                },
                capabilities: vec![],
            });
        } else {
            cmd.arg("--no-sandbox");
        }
        for (k, v) in &opts.params {
            cmd.arg("--param").arg(format!("{k}={v}"));
        }
        let mixer = opts
            .mixer
            .clone()
            .unwrap_or_else(crate::audio::default_mixer);
        cmd.arg("--audio-rate").arg(mixer.rate.to_string());
        let child = crate::spawn::start(&cmd, &host_end)
            .with_context(|| format!("starting {}", runner.display()))?;
        drop(guest_end);

        let chan = Arc::new(host_end);
        let buffers: Arc<Mutex<Option<Buffers>>> = Arc::new(Mutex::new(None));
        let (tx, events) = mpsc::channel();
        let identity = opts
            .identity
            .clone()
            .unwrap_or_else(|| package.dir.display().to_string());
        let grants = opts
            .remember_grants
            .then(|| grants_file(opts.grants_dir.as_deref(), package.name(), &identity));
        let broker = Broker::new(
            chan.clone(),
            package.manifest.permissions.clone(),
            opts.permissions,
            grants,
            tx.clone(),
            opts.wake.clone(),
        );
        let sound = Arc::new(Sound {
            mixer,
            source: Mutex::new(None),
            extra: Mutex::new(Vec::new()),
            muted: Default::default(),
        });
        let media = crate::media::Media::new(
            chan.clone(),
            broker.clone(),
            sound.clone(),
            &package.dir,
            opts.sandbox,
        );
        let latest: Arc<Mutex<Option<Frame>>> = Arc::new(Mutex::new(None));
        {
            let chan = chan.clone();
            let buffers = buffers.clone();
            let wake = opts.wake.clone();
            let broker = broker.clone();
            let sound = sound.clone();
            let media = media.clone();
            let latest = latest.clone();
            std::thread::Builder::new()
                .name(format!("guest {}", package.name()))
                .spawn(move || {
                    read_loop(
                        &chan,
                        &buffers,
                        &broker,
                        &sound,
                        &media,
                        &latest,
                        &tx,
                        wake.as_deref(),
                    )
                })?;
        }
        Ok(Guest {
            package,
            chan,
            child,
            buffers,
            generation: 0,
            events,
            latest,
            broker,
            sound,
            media,
        })
    }

    /// Size the page in physical pixels. Allocates fresh frame buffers.
    pub fn configure(&mut self, width: u32, height: u32, scale: f64) -> Result<()> {
        let (width, height) = (width.max(1), height.max(1));
        let stride = argb32_stride(width);
        let shm = Shm::create(stride as usize * height as usize * 2)?;
        self.generation += 1;
        let generation = self.generation;
        let fd = shm.raw_handle();
        *self.buffers.lock().unwrap() = Some(Buffers {
            generation,
            width,
            height,
            stride,
            shm,
        });
        self.chan.send(
            &HostMsg::Configure {
                generation,
                width,
                height,
                stride,
                scale,
            },
            Some(fd),
        )
    }

    pub fn tick(&self, dt: f64) -> Result<()> {
        self.chan.send(&HostMsg::Tick { dt }, None)
    }

    /// Pointer event in logical pixels. `kind` is a `UW_POINTER_*` value.
    pub fn pointer(&self, kind: u32, button: u32, x: f64, y: f64) -> Result<()> {
        self.chan
            .send(&HostMsg::Pointer { kind, button, x, y }, None)
    }

    /// Open or close the layout editor: while open, the page gets no
    /// pointer events and its scene arrives as [`GuestEvent::Scene`].
    pub fn set_edit_mode(&self, on: bool) -> Result<()> {
        self.chan.send(&HostMsg::EditMode(on), None)
    }

    /// Change the page's scene from the layout editor.
    pub fn edit(&self, op: EditOp) -> Result<()> {
        self.chan.send(&HostMsg::Edit(Box::new(op)), None)
    }

    /// Run a package without a window just long enough to read its scene
    /// (for exporting it). Permission requests are refused.
    pub fn read_scene(package_dir: impl AsRef<Path>, opts: SpawnOptions) -> Result<SceneDoc> {
        let mut g = Guest::spawn(package_dir, opts)?;
        let result = (|| {
            g.configure(960, 640, 1.0)?;
            g.set_edit_mode(true)?;
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            let mut doc = None;
            // a page may build its scene over a few events; wait until it settles
            let mut quiet_since = std::time::Instant::now();
            while std::time::Instant::now() < deadline {
                match g.next_event(Duration::from_millis(50)) {
                    Some(GuestEvent::Scene(d)) => {
                        doc = Some(d);
                        quiet_since = std::time::Instant::now();
                    }
                    Some(GuestEvent::Error(e)) => anyhow::bail!("the page failed: {e}"),
                    Some(GuestEvent::Exited(_)) => break,
                    Some(GuestEvent::PermissionRequest { id, .. }) => g.answer(id, Decision::Deny),
                    _ => {}
                }
                if doc.is_some() && quiet_since.elapsed() > Duration::from_millis(400) {
                    break;
                }
            }
            doc.context("the page showed nothing")
        })();
        g.shutdown();
        result
    }

    pub fn try_event(&self) -> Option<GuestEvent> {
        self.events.try_recv().ok().map(|e| self.newest(e))
    }

    /// Wait up to `timeout` for the next event.
    pub fn next_event(&self, timeout: Duration) -> Option<GuestEvent> {
        self.events
            .recv_timeout(timeout)
            .ok()
            .map(|e| self.newest(e))
    }

    /// A frame event stands for "there's a frame": hand out the newest.
    fn newest(&self, e: GuestEvent) -> GuestEvent {
        match e {
            GuestEvent::Frame(_) => self
                .latest
                .lock()
                .unwrap()
                .take()
                .map_or(e, GuestEvent::Frame),
            e => e,
        }
    }

    /// Silence (or unsilence) this package.
    pub fn set_muted(&self, muted: bool) {
        self.sound
            .muted
            .store(muted, std::sync::atomic::Ordering::Relaxed);
        let gain = self.sound.gain();
        if let Some(s) = self.sound.source.lock().unwrap().as_ref() {
            s.set_gain(gain);
        }
        for s in self.sound.extra.lock().unwrap().iter() {
            s.set_gain(gain);
        }
    }

    pub fn is_muted(&self) -> bool {
        self.sound.muted.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Frames of this package's sound the output has consumed so far.
    pub fn frames_played(&self) -> u64 {
        self.sound
            .source
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |s| s.played.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Whether the package has asked for sound (and not stopped).
    pub fn has_sound(&self) -> bool {
        self.sound.source.lock().unwrap().is_some() || !self.sound.extra.lock().unwrap().is_empty()
    }

    /// Answer a [`GuestEvent::PermissionRequest`].
    pub fn answer(&self, id: u64, decision: Decision) {
        self.broker.answer(id, decision)
    }

    /// The package's declared permissions and the decision on each.
    pub fn permissions(&self) -> Vec<(Permission, GrantState)> {
        self.broker.permissions()
    }

    /// Forget the decision on a permission; the package will be asked
    /// again next time it uses it.
    pub fn forget(&self, permission: &Permission) {
        self.broker.forget(permission)
    }

    pub fn shutdown(mut self) {
        self.broker.close_all();
        self.media.close_all();
        self.sound.stop();
        let _ = self.chan.send(&HostMsg::Shutdown, None);
        for _ in 0..50 {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        self.broker.close_all();
        self.media.close_all();
        self.sound.stop();
        if let Ok(None) = self.child.try_wait() {
            let _ = self.chan.send(&HostMsg::Shutdown, None);
            std::thread::sleep(Duration::from_millis(20));
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[allow(clippy::too_many_arguments)] // the reader thread's whole world
fn read_loop(
    chan: &Channel,
    buffers: &Mutex<Option<Buffers>>,
    broker: &Arc<Broker>,
    sound: &Sound,
    media: &Arc<crate::media::Media>,
    latest: &Mutex<Option<Frame>>,
    tx: &mpsc::Sender<GuestEvent>,
    wake: Option<&(dyn Fn() + Send + Sync)>,
) {
    let send = |e: GuestEvent| {
        let _ = tx.send(e);
        if let Some(w) = wake {
            w();
        }
    };
    loop {
        let msg = match chan.recv::<GuestMsg>() {
            Ok(Some((msg, _))) => msg,
            Ok(None) => break,
            Err(e) => {
                send(GuestEvent::Error(format!("ipc: {e:#}")));
                break;
            }
        };
        match msg {
            GuestMsg::Hello { sandboxed, .. } => send(GuestEvent::Started { sandboxed }),
            GuestMsg::Title(t) => send(GuestEvent::Title(t)),
            GuestMsg::Animating(on) => send(GuestEvent::Animating(on)),
            GuestMsg::Log(l) => send(GuestEvent::Log(l)),
            GuestMsg::Error(e) => send(GuestEvent::Error(e)),
            GuestMsg::Scene(doc) => send(GuestEvent::Scene(doc)),
            GuestMsg::Net { handle, op } => broker.request(handle, op),
            GuestMsg::AudioStart => {
                if let Err(e) = sound.start(chan) {
                    send(GuestEvent::Log(format!("no sound: {e:#}")));
                }
            }
            GuestMsg::AudioStop => sound.stop(),
            GuestMsg::MediaOpen {
                id,
                source,
                looping,
                broadcast,
            } => media.open(id, source, looping, broadcast),
            GuestMsg::Media { id, cmd } => media.cmd(id, cmd),
            GuestMsg::Frame { generation, index } => {
                let frame = {
                    let guard = buffers.lock().unwrap();
                    match guard.as_ref() {
                        Some(b) if b.generation == generation && index < 2 => {
                            Some(copy_frame(b, index))
                        }
                        _ => None, // drawn into buffers we've replaced
                    }
                };
                if let Some(frame) = frame {
                    let _ = chan.send(&HostMsg::Release { generation, index }, None);
                    let first = {
                        let mut slot = latest.lock().unwrap();
                        let first = slot.is_none();
                        *slot = Some(frame);
                        first
                    };
                    if first {
                        send(GuestEvent::Frame(Frame {
                            width: 0,
                            height: 0,
                            rgba: Vec::new(),
                        }));
                    } else if let Some(w) = wake {
                        w();
                    }
                }
            }
        }
    }
    // the reader can't wait() on the child; report the socket closing and
    // let the owner collect the status
    broker.close_all();
    media.close_all();
    sound.stop();
    send(GuestEvent::Exited(None));
}

/// cairo ARGB32 is BGRA in memory on little-endian machines; convert to
/// RGBA (both premultiplied).
fn copy_frame(b: &Buffers, index: u32) -> Frame {
    let len = b.stride as usize * b.height as usize;
    let src = &b.shm.as_mut_slice()[index as usize * len..][..len];
    let mut rgba = Vec::with_capacity((b.width * b.height * 4) as usize);
    for row in src.chunks_exact(b.stride as usize) {
        for px in row[..(b.width * 4) as usize].chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    Frame {
        width: b.width,
        height: b.height,
        rgba,
    }
}

impl Guest {
    /// Exit status once the process is gone.
    pub fn exit_status(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}
