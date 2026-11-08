//! demos/08draw_cb.vala, ported. In libmentiras a shape could have a draw
//! callback; here a native page draws with raw cairo in `draw`, on top of
//! its shapes. The 60 strokes spin inside the red shape, in its own
//! coordinates (the shape's matrix, then its size as the unit square).

use std::f64::consts::PI;

use underweb::{Layout, Matrix, Page, Shape, Window, cairo};

const PLACE: Matrix = Matrix::new(1.0, 0.0, 0.0, 1.0, 200.0, 150.0);
const SIZE: (f64, f64) = (400.0, 300.0);

#[derive(Default)]
struct Spinner {
    ms: f64,
}

impl Page for Spinner {
    fn activate(&mut self, window: &Window) {
        window.set_title("Draw callbacks");
        let layout = Layout::new();
        layout.set_background(Some((0.0, 0.0, 1.0, 1.0)));
        window.add_layout(&layout);

        let s = Shape::new();
        s.set_name("abstract shape")
            .rectangle(0.0, 0.0, SIZE.0, SIZE.1)
            .set_matrix(PLACE)
            .set_fill_color(1.0, 0.0, 0.0, 1.0)
            .set_text("Packages can draw anything with cairo on top of their shapes.");
        layout.append(&s);
        window.set_animating(true);
    }

    fn update(&mut self, window: &Window, dt: f64) {
        // the old demo counted frames of a 13 ms timer
        self.ms += dt * 1000.0 / 13.0;
        window.redraw();
    }

    fn draw(&mut self, _window: &Window, cr: &cairo::Context, _w: f64, _h: f64) {
        let m = PLACE;
        cr.transform(cairo::Matrix::new(m.xx, m.yx, m.xy, m.yy, m.x0, m.y0));
        cr.scale(SIZE.0, SIZE.1);
        let len = 1.0 / 25.0;
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_width(len);
        for i in 0..60 {
            let i = i as f64;
            let _ = cr.save();
            cr.translate(0.5, 0.5);
            cr.rotate(PI / 180.0 * (self.ms + 10.0 * i) * 0.36);
            let y = 0.33 + 0.0825 * ((self.ms + 10.0 * i) / 1000.0 * 10.0 * PI).sin();
            cr.translate(0.0, y);
            cr.rotate(PI / 180.0 * 6.0 * i);
            cr.set_source_rgba(0.25, 0.5, 1.0, i * 0.01);
            cr.move_to(-len, 0.0);
            cr.line_to(len, 0.0);
            let _ = cr.stroke();
            let _ = cr.restore();
        }
    }
}

underweb::export_page!(Spinner);
