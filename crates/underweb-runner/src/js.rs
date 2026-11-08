//! JavaScript and TypeScript packages, run in V8 through deno_core.
//!
//! A JS/TS page is an ES module exporting any of `activate()`,
//! `update(dt)`, `pointer(ev)` and `deactivate()`. There are no Deno or
//! Node APIs: the only capabilities are the ops below, which go to the
//! same [`Scene`] that native packages use. TypeScript is transpiled
//! (not type-checked) with deno_ast.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result, anyhow};
use deno_ast::{MediaType, ParseParams};
use deno_core::error::ModuleLoaderError;
use deno_core::{
    JsRuntime, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader,
    ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, OpState, ResolutionKind,
    RuntimeOptions, op2, resolve_import,
};
use deno_error::JsErrorBox;
use underweb_abi::UwPointerEvent;
use underweb_guest::{PathOp, Rgba, Scene, Shape};
use underweb_ipc::{Channel, GuestMsg, NetEvent, NetEventKind, NetOp};
use underweb_pkg::Package;

use crate::audio::Audio;
use crate::net::Net;
use crate::page::Page;
use underweb_audio::pd::{self, PdMessage, PdValue};
use underweb_audio::ugens::Wave;

const API_SPECIFIER: &str = "underweb:api";
const MAIN_SPECIFIER: &str = "underweb:main";

type SharedScene = Rc<RefCell<Scene>>;

/// Log lines are queued in the op state and flushed by the runner.
#[derive(Default)]
struct Logs(Vec<String>);

fn scene(state: &mut OpState) -> std::cell::RefMut<'_, Scene> {
    state.borrow::<SharedScene>().borrow_mut()
}

fn with_shape(state: &mut OpState, id: u32, f: impl FnOnce(&mut Shape)) {
    f(scene(state).shape_mut(id))
}

#[op2(fast)]
fn op_uw_set_title(state: &mut OpState, #[string] title: &str) {
    scene(state).set_title(title)
}
#[op2(fast)]
fn op_uw_add_layout(state: &mut OpState, layout: u32) {
    scene(state).add_layout(layout)
}
#[op2(fast)]
fn op_uw_set_animating(state: &mut OpState, on: bool) {
    scene(state).animating = on
}
#[op2(fast)]
fn op_uw_request_redraw(state: &mut OpState) {
    scene(state).request_redraw()
}
#[op2(fast)]
fn op_uw_width(state: &mut OpState) -> f64 {
    scene(state).width
}
#[op2(fast)]
fn op_uw_height(state: &mut OpState) -> f64 {
    scene(state).height
}
#[op2]
#[string]
fn op_uw_param(state: &mut OpState, #[string] key: &str) -> Option<String> {
    scene(state).params.get(key).cloned()
}
#[op2(fast)]
fn op_uw_log(state: &mut OpState, #[string] msg: &str) {
    state.borrow_mut::<Logs>().0.push(msg.to_string())
}
#[op2(fast)]
fn op_uw_layout_new(state: &mut OpState) -> u32 {
    scene(state).layout_new()
}
#[op2(fast)]
fn op_uw_layout_append(state: &mut OpState, layout: u32, shape: u32) {
    scene(state).layout_append(layout, shape)
}
#[op2(fast)]
fn op_uw_shape_new(state: &mut OpState) -> u32 {
    scene(state).shape_new()
}
#[op2(fast)]
fn op_uw_shape_clear_path(state: &mut OpState, id: u32) {
    with_shape(state, id, |s| s.path.clear())
}
#[op2(fast)]
fn op_uw_shape_rectangle(state: &mut OpState, id: u32, x: f64, y: f64, w: f64, h: f64) {
    with_shape(state, id, |s| s.path.push(PathOp::Rectangle(x, y, w, h)))
}
#[op2(fast)]
fn op_uw_shape_move_to(state: &mut OpState, id: u32, x: f64, y: f64) {
    with_shape(state, id, |s| s.path.push(PathOp::MoveTo(x, y)))
}
#[op2(fast)]
fn op_uw_shape_line_to(state: &mut OpState, id: u32, x: f64, y: f64) {
    with_shape(state, id, |s| s.path.push(PathOp::LineTo(x, y)))
}
#[op2(fast)]
#[allow(clippy::too_many_arguments)]
fn op_uw_shape_curve_to(
    state: &mut OpState,
    id: u32,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
) {
    with_shape(state, id, |s| {
        s.path.push(PathOp::CurveTo(a, b, c, d, e, f))
    })
}
#[op2(fast)]
fn op_uw_shape_close_path(state: &mut OpState, id: u32) {
    with_shape(state, id, |s| s.path.push(PathOp::Close))
}
#[op2(fast)]
fn op_uw_shape_set_fill_color(state: &mut OpState, id: u32, r: f64, g: f64, b: f64, a: f64) {
    with_shape(state, id, |s| s.fill = Some(Rgba(r, g, b, a)))
}
#[op2(fast)]
fn op_uw_shape_set_stroke_color(state: &mut OpState, id: u32, r: f64, g: f64, b: f64, a: f64) {
    with_shape(state, id, |s| s.stroke = Some(Rgba(r, g, b, a)))
}
#[op2(fast)]
fn op_uw_shape_set_stroke_width(state: &mut OpState, id: u32, w: f64) {
    with_shape(state, id, |s| s.stroke_width = w)
}
#[op2(fast)]
fn op_uw_shape_set_text(state: &mut OpState, id: u32, #[string] text: &str) {
    with_shape(state, id, |s| s.text = Some(text.to_string()))
}
#[op2(fast)]
fn op_uw_shape_set_text_color(state: &mut OpState, id: u32, r: f64, g: f64, b: f64, a: f64) {
    with_shape(state, id, |s| s.text_color = Rgba(r, g, b, a))
}
#[op2(fast)]
fn op_uw_shape_set_font(state: &mut OpState, id: u32, #[string] font: &str) {
    with_shape(state, id, |s| s.font = font.to_string())
}

// ---- network: requests go to the host's broker -------------------------

/// Events waiting for JS, and the payload of the one being delivered.
#[derive(Default)]
struct NetInbox {
    queue: VecDeque<NetEvent>,
    data: Vec<u8>,
}

#[derive(serde::Serialize)]
struct JsNetEvent {
    handle: u32,
    kind: &'static str,
    text: String,
    status: u32,
    new_handle: u32,
}

fn net(state: &OpState) -> &Net {
    state.borrow::<Net>()
}

#[op2(fast)]
fn op_uw_net_tcp_connect(state: &mut OpState, #[string] host: String, port: u32) -> u32 {
    net(state).open(NetOp::TcpConnect {
        host,
        port: port as u16,
    })
}
#[op2(fast)]
fn op_uw_net_tcp_listen(state: &mut OpState, port: u32) -> u32 {
    net(state).open(NetOp::TcpListen { port: port as u16 })
}
#[op2(fast)]
fn op_uw_net_udp_open(state: &mut OpState, port: u32) -> u32 {
    net(state).open(NetOp::UdpOpen { port: port as u16 })
}
#[op2(fast)]
fn op_uw_net_send(state: &mut OpState, handle: u32, #[buffer] data: &[u8]) {
    net(state).on(
        handle,
        NetOp::Send {
            data: data.to_vec(),
        },
    )
}
#[op2(fast)]
fn op_uw_net_udp_send(
    state: &mut OpState,
    handle: u32,
    #[string] host: String,
    port: u32,
    #[buffer] data: &[u8],
) {
    net(state).on(
        handle,
        NetOp::UdpSend {
            host,
            port: port as u16,
            data: data.to_vec(),
        },
    )
}
#[op2(fast)]
fn op_uw_net_close(state: &mut OpState, handle: u32) {
    net(state).on(handle, NetOp::Close)
}
#[op2]
fn op_uw_fetch(
    state: &mut OpState,
    #[string] method: String,
    #[string] url: String,
    #[serde] headers: Vec<(String, String)>,
    #[buffer] body: &[u8],
) -> u32 {
    net(state).open(NetOp::Fetch {
        method,
        url,
        headers,
        body: body.to_vec(),
    })
}
/// The next queued event (its bytes go to `op_uw_net_data`), or null.
#[op2]
#[serde]
fn op_uw_net_next(state: &mut OpState) -> Option<JsNetEvent> {
    let inbox = state.borrow_mut::<NetInbox>();
    let ev = inbox.queue.pop_front()?;
    inbox.data = ev.data;
    let kind = match ev.kind {
        NetEventKind::Connected => "connected",
        NetEventKind::Data => "data",
        NetEventKind::Closed => "closed",
        NetEventKind::Error => "error",
        NetEventKind::Accepted => "accepted",
        NetEventKind::Response => "response",
    };
    Some(JsNetEvent {
        handle: ev.handle,
        kind,
        text: ev.text,
        status: ev.status,
        new_handle: ev.new_handle,
    })
}
#[op2]
#[buffer]
fn op_uw_net_data(state: &mut OpState) -> Vec<u8> {
    std::mem::take(&mut state.borrow_mut::<NetInbox>().data)
}

// ---- storage: small key/value files in the package's data directory ----

struct DataDir(PathBuf);

const MAX_KEY: usize = 200;
const MAX_VALUE: usize = 16 << 20;

fn key_path(state: &OpState, key: &str) -> Result<PathBuf, JsErrorBox> {
    if key.is_empty() || key.len() > MAX_KEY {
        return Err(JsErrorBox::type_error(format!(
            "storage keys are 1 to {MAX_KEY} bytes"
        )));
    }
    let name: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    Ok(state.borrow::<DataDir>().0.join("storage").join(name))
}

#[op2]
#[string]
fn op_uw_storage_get(
    state: &mut OpState,
    #[string] key: &str,
) -> Result<Option<String>, JsErrorBox> {
    Ok(std::fs::read_to_string(key_path(state, key)?).ok())
}
#[op2(fast)]
fn op_uw_storage_set(
    state: &mut OpState,
    #[string] key: &str,
    #[string] value: &str,
) -> Result<(), JsErrorBox> {
    if value.len() > MAX_VALUE {
        return Err(JsErrorBox::range_error("storage values are at most 16 MB"));
    }
    let path = key_path(state, key)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(JsErrorBox::from_err)?;
    std::fs::write(path, value).map_err(JsErrorBox::from_err)
}
#[op2(fast)]
fn op_uw_storage_remove(state: &mut OpState, #[string] key: &str) -> Result<(), JsErrorBox> {
    let _ = std::fs::remove_file(key_path(state, key)?);
    Ok(())
}
#[op2]
#[serde]
fn op_uw_storage_keys(state: &mut OpState) -> Vec<String> {
    let dir = state.borrow::<DataDir>().0.join("storage");
    let mut keys: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let hex = e.file_name().into_string().ok()?;
            let bytes: Option<Vec<u8>> = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
                .collect();
            String::from_utf8(bytes?).ok()
        })
        .collect();
    keys.sort();
    keys
}

// ---- audio: oscillators and Pure Data, rendered off the main thread -----

type SharedAudio = Rc<Audio>;

fn audio(state: &OpState) -> &Audio {
    state.borrow::<SharedAudio>()
}

fn js_err(e: anyhow::Error) -> JsErrorBox {
    JsErrorBox::generic(format!("{e:#}"))
}

#[op2(fast)]
fn op_uw_audio_rate(state: &mut OpState) -> u32 {
    audio(state).rate()
}
#[op2(fast)]
fn op_uw_audio_start(state: &mut OpState) -> Result<(), JsErrorBox> {
    audio(state).start().map_err(js_err)
}
#[op2(fast)]
fn op_uw_audio_stop(state: &mut OpState) {
    audio(state).stop()
}
#[op2(fast)]
fn op_uw_audio_volume(state: &mut OpState, v: f64) {
    audio(state).engine.set_volume(v as f32)
}
#[op2(fast)]
fn op_uw_osc_new(
    state: &mut OpState,
    #[string] wave: &str,
    freq: f64,
    amp: f64,
    pan: f64,
) -> Result<u32, JsErrorBox> {
    let wave = Wave::parse(wave)
        .ok_or_else(|| JsErrorBox::type_error(format!("unknown wave {wave:?}")))?;
    let a = audio(state);
    a.start().map_err(js_err)?;
    let id = a
        .engine
        .osc_new(wave, 0.0, 0.0, 0.0)
        .ok_or_else(|| JsErrorBox::range_error("too many oscillators"))?;
    a.engine.osc_set(id, freq as f32, amp as f32, pan as f32);
    Ok(id)
}
#[op2(fast)]
fn op_uw_osc_set(state: &mut OpState, id: u32, freq: f64, amp: f64, pan: f64) {
    audio(state)
        .engine
        .osc_set(id, freq as f32, amp as f32, pan as f32)
}
#[op2(fast)]
fn op_uw_osc_free(state: &mut OpState, id: u32) {
    audio(state).engine.osc_free(id)
}
/// The last `frames` frames played (mono), as little-endian f32 bytes.
#[op2]
#[buffer]
fn op_uw_audio_scope(state: &mut OpState, frames: u32) -> Vec<u8> {
    audio(state)
        .engine
        .scope(frames as usize)
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect()
}
#[op2(fast)]
fn op_uw_pd_open(state: &mut OpState, #[string] path: &str) -> Result<(), JsErrorBox> {
    audio(state).pd_open(path).map_err(js_err)
}
fn pd_value(v: &serde_json::Value) -> Result<PdValue, JsErrorBox> {
    Ok(match v {
        serde_json::Value::Null => PdValue::Bang,
        serde_json::Value::Number(n) => PdValue::Float(n.as_f64().unwrap_or(0.0) as f32),
        serde_json::Value::String(s) => PdValue::Symbol(s.clone()),
        serde_json::Value::Bool(b) => PdValue::Float(if *b { 1.0 } else { 0.0 }),
        serde_json::Value::Array(a) => {
            PdValue::List(a.iter().map(pd_value).collect::<Result<_, _>>()?)
        }
        _ => {
            return Err(JsErrorBox::type_error(
                "send numbers, strings, arrays of them, or nothing (bang)",
            ));
        }
    })
}
fn json_value(v: PdValue) -> serde_json::Value {
    match v {
        PdValue::Bang => serde_json::Value::Null,
        PdValue::Float(x) => serde_json::json!(x),
        PdValue::Symbol(s) => serde_json::Value::String(s),
        PdValue::List(l) => serde_json::Value::Array(l.into_iter().map(json_value).collect()),
    }
}
#[op2]
fn op_uw_pd_send(
    #[string] receiver: String,
    #[serde] value: serde_json::Value,
) -> Result<(), JsErrorBox> {
    pd::send(&receiver, &pd_value(&value)?).map_err(js_err)
}
#[op2(fast)]
fn op_uw_pd_subscribe(#[string] receiver: &str) -> Result<(), JsErrorBox> {
    pd::subscribe(receiver).map_err(js_err)
}
#[derive(Default)]
struct PdInbox(Vec<PdMessage>);
#[op2]
#[serde]
fn op_uw_pd_next(state: &mut OpState) -> Vec<(String, serde_json::Value)> {
    std::mem::take(&mut state.borrow_mut::<PdInbox>().0)
        .into_iter()
        .map(|m| (m.receiver, json_value(m.value)))
        .collect()
}

deno_core::extension!(
    underweb,
    ops = [
        op_uw_set_title, op_uw_add_layout, op_uw_set_animating, op_uw_request_redraw,
        op_uw_width, op_uw_height, op_uw_param, op_uw_log,
        op_uw_layout_new, op_uw_layout_append,
        op_uw_shape_new, op_uw_shape_clear_path, op_uw_shape_rectangle, op_uw_shape_move_to,
        op_uw_shape_line_to, op_uw_shape_curve_to, op_uw_shape_close_path,
        op_uw_shape_set_fill_color, op_uw_shape_set_stroke_color, op_uw_shape_set_stroke_width,
        op_uw_shape_set_text, op_uw_shape_set_text_color, op_uw_shape_set_font,
        op_uw_net_tcp_connect, op_uw_net_tcp_listen, op_uw_net_udp_open, op_uw_net_send,
        op_uw_net_udp_send, op_uw_net_close, op_uw_fetch, op_uw_net_next, op_uw_net_data,
        op_uw_storage_get, op_uw_storage_set, op_uw_storage_remove, op_uw_storage_keys,
        op_uw_audio_rate, op_uw_audio_start, op_uw_audio_stop, op_uw_audio_volume,
        op_uw_osc_new, op_uw_osc_set, op_uw_osc_free, op_uw_audio_scope,
        op_uw_pd_open, op_uw_pd_send, op_uw_pd_subscribe, op_uw_pd_next,
    ],
    esm_entry_point = "ext:underweb/underweb.js",
    esm = [dir "js", "underweb.js"],
    options = { scene: SharedScene, net: Net, data_dir: PathBuf, audio: SharedAudio },
    state = |state, options| {
        state.put::<SharedScene>(options.scene);
        state.put(Logs::default());
        state.put(options.net);
        state.put(NetInbox::default());
        state.put(DataDir(options.data_dir));
        state.put::<SharedAudio>(options.audio);
        state.put(PdInbox::default());
    },
);

/// Loads modules from the package directory only, transpiling TS.
struct PackageLoader {
    root: PathBuf,
}

impl ModuleLoader for PackageLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        if specifier == "underweb" {
            return Ok(ModuleSpecifier::parse(API_SPECIFIER).unwrap());
        }
        resolve_import(specifier, referrer).map_err(JsErrorBox::from_err)
    }

    fn load(
        &self,
        specifier: &ModuleSpecifier,
        _referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        ModuleLoadResponse::Sync(self.load_sync(specifier))
    }
}

impl PackageLoader {
    fn load_sync(&self, specifier: &ModuleSpecifier) -> Result<ModuleSource, ModuleLoaderError> {
        if specifier.as_str() == API_SPECIFIER {
            let code = "const u = globalThis.__uw;\n\
                        export const Layout = u.Layout, Shape = u.Shape, window = u.window, net = u.net, storage = u.storage, audio = u.audio;\n";
            return Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(code.to_string().into()),
                specifier,
                None,
            ));
        }
        let path = specifier.to_file_path().map_err(|_| {
            JsErrorBox::generic(format!("cannot import {specifier}: only package files"))
        })?;
        if !path.starts_with(&self.root) {
            return Err(JsErrorBox::generic(format!(
                "{specifier} is outside the package"
            )));
        }
        let media_type = MediaType::from_path(&path);
        let transpile = match media_type {
            MediaType::JavaScript | MediaType::Mjs => false,
            MediaType::TypeScript | MediaType::Mts | MediaType::Tsx | MediaType::Jsx => true,
            _ => {
                return Err(JsErrorBox::generic(format!(
                    "unsupported module type: {specifier}"
                )));
            }
        };
        let code = std::fs::read_to_string(&path).map_err(JsErrorBox::from_err)?;
        let code = if transpile {
            let parsed = deno_ast::parse_module(ParseParams {
                specifier: specifier.clone(),
                text: code.into(),
                media_type,
                capture_tokens: false,
                scope_analysis: false,
                maybe_syntax: None,
            })
            .map_err(JsErrorBox::from_err)?;
            parsed
                .transpile(
                    &deno_ast::TranspileOptions::default(),
                    &deno_ast::TranspileModuleOptions { module_kind: None },
                    &deno_ast::EmitOptions {
                        source_map: deno_ast::SourceMapOption::Inline,
                        ..Default::default()
                    },
                )
                .map_err(JsErrorBox::from_err)?
                .into_source()
                .text
        } else {
            code
        };
        Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            specifier,
            None,
        ))
    }
}

pub struct JsPage {
    runtime: JsRuntime,
    tokio: tokio::runtime::Runtime,
    entry: ModuleSpecifier,
    chan: *const Channel,
}

impl JsPage {
    /// Creates the V8 isolate. Runs before lockdown; loads no package code.
    pub fn new(
        pkg: &Package,
        scene: SharedScene,
        chan: &Channel,
        data: &Path,
        audio: SharedAudio,
    ) -> Result<JsPage> {
        let entry = pkg.entry_path().context("no entry")?;
        let entry =
            ModuleSpecifier::from_file_path(&entry).map_err(|_| anyhow!("bad entry path"))?;
        // V8 schedules delayed work (garbage collection) through tokio:
        // the isolate must be created inside a runtime with timers
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let runtime = {
            let _context = tokio.enter();
            JsRuntime::new(RuntimeOptions {
                module_loader: Some(Rc::new(PackageLoader {
                    root: pkg.dir.clone(),
                })),
                extensions: vec![underweb::init(
                    scene,
                    Net::new(chan),
                    data.to_path_buf(),
                    audio,
                )],
                ..Default::default()
            })
        };
        Ok(JsPage {
            runtime,
            tokio,
            entry,
            chan,
        })
    }

    /// Load and evaluate the package's entry module. Runs after lockdown.
    pub fn load(&mut self) -> Result<()> {
        let main = ModuleSpecifier::parse(MAIN_SPECIFIER).unwrap();
        let code = format!(
            "import * as page from {};\nglobalThis.__uw_page = page;\n",
            serde_json_string(self.entry.as_str())
        );
        let rt = &mut self.runtime;
        self.tokio.block_on(async {
            let id = rt.load_main_es_module_from_code(&main, code).await?;
            let result = rt.mod_evaluate(id);
            rt.run_event_loop(Default::default()).await?;
            result.await?;
            Ok::<(), anyhow::Error>(())
        })?;
        self.flush_logs();
        Ok(())
    }

    fn dispatch(&mut self, name: &str, arg: &str) -> Result<()> {
        let script = format!(
            "globalThis.__uw.dispatch({}, {arg})",
            serde_json_string(name)
        );
        let result = self.runtime.execute_script("<underweb>", script);
        self.flush_logs();
        result.map_err(|e| anyhow!("{e}"))?;
        // settle promises from async handlers
        let rt = &mut self.runtime;
        self.tokio.block_on(rt.run_event_loop(Default::default()))?;
        self.flush_logs();
        Ok(())
    }

    fn flush_logs(&mut self) {
        let logs = std::mem::take(&mut self.runtime.op_state().borrow_mut().borrow_mut::<Logs>().0);
        let chan = unsafe { &*self.chan };
        for l in logs {
            let _ = chan.send(&GuestMsg::Log(l), None);
        }
    }
}

impl Page for JsPage {
    fn activate(&mut self) -> Result<()> {
        self.dispatch("activate", "undefined")
    }

    fn update(&mut self, dt: f64) -> Result<()> {
        self.dispatch("update", &format!("{dt}"))
    }

    fn pointer(&mut self, ev: &UwPointerEvent) -> Result<()> {
        let kind = ["move", "press", "release"]
            .get(ev.kind as usize)
            .unwrap_or(&"move");
        let arg = format!(
            "{{kind: \"{kind}\", button: {}, x: {}, y: {}, shape: {}}}",
            ev.button, ev.x, ev.y, ev.shape
        );
        self.dispatch("pointer", &arg)
    }

    fn net(&mut self, ev: NetEvent) -> Result<()> {
        self.runtime
            .op_state()
            .borrow_mut()
            .borrow_mut::<NetInbox>()
            .queue
            .push_back(ev);
        let result = self
            .runtime
            .execute_script("<underweb>", "globalThis.__uw.netEvents()");
        self.flush_logs();
        result.map_err(|e| anyhow!("{e}"))?;
        let rt = &mut self.runtime;
        self.tokio.block_on(rt.run_event_loop(Default::default()))?;
        self.flush_logs();
        Ok(())
    }

    fn pd(&mut self, msgs: Vec<PdMessage>) -> Result<()> {
        let (print, msgs): (Vec<_>, Vec<_>) = msgs.into_iter().partition(|m| m.receiver == "print");
        let chan = unsafe { &*self.chan };
        for p in print {
            if let PdValue::Symbol(line) = p.value {
                let _ = chan.send(&GuestMsg::Log(format!("pd: {line}")), None);
            }
        }
        if msgs.is_empty() {
            return Ok(());
        }
        self.runtime
            .op_state()
            .borrow_mut()
            .borrow_mut::<PdInbox>()
            .0
            .extend(msgs);
        let result = self
            .runtime
            .execute_script("<underweb>", "globalThis.__uw.pdMessages()");
        self.flush_logs();
        result.map_err(|e| anyhow!("{e}"))?;
        let rt = &mut self.runtime;
        self.tokio.block_on(rt.run_event_loop(Default::default()))?;
        self.flush_logs();
        Ok(())
    }

    fn deactivate(&mut self) {
        let _ = self.dispatch("deactivate", "undefined");
    }
}

/// A JS string literal.
fn serde_json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
