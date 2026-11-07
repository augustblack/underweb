//! A Rust package: one shape through the curated API, plus a spinning
//! star drawn with raw cairo in `draw`.

use std::f64::consts::PI;

use underweb::{Layout, Page, PointerEvent, PointerKind, Shape, Window, cairo};

#[derive(Default)]
struct Hello {
    shape: Option<Shape>,
    angle: f64,
    clicks: u32,
}

impl Page for Hello {
    fn activate(&mut self, window: &Window) {
        window.set_title("Rust shape");
        let layout = Layout::new();
        window.add_layout(&layout);

        let s = Shape::new();
        s.rectangle(100.0, 100.0, 300.0, 200.0)
            .set_fill_color(0.2, 0.6, 1.0, 1.0)
            .set_stroke_color(0.0, 0.0, 0.0, 1.0)
            .set_stroke_width(2.0)
            .set_font("Serif 15")
            .set_text(
                "Hello from Rust. This page is a shared library running in a \
                 sandboxed process. The star is raw cairo.",
            );
        layout.append(&s);
        self.shape = Some(s);
        window.set_animating(true);
    }

    fn update(&mut self, window: &Window, dt: f64) {
        self.angle += dt;
        window.redraw();
    }

    fn pointer(&mut self, window: &Window, ev: &PointerEvent) {
        if ev.kind == PointerKind::Press && ev.shape.is_some() && ev.shape == self.shape {
            self.clicks += 1;
            let t = self.clicks as f64;
            self.shape.unwrap().set_fill_color(
                (t * 0.3).sin().abs(),
                0.6,
                (t * 0.7).cos().abs(),
                1.0,
            );
            window.log(&format!("clicked {} times", self.clicks));
        }
    }

    fn draw(&mut self, _window: &Window, cr: &cairo::Context, _w: f64, _h: f64) {
        cr.translate(560.0, 200.0);
        cr.rotate(self.angle);
        for i in 0..10 {
            let r = if i % 2 == 0 { 90.0 } else { 38.0 };
            let a = i as f64 * PI / 5.0;
            cr.line_to(r * a.cos(), r * a.sin());
        }
        cr.close_path();
        cr.set_source_rgb(1.0, 0.8, 0.1);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(0.4, 0.3, 0.0);
        cr.set_line_width(3.0);
        let _ = cr.stroke();
    }
}

underweb::export_page!(Hello);
