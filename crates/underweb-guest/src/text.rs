//! Text that flows inside a shape's outline, word by word (Mentiras'
//! `TextFillType.FILL_WORD`).
//!
//! The path is rasterised once into a mask; each line of text goes into
//! the horizontal runs where the whole line height is inside the shape.

use crate::{PathOp, append_path};

/// Gap kept between words and the outline.
const MARGIN: f64 = 3.0;

pub(crate) fn flow(
    cr: &cairo::Context,
    path: &[PathOp],
    (x0, y0, x1, y1): (f64, f64, f64, f64),
    text: &str,
    markup: bool,
    font: &pango::FontDescription,
) {
    let (w, h) = ((x1 - x0).ceil() as i32, (y1 - y0).ceil() as i32);
    if w <= 0 || h <= 0 || w > 8192 || h > 8192 {
        return;
    }
    let Some(mask) = rasterise(path, x0, y0, w, h) else {
        return;
    };
    let plain;
    let text = if markup {
        // flowing text is placed word by word: markup styles are dropped
        plain = pango::parse_markup(text, '\0')
            .map(|(_, t, _)| t.to_string())
            .unwrap_or_else(|_| text.to_string());
        &plain
    } else {
        text
    };

    let layout = pangocairo::functions::create_layout(cr);
    layout.set_font_description(Some(font));
    let measure = |s: &str| {
        layout.set_text(s);
        let (_, logical) = layout.pixel_extents();
        (logical.width() as f64, logical.height() as f64)
    };
    let (_, line) = measure("Ag");
    let space = measure("a a").0 - measure("aa").0;
    if line <= 0.0 {
        return;
    }

    let mut y = MARGIN;
    for para in text.split('\n') {
        let mut words = para.split_whitespace().peekable();
        if words.peek().is_none() {
            y += line;
            continue;
        }
        while words.peek().is_some() {
            if y + line > h as f64 {
                return;
            }
            for (start, end) in mask.spans(y, line) {
                let mut x = start;
                while let Some(word) = words.peek() {
                    let (ww, _) = measure(word);
                    if x + ww > end {
                        break;
                    }
                    // `measure` left the word in the layout
                    cr.move_to(x0 + x, y0 + y);
                    pangocairo::functions::show_layout(cr, &layout);
                    x += ww + space;
                    words.next();
                }
            }
            y += line;
        }
    }
}

struct Mask {
    data: Vec<u8>,
    stride: usize,
    w: i32,
    h: i32,
}

fn rasterise(path: &[PathOp], x0: f64, y0: f64, w: i32, h: i32) -> Option<Mask> {
    let mut surface = cairo::ImageSurface::create(cairo::Format::A8, w, h).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        cr.translate(-x0, -y0);
        append_path(&cr, path);
        cr.fill().ok()?;
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data().ok()?.to_vec();
    Some(Mask { data, stride, w, h })
}

impl Mask {
    fn inside(&self, x: i32, y: i32) -> bool {
        x >= 0
            && y >= 0
            && x < self.w
            && y < self.h
            && self.data[y as usize * self.stride + x as usize] > 127
    }

    /// Horizontal runs where a line from `y` to `y + height` fits, with a
    /// margin on both sides.
    fn spans(&self, y: f64, height: f64) -> Vec<(f64, f64)> {
        let rows = [
            y.ceil() as i32,
            (y + height / 2.0) as i32,
            (y + height).floor() as i32 - 1,
        ];
        let mut out = Vec::new();
        let mut start = None;
        for x in 0..=self.w {
            let ok = x < self.w && rows.iter().all(|&r| self.inside(x, r));
            match (ok, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    let (a, b) = (s as f64 + MARGIN, x as f64 - MARGIN);
                    if b > a {
                        out.push((a, b));
                    }
                    start = None;
                }
                _ => {}
            }
        }
        out
    }
}
