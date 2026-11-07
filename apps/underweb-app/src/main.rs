//! The Underweb browser: tabs, an address bar, and one sandboxed runner
//! process per open package.
//!
//! ```text
//! underweb [--no-sandbox] [ADDRESS...]
//! ```
//!
//! An address is `uw://<domain>/<name>[@version]` or a package directory
//! (or `file://` URL), optionally with `?key=value&...` parameters for the
//! page. Names of the bundled examples (`hello-js`, `hello-ts`,
//! `hello-rust`, `hello-c`) work too.
//!
//! `UNDERWEB_SNAPSHOT=out.png` saves a screenshot of the window once the
//! visible tab has drawn (after `UNDERWEB_SNAPSHOT_DELAY` seconds, default 2)
//! and quits.

mod tab;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use tab::Tab;
use underweb_core::Store;

struct App {
    store: Arc<Store>,
    tabs: Vec<Tab>,
    active: usize,
    address: String,
    sandbox: bool,
    show_log: bool,
    snapshot: Option<Snapshot>,
}

struct Snapshot {
    path: PathBuf,
    delay: Duration,
    ready_since: Option<Instant>,
    requested: bool,
}

impl App {
    fn new(addresses: Vec<String>, sandbox: bool) -> App {
        let snapshot = std::env::var_os("UNDERWEB_SNAPSHOT").map(|p| Snapshot {
            path: p.into(),
            delay: Duration::from_secs_f64(
                std::env::var("UNDERWEB_SNAPSHOT_DELAY")
                    .ok()
                    .and_then(|d| d.parse().ok())
                    .unwrap_or(2.0),
            ),
            ready_since: None,
            requested: false,
        });
        let addresses = if addresses.is_empty() {
            vec!["hello-js".to_string()]
        } else {
            addresses
        };
        let mut store = Store::new(Store::default_root());
        store.sandbox = sandbox;
        App {
            store: Arc::new(store),
            address: addresses[0].clone(),
            tabs: addresses.into_iter().map(Tab::new).collect(),
            active: 0,
            sandbox,
            show_log: false,
            snapshot,
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let mut close = None;
            for (i, tab) in self.tabs.iter().enumerate() {
                let label = format!("{} {}", tab.status.icon(), tab.label())
                    .trim()
                    .to_string();
                if ui.selectable_label(i == self.active, label).clicked() {
                    self.active = i;
                    self.address = tab.address.clone();
                }
                if ui.small_button("×").on_hover_text("Close tab").clicked() {
                    close = Some(i);
                }
                ui.separator();
            }
            if ui.button("+").on_hover_text("New tab").clicked() {
                self.tabs.push(Tab::new(String::new()));
                self.active = self.tabs.len() - 1;
                self.address.clear();
            }
            if let Some(i) = close {
                self.tabs.remove(i);
                if self.tabs.is_empty() {
                    self.tabs.push(Tab::new(String::new()));
                }
                self.active = self.active.min(self.tabs.len() - 1);
                self.address = self.tabs[self.active].address.clone();
            }
        });
    }

    fn address_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("⟳").on_hover_text("Reload").clicked() {
                self.tabs[self.active].reload();
            }
            let status = &self.tabs[self.active].status;
            let (text, color) = status.badge();
            ui.label(egui::RichText::new(text).color(color).strong())
                .on_hover_text(status.describe());
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.address)
                    .hint_text("uw://example.org/package, or a package directory")
                    .desired_width(ui.available_width() - 60.0),
            );
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.tabs[self.active].navigate(self.address.trim().to_string());
            }
            ui.toggle_value(&mut self.show_log, "Log");
        });
    }

    fn take_snapshot(&mut self, ctx: &egui::Context) {
        let Some(snap) = self.snapshot.as_mut() else {
            return;
        };
        let tab = &self.tabs[self.active];
        if snap.ready_since.is_none() && (tab.has_frame() || tab.status.is_final()) {
            snap.ready_since = Some(Instant::now());
        }
        if let Some(t) = snap.ready_since {
            if !snap.requested && t.elapsed() >= snap.delay {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                snap.requested = true;
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            match write_png(&snap.path, &image) {
                Ok(()) => println!("wrote snapshot to {}", snap.path.display()),
                Err(e) => eprintln!("snapshot failed: {e:#}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for tab in &mut self.tabs {
            tab.sandbox = self.sandbox;
            tab.poll(ctx, &self.store);
        }
        egui::TopBottomPanel::top("chrome").show(ctx, |ui| {
            self.tab_bar(ui);
            self.address_bar(ui);
        });
        if self.show_log {
            egui::TopBottomPanel::bottom("log")
                .resizable(true)
                .default_height(140.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in &self.tabs[self.active].log {
                                ui.monospace(line);
                            }
                        });
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::WHITE))
            .show(ctx, |ui| {
                self.tabs[self.active].show(ui);
            });
        // background tabs keep running but don't animate
        let title = format!("{} — Underweb", self.tabs[self.active].label());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        self.take_snapshot(ctx);
    }
}

fn write_png(path: &std::path::Path, image: &egui::ColorImage) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, image.size[0] as u32, image.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|c| c.to_srgba_unmultiplied())
        .collect();
    enc.write_header()?.write_image_data(&data)?;
    Ok(())
}

fn main() -> eframe::Result<()> {
    let mut sandbox = std::env::var_os("UNDERWEB_NO_SANDBOX").is_none();
    let mut addresses = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--no-sandbox" => sandbox = false,
            "-h" | "--help" => {
                println!("usage: underweb [--no-sandbox] [ADDRESS...]");
                return Ok(());
            }
            _ => addresses.push(a),
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 680.0])
            .with_title("Underweb"),
        ..Default::default()
    };
    eframe::run_native(
        "Underweb",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(addresses, sandbox)))),
    )
}
