//! One browser tab: an address, a runner process and its latest frame.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use eframe::egui;
use underweb_core::{
    Guest, GuestEvent, SpawnOptions, UW_POINTER_MOVE, UW_POINTER_PRESS, UW_POINTER_RELEASE,
};

pub enum Status {
    Empty,
    Starting,
    Running { sandboxed: bool },
    Failed(String),
    Exited(String),
}

impl Status {
    pub fn icon(&self) -> &'static str {
        match self {
            Status::Empty | Status::Running { .. } => "",
            Status::Starting => "…",
            Status::Failed(_) | Status::Exited(_) => "⚠",
        }
    }

    pub fn badge(&self) -> (&'static str, egui::Color32) {
        match self {
            Status::Running { sandboxed: true } => {
                ("sandboxed", egui::Color32::from_rgb(30, 140, 60))
            }
            Status::Running { sandboxed: false } => ("UNSANDBOXED", egui::Color32::RED),
            Status::Starting => ("starting", egui::Color32::GRAY),
            Status::Empty => ("", egui::Color32::GRAY),
            Status::Failed(_) | Status::Exited(_) => ("stopped", egui::Color32::RED),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Status::Running { sandboxed: true } => {
                "The package runs in its own process with no access to your files, the network, or other programs.".into()
            }
            Status::Running { sandboxed: false } => "The sandbox is off (--no-sandbox). Development only.".into(),
            Status::Failed(e) | Status::Exited(e) => e.clone(),
            Status::Starting => "Starting the package…".into(),
            Status::Empty => "Type a package address.".into(),
        }
    }

    pub fn is_final(&self) -> bool {
        matches!(self, Status::Failed(_) | Status::Exited(_) | Status::Empty)
    }
}

pub struct Tab {
    pub address: String,
    pub status: Status,
    pub log: Vec<String>,
    pub sandbox: bool,
    title: String,
    guest: Option<Guest>,
    texture: Option<egui::TextureHandle>,
    configured: Option<(u32, u32, f32)>,
    animating: bool,
    last_tick: Option<Instant>,
    started: bool,
}

impl Tab {
    pub fn new(address: String) -> Tab {
        Tab {
            address,
            status: Status::Empty,
            log: Vec::new(),
            sandbox: true,
            title: String::new(),
            guest: None,
            texture: None,
            configured: None,
            animating: false,
            last_tick: None,
            started: false,
        }
    }

    pub fn label(&self) -> String {
        if !self.title.is_empty() {
            self.title.clone()
        } else if self.address.is_empty() {
            "New tab".into()
        } else {
            self.address.clone()
        }
    }

    pub fn has_frame(&self) -> bool {
        self.texture.is_some()
    }

    pub fn navigate(&mut self, address: String) {
        self.address = address;
        self.reload();
    }

    pub fn reload(&mut self) {
        if let Some(g) = self.guest.take() {
            g.shutdown();
        }
        self.title.clear();
        self.texture = None;
        self.configured = None;
        self.animating = false;
        self.last_tick = None;
        self.started = false;
        self.status = Status::Empty;
    }

    fn start(&mut self, ctx: &egui::Context) {
        self.started = true;
        if self.address.is_empty() {
            return;
        }
        let (dir, params) = match resolve(&self.address) {
            Ok(r) => r,
            Err(e) => {
                self.status = Status::Failed(e);
                return;
            }
        };
        let wake_ctx = ctx.clone();
        let opts = SpawnOptions {
            sandbox: self.sandbox,
            params,
            wake: Some(Arc::new(move || wake_ctx.request_repaint())),
            ..Default::default()
        };
        match Guest::spawn(&dir, opts) {
            Ok(g) => {
                self.log
                    .push(format!("── {} ({})", g.package.name(), dir.display()));
                self.guest = Some(g);
                self.status = Status::Starting;
            }
            Err(e) => self.status = Status::Failed(format!("{e:#}")),
        }
    }

    /// Drain runner events.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.start(ctx);
        }
        let Some(guest) = self.guest.as_mut() else {
            return;
        };
        while let Some(ev) = guest.try_event() {
            match ev {
                GuestEvent::Started { sandboxed } => self.status = Status::Running { sandboxed },
                GuestEvent::Title(t) => self.title = t,
                GuestEvent::Animating(on) => self.animating = on,
                GuestEvent::Log(l) => self.log.push(l),
                GuestEvent::Error(e) => {
                    self.log.push(format!("error: {e}"));
                    self.status = Status::Failed(e);
                }
                GuestEvent::Frame(f) => {
                    let image = egui::ColorImage::from_rgba_premultiplied(
                        [f.width as usize, f.height as usize],
                        &f.rgba,
                    );
                    match &mut self.texture {
                        Some(t) => t.set(image, egui::TextureOptions::LINEAR),
                        None => {
                            self.texture =
                                Some(ctx.load_texture("page", image, egui::TextureOptions::LINEAR))
                        }
                    }
                }
                GuestEvent::Exited(_) => {
                    // the process may take a moment to be reapable
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    let why = match guest.exit_status() {
                        Some(s) if s.success() => "The package exited.".to_string(),
                        Some(s) => format!("The package crashed ({s})."),
                        None => "The package stopped.".to_string(),
                    };
                    self.log.push(why.clone());
                    if !matches!(self.status, Status::Failed(_)) {
                        self.status = Status::Exited(why);
                    }
                    self.animating = false;
                }
            }
        }
    }

    /// Draw the page into the central panel and forward input.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let (rect, _response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

        if let Status::Failed(e) | Status::Exited(e) = &self.status {
            let msg = if self.texture.is_some() {
                e.clone()
            } else {
                format!("Could not show this page.\n\n{e}")
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(16.0),
                egui::Color32::DARK_RED,
            );
            if self.texture.is_none() {
                return;
            }
        }

        let Some(guest) = self.guest.as_mut() else {
            return;
        };
        let ppp = ui.ctx().pixels_per_point();
        let size = (
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
            ppp,
        );
        if self.configured != Some(size)
            && size.0 > 0
            && size.1 > 0
            && guest.configure(size.0, size.1, ppp as f64).is_ok()
        {
            self.configured = Some(size);
        }

        if let Some(tex) = &self.texture {
            // a frame from before a resize is stretched until the new one arrives
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
        }

        // input, in page coordinates
        let events = ui.input(|i| i.events.clone());
        for e in events {
            match e {
                egui::Event::PointerMoved(p) if rect.contains(p) => {
                    let p = p - rect.min;
                    let _ = guest.pointer(UW_POINTER_MOVE, 0, p.x as f64, p.y as f64);
                }
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } if rect.contains(pos) || !pressed => {
                    let p = pos - rect.min;
                    let kind = if pressed {
                        UW_POINTER_PRESS
                    } else {
                        UW_POINTER_RELEASE
                    };
                    let button = match button {
                        egui::PointerButton::Primary => 1,
                        egui::PointerButton::Secondary => 3,
                        egui::PointerButton::Middle => 2,
                        _ => 0,
                    };
                    let _ = guest.pointer(kind, button, p.x as f64, p.y as f64);
                }
                _ => {}
            }
        }

        // drive animation from the display's frame rate
        if self.animating {
            let now = Instant::now();
            let dt = self
                .last_tick
                .map_or(1.0 / 60.0, |t| (now - t).as_secs_f64().min(0.1));
            self.last_tick = Some(now);
            let _ = guest.tick(dt);
            ui.ctx().request_repaint();
        } else {
            self.last_tick = None;
        }
    }
}

/// Address → package directory + parameters.
fn resolve(address: &str) -> Result<(PathBuf, Vec<(String, String)>), String> {
    let address = address.strip_prefix("file://").unwrap_or(address);
    if address.starts_with("uw://") || address.starts_with("http") {
        return Err(format!(
            "{address}: remote packages and web pages come in a later version"
        ));
    }
    let (path, query) = address.split_once('?').unwrap_or((address, ""));
    let params = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.to_string(), v.to_string())
        })
        .collect();
    let candidates = [
        PathBuf::from(path),
        PathBuf::from("examples").join(path),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(path),
    ];
    candidates
        .into_iter()
        .find(|p| p.join("underweb.toml").exists())
        .map(|p| (p, params))
        .ok_or_else(|| format!("No package at {path:?} (looking for underweb.toml)"))
}
