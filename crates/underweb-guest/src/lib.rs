//! The curated page API, implemented once in Rust.
//!
//! This is the successor of `libmentiras`: a window holds layouts, a layout
//! holds shapes, and a shape is a cairo path with fill, stroke and pango
//! text. The runner exposes it to native packages through the C ABI in
//! `underweb-abi` and to JS/TS packages through V8 ops; both front ends call
//! into [`Scene`].
//!
//! The scene is retained (not immediate-mode) so the host can hit-test and,
//! later, so the layout editor can inspect and edit it.

use std::collections::HashMap;

pub use cairo;
pub use pango;

pub type Id = u32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba(pub f64, pub f64, pub f64, pub f64);

impl Rgba {
    pub const BLACK: Rgba = Rgba(0.0, 0.0, 0.0, 1.0);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathOp {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    CurveTo(f64, f64, f64, f64, f64, f64),
    Rectangle(f64, f64, f64, f64),
    Close,
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub path: Vec<PathOp>,
    pub fill: Option<Rgba>,
    pub stroke: Option<Rgba>,
    pub stroke_width: f64,
    pub text: Option<String>,
    pub text_color: Rgba,
    pub font: String,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            path: Vec::new(),
            fill: None,
            stroke: Some(Rgba::BLACK),
            stroke_width: 1.0,
            text: None,
            text_color: Rgba::BLACK,
            font: "Sans 14".into(),
        }
    }
}

/// Inner padding between a shape's bounding box and its text.
const TEXT_PADDING: f64 = 10.0;

#[derive(Debug, Default, Clone)]
pub struct Layout {
    pub shapes: Vec<Id>,
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
    dirty: bool,
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

    pub fn layout_append(&mut self, layout: Id, shape: Id) {
        if self.shape(shape).is_none() {
            return;
        }
        if let Some(l) = self.layouts.get_mut((layout as usize).wrapping_sub(1)) {
            l.shapes.push(shape);
            self.dirty = true;
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

    // ---- rendering ------------------------------------------------------

    /// Draw every visible layout. The context is in logical pixels.
    pub fn render(&self, cr: &cairo::Context) {
        for &l in &self.window_layouts {
            let Some(layout) = self.layout(l) else {
                continue;
            };
            for &s in &layout.shapes {
                if let Some(shape) = self.shape(s) {
                    render_shape(cr, shape);
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
                cr.new_path();
                append_path(&cr, &shape.path);
                cr.set_line_width(shape.stroke_width);
                let inside = cr.in_fill(x, y).unwrap_or(false)
                    || (shape.stroke.is_some() && cr.in_stroke(x, y).unwrap_or(false));
                if inside {
                    return s;
                }
            }
        }
        0
    }
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

fn render_shape(cr: &cairo::Context, shape: &Shape) {
    cr.new_path();
    append_path(cr, &shape.path);
    let (x0, y0, x1, _y1) = cr.path_extents().unwrap_or((0.0, 0.0, 0.0, 0.0));
    if let Some(fill) = shape.fill {
        set_color(cr, fill);
        let _ = cr.fill_preserve();
    }
    if let Some(stroke) = shape.stroke {
        set_color(cr, stroke);
        cr.set_line_width(shape.stroke_width);
        let _ = cr.stroke_preserve();
    }
    cr.new_path();

    if let Some(text) = &shape.text {
        let layout = pangocairo::functions::create_layout(cr);
        layout.set_font_description(Some(&pango::FontDescription::from_string(&shape.font)));
        let width = (x1 - x0 - 2.0 * TEXT_PADDING).max(1.0);
        layout.set_width((width * pango::SCALE as f64) as i32);
        layout.set_wrap(pango::WrapMode::WordChar);
        layout.set_text(text);
        set_color(cr, shape.text_color);
        cr.move_to(x0 + TEXT_PADDING, y0 + TEXT_PADDING);
        pangocairo::functions::show_layout(cr, &layout);
        cr.new_path();
    }
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
        let (w, h) = (200u32, 100u32);
        let mut buf = vec![0u8; (w * 4 * h) as usize];
        unsafe { render_frame(&s, buf.as_mut_ptr(), w, h, w * 4, 1.0, |_| {}) };
        // pixel (100, 55) is inside the red rect, below the text: BGRA
        let i = ((55 * w + 100) * 4) as usize;
        assert_eq!(&buf[i..i + 4], &[0, 0, 255, 255]);
        // outside is the white background
        let i = ((90 * w + 190) * 4) as usize;
        assert_eq!(&buf[i..i + 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn bad_ids_are_ignored() {
        let mut s = Scene::new();
        s.shape_mut(99).text = Some("x".into());
        s.layout_append(5, 7);
        s.add_layout(3);
        assert_eq!(s.hit_test(0.0, 0.0), 0);
    }
}
