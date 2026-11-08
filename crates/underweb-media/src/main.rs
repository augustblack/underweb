//! `underweb-media`: one process per video stream.
//!
//! ```text
//! underweb-media --ipc END --source SPEC [--package DIR] [--audio-rate HZ]
//!                [--loop] [--broadcast] [--no-sandbox]
//! ```
//!
//! SPEC is `file:<path in DIR>`, `stream` (bytes arrive from the browser),
//! `camera`, or `test:<videotestsrc pattern>`. Video decoders parse
//! untrusted data, so the process locks itself down (no network, no
//! files but the package's, no programs) right after building the
//! pipeline and before any media data is read. It reaches the outside
//! only through the browser: frames to the mailbox the package draws
//! from, sound to an audio ring, encoded broadcast data to the browser,
//! which sends it on.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use underweb_audio::AudioRing;
use underweb_ipc::{Channel, FromMedia, MediaCmd, MediaEvent, Shm, ToMedia, VideoMailbox};

struct Args {
    ipc: String,
    source: String,
    package: Option<PathBuf>,
    audio_rate: u32,
    looping: bool,
    broadcast: bool,
    sandbox: bool,
}

fn parse() -> Result<Args> {
    let mut a = Args {
        ipc: String::new(),
        source: String::new(),
        package: None,
        audio_rate: 0,
        looping: false,
        broadcast: false,
        sandbox: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--ipc" => a.ipc = val()?,
            "--source" => a.source = val()?,
            "--package" => a.package = Some(PathBuf::from(val()?)),
            "--audio-rate" => a.audio_rate = val()?.parse()?,
            "--loop" => a.looping = true,
            "--broadcast" => a.broadcast = true,
            "--no-sandbox" => a.sandbox = false,
            other => bail!("unknown argument {other:?}"),
        }
    }
    if a.ipc.is_empty() || a.source.is_empty() {
        bail!("usage: underweb-media --ipc END --source SPEC ...");
    }
    Ok(a)
}

/// State shared with GStreamer's streaming threads.
struct Shared {
    chan: Arc<Channel>,
    mailbox: Mutex<Option<VideoMailbox>>,
    ring: Mutex<Option<AudioRing>>,
    broadcasting: AtomicBool,
    has_audio: AtomicBool,
}

impl Shared {
    fn send(&self, m: FromMedia) {
        let _ = self.chan.send(&m, None);
    }
}

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("underweb-media: {e:#}");
            std::process::exit(2);
        }
    };
    let chan = match unsafe { Channel::from_arg(&args.ipc) } {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("underweb-media: {e:#}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&args, chan.clone()) {
        let _ = chan.send(&FromMedia::Event(MediaEvent::Error(format!("{e:#}"))), None);
        std::process::exit(1);
    }
}

fn make(name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(name)
        .build()
        .with_context(|| format!("GStreamer element {name} is missing"))
}

fn run(args: &Args, chan: Arc<Channel>) -> Result<()> {
    gst::init()?;
    // Apple's hardware decoders need services the sandbox doesn't allow;
    // the software ones do the job
    let registry = gst::Registry::get();
    for name in ["vtdec", "vtdec_hw"] {
        if let Some(f) = registry.find_feature(name, gst::ElementFactory::static_type()) {
            f.set_rank(gst::Rank::NONE);
        }
    }

    let shared = Arc::new(Shared {
        chan: chan.clone(),
        mailbox: Mutex::new(None),
        ring: Mutex::new(None),
        broadcasting: AtomicBool::new(false),
        has_audio: AtomicBool::new(false),
    });
    let pipeline = gst::Pipeline::new();
    let video_in = video_chain(&pipeline, &shared, args.broadcast)?;

    let mut appsrc = None;
    match args.source.as_str() {
        s if s.starts_with("test:") => {
            let src = gst::ElementFactory::make("videotestsrc")
                .property_from_str("pattern", &s[5..])
                .property("is-live", true)
                .build()
                .context("videotestsrc")?;
            let caps = gst::ElementFactory::make("capsfilter")
                .property(
                    "caps",
                    gst::Caps::builder("video/x-raw")
                        .field("width", 640)
                        .field("height", 360)
                        .field("framerate", gst::Fraction::new(30, 1))
                        .build(),
                )
                .build()?;
            pipeline.add_many([&src, &caps])?;
            gst::Element::link_many([&src, &caps, &video_in])?;
        }
        "camera" => {
            let src = make("autovideosrc")?;
            pipeline.add(&src)?;
            src.link(&video_in)?;
        }
        s => {
            let src = if let Some(rel) = s.strip_prefix("file:") {
                let dir = args
                    .package
                    .as_ref()
                    .context("file sources need --package")?;
                let path = dir.join(rel);
                if !path.starts_with(dir) || rel.contains("..") {
                    bail!("{rel} is outside the package");
                }
                gst::ElementFactory::make("filesrc")
                    .property("location", path.to_string_lossy().as_ref())
                    .build()?
            } else if s == "stream" {
                let src = gst_app::AppSrc::builder()
                    .format(gst::Format::Bytes)
                    .build();
                appsrc = Some(src.clone());
                src.upcast()
            } else {
                bail!("unknown source {s:?}");
            };
            let decode = make("decodebin")?;
            pipeline.add_many([&src, &decode])?;
            src.link(&decode)?;
            let (v, p, sh, rate) = (
                video_in.clone(),
                pipeline.clone(),
                shared.clone(),
                args.audio_rate,
            );
            let linked_video = Arc::new(AtomicBool::new(false));
            decode.connect_pad_added(move |_, pad| {
                let caps = pad.current_caps().or_else(|| Some(pad.query_caps(None)));
                let Some(name) = caps
                    .as_ref()
                    .and_then(|c| c.structure(0))
                    .map(|s| s.name().to_string())
                else {
                    return;
                };
                if name.starts_with("video/") && !linked_video.swap(true, Ordering::SeqCst) {
                    let _ = pad.link(&v.static_pad("sink").unwrap());
                } else if name.starts_with("audio/")
                    && rate > 0
                    && !sh.has_audio.swap(true, Ordering::SeqCst)
                {
                    match audio_chain(&p, &sh, rate) {
                        Ok(sink) => {
                            let _ = pad.link(&sink.static_pad("sink").unwrap());
                        }
                        Err(e) => eprintln!("underweb-media: no sound: {e:#}"),
                    }
                }
            });
        }
    }

    // Everything that needs broad file access is loaded; lock down before
    // the first byte of media is parsed.
    if args.sandbox {
        let read_only: Vec<PathBuf> = args.package.iter().cloned().collect();
        underweb_sandbox::lockdown(&underweb_sandbox::Policy {
            read_only,
            read_write: vec![],
            allow_exec: false,
        })?;
    }
    pipeline.set_state(gst::State::Paused)?;

    // bus: end of stream, errors
    {
        let (pipeline, shared, looping) = (pipeline.clone(), shared.clone(), args.looping);
        std::thread::spawn(move || {
            let bus = pipeline.bus().unwrap();
            loop {
                let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) else {
                    continue;
                };
                match msg.view() {
                    gst::MessageView::Eos(_) => {
                        if looping {
                            let _ = pipeline.seek_simple(
                                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                                gst::ClockTime::ZERO,
                            );
                        } else {
                            shared.send(FromMedia::Event(MediaEvent::Ended));
                        }
                    }
                    gst::MessageView::Error(e) => {
                        shared.send(FromMedia::Event(MediaEvent::Error(e.error().to_string())));
                        std::process::exit(1);
                    }
                    _ => {}
                }
            }
        });
    }
    // position and duration, for the page
    {
        let (pipeline, shared) = (pipeline.clone(), shared.clone());
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let pos = pipeline
                    .query_position::<gst::ClockTime>()
                    .map_or(0.0, |t| t.nseconds() as f64 / 1e9);
                let dur = pipeline
                    .query_duration::<gst::ClockTime>()
                    .map_or(0.0, |t| t.nseconds() as f64 / 1e9);
                if let Some(m) = shared.mailbox.lock().unwrap().as_ref() {
                    m.set_position(pos, dur);
                }
            }
        });
    }

    while let Some((msg, fd)) = chan.recv::<ToMedia>()? {
        match msg {
            ToMedia::Cmd(MediaCmd::Play) => {
                pipeline.set_state(gst::State::Playing)?;
            }
            ToMedia::Cmd(MediaCmd::Pause) => {
                pipeline.set_state(gst::State::Paused)?;
            }
            ToMedia::Cmd(MediaCmd::Seek(s)) => {
                let t = gst::ClockTime::from_nseconds((s.max(0.0) * 1e9) as u64);
                let _ = pipeline.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, t);
            }
            ToMedia::Cmd(MediaCmd::Close) => break,
            ToMedia::Input(bytes) => {
                if let Some(src) = &appsrc {
                    let _ = src.push_buffer(gst::Buffer::from_mut_slice(bytes));
                }
            }
            ToMedia::InputEnd => {
                if let Some(src) = &appsrc {
                    let _ = src.end_of_stream();
                }
            }
            ToMedia::AudioReady { capacity, .. } => {
                if let Some(fd) = fd {
                    let shm = Shm::from_handle(fd, AudioRing::bytes_for(capacity as usize))?;
                    *shared.ring.lock().unwrap() = Some(AudioRing::open(shm)?);
                }
            }
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    Ok(())
}

/// videoconvert → BGRA → (tee → encoder for broadcasts) → appsink writing
/// the mailbox. Returns the element to link sources to.
fn video_chain(
    pipeline: &gst::Pipeline,
    shared: &Arc<Shared>,
    broadcast: bool,
) -> Result<gst::Element> {
    let convert = make("videoconvert")?;
    let caps = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("format", "BGRA")
                .build(),
        )
        .build()?;
    let sink = gst_app::AppSink::builder()
        .sync(true)
        .max_buffers(2)
        .drop(true)
        .build();
    let sh = shared.clone();
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |s| {
                let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                frame(&sh, &sample);
                Ok(gst::FlowSuccess::Ok)
            })
            .new_preroll(move |s| {
                let _ = s.pull_preroll();
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    pipeline.add_many([&convert, &caps])?;
    pipeline.add(&sink)?;
    gst::Element::link_many([&convert, &caps])?;
    if broadcast {
        let tee = make("tee")?;
        let (q1, q2) = (make("queue")?, make("queue")?);
        let conv2 = make("videoconvert")?;
        let enc = make("theoraenc")?;
        let mux = make("oggmux")?;
        let out = gst_app::AppSink::builder().sync(false).build();
        let sh = shared.clone();
        out.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) {
                        if !sh.broadcasting.swap(true, Ordering::SeqCst) {
                            sh.send(FromMedia::Event(MediaEvent::Broadcasting));
                        }
                        sh.send(FromMedia::Encoded(map.as_slice().to_vec()));
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        pipeline.add_many([&tee, &q1, &q2, &conv2, &enc, &mux])?;
        pipeline.add(&out)?;
        gst::Element::link_many([&caps, &tee])?;
        gst::Element::link_many([&tee, &q1, sink.upcast_ref()])?;
        gst::Element::link_many([&tee, &q2, &conv2, &enc, &mux, out.upcast_ref()])?;
    } else {
        caps.link(&sink)?;
    }
    Ok(convert)
}

fn frame(shared: &Shared, sample: &gst::Sample) {
    let Some(s) = sample
        .caps()
        .and_then(|c| c.structure(0).map(|s| s.to_owned()))
    else {
        return;
    };
    let (Ok(w), Ok(h)) = (s.get::<i32>("width"), s.get::<i32>("height")) else {
        return;
    };
    let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) else {
        return;
    };
    let mut mb = shared.mailbox.lock().unwrap();
    if mb
        .as_ref()
        .is_none_or(|m| m.width != w as u32 || m.height != h as u32)
    {
        match VideoMailbox::create(w as u32, h as u32) {
            Ok(m) => {
                let ready = MediaEvent::Ready {
                    width: w as u32,
                    height: h as u32,
                    duration: 0.0,
                    has_audio: shared.has_audio.load(Ordering::SeqCst),
                };
                let _ = shared
                    .chan
                    .send(&FromMedia::Event(ready), Some(m.raw_handle()));
                *mb = Some(m);
            }
            Err(e) => {
                shared.send(FromMedia::Event(MediaEvent::Error(format!("{e:#}"))));
                return;
            }
        }
    }
    let stride = map.len() / h as usize;
    mb.as_ref().unwrap().write_frame(map.as_slice(), stride);
}

/// Decoded sound → the browser's sample format → the ring (once the
/// browser has sent one; until then, or when full, it's dropped).
fn audio_chain(pipeline: &gst::Pipeline, shared: &Arc<Shared>, rate: u32) -> Result<gst::Element> {
    let q = make("queue")?;
    let conv = make("audioconvert")?;
    let res = make("audioresample")?;
    let caps = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("format", "F32LE")
                .field("layout", "interleaved")
                .field("channels", 2)
                .field("rate", rate as i32)
                .build(),
        )
        .build()?;
    let sink = gst_app::AppSink::builder().sync(true).build();
    let sh = shared.clone();
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |s| {
                let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                if let (Some(map), Some(ring)) = (
                    sample.buffer().and_then(|b| b.map_readable().ok()),
                    sh.ring.lock().unwrap().as_ref(),
                ) {
                    let floats: Vec<f32> = map
                        .as_slice()
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .collect();
                    // wait a little for room rather than drop sound; the
                    // pipeline clock keeps this close to real time anyway
                    let mut done = 0;
                    for _ in 0..100 {
                        done += ring.write(&floats[done * 2..]);
                        if done * 2 >= floats.len() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    pipeline.add_many([&q, &conv, &res, &caps])?;
    pipeline.add(&sink)?;
    gst::Element::link_many([&q, &conv, &res, &caps, sink.upcast_ref()])?;
    for e in [&q, &conv, &res, &caps] {
        e.sync_state_with_parent()?;
    }
    sink.sync_state_with_parent()?;
    shared.send(FromMedia::WantsAudio);
    Ok(q)
}
