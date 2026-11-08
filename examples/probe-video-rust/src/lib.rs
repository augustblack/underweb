//! Video from a native package: ?src= shown in a shape.

use underweb::video::{self, Kind, MediaEvent};
use underweb::{Layout, Page, Shape, Window};

#[derive(Default)]
struct Probe;

impl Page for Probe {
    fn activate(&mut self, w: &Window) {
        w.set_title("video probe (rust)");
        let layout = Layout::new();
        w.add_layout(&layout);
        let v = video::open(&w.param("src").unwrap_or("test:smpte".into()), true);
        let box_ = Shape::new();
        box_.rectangle(0.0, 0.0, 640.0, 360.0)
            .set_fill_color(1.0, 0.0, 1.0, 1.0)
            .set_video(Some(v));
        layout.append(&box_);
        v.play();
    }

    fn media(&mut self, w: &Window, ev: &MediaEvent) {
        match ev.kind {
            Kind::Ready => {
                let info = ev.video.info();
                w.log(&format!(
                    "PROBE ready {}x{} info={}",
                    ev.width,
                    ev.height,
                    info.is_some()
                ));
            }
            Kind::Error => w.log(&format!("PROBE error {}", ev.text)),
            _ => {}
        }
    }
}

underweb::export_page!(Probe);
