//! Native (Rust or C) packages: a shared library called through the C ABI
//! in `underweb-abi`.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use libloading::Library;
use underweb_abi::*;
use underweb_guest::{PathOp, Rgba, Scene, Shape, cairo};
use underweb_ipc::{Channel, GuestMsg, NetEvent, NetEventKind, NetOp};

use crate::audio::Audio;
use crate::media::MediaCtl;
use crate::net::{Net, parse_headers};
use crate::page::Page;
use underweb_audio::pd::{self, PdMessage, PdValue};
use underweb_audio::ugens::Wave;
use underweb_ipc::{Broadcast, MediaCmd, MediaEvent};

struct Ctx {
    scene: Rc<RefCell<Scene>>,
    /// Owns the string last returned by `param`.
    param: RefCell<Option<CString>>,
    /// Raw pointer to the channel, alive for the whole run.
    chan: *const Channel,
    net: Net,
    data_dir: CString,
    audio: Rc<Audio>,
    media: MediaCtl,
}

pub struct NativePage {
    host: Box<UwHost>,
    _ctx: Box<Ctx>,
    activate: UwActivateFn,
    update: Option<UwUpdateFn>,
    pointer: Option<UwPointerFn>,
    draw: Option<UwDrawFn>,
    deactivate: Option<UwDeactivateFn>,
    net: Option<UwNetFn>,
    pd: Option<UwPdFn>,
    media: Option<UwMediaFn>,
    // last: unloading must happen after everything above
    _lib: Library,
}

impl NativePage {
    pub fn load(
        path: &Path,
        scene: Rc<RefCell<Scene>>,
        chan: &Channel,
        data: &Path,
        audio: Rc<Audio>,
    ) -> Result<NativePage> {
        let lib =
            unsafe { Library::new(path) }.with_context(|| format!("loading {}", path.display()))?;
        unsafe {
            let version: UwAbiVersionFn =
                *lib.get(b"uw_abi_version\0").context("uw_abi_version")?;
            let v = version();
            if !(UW_ABI_MIN_VERSION..=UW_ABI_VERSION).contains(&v) {
                bail!(
                    "package built for ABI {v}; this runner supports {UW_ABI_MIN_VERSION} to {UW_ABI_VERSION}"
                );
            }
            let activate: UwActivateFn = *lib.get(b"uw_activate\0").context("uw_activate")?;
            let update = lib.get::<UwUpdateFn>(b"uw_update\0").ok().map(|s| *s);
            let pointer = lib.get::<UwPointerFn>(b"uw_pointer\0").ok().map(|s| *s);
            let draw = lib.get::<UwDrawFn>(b"uw_draw\0").ok().map(|s| *s);
            let deactivate = lib
                .get::<UwDeactivateFn>(b"uw_deactivate\0")
                .ok()
                .map(|s| *s);
            let net = lib.get::<UwNetFn>(b"uw_net\0").ok().map(|s| *s);
            let pd = lib.get::<UwPdFn>(b"uw_pd\0").ok().map(|s| *s);
            let media = lib.get::<UwMediaFn>(b"uw_media\0").ok().map(|s| *s);

            let mut ctx = Box::new(Ctx {
                scene,
                param: RefCell::new(None),
                chan,
                net: Net::new(chan),
                data_dir: CString::new(data.to_string_lossy().as_bytes())?,
                audio,
                media: MediaCtl::new(chan),
            });
            let host = Box::new(host_table(&mut *ctx as *mut Ctx as *mut c_void));
            Ok(NativePage {
                host,
                _ctx: ctx,
                activate,
                update,
                pointer,
                draw,
                deactivate,
                net,
                pd,
                media,
                _lib: lib,
            })
        }
    }
}

impl Page for NativePage {
    fn activate(&mut self) -> Result<()> {
        unsafe { (self.activate)(&*self.host) };
        Ok(())
    }

    fn update(&mut self, dt: f64) -> Result<()> {
        if let Some(f) = self.update {
            unsafe { f(&*self.host, dt) };
        }
        Ok(())
    }

    fn pointer(&mut self, ev: &UwPointerEvent) -> Result<()> {
        if let Some(f) = self.pointer {
            unsafe { f(&*self.host, ev) };
        }
        Ok(())
    }

    fn net(&mut self, ev: NetEvent) -> Result<()> {
        let Some(f) = self.net else { return Ok(()) };
        let kind = match ev.kind {
            NetEventKind::Connected => UW_NET_CONNECTED,
            NetEventKind::Data => UW_NET_DATA,
            NetEventKind::Closed => UW_NET_CLOSED,
            NetEventKind::Error => UW_NET_ERROR,
            NetEventKind::Accepted => UW_NET_ACCEPTED,
            NetEventKind::Response => UW_NET_RESPONSE,
        };
        let text = CString::new(ev.text.replace('\0', "")).unwrap_or_default();
        let raw = UwNetEvent {
            kind,
            handle: ev.handle,
            new_handle: ev.new_handle,
            status: ev.status,
            data: ev.data.as_ptr(),
            len: ev.data.len(),
            text: text.as_ptr(),
        };
        unsafe { f(&*self.host, &raw) };
        Ok(())
    }

    fn media(&mut self, id: u32, ev: &MediaEvent) -> Result<()> {
        let Some(f) = self.media else { return Ok(()) };
        let (kind, width, height, text) = match ev {
            MediaEvent::Ready { width, height, .. } => {
                (UW_MEDIA_READY, *width, *height, String::new())
            }
            MediaEvent::Ended => (UW_MEDIA_ENDED, 0, 0, String::new()),
            MediaEvent::Broadcasting => (UW_MEDIA_BROADCASTING, 0, 0, String::new()),
            MediaEvent::Error(e) => (UW_MEDIA_ERROR, 0, 0, e.clone()),
        };
        let text = CString::new(text.replace('\0', "")).unwrap_or_default();
        let raw = UwMediaEvent {
            id,
            kind,
            width,
            height,
            text: text.as_ptr(),
        };
        unsafe { f(&*self.host, &raw) };
        Ok(())
    }

    fn pd(&mut self, msgs: Vec<PdMessage>) -> Result<()> {
        for m in msgs {
            if m.receiver == "print" {
                if let PdValue::Symbol(line) = &m.value {
                    let _ = unsafe { &*self._ctx.chan }
                        .send(&GuestMsg::Log(format!("pd: {line}")), None);
                }
                continue;
            }
            let Some(f) = self.pd else { continue };
            let receiver = CString::new(m.receiver.replace('\0', "")).unwrap_or_default();
            let mut symbol = CString::default();
            let mut list: Vec<f32> = Vec::new();
            let (kind, value) = match &m.value {
                PdValue::Bang => (UW_PD_BANG, 0.0),
                PdValue::Float(x) => (UW_PD_FLOAT, *x),
                PdValue::Symbol(s) => {
                    symbol = CString::new(s.replace('\0', "")).unwrap_or_default();
                    (UW_PD_SYMBOL, 0.0)
                }
                PdValue::List(items) => {
                    list = items
                        .iter()
                        .filter_map(|i| {
                            if let PdValue::Float(x) = i {
                                Some(*x)
                            } else {
                                None
                            }
                        })
                        .collect();
                    (UW_PD_LIST, 0.0)
                }
            };
            let ev = UwPdEvent {
                receiver: receiver.as_ptr(),
                kind,
                value,
                symbol: symbol.as_ptr(),
                list: list.as_ptr(),
                len: list.len(),
            };
            unsafe { f(&*self.host, &ev) };
        }
        Ok(())
    }

    fn draw(&mut self, cr: &cairo::Context, w: f64, h: f64) {
        if let Some(f) = self.draw {
            unsafe { f(&*self.host, cr.to_raw_none() as *mut c_void, w, h) };
        }
    }

    fn deactivate(&mut self) {
        if let Some(f) = self.deactivate {
            unsafe { f(&*self.host) };
        }
    }
}

// ---- the function table -------------------------------------------------

fn ctx<'a>(p: *mut c_void) -> &'a Ctx {
    unsafe { &*(p as *const Ctx) }
}

/// Run `f` on the scene. Calls made while the scene is being rendered
/// (from `uw_draw`) can't modify it and are ignored.
fn with_scene<R: Default>(p: *mut c_void, f: impl FnOnce(&mut Scene) -> R) -> R {
    match ctx(p).scene.try_borrow_mut() {
        Ok(mut s) => f(&mut s),
        Err(_) => R::default(),
    }
}

fn shape(p: *mut c_void, id: u32, f: impl FnOnce(&mut Shape)) {
    with_scene(p, |s| f(s.shape_mut(id)))
}

fn string(s: *const c_char) -> String {
    if s.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
    }
}

extern "C" fn set_title(p: *mut c_void, t: *const c_char) {
    with_scene(p, |s| s.set_title(&string(t)))
}
extern "C" fn add_layout(p: *mut c_void, l: u32) {
    with_scene(p, |s| s.add_layout(l))
}
extern "C" fn set_animating(p: *mut c_void, on: bool) {
    with_scene(p, |s| s.animating = on)
}
extern "C" fn request_redraw(p: *mut c_void) {
    with_scene(p, |s| s.request_redraw())
}
extern "C" fn width(p: *mut c_void) -> f64 {
    ctx(p).scene.try_borrow().map(|s| s.width).unwrap_or(0.0)
}
extern "C" fn height(p: *mut c_void) -> f64 {
    ctx(p).scene.try_borrow().map(|s| s.height).unwrap_or(0.0)
}
extern "C" fn param(p: *mut c_void, key: *const c_char) -> *const c_char {
    let c = ctx(p);
    let value = c
        .scene
        .try_borrow()
        .ok()
        .and_then(|s| s.params.get(&string(key)).cloned());
    let mut slot = c.param.borrow_mut();
    *slot = value.and_then(|v| CString::new(v).ok());
    slot.as_ref().map_or(std::ptr::null(), |s| s.as_ptr())
}
extern "C" fn log(p: *mut c_void, m: *const c_char) {
    let chan = unsafe { &*ctx(p).chan };
    let _ = chan.send(&GuestMsg::Log(string(m)), None);
}
extern "C" fn layout_new(p: *mut c_void) -> u32 {
    with_scene(p, |s| s.layout_new())
}
extern "C" fn layout_append(p: *mut c_void, l: u32, sh: u32) {
    with_scene(p, |s| s.layout_append(l, sh))
}
extern "C" fn shape_new(p: *mut c_void) -> u32 {
    with_scene(p, |s| s.shape_new())
}
extern "C" fn shape_clear_path(p: *mut c_void, id: u32) {
    shape(p, id, |s| s.path.clear())
}
extern "C" fn shape_rectangle(p: *mut c_void, id: u32, x: f64, y: f64, w: f64, h: f64) {
    shape(p, id, |s| s.path.push(PathOp::Rectangle(x, y, w, h)))
}
extern "C" fn shape_move_to(p: *mut c_void, id: u32, x: f64, y: f64) {
    shape(p, id, |s| s.path.push(PathOp::MoveTo(x, y)))
}
extern "C" fn shape_line_to(p: *mut c_void, id: u32, x: f64, y: f64) {
    shape(p, id, |s| s.path.push(PathOp::LineTo(x, y)))
}
#[allow(clippy::too_many_arguments)]
extern "C" fn shape_curve_to(
    p: *mut c_void,
    id: u32,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
) {
    shape(p, id, |s| s.path.push(PathOp::CurveTo(a, b, c, d, e, f)))
}
extern "C" fn shape_close_path(p: *mut c_void, id: u32) {
    shape(p, id, |s| s.path.push(PathOp::Close))
}
extern "C" fn shape_set_fill_color(p: *mut c_void, id: u32, r: f64, g: f64, b: f64, a: f64) {
    shape(p, id, |s| s.fill = Some(Rgba(r, g, b, a)))
}
extern "C" fn shape_set_stroke_color(p: *mut c_void, id: u32, r: f64, g: f64, b: f64, a: f64) {
    shape(p, id, |s| s.stroke = Some(Rgba(r, g, b, a)))
}
extern "C" fn shape_set_stroke_width(p: *mut c_void, id: u32, w: f64) {
    shape(p, id, |s| s.stroke_width = w)
}
extern "C" fn shape_set_text(p: *mut c_void, id: u32, t: *const c_char) {
    let t = string(t);
    shape(p, id, |s| s.text = Some(t))
}
extern "C" fn shape_set_text_color(p: *mut c_void, id: u32, r: f64, g: f64, b: f64, a: f64) {
    shape(p, id, |s| s.text_color = Rgba(r, g, b, a))
}
extern "C" fn shape_set_font(p: *mut c_void, id: u32, f: *const c_char) {
    let f = string(f);
    shape(p, id, |s| s.font = f)
}

fn bytes<'a>(data: *const u8, len: usize) -> &'a [u8] {
    if data.is_null() || len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    }
}
extern "C" fn data_dir(p: *mut c_void) -> *const c_char {
    ctx(p).data_dir.as_ptr()
}
extern "C" fn net_tcp_connect(p: *mut c_void, host: *const c_char, port: u16) -> u32 {
    ctx(p).net.open(NetOp::TcpConnect {
        host: string(host),
        port,
    })
}
extern "C" fn net_tcp_listen(p: *mut c_void, port: u16) -> u32 {
    ctx(p).net.open(NetOp::TcpListen { port })
}
extern "C" fn net_udp_open(p: *mut c_void, port: u16) -> u32 {
    ctx(p).net.open(NetOp::UdpOpen { port })
}
extern "C" fn net_send(p: *mut c_void, handle: u32, data: *const u8, len: usize) {
    ctx(p).net.on(
        handle,
        NetOp::Send {
            data: bytes(data, len).to_vec(),
        },
    )
}
extern "C" fn net_udp_send(
    p: *mut c_void,
    handle: u32,
    host: *const c_char,
    port: u16,
    data: *const u8,
    len: usize,
) {
    ctx(p).net.on(
        handle,
        NetOp::UdpSend {
            host: string(host),
            port,
            data: bytes(data, len).to_vec(),
        },
    )
}
extern "C" fn net_close(p: *mut c_void, handle: u32) {
    ctx(p).net.on(handle, NetOp::Close)
}
extern "C" fn fetch(
    p: *mut c_void,
    method: *const c_char,
    url: *const c_char,
    headers: *const c_char,
    body: *const u8,
    len: usize,
) -> u32 {
    let method = if method.is_null() {
        "GET".to_string()
    } else {
        string(method)
    };
    ctx(p).net.open(NetOp::Fetch {
        method,
        url: string(url),
        headers: parse_headers(&string(headers)),
        body: bytes(body, len).to_vec(),
    })
}

// ---- audio ----

fn audio<'a>(p: *mut c_void) -> &'a Audio {
    &ctx(p).audio
}
fn log_err(p: *mut c_void, r: anyhow::Result<()>) -> bool {
    match r {
        Ok(()) => true,
        Err(e) => {
            let _ = unsafe { &*ctx(p).chan }.send(&GuestMsg::Log(format!("{e:#}")), None);
            false
        }
    }
}
extern "C" fn audio_rate(p: *mut c_void) -> u32 {
    audio(p).rate()
}
extern "C" fn audio_start(p: *mut c_void) -> bool {
    log_err(p, audio(p).start())
}
extern "C" fn audio_stop(p: *mut c_void) {
    audio(p).stop()
}
extern "C" fn audio_volume(p: *mut c_void, v: f32) {
    audio(p).engine.set_volume(v)
}
extern "C" fn osc_new(p: *mut c_void, wave: u32, freq: f32, amp: f32, pan: f32) -> u32 {
    let a = audio(p);
    if !log_err(p, a.start()) {
        return 0;
    }
    let Some(id) = a
        .engine
        .osc_new(Wave::from_index(wave).unwrap_or(Wave::Sine), 0.0, 0.0, 0.0)
    else {
        return 0;
    };
    a.engine.osc_set(id, freq, amp, pan);
    id
}
extern "C" fn osc_set(p: *mut c_void, id: u32, freq: f32, amp: f32, pan: f32) {
    audio(p).engine.osc_set(id, freq, amp, pan)
}
extern "C" fn osc_free(p: *mut c_void, id: u32) {
    audio(p).engine.osc_free(id)
}
extern "C" fn audio_scope(p: *mut c_void, out: *mut f32, count: usize) -> usize {
    if out.is_null() {
        return 0;
    }
    let s = audio(p).engine.scope(count);
    unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), out, s.len()) };
    s.len()
}
/// The package's callback and its pointer, moved to the audio thread.
struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}
extern "C" fn audio_callback(p: *mut c_void, cb: Option<UwAudioFn>, user: *mut c_void) {
    let a = audio(p);
    match cb {
        None => a.engine.set_callback(None),
        Some(cb) => {
            if !log_err(p, a.start()) {
                return;
            }
            let user = SendPtr(user);
            a.engine
                .set_callback(Some(Box::new(move |frames: &mut [f32]| {
                    let u = &user;
                    cb(u.0, frames.as_mut_ptr(), frames.len() / 2, 2)
                })));
        }
    }
}
extern "C" fn pd_open(p: *mut c_void, path: *const c_char) -> bool {
    log_err(p, audio(p).pd_open(&string(path)))
}
extern "C" fn pd_bang(p: *mut c_void, r: *const c_char) {
    log_err(p, pd::send(&string(r), &PdValue::Bang));
}
extern "C" fn pd_float(p: *mut c_void, r: *const c_char, v: f32) {
    log_err(p, pd::send(&string(r), &PdValue::Float(v)));
}
extern "C" fn pd_symbol(p: *mut c_void, r: *const c_char, s: *const c_char) {
    log_err(p, pd::send(&string(r), &PdValue::Symbol(string(s))));
}
extern "C" fn pd_list(p: *mut c_void, r: *const c_char, values: *const f32, count: usize) {
    let v = if values.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(values, count) }
    };
    log_err(
        p,
        pd::send(
            &string(r),
            &PdValue::List(v.iter().map(|x| PdValue::Float(*x)).collect()),
        ),
    );
}
extern "C" fn pd_subscribe(p: *mut c_void, r: *const c_char) {
    log_err(p, pd::subscribe(&string(r)));
}

// ---- video ----

extern "C" fn media_open(p: *mut c_void, source: *const c_char, looping: bool) -> u32 {
    ctx(p).media.open(&string(source), looping, None)
}
extern "C" fn media_broadcast(
    p: *mut c_void,
    source: *const c_char,
    url: *const c_char,
    password: *const c_char,
) -> u32 {
    ctx(p).media.open(
        &string(source),
        false,
        Some(Broadcast {
            url: string(url),
            password: string(password),
        }),
    )
}
extern "C" fn media_play(p: *mut c_void, id: u32) {
    ctx(p).media.cmd(id, MediaCmd::Play)
}
extern "C" fn media_pause(p: *mut c_void, id: u32) {
    ctx(p).media.cmd(id, MediaCmd::Pause)
}
extern "C" fn media_seek(p: *mut c_void, id: u32, seconds: f64) {
    ctx(p).media.cmd(id, MediaCmd::Seek(seconds))
}
extern "C" fn media_close(p: *mut c_void, id: u32) {
    with_scene(p, |s| s.video_remove(id));
    ctx(p).media.cmd(id, MediaCmd::Close)
}
extern "C" fn media_info(p: *mut c_void, id: u32, out: *mut UwMediaInfo) -> bool {
    let Ok(scene) = ctx(p).scene.try_borrow() else {
        return false;
    };
    let (Some(v), false) = (scene.video(id), out.is_null()) else {
        return false;
    };
    unsafe {
        *out = UwMediaInfo {
            width: v.width(),
            height: v.height(),
            position: v.position(),
            duration: v.duration(),
        }
    };
    true
}
extern "C" fn shape_set_video(p: *mut c_void, shape_id: u32, video: u32) {
    shape(p, shape_id, |s| s.video = (video != 0).then_some(video))
}

fn host_table(ctx: *mut c_void) -> UwHost {
    UwHost {
        abi_version: UW_ABI_VERSION,
        ctx,
        set_title,
        add_layout,
        set_animating,
        request_redraw,
        width,
        height,
        param,
        log,
        layout_new,
        layout_append,
        shape_new,
        shape_clear_path,
        shape_rectangle,
        shape_move_to,
        shape_line_to,
        shape_curve_to,
        shape_close_path,
        shape_set_fill_color,
        shape_set_stroke_color,
        shape_set_stroke_width,
        shape_set_text,
        shape_set_text_color,
        shape_set_font,
        data_dir,
        net_tcp_connect,
        net_tcp_listen,
        net_udp_open,
        net_send,
        net_udp_send,
        net_close,
        fetch,
        audio_rate,
        audio_start,
        audio_stop,
        audio_volume,
        osc_new,
        osc_set,
        osc_free,
        audio_scope,
        audio_callback,
        pd_open,
        pd_bang,
        pd_float,
        pd_symbol,
        pd_list,
        pd_subscribe,
        media_open,
        media_broadcast,
        media_play,
        media_pause,
        media_seek,
        media_close,
        media_info,
        shape_set_video,
    }
}
