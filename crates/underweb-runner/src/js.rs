//! JavaScript and TypeScript packages, run in V8 through deno_core.
//!
//! A JS/TS page is an ES module exporting any of `activate()`,
//! `update(dt)`, `pointer(ev)` and `deactivate()`. There are no Deno or
//! Node APIs: the only capabilities are the ops below, which go to the
//! same [`Scene`] that native packages use. TypeScript is transpiled
//! (not type-checked) with deno_ast.

use std::cell::RefCell;
use std::path::PathBuf;
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
use underweb_ipc::{Channel, GuestMsg};
use underweb_pkg::Package;

use crate::page::Page;

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
    ],
    esm_entry_point = "ext:underweb/underweb.js",
    esm = [dir "js", "underweb.js"],
    options = { scene: SharedScene },
    state = |state, options| {
        state.put::<SharedScene>(options.scene);
        state.put(Logs::default());
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
                        export const Layout = u.Layout, Shape = u.Shape, window = u.window;\n";
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
    pub fn new(pkg: &Package, scene: SharedScene, chan: &Channel) -> Result<JsPage> {
        let entry = pkg.entry_path().context("no entry")?;
        let entry =
            ModuleSpecifier::from_file_path(&entry).map_err(|_| anyhow!("bad entry path"))?;
        let runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(Rc::new(PackageLoader {
                root: pkg.dir.clone(),
            })),
            extensions: vec![underweb::init(scene)],
            ..Default::default()
        });
        let tokio = tokio::runtime::Builder::new_current_thread().build()?;
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
