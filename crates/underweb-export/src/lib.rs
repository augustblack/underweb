//! The layout editor's export: a page's scene as a package of its own, in
//! JavaScript, TypeScript, Rust or C.
//!
//! This replaces underweb's `build_files/header.*` and `footer.*`
//! templates: the old editor pasted shape code between them; this writes a
//! complete package (manifest, source, and the images and videos it uses)
//! that the browser opens as is. Only the scene is exported. Event
//! handlers and animation are code, and stay in the original.

use std::fmt::Write as _;
use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use underweb_ipc::scene::{Matrix, PathOp, Rgba, SceneDoc, ShapeDoc};
pub use underweb_pkg::valid_name;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Js,
    Ts,
    Rust,
    C,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::Js, Lang::Ts, Lang::Rust, Lang::C];

    pub fn parse(s: &str) -> Option<Lang> {
        match s.to_ascii_lowercase().as_str() {
            "js" | "javascript" => Some(Lang::Js),
            "ts" | "typescript" => Some(Lang::Ts),
            "rust" | "rs" => Some(Lang::Rust),
            "c" => Some(Lang::C),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Lang::Js => "JavaScript",
            Lang::Ts => "TypeScript",
            Lang::Rust => "Rust",
            Lang::C => "C",
        }
    }

    /// Where the page's code goes in the package.
    pub fn source_file(self) -> &'static str {
        match self {
            Lang::Js => "main.js",
            Lang::Ts => "main.ts",
            Lang::Rust => "src/lib.rs",
            Lang::C => "main.c",
        }
    }
}

/// A package to write: files by relative path, plus package files the
/// scene uses (images, videos) to copy from the original.
#[derive(Debug, Default)]
pub struct Export {
    pub files: Vec<(String, String)>,
    pub assets: Vec<String>,
    /// Things the export could not carry over.
    pub notes: Vec<String>,
}

/// The page's code: builds the scene in `activate`.
pub fn source(doc: &SceneDoc, lang: Lang) -> String {
    Gen::new(lang).page(doc)
}

/// A whole package called `name`.
pub fn package(doc: &SceneDoc, lang: Lang, name: &str) -> Result<Export> {
    if !valid_name(name) {
        bail!("{name:?} is not a package name (lowercase letters, digits, - and _)");
    }
    let mut ex = Export::default();
    let mut camera = false;
    let mut fetch: Vec<String> = Vec::new();
    for s in doc.shapes() {
        match s.image.as_deref() {
            Some("") => ex.notes.push(format!(
                "{}: its image was made from bytes at run time, so it isn't exported",
                label(s)
            )),
            Some(f) => push_unique(&mut ex.assets, f),
            None => {}
        }
        match s.video.as_deref() {
            None => {}
            Some("") => ex
                .notes
                .push(format!("{}: video of unknown source left out", label(s))),
            Some("camera") => camera = true,
            Some(v) if v.starts_with("test:") => {}
            Some(v) if v.contains("://") => push_unique(&mut fetch, &origin(v)),
            Some(v) => push_unique(&mut ex.assets, v),
        }
    }

    let kind = match lang {
        Lang::Js => "js",
        Lang::Ts => "ts",
        Lang::Rust => "rust",
        Lang::C => "c",
    };
    let mut manifest =
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nkind = \"{kind}\"\n");
    if matches!(lang, Lang::Js | Lang::Ts) {
        let _ = writeln!(manifest, "entry = \"{}\"", lang.source_file());
    }
    manifest.push_str("description = \"Made with the Underweb layout editor.\"\n");
    if lang == Lang::C {
        manifest.push_str("\n# the browser compiles this when it has no prebuilt library\n[build]\nsources = [\"main.c\"]\n");
    }
    if camera || !fetch.is_empty() {
        manifest.push_str("\n[permissions]\n");
        if camera {
            manifest.push_str("camera = true\n");
        }
        if !fetch.is_empty() {
            let list: Vec<String> = fetch.iter().map(|f| format!("\"{f}\"")).collect();
            let _ = writeln!(manifest, "fetch = [{}]", list.join(", "));
        }
    }
    ex.files.push(("underweb.toml".into(), manifest));
    if lang == Lang::Rust {
        ex.files.push((
            "Cargo.toml".into(),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
                 [lib]\ncrate-type = [\"cdylib\"]\n\n\
                 [dependencies]\nunderweb = \"0.1\"\n\n\
                 # a package of its own, even inside another workspace\n[workspace]\n"
            ),
        ));
    }
    ex.files
        .push((lang.source_file().into(), source(doc, lang)));
    Ok(ex)
}

/// Write the package into `out` (created if missing), copying images and
/// videos from the original package directory `from`. Returns notes.
pub fn write(
    doc: &SceneDoc,
    lang: Lang,
    name: &str,
    from: Option<&Path>,
    out: &Path,
) -> Result<Vec<String>> {
    let mut ex = package(doc, lang, name)?;
    if out.join("underweb.toml").exists() && from.is_some_and(|f| same_dir(f, out)) {
        bail!("won't export over the package being edited");
    }
    for (rel, text) in &ex.files {
        let p = out.join(rel);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(&p, text).with_context(|| format!("writing {}", p.display()))?;
    }
    for asset in &ex.assets {
        let rel = Path::new(asset);
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            ex.notes
                .push(format!("{asset}: not a package file, not copied"));
            continue;
        }
        let Some(src) = from.map(|f| f.join(rel)).filter(|p| p.is_file()) else {
            ex.notes
                .push(format!("{asset}: not found, copy it into the package"));
            continue;
        };
        let dst = out.join(rel);
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::copy(&src, &dst).with_context(|| format!("copying {asset}"))?;
    }
    Ok(ex.notes)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x == s) {
        v.push(s.to_string());
    }
}

fn label(s: &ShapeDoc) -> String {
    if s.name.is_empty() {
        format!("shape {}", s.id)
    } else {
        format!("{:?}", s.name)
    }
}

/// "https://host:port/" of a URL, for a fetch permission.
fn origin(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    format!("{scheme}://{host}/")
}

// ---- code generation -------------------------------------------------------

struct Gen {
    lang: Lang,
    out: String,
}

impl Gen {
    fn new(lang: Lang) -> Gen {
        Gen {
            lang,
            out: String::new(),
        }
    }

    fn line(&mut self, s: &str) {
        let indent = match self.lang {
            Lang::Js | Lang::Ts => "  ",
            Lang::Rust => "        ",
            Lang::C => "    ",
        };
        self.out.push_str(indent);
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn num(&self, v: f64) -> String {
        num(v, matches!(self.lang, Lang::Rust | Lang::C))
    }

    fn str(&self, s: &str) -> String {
        match self.lang {
            Lang::Js | Lang::Ts => js_str(s),
            Lang::Rust => format!("{s:?}"),
            Lang::C => c_str(s),
        }
    }

    fn nums(&self, v: &[f64]) -> String {
        v.iter()
            .map(|x| self.num(*x))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `var.method(args)` in each language.
    fn call(&mut self, var: &str, method: &str, args: &str) {
        let s = match self.lang {
            Lang::Js | Lang::Ts => format!("{var}.{}({args});", camel(method)),
            Lang::Rust => format!("{var}.{method}({args});"),
            Lang::C => {
                let f = c_function(method);
                if args.is_empty() {
                    format!("UW(h, {f}, {var});")
                } else {
                    format!("UW(h, {f}, {var}, {args});")
                }
            }
        };
        self.line(&s);
    }

    fn page(mut self, doc: &SceneDoc) -> String {
        let uses_matrix = doc.shapes().any(|s| !s.matrix.is_identity());
        let uses_image = doc
            .shapes()
            .any(|s| s.image.as_deref().is_some_and(|i| !i.is_empty()));
        let uses_video = doc.shapes().any(|s| playable(s.video.as_deref()).is_some());
        self.header(uses_matrix, uses_image, uses_video);
        if !doc.title.is_empty() {
            let t = self.str(&doc.title);
            self.line(&match self.lang {
                Lang::Js | Lang::Ts => format!("window.title = {t};"),
                Lang::Rust => format!("window.set_title({t});"),
                Lang::C => format!("UW(h, set_title, {t});"),
            });
        }
        let mut n = 0;
        for (i, layout) in doc.layouts.iter().enumerate() {
            let l = format!("layout{i}");
            self.out.push('\n');
            let (new, add) = match self.lang {
                Lang::Js | Lang::Ts => (
                    format!("const {l} = new Layout();"),
                    format!("window.addLayout({l});"),
                ),
                Lang::Rust => (
                    format!("let {l} = Layout::new();"),
                    format!("window.add_layout(&{l});"),
                ),
                Lang::C => (
                    format!("uint32_t {l} = UW(h, layout_new);"),
                    format!("UW(h, add_layout, {l});"),
                ),
            };
            self.line(&new);
            self.line(&add);
            if let Some(Rgba(r, g, b, a)) = layout.background {
                let args = self.nums(&[r, g, b, a]);
                self.line(&match self.lang {
                    Lang::Js | Lang::Ts => format!("{l}.setBackground({args});"),
                    Lang::Rust => format!("{l}.set_background(Some(({args})));"),
                    Lang::C => format!("UW(h, layout_set_background, {l}, {args});"),
                });
            }
            for shape in &layout.shapes {
                self.out.push('\n');
                self.shape(&format!("shape{n}"), shape, &l);
                n += 1;
            }
        }
        self.footer();
        self.out
    }

    fn header(&mut self, matrix: bool, image: bool, video: bool) {
        let note = "Made with the Underweb layout editor. Only the scene is exported:\n\
                    add event handlers (pointer, update) and animation here.";
        match self.lang {
            Lang::Js | Lang::Ts => {
                for l in note.lines() {
                    let _ = writeln!(self.out, "// {l}");
                }
                let mut names = vec!["Layout", "Shape", "window"];
                if matrix {
                    names.push("Matrix");
                }
                if image {
                    names.push("Image");
                }
                if video {
                    names.push("video");
                }
                names.sort_by_key(|n| n.to_ascii_lowercase());
                let ret = if self.lang == Lang::Ts { ": void" } else { "" };
                let _ = write!(
                    self.out,
                    "import {{ {} }} from \"underweb\";\n\nexport function activate(){ret} {{\n",
                    names.join(", ")
                );
            }
            Lang::Rust => {
                for l in note.lines() {
                    let _ = writeln!(self.out, "//! {l}");
                }
                let mut names = vec!["Layout", "Page", "Shape", "Window"];
                if matrix {
                    names.push("Matrix");
                }
                if image {
                    names.push("Image");
                }
                if video {
                    names.push("video");
                }
                names.sort_by_key(|n| n.to_ascii_lowercase());
                let _ = write!(
                    self.out,
                    "\nuse underweb::{{{}}};\n\n#[derive(Default)]\nstruct Exported;\n\n\
                     impl Page for Exported {{\n    fn activate(&mut self, window: &Window) {{\n",
                    names.join(", ")
                );
            }
            Lang::C => {
                let _ = write!(
                    self.out,
                    "/* {} */\n#include \"underweb.h\"\n\nuint32_t uw_abi_version(void) {{ return UW_ABI_VERSION; }}\n\n\
                     void uw_activate(const UwHost *h)\n{{\n",
                    note.replace('\n', "\n * ")
                );
            }
        }
    }

    fn footer(&mut self) {
        self.out.push_str(match self.lang {
            Lang::Js | Lang::Ts => "}\n",
            Lang::Rust => "    }\n}\n\nunderweb::export_page!(Exported);\n",
            Lang::C => "}\n",
        });
    }

    fn shape(&mut self, v: &str, s: &ShapeDoc, layout: &str) {
        let d = ShapeDoc::default();
        self.line(&match self.lang {
            Lang::Js | Lang::Ts => format!("const {v} = new Shape();"),
            Lang::Rust => format!("let {v} = Shape::new();"),
            Lang::C => format!("uint32_t {v} = UW(h, shape_new);"),
        });
        if !s.name.is_empty() {
            let name = self.str(&s.name);
            match self.lang {
                Lang::Js | Lang::Ts => self.line(&format!("{v}.name = {name};")),
                _ => self.call(v, "set_name", &name),
            }
        }
        for op in &s.path {
            match *op {
                PathOp::MoveTo(x, y) => {
                    let a = self.nums(&[x, y]);
                    self.call(v, "move_to", &a)
                }
                PathOp::LineTo(x, y) => {
                    let a = self.nums(&[x, y]);
                    self.call(v, "line_to", &a)
                }
                PathOp::CurveTo(a, b, c, dd, e, f) => {
                    let a = self.nums(&[a, b, c, dd, e, f]);
                    self.call(v, "curve_to", &a)
                }
                PathOp::Rectangle(x, y, w, h) => {
                    let a = self.nums(&[x, y, w, h]);
                    self.call(v, "rectangle", &a)
                }
                PathOp::Close => self.call(v, "close_path", ""),
            }
        }
        if !s.matrix.is_identity() {
            let Matrix {
                xx,
                yx,
                xy,
                yy,
                x0,
                y0,
            } = s.matrix;
            let a = self.nums(&[xx, yx, xy, yy, x0, y0]);
            match self.lang {
                Lang::Js | Lang::Ts => self.line(&format!("{v}.matrix = new Matrix({a});")),
                Lang::Rust => self.call(v, "set_matrix", &format!("Matrix::new({a})")),
                Lang::C => self.call(v, "set_matrix", &a),
            }
        }
        if s.fill != d.fill {
            self.color(v, "set_fill_color", s.fill);
        }
        if s.stroke != d.stroke {
            self.color(v, "set_stroke_color", s.stroke);
        }
        if s.stroke_width != d.stroke_width {
            let w = self.num(s.stroke_width);
            match self.lang {
                Lang::Js | Lang::Ts => self.line(&format!("{v}.strokeWidth = {w};")),
                _ => self.call(v, "set_stroke_width", &w),
            }
        }
        if s.font != d.font {
            let f = self.str(&s.font);
            match self.lang {
                Lang::Js | Lang::Ts => self.line(&format!("{v}.font = {f};")),
                _ => self.call(v, "set_font", &f),
            }
        }
        if s.text_color != d.text_color {
            self.color(v, "set_text_color", Some(s.text_color));
        }
        if s.markup || s.flow {
            match self.lang {
                Lang::Js | Lang::Ts => {
                    if s.markup {
                        self.line(&format!("{v}.markup = true;"));
                    }
                    if s.flow {
                        self.line(&format!("{v}.flow = true;"));
                    }
                }
                Lang::Rust => self.call(v, "set_text_mode", &format!("{}, {}", s.markup, s.flow)),
                Lang::C => {
                    let flags = match (s.markup, s.flow) {
                        (true, true) => "UW_TEXT_MARKUP | UW_TEXT_FLOW",
                        (true, false) => "UW_TEXT_MARKUP",
                        _ => "UW_TEXT_FLOW",
                    };
                    self.call(v, "set_text_flags", flags)
                }
            }
        }
        if let Some(text) = &s.text {
            let t = self.str(text);
            match self.lang {
                Lang::Js | Lang::Ts => self.line(&format!("{v}.text = {t};")),
                _ => self.call(v, "set_text", &t),
            }
        }
        if let Some(img) = s.image.as_deref().filter(|i| !i.is_empty()) {
            let p = self.str(img);
            match self.lang {
                Lang::Js | Lang::Ts => self.line(&format!("{v}.image = Image.open({p});")),
                Lang::Rust => self.call(v, "set_image", &format!("Image::open({p})")),
                // UW() can't be nested: `##__VA_ARGS__` stops the inner one expanding
                Lang::C => {
                    self.line(&format!("uint32_t {v}_image = UW(h, image_open, {p});"));
                    self.call(v, "set_image", &format!("{v}_image"))
                }
            }
        }
        if let Some((src, looping)) = playable(s.video.as_deref()) {
            let p = self.str(src);
            match self.lang {
                Lang::Js | Lang::Ts => self.line(&format!(
                    "{v}.video = video.open({p}{});",
                    if looping { ", { loop: true }" } else { "" }
                )),
                Lang::Rust => self.call(
                    v,
                    "set_video",
                    &format!("Some(video::open({p}, {looping}))"),
                ),
                Lang::C => {
                    self.line(&format!(
                        "uint32_t {v}_video = UW(h, media_open, {p}, {looping});"
                    ));
                    self.call(v, "set_video", &format!("{v}_video"))
                }
            }
        }
        self.line(&match self.lang {
            Lang::Js | Lang::Ts => format!("{layout}.append({v});"),
            Lang::Rust => format!("{layout}.append(&{v});"),
            Lang::C => format!("UW(h, layout_append, {layout}, {v});"),
        });
    }

    /// `None` is written as a transparent colour, which means "none".
    fn color(&mut self, v: &str, method: &str, c: Option<Rgba>) {
        let Rgba(r, g, b, a) = c.unwrap_or(Rgba(0.0, 0.0, 0.0, 0.0));
        let args = self.nums(&[r, g, b, a]);
        self.call(v, method, &args);
    }
}

/// A video source worth exporting, and whether to loop it (package files
/// do; live sources can't).
fn playable(v: Option<&str>) -> Option<(&str, bool)> {
    let v = v.filter(|v| !v.is_empty())?;
    Some((
        v,
        !(v == "camera" || v.starts_with("test:") || v.contains("://")),
    ))
}

fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in snake.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn c_function(method: &str) -> String {
    format!("shape_{method}")
}

/// Short when that's exact, else every digit; `float` adds ".0" to whole
/// numbers (Rust and C literals).
fn num(v: f64, float: bool) -> String {
    let v = if v == 0.0 { 0.0 } else { v }; // no "-0"
    let short = {
        let s = format!("{v:.6}");
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s == "-0" { "0".to_string() } else { s }
    };
    let mut s = if short.parse::<f64>().ok() == Some(v) {
        short
    } else {
        format!("{v}")
    };
    if float && !s.contains('.') && s.chars().all(|c| c.is_ascii_digit() || c == '-') {
        s.push_str(".0");
    }
    s
}

fn js_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A C string literal; UTF-8 stays as is, control bytes become octal.
fn c_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '?' => out.push_str("\\?"), // no trigraphs
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use underweb_ipc::scene::LayoutDoc;

    fn doc() -> SceneDoc {
        SceneDoc {
            title: "Say \"hi\"".into(),
            layouts: vec![LayoutDoc {
                id: 1,
                background: Some(Rgba(1.0, 0.9, 1.0, 1.0)),
                shapes: vec![
                    ShapeDoc {
                        id: 1,
                        name: "red".into(),
                        path: vec![
                            PathOp::MoveTo(319.0, 12.0),
                            PathOp::CurveTo(404.179932, 13.5, 479.99, 64.8, 462.0, 110.0),
                            PathOp::LineTo(351.0, 268.0),
                            PathOp::Close,
                        ],
                        fill: Some(Rgba(1.0, 0.0, 0.0, 0.5)),
                        stroke: None,
                        text: Some("line one\nline \"two\"?".into()),
                        flow: true,
                        matrix: Matrix::IDENTITY.translate(10.0, 0.0).rotate(0.3),
                        ..Default::default()
                    },
                    ShapeDoc {
                        id: 2,
                        path: vec![PathOp::Rectangle(0.0, 0.0, 10.0, 10.0)],
                        image: Some("pics/a.png".into()),
                        video: Some("https://example.org/v.webm".into()),
                        ..Default::default()
                    },
                ],
            }],
        }
    }

    #[test]
    fn numbers_are_short_but_exact() {
        assert_eq!(num(319.0, false), "319");
        assert_eq!(num(319.0, true), "319.0");
        assert_eq!(num(0.073518, true), "0.073518");
        assert_eq!(num(-0.0, false), "0");
        let third = 1.0 / 3.0;
        assert_eq!(num(third, false).parse::<f64>().unwrap(), third);
    }

    #[test]
    fn each_language_says_the_same_thing() {
        let d = doc();
        let js = source(&d, Lang::Js);
        assert!(
            js.contains(
                "import { Image, Layout, Matrix, Shape, video, window } from \"underweb\";"
            ),
            "{js}"
        );
        assert!(js.contains("window.title = \"Say \\\"hi\\\"\";"));
        assert!(js.contains("shape0.curveTo(404.179932, 13.5, 479.99, 64.8, 462, 110);"));
        assert!(
            js.contains("shape0.setStrokeColor(0, 0, 0, 0);"),
            "no stroke"
        );
        assert!(js.contains("shape0.flow = true;"));
        assert!(js.contains("shape1.image = Image.open(\"pics/a.png\");"));
        assert!(js.contains("layout0.setBackground(1, 0.9, 1, 1);"));

        let ts = source(&d, Lang::Ts);
        assert!(ts.contains("export function activate(): void {"));

        let rs = source(&d, Lang::Rust);
        assert!(
            rs.contains("shape0.curve_to(404.179932, 13.5, 479.99, 64.8, 462.0, 110.0);"),
            "{rs}"
        );
        assert!(rs.contains("shape0.set_text_mode(false, true);"));
        assert!(rs.contains(
            "shape1.set_video(Some(video::open(\"https://example.org/v.webm\", false)));"
        ));
        assert!(rs.contains("underweb::export_page!(Exported);"));

        let c = source(&d, Lang::C);
        assert!(
            c.contains("UW(h, shape_set_text, shape0, \"line one\\nline \\\"two\\\"\\?\");"),
            "{c}"
        );
        assert!(c.contains("UW(h, shape_set_text_flags, shape0, UW_TEXT_FLOW);"));
        assert!(c.contains("uint32_t shape1_image = UW(h, image_open, \"pics/a.png\");\n    UW(h, shape_set_image, shape1, shape1_image);"), "{c}");
        assert!(c.contains("UW(h, layout_set_background, layout0, 1.0, 0.9, 1.0, 1.0);"));
    }

    #[test]
    fn packages_declare_what_their_scene_needs() {
        let ex = package(&doc(), Lang::C, "my-page").unwrap();
        let manifest = &ex.files[0].1;
        assert!(manifest.contains("kind = \"c\""));
        assert!(manifest.contains("sources = [\"main.c\"]"));
        assert!(manifest.contains("fetch = [\"https://example.org/\"]"));
        assert_eq!(ex.assets, vec!["pics/a.png".to_string()]);
        let rs = package(&doc(), Lang::Rust, "my-page").unwrap();
        assert!(
            rs.files
                .iter()
                .any(|(p, t)| p == "Cargo.toml" && t.contains("underweb = \"0.1\""))
        );
        assert!(package(&doc(), Lang::Js, "Bad Name").is_err());
    }
}
