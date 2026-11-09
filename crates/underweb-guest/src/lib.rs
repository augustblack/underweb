//! The curated page API, implemented once in Rust.
//!
//! This is the successor of `libmentiras`: a window holds layouts, a layout
//! holds shapes, and a shape is a cairo path with fill, stroke, an image or
//! video inside it, and pango text. The runner exposes it to native
//! packages through the C ABI in `underweb-abi` and to JS/TS packages
//! through V8 ops; both front ends call into [`Scene`].
//!
//! The scene is retained (not immediate-mode) so the host can hit-test,
//! and so the layout editor can inspect it ([`Scene::doc`]) and change it
//! ([`Scene::apply`]).

mod text;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use underweb_ipc::VideoMailbox;
pub use underweb_ipc::scene::{EditOp, Id, LayoutDoc, Matrix, PathOp, Rgba, SceneDoc, ShapeDoc};

pub use cairo;
pub use pango;

#[derive(Debug, Clone)]
pub struct Shape {
    pub name: String,
    pub path: Vec<PathOp>,
    /// From the shape's own coordinates to the window's.
    pub matrix: Matrix,
    pub fill: Option<Rgba>,
    pub stroke: Option<Rgba>,
    pub stroke_width: f64,
    pub text: Option<String>,
    pub text_color: Rgba,
    pub font: String,
    /// The text is Pango markup.
    pub markup: bool,
    /// The text flows word by word inside the path.
    pub flow: bool,
    /// An image drawn at the shape's origin, clipped to the path.
    pub image: Option<Id>,
    /// A video stream shown inside the path.
    pub video: Option<Id>,
    /// How the video is shown.
    pub video_filter: VideoFilter,
}

/// A picture effect on a shape's video (`UW_FILTER_*`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum VideoFilter {
    #[default]
    None,
    Grayscale,
    Invert,
    /// Blocks of this many pixels (of the video).
    Pixelate(u32),
    /// This many levels per colour channel.
    Posterize(u32),
}

impl VideoFilter {
    /// From a `UW_FILTER_*` and its amount (0 for the default).
    pub fn from_abi(kind: u32, amount: f64) -> VideoFilter {
        let n = |default: u32| {
            if amount >= 1.0 {
                (amount as u32).clamp(2, 256)
            } else {
                default
            }
        };
        match kind {
            underweb_abi::UW_FILTER_GRAYSCALE => VideoFilter::Grayscale,
            underweb_abi::UW_FILTER_INVERT => VideoFilter::Invert,
            underweb_abi::UW_FILTER_PIXELATE => VideoFilter::Pixelate(n(12)),
            underweb_abi::UW_FILTER_POSTERIZE => VideoFilter::Posterize(n(4)),
            _ => VideoFilter::None,
        }
    }

    /// A filtered copy of `frame`, or `None` to show it as it is.
    pub fn apply(self, frame: &cairo::ImageSurface) -> Option<cairo::ImageSurface> {
        if self == VideoFilter::None {
            return None;
        }
        let (w, h) = (frame.width(), frame.height());
        let mut out = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
        {
            let cr = cairo::Context::new(&out).ok()?;
            cr.set_source_surface(frame, 0.0, 0.0).ok()?;
            cr.paint().ok()?;
        }
        out.flush();
        let stride = out.stride() as usize;
        let (w, h) = (w as usize, h as usize);
        {
            let mut data = out.data().ok()?;
            // premultiplied BGRA, so colours never exceed alpha
            match self {
                VideoFilter::Grayscale => {
                    for px in data.chunks_exact_mut(4) {
                        let y = (px[2] as u32 * 77 + px[1] as u32 * 150 + px[0] as u32 * 29) >> 8;
                        px[0] = y as u8;
                        px[1] = y as u8;
                        px[2] = y as u8;
                    }
                }
                VideoFilter::Invert => {
                    for px in data.chunks_exact_mut(4) {
                        let a = px[3];
                        for c in &mut px[..3] {
                            *c = a - (*c).min(a);
                        }
                    }
                }
                VideoFilter::Posterize(levels) => {
                    let steps = (levels.max(2) - 1) as f64;
                    for px in data.chunks_exact_mut(4) {
                        let a = px[3];
                        for c in &mut px[..3] {
                            let v = (*c as f64 / 255.0 * steps).round() / steps * 255.0;
                            *c = (v as u8).min(a);
                        }
                    }
                }
                VideoFilter::Pixelate(block) => {
                    let b = block.max(2) as usize;
                    for by in (0..h).step_by(b) {
                        for bx in (0..w).step_by(b) {
                            let (y1, x1) = ((by + b).min(h), (bx + b).min(w));
                            let mut sum = [0u32; 4];
                            for y in by..y1 {
                                for x in bx..x1 {
                                    let p = &data[y * stride + x * 4..][..4];
                                    for (t, v) in sum.iter_mut().zip(p) {
                                        *t += *v as u32;
                                    }
                                }
                            }
                            let n = ((y1 - by) * (x1 - bx)) as u32;
                            let avg = sum.map(|v| (v / n) as u8);
                            for y in by..y1 {
                                for x in bx..x1 {
                                    data[y * stride + x * 4..][..4].copy_from_slice(&avg);
                                }
                            }
                        }
                    }
                }
                VideoFilter::None => {}
            }
        }
        out.mark_dirty();
        Some(out)
    }
}

impl Default for Shape {
    fn default() -> Self {
        let d = ShapeDoc::default();
        Shape {
            name: d.name,
            path: d.path,
            matrix: d.matrix,
            fill: d.fill,
            stroke: d.stroke,
            stroke_width: d.stroke_width,
            text: d.text,
            text_color: d.text_color,
            font: d.font,
            markup: d.markup,
            flow: d.flow,
            image: None,
            video: None,
            video_filter: VideoFilter::None,
        }
    }
}

/// A fill or stroke colour; fully transparent means none.
pub fn paint(r: f64, g: f64, b: f64, a: f64) -> Option<Rgba> {
    (a > 0.0).then_some(Rgba(r, g, b, a))
}

/// Inner padding between a shape's bounding box and its text.
const TEXT_PADDING: f64 = 10.0;
/// Images larger than this are refused (pixels).
const MAX_IMAGE_PIXELS: u64 = 64 << 20;

#[derive(Debug, Default, Clone)]
pub struct Layout {
    pub shapes: Vec<Id>,
    /// Painted over the whole window under the layout's shapes.
    pub background: Option<Rgba>,
}

/// A decoded image: where it came from ("" for bytes) and its pixels.
pub struct Image {
    pub source: String,
    surface: cairo::ImageSurface,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Image({:?} {}x{})",
            self.source,
            self.width(),
            self.height()
        )
    }
}

impl Image {
    pub fn width(&self) -> u32 {
        self.surface.width() as u32
    }
    pub fn height(&self) -> u32 {
        self.surface.height() as u32
    }
}

/// A video stream's mailbox and the last frame turned into a surface.
pub struct Video {
    mailbox: VideoMailbox,
    cache: RefCell<(u64, Option<cairo::ImageSurface>)>,
}

impl std::fmt::Debug for Video {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Video({}x{})", self.mailbox.width, self.mailbox.height)
    }
}

impl Video {
    /// The newest frame, copied out of the mailbox only when it changed.
    fn surface(&self) -> Option<cairo::ImageSurface> {
        let mut cache = self.cache.borrow_mut();
        let frames = self.mailbox.frames();
        if frames != cache.0 || cache.1.is_none() {
            let mut buf = Vec::new();
            if let Some(n) = self.mailbox.read_frame(&mut buf) {
                let m = &self.mailbox;
                cache.1 = cairo::ImageSurface::create_for_data(
                    buf,
                    cairo::Format::ARgb32,
                    m.width as i32,
                    m.height as i32,
                    m.stride as i32,
                )
                .ok();
                cache.0 = n;
            }
        }
        cache.1.clone()
    }

    pub fn width(&self) -> u32 {
        self.mailbox.width
    }
    pub fn height(&self) -> u32 {
        self.mailbox.height
    }
    pub fn position(&self) -> f64 {
        self.mailbox.position()
    }
    pub fn duration(&self) -> f64 {
        self.mailbox.duration()
    }
}

/// Everything a page has built, plus the window state it can see.
#[derive(Debug, Default)]
pub struct Scene {
    shapes: Vec<Shape>,
    layouts: Vec<Layout>,
    /// Layouts shown in the window, bottom to top.
    window_layouts: Vec<Id>,
    pub title: String,
    /// Logical size of the window.
    pub width: f64,
    pub height: f64,
    pub params: HashMap<String, String>,
    pub animating: bool,
    /// Where package files (images) are read from.
    pub package_dir: Option<PathBuf>,
    dirty: bool,
    images: Vec<Image>,
    videos: HashMap<Id, Video>,
    /// What each video stream was opened from, for the editor.
    video_sources: HashMap<Id, String>,
    /// Target for calls with unknown shape ids; never drawn.
    scratch: Shape,
}

impl Scene {
    pub fn new() -> Scene {
        Scene {
            dirty: true,
            ..Default::default()
        }
    }

    // ---- window ---------------------------------------------------------

    pub fn set_title(&mut self, title: &str) {
        self.title = title.to_string();
    }

    pub fn add_layout(&mut self, layout: Id) {
        if self.layout(layout).is_some() && !self.window_layouts.contains(&layout) {
            self.window_layouts.push(layout);
            self.dirty = true;
        }
    }

    pub fn request_redraw(&mut self) {
        self.dirty = true;
    }

    /// True once after any change; the runner redraws then.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    // ---- layouts --------------------------------------------------------

    pub fn layout_new(&mut self) -> Id {
        self.layouts.push(Layout::default());
        self.layouts.len() as Id
    }

    pub fn layout(&self, id: Id) -> Option<&Layout> {
        self.layouts.get((id as usize).wrapping_sub(1))
    }

    fn layout_mut(&mut self, id: Id) -> Option<&mut Layout> {
        self.dirty = true;
        self.layouts.get_mut((id as usize).wrapping_sub(1))
    }

    pub fn layout_append(&mut self, layout: Id, shape: Id) {
        if self.shape(shape).is_none() {
            return;
        }
        if let Some(l) = self.layout_mut(layout) {
            l.shapes.push(shape);
        }
    }

    pub fn layout_set_background(&mut self, layout: Id, color: Option<Rgba>) {
        if let Some(l) = self.layout_mut(layout) {
            l.background = color;
        }
    }

    // ---- shapes ---------------------------------------------------------

    pub fn shape_new(&mut self) -> Id {
        self.shapes.push(Shape::default());
        self.shapes.len() as Id
    }

    pub fn shape(&self, id: Id) -> Option<&Shape> {
        self.shapes.get((id as usize).wrapping_sub(1))
    }

    /// Mutable access marks the scene dirty. Unknown ids are ignored (the
    /// change goes to a scratch shape), so a buggy page can't crash the
    /// runner.
    pub fn shape_mut(&mut self, id: Id) -> &mut Shape {
        self.dirty = true;
        match self.shapes.get_mut((id as usize).wrapping_sub(1)) {
            Some(s) => s,
            None => &mut self.scratch,
        }
    }

    pub fn shape_set_video_filter(&mut self, id: Id, filter: VideoFilter) {
        self.shape_mut(id).video_filter = filter;
        self.request_redraw();
    }

    /// Show an image in the shape. A shape with no path yet gets the
    /// image's rectangle.
    pub fn shape_set_image(&mut self, id: Id, image: Option<Id>) {
        let size = image
            .and_then(|i| self.image(i))
            .map(|i| (i.width(), i.height()));
        let shape = self.shape_mut(id);
        shape.image = image.filter(|_| size.is_some());
        if let (Some((w, h)), true) = (size, shape.path.is_empty()) {
            shape
                .path
                .push(PathOp::Rectangle(0.0, 0.0, w as f64, h as f64));
        }
    }

    // ---- images ---------------------------------------------------------

    /// Decode an image (PNG, JPEG or GIF). `source` is remembered for the
    /// editor.
    pub fn image_from_bytes(&mut self, source: &str, bytes: &[u8]) -> Result<Id, String> {
        let img = image::load_from_memory(bytes).map_err(|e| format!("bad image: {e}"))?;
        let (w, h) = (img.width(), img.height());
        if w as u64 * h as u64 > MAX_IMAGE_PIXELS {
            return Err(format!("image too large ({w}x{h})"));
        }
        let rgba = img.into_rgba8();
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w as i32, h as i32)
            .map_err(|e| e.to_string())?;
        let stride = surface.stride() as usize;
        {
            let mut data = surface.data().map_err(|e| e.to_string())?;
            for (y, row) in rgba.rows().enumerate() {
                let out = &mut data[y * stride..y * stride + w as usize * 4];
                for (px, o) in row.zip(out.chunks_exact_mut(4)) {
                    let [r, g, b, a] = px.0;
                    let pm = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
                    // cairo ARGB32 is native-endian: B G R A in memory
                    o.copy_from_slice(&u32::from_be_bytes([a, pm(r), pm(g), pm(b)]).to_ne_bytes());
                }
            }
        }
        surface.mark_dirty();
        self.images.push(Image {
            source: source.to_string(),
            surface,
        });
        Ok(self.images.len() as Id)
    }

    /// Load an image file from the package (a relative path).
    pub fn image_open(&mut self, rel: &str) -> Result<Id, String> {
        let path = self.package_file(rel)?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{rel}: {e}"))?;
        self.image_from_bytes(rel, &bytes)
    }

    pub fn image(&self, id: Id) -> Option<&Image> {
        self.images.get((id as usize).wrapping_sub(1))
    }

    fn package_file(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        if rel.is_empty()
            || p.is_absolute()
            || p.components().any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!("{rel:?}: files are named relative to the package"));
        }
        let dir = self.package_dir.as_ref().ok_or("no package directory")?;
        Ok(dir.join(p))
    }

    // ---- video ----------------------------------------------------------

    pub fn video_attach(&mut self, id: Id, mailbox: VideoMailbox) {
        self.videos.insert(
            id,
            Video {
                mailbox,
                cache: RefCell::new((0, None)),
            },
        );
        self.dirty = true;
    }

    pub fn video_remove(&mut self, id: Id) {
        self.videos.remove(&id);
        self.video_sources.remove(&id);
        self.dirty = true;
    }

    /// Remember what stream `id` was opened from (for the editor).
    pub fn video_source(&mut self, id: Id, source: &str) {
        self.video_sources.insert(id, source.to_string());
    }

    pub fn video(&self, id: Id) -> Option<&Video> {
        self.videos.get(&id)
    }

    pub fn has_videos(&self) -> bool {
        !self.videos.is_empty()
    }

    /// Whether a video shown in a shape has a frame not drawn yet.
    pub fn videos_changed(&self) -> bool {
        self.videos.iter().any(|(id, v)| {
            v.mailbox.frames() != v.cache.borrow().0
                && self.shapes.iter().any(|s| s.video == Some(*id))
        })
    }

    // ---- the editor's view ----------------------------------------------

    /// The shown layouts and their shapes as plain data.
    pub fn doc(&self) -> SceneDoc {
        SceneDoc {
            title: self.title.clone(),
            layouts: self
                .window_layouts
                .iter()
                .filter_map(|&l| {
                    let layout = self.layout(l)?;
                    Some(LayoutDoc {
                        id: l,
                        background: layout.background,
                        shapes: layout
                            .shapes
                            .iter()
                            .filter_map(|&s| Some(self.shape_doc(s, self.shape(s)?)))
                            .collect(),
                    })
                })
                .collect(),
        }
    }

    fn shape_doc(&self, id: Id, s: &Shape) -> ShapeDoc {
        ShapeDoc {
            id,
            name: s.name.clone(),
            path: s.path.clone(),
            matrix: s.matrix,
            fill: s.fill,
            stroke: s.stroke,
            stroke_width: s.stroke_width,
            text: s.text.clone(),
            text_color: s.text_color,
            font: s.font.clone(),
            markup: s.markup,
            flow: s.flow,
            image: s
                .image
                .and_then(|i| self.image(i))
                .map(|i| i.source.clone()),
            video: s
                .video
                .map(|v| self.video_sources.get(&v).cloned().unwrap_or_default()),
        }
    }

    /// Apply a layout-editor change. Returns the new shape's id for
    /// [`EditOp::NewShape`].
    pub fn apply(&mut self, op: EditOp) -> Result<Option<Id>, String> {
        match op {
            EditOp::SetShape(doc) => {
                if self.shape(doc.id).is_none() {
                    return Err(format!("no shape {}", doc.id));
                }
                self.set_from_doc(doc.id, doc)?;
                Ok(None)
            }
            EditOp::NewShape { layout, shape } => {
                let layout = if self.window_layouts.contains(&layout) {
                    layout
                } else {
                    let l = self.layout_new();
                    self.add_layout(l);
                    l
                };
                let id = self.shape_new();
                self.set_from_doc(id, shape)?;
                self.layout_append(layout, id);
                Ok(Some(id))
            }
            EditOp::Delete(id) => {
                for l in &mut self.layouts {
                    l.shapes.retain(|&s| s != id);
                }
                self.dirty = true;
                Ok(None)
            }
            EditOp::Restack { id, up } => {
                for l in &mut self.layouts {
                    if let Some(i) = l.shapes.iter().position(|&s| s == id) {
                        let j = if up { i + 1 } else { i.wrapping_sub(1) };
                        if j < l.shapes.len() {
                            l.shapes.swap(i, j);
                        }
                    }
                }
                self.dirty = true;
                Ok(None)
            }
            EditOp::SetBackground { layout, color } => {
                self.layout_set_background(layout, color);
                Ok(None)
            }
            EditOp::SetTitle(t) => {
                self.title = t;
                Ok(None)
            }
        }
    }

    fn set_from_doc(&mut self, id: Id, d: ShapeDoc) -> Result<(), String> {
        // a new image source is loaded; the same one (or "") keeps the image
        let old = self.shape(id).and_then(|s| s.image);
        let old_source = old.and_then(|i| self.image(i)).map(|i| i.source.clone());
        let image = match &d.image {
            None => None,
            Some(src) if src.is_empty() || Some(src) == old_source.as_ref() => old,
            Some(src) => Some(self.image_open(src)?),
        };
        let s = self.shape_mut(id);
        s.name = d.name;
        s.path = d.path;
        s.matrix = d.matrix;
        s.fill = d.fill;
        s.stroke = d.stroke;
        s.stroke_width = d.stroke_width;
        s.text = d.text;
        s.text_color = d.text_color;
        s.font = d.font;
        s.markup = d.markup;
        s.flow = d.flow;
        self.shape_set_image(id, image);
        Ok(())
    }

    // ---- rendering ------------------------------------------------------

    /// Draw every visible layout. The context is in logical pixels.
    pub fn render(&self, cr: &cairo::Context) {
        for &l in &self.window_layouts {
            let Some(layout) = self.layout(l) else {
                continue;
            };
            if let Some(bg) = layout.background {
                set_color(cr, bg);
                let _ = cr.paint();
            }
            for &s in &layout.shapes {
                if let Some(shape) = self.shape(s) {
                    let video = shape
                        .video
                        .and_then(|v| self.videos.get(&v))
                        .and_then(Video::surface)
                        .map(|f| shape.video_filter.apply(&f).unwrap_or(f));
                    let image = shape.image.and_then(|i| self.image(i)).map(|i| &i.surface);
                    render_shape(cr, shape, image, video.as_ref());
                }
            }
        }
    }

    /// Topmost shape whose fill (or stroke) contains the point, or 0.
    pub fn hit_test(&self, x: f64, y: f64) -> Id {
        let surface = cairo::ImageSurface::create(cairo::Format::A8, 1, 1).unwrap();
        let cr = cairo::Context::new(&surface).unwrap();
        for &l in self.window_layouts.iter().rev() {
            let Some(layout) = self.layout(l) else {
                continue;
            };
            for &s in layout.shapes.iter().rev() {
                let Some(shape) = self.shape(s) else { continue };
                if shape.matrix.invert().is_none() {
                    continue;
                }
                cr.set_matrix(to_cairo(&shape.matrix));
                cr.new_path();
                append_path(&cr, &shape.path);
                cr.set_line_width(shape.stroke_width);
                let (ux, uy) = cr.device_to_user(x, y).unwrap_or((x, y));
                let inside = cr.in_fill(ux, uy).unwrap_or(false)
                    || (shape.stroke.is_some() && cr.in_stroke(ux, uy).unwrap_or(false));
                if inside {
                    return s;
                }
            }
        }
        0
    }
}

fn to_cairo(m: &Matrix) -> cairo::Matrix {
    cairo::Matrix::new(m.xx, m.yx, m.xy, m.yy, m.x0, m.y0)
}

fn append_path(cr: &cairo::Context, path: &[PathOp]) {
    for op in path {
        match *op {
            PathOp::MoveTo(x, y) => cr.move_to(x, y),
            PathOp::LineTo(x, y) => cr.line_to(x, y),
            PathOp::CurveTo(a, b, c, d, e, f) => cr.curve_to(a, b, c, d, e, f),
            PathOp::Rectangle(x, y, w, h) => cr.rectangle(x, y, w, h),
            PathOp::Close => cr.close_path(),
        }
    }
}

fn set_color(cr: &cairo::Context, c: Rgba) {
    cr.set_source_rgba(c.0, c.1, c.2, c.3);
}

fn render_shape(
    cr: &cairo::Context,
    shape: &Shape,
    image: Option<&cairo::ImageSurface>,
    video: Option<&cairo::ImageSurface>,
) {
    if shape.matrix.invert().is_none() {
        return; // squashed flat: nothing to see
    }
    let _ = cr.save();
    cr.transform(to_cairo(&shape.matrix));
    cr.new_path();
    append_path(cr, &shape.path);
    let (x0, y0, x1, y1) = cr.path_extents().unwrap_or((0.0, 0.0, 0.0, 0.0));
    if let Some(fill) = shape.fill {
        set_color(cr, fill);
        let _ = cr.fill_preserve();
    }
    if let Some(img) = image {
        // at the shape's origin, clipped to the path (as Mentiras.Image)
        let _ = cr.save();
        cr.clip_preserve();
        let _ = cr.set_source_surface(img, 0.0, 0.0);
        let _ = cr.paint();
        let _ = cr.restore();
    }
    if let Some(frame) = video {
        // fill the shape's bounds, keeping the picture's proportions,
        // clipped to the path (videoplayer.vala did the same)
        let (bw, bh) = (x1 - x0, y1 - y0);
        let (fw, fh) = (frame.width() as f64, frame.height() as f64);
        if bw > 0.0 && bh > 0.0 && fw > 0.0 && fh > 0.0 {
            let _ = cr.save();
            cr.clip_preserve();
            let s = (bw / fw).max(bh / fh);
            cr.translate(x0 + (bw - fw * s) / 2.0, y0 + (bh - fh * s) / 2.0);
            cr.scale(s, s);
            let _ = cr.set_source_surface(frame, 0.0, 0.0);
            let _ = cr.paint();
            let _ = cr.restore();
        }
    }
    if let Some(stroke) = shape.stroke {
        set_color(cr, stroke);
        cr.set_line_width(shape.stroke_width);
        let _ = cr.stroke_preserve();
    }
    cr.new_path();

    if let Some(text) = shape.text.as_deref().filter(|t| !t.is_empty()) {
        let font = pango::FontDescription::from_string(&shape.font);
        set_color(cr, shape.text_color);
        if shape.flow {
            text::flow(cr, &shape.path, (x0, y0, x1, y1), text, shape.markup, &font);
        } else {
            let layout = pangocairo::functions::create_layout(cr);
            layout.set_font_description(Some(&font));
            let width = (x1 - x0 - 2.0 * TEXT_PADDING).max(1.0);
            layout.set_width((width * pango::SCALE as f64) as i32);
            layout.set_wrap(pango::WrapMode::WordChar);
            if shape.markup {
                layout.set_markup(text);
            } else {
                layout.set_text(text);
            }
            cr.move_to(x0 + TEXT_PADDING, y0 + TEXT_PADDING);
            pangocairo::functions::show_layout(cr, &layout);
        }
        cr.new_path();
    }
    let _ = cr.restore();
}

/// Render a frame into a cairo ARGB32 buffer of `width` x `height` physical
/// pixels. `extra` runs after the scene, for packages that draw with raw
/// cairo.
///
/// # Safety
/// `data` must point to at least `stride * height` writable bytes that
/// nothing else touches during the call.
pub unsafe fn render_frame(
    scene: &Scene,
    data: *mut u8,
    width: u32,
    height: u32,
    stride: u32,
    scale: f64,
    extra: impl FnOnce(&cairo::Context),
) {
    let surface = unsafe {
        cairo::ImageSurface::create_for_data_unsafe(
            data,
            cairo::Format::ARgb32,
            width as i32,
            height as i32,
            stride as i32,
        )
    }
    .expect("frame surface");
    {
        let cr = cairo::Context::new(&surface).expect("cairo context");
        cr.set_source_rgb(1.0, 1.0, 1.0);
        let _ = cr.paint();
        cr.scale(scale, scale);
        scene.render(&cr);
        let _ = cr.save();
        extra(&cr);
        let _ = cr.restore();
    }
    surface.flush();
}

/// Load fontconfig/CoreText state and a font before the sandbox closes
/// most of the filesystem.
pub fn warm_up_fonts() {
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 64, 16).unwrap();
    let cr = cairo::Context::new(&surface).unwrap();
    let layout = pangocairo::functions::create_layout(&cr);
    layout.set_font_description(Some(&pango::FontDescription::from_string("Sans 14")));
    layout.set_text("Underweb");
    pangocairo::functions::show_layout(&cr, &layout);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_scene() -> (Scene, Id) {
        let mut s = Scene::new();
        let l = s.layout_new();
        let sh = s.shape_new();
        s.shape_mut(sh)
            .path
            .push(PathOp::Rectangle(10.0, 10.0, 100.0, 50.0));
        s.shape_mut(sh).fill = Some(Rgba(1.0, 0.0, 0.0, 1.0));
        s.layout_append(l, sh);
        s.add_layout(l);
        (s, sh)
    }

    fn render(s: &Scene, w: u32, h: u32) -> Vec<u8> {
        let mut buf = vec![0u8; (w * 4 * h) as usize];
        unsafe { render_frame(s, buf.as_mut_ptr(), w, h, w * 4, 1.0, |_| {}) };
        buf
    }

    fn pixel(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        buf[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn hit_test_finds_topmost() {
        let (mut s, a) = rect_scene();
        assert_eq!(s.hit_test(50.0, 30.0), a);
        assert_eq!(s.hit_test(500.0, 30.0), 0);
        let l = s.layout_new();
        let b = s.shape_new();
        s.shape_mut(b)
            .path
            .push(PathOp::Rectangle(40.0, 20.0, 10.0, 10.0));
        s.layout_append(l, b);
        s.add_layout(l);
        assert_eq!(s.hit_test(45.0, 25.0), b);
    }

    #[test]
    fn renders_fill_into_buffer() {
        let (mut s, sh) = rect_scene();
        s.shape_mut(sh).text = Some("hello".into());
        let buf = render(&s, 200, 100);
        // pixel (100, 55) is inside the red rect, below the text: BGRA
        assert_eq!(pixel(&buf, 200, 100, 55), [0, 0, 255, 255]);
        // outside is the white background
        assert_eq!(pixel(&buf, 200, 190, 90), [255, 255, 255, 255]);
    }

    #[test]
    fn matrix_moves_drawing_and_hit_testing() {
        let (mut s, sh) = rect_scene();
        s.shape_mut(sh).stroke = None;
        s.shape_mut(sh).matrix = Matrix::IDENTITY.translate(100.0, 20.0);
        assert_eq!(s.hit_test(50.0, 30.0), 0);
        assert_eq!(s.hit_test(150.0, 50.0), sh);
        let buf = render(&s, 300, 120);
        assert_eq!(pixel(&buf, 300, 50, 30), [255, 255, 255, 255]);
        assert_eq!(pixel(&buf, 300, 150, 50), [0, 0, 255, 255]);
    }

    #[test]
    fn layout_background_and_image() {
        let (mut s, sh) = rect_scene();
        s.layout_set_background(1, Some(Rgba(0.0, 1.0, 0.0, 1.0)));
        // a blue PNG, made with cairo
        let png = {
            let img = cairo::ImageSurface::create(cairo::Format::ARgb32, 200, 200).unwrap();
            let cr = cairo::Context::new(&img).unwrap();
            cr.set_source_rgb(0.0, 0.0, 1.0);
            cr.paint().unwrap();
            drop(cr);
            let mut out = Vec::new();
            img.write_to_png(&mut out).unwrap();
            out
        };
        let img = s.image_from_bytes("blue.png", &png).unwrap();
        s.shape_set_image(sh, Some(img));
        let buf = render(&s, 200, 100);
        assert_eq!(pixel(&buf, 200, 190, 90), [0, 255, 0, 255], "background");
        assert_eq!(
            pixel(&buf, 200, 50, 30),
            [255, 0, 0, 255],
            "image in the path"
        );
        assert_eq!(
            s.doc().layouts[0].shapes[0].image.as_deref(),
            Some("blue.png")
        );
        assert!(s.image_from_bytes("x", b"not an image").is_err());
    }

    #[test]
    fn image_paths_stay_in_the_package() {
        let mut s = Scene::new();
        s.package_dir = Some(PathBuf::from("/tmp"));
        for bad in ["../x.png", "/etc/passwd", "a/../../b.png", ""] {
            assert!(s.image_open(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn doc_and_edits_round_trip() {
        let (mut s, sh) = rect_scene();
        s.shape_mut(sh).name = "box".into();
        let doc = s.doc();
        assert_eq!(doc.layouts.len(), 1);
        let mut d = doc.layouts[0].shapes[0].clone();
        assert_eq!(d.name, "box");
        d.matrix = Matrix::IDENTITY.translate(5.0, 5.0);
        d.text = Some("moved".into());
        s.apply(EditOp::SetShape(d.clone())).unwrap();
        assert_eq!(s.doc().layouts[0].shapes[0], d);

        let new = s
            .apply(EditOp::NewShape {
                layout: 1,
                shape: ShapeDoc {
                    path: vec![PathOp::Rectangle(0.0, 0.0, 5.0, 5.0)],
                    ..Default::default()
                },
            })
            .unwrap()
            .unwrap();
        assert_eq!(s.doc().layouts[0].shapes.len(), 2);
        s.apply(EditOp::Restack { id: new, up: false }).unwrap();
        assert_eq!(s.doc().layouts[0].shapes[0].id, new);
        s.apply(EditOp::Delete(new)).unwrap();
        assert_eq!(s.doc().layouts[0].shapes.len(), 1);
        // a shape for an empty page gets a layout of its own
        let mut empty = Scene::new();
        empty
            .apply(EditOp::NewShape {
                layout: 0,
                shape: ShapeDoc::default(),
            })
            .unwrap();
        assert_eq!(empty.doc().layouts.len(), 1);
    }

    #[test]
    fn flowing_text_stays_inside_the_path() {
        let mut s = Scene::new();
        let l = s.layout_new();
        let sh = s.shape_new();
        // a triangle: text must not appear in the empty top-left corner
        let shape = s.shape_mut(sh);
        shape.path = vec![
            PathOp::MoveTo(0.0, 0.0),
            PathOp::LineTo(300.0, 300.0),
            PathOp::LineTo(0.0, 300.0),
            PathOp::Close,
        ];
        shape.stroke = None;
        shape.flow = true;
        shape.text = Some("word ".repeat(400));
        shape.text_color = Rgba::BLACK;
        s.layout_append(l, sh);
        s.add_layout(l);
        let buf = render(&s, 300, 300);
        let dark = |x0: u32, y0: u32, x1: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| pixel(&buf, 300, x, y)[0] < 128)
                .count()
        };
        assert!(dark(10, 200, 150, 290) > 100, "text inside the triangle");
        assert_eq!(dark(200, 0, 300, 100), 0, "nothing outside it");
    }

    #[test]
    fn bad_ids_are_ignored() {
        let mut s = Scene::new();
        s.shape_mut(99).text = Some("x".into());
        s.layout_append(5, 7);
        s.add_layout(3);
        s.shape_set_image(99, Some(42));
        assert_eq!(s.hit_test(0.0, 0.0), 0);
        assert!(
            s.apply(EditOp::SetShape(ShapeDoc {
                id: 9,
                ..Default::default()
            }))
            .is_err()
        );
    }

    /// A 4x4 frame: left half (r, g, b), right half white.
    fn frame(r: u8, g: u8, b: u8) -> cairo::ImageSurface {
        let mut f = cairo::ImageSurface::create(cairo::Format::ARgb32, 4, 4).unwrap();
        let stride = f.stride() as usize;
        {
            let mut d = f.data().unwrap();
            for y in 0..4 {
                for x in 0..4 {
                    let px = if x < 2 {
                        [b, g, r, 255]
                    } else {
                        [255, 255, 255, 255]
                    };
                    d[y * stride + x * 4..][..4].copy_from_slice(&px);
                }
            }
        }
        f
    }

    fn px(f: &mut cairo::ImageSurface, x: usize, y: usize) -> [u8; 4] {
        let stride = f.stride() as usize;
        let d = f.data().unwrap();
        d[y * stride + x * 4..][..4].try_into().unwrap()
    }

    #[test]
    fn video_filters() {
        let src = frame(200, 40, 0);
        assert!(VideoFilter::None.apply(&src).is_none());

        let mut g = VideoFilter::Grayscale.apply(&src).unwrap();
        let [b, gr, r, a] = px(&mut g, 0, 0);
        assert!(b == gr && gr == r && a == 255, "{:?}", [b, gr, r, a]);
        assert!(
            (80..=90).contains(&r),
            "luma of (200,40,0) is about 84: {r}"
        );

        let mut i = VideoFilter::Invert.apply(&src).unwrap();
        assert_eq!(px(&mut i, 0, 0), [255, 215, 55, 255]);
        assert_eq!(px(&mut i, 3, 3), [0, 0, 0, 255]);

        let mut p = VideoFilter::Posterize(2).apply(&src).unwrap();
        assert_eq!(px(&mut p, 0, 0), [0, 0, 255, 255]);

        // one 4x4 block: everything is the average of both halves
        let mut q = VideoFilter::Pixelate(4).apply(&src).unwrap();
        let avg = px(&mut q, 0, 0);
        assert_eq!(avg, [127, 147, 227, 255]);
        assert_eq!(px(&mut q, 3, 3), avg);
    }

    #[test]
    fn filters_from_the_abi() {
        use underweb_abi::*;
        assert_eq!(
            VideoFilter::from_abi(UW_FILTER_PIXELATE, 0.0),
            VideoFilter::Pixelate(12)
        );
        assert_eq!(
            VideoFilter::from_abi(UW_FILTER_POSTERIZE, 3.0),
            VideoFilter::Posterize(3)
        );
        assert_eq!(VideoFilter::from_abi(99, 1.0), VideoFilter::None);
    }
}
