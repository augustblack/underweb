//! One browser tab: an address, a runner process and its latest frame.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use crate::editor::Editor;
use eframe::egui;
use underweb_core::{
    Decision, GrantState, Guest, GuestEvent, Origin, Resolved, SpawnOptions, Store, Trust,
    UW_POINTER_MOVE, UW_POINTER_PRESS, UW_POINTER_RELEASE, Verdict,
};

/// Messages from the thread that resolves (downloads, builds) a package.
enum Loading {
    Progress(String),
    Done(Box<Result<(Resolved, Option<Verdict>), String>>),
}

pub enum Status {
    Empty,
    /// Downloading or building; the latest progress line.
    Loading(String),
    Starting,
    Running {
        sandboxed: bool,
    },
    Failed(String),
    Exited(String),
}

impl Status {
    pub fn icon(&self) -> &'static str {
        match self {
            Status::Empty | Status::Running { .. } => "",
            Status::Starting | Status::Loading(_) => "…",
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
            Status::Loading(_) => ("loading", egui::Color32::GRAY),
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
            Status::Loading(l) => l.clone(),
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
    pub allow_declared: bool,
    title: String,
    guest: Option<Guest>,
    texture: Option<egui::TextureHandle>,
    configured: Option<(u32, u32, f32)>,
    animating: bool,
    last_tick: Option<Instant>,
    started: bool,
    loading: Option<Receiver<Loading>>,
    /// Permission requests waiting for the user: (id, description).
    prompts: Vec<(u64, String)>,
    /// Who published and vouches for the package (remote packages).
    pub verdict: Option<Verdict>,
    /// The layout editor, while open.
    editor: Option<Editor>,
    /// Open the editor as soon as the page runs (`--edit`).
    pub edit_on_start: bool,
}

impl Tab {
    pub fn new(address: String) -> Tab {
        Tab {
            address,
            status: Status::Empty,
            log: Vec::new(),
            sandbox: true,
            allow_declared: false,
            title: String::new(),
            guest: None,
            texture: None,
            configured: None,
            animating: false,
            last_tick: None,
            started: false,
            loading: None,
            prompts: Vec::new(),
            verdict: None,
            editor: None,
            edit_on_start: false,
        }
    }

    pub fn editing(&self) -> bool {
        self.editor.is_some()
    }

    /// Open or close the layout editor.
    pub fn set_editing(&mut self, on: bool) {
        let Some(g) = &self.guest else { return };
        if on == self.editor.is_some() || g.set_edit_mode(on).is_err() {
            return;
        }
        self.editor = on.then(|| Editor::new(g.package.name()));
    }

    /// The editor's side panel, if it's open.
    pub fn editor_panel(&mut self, ctx: &egui::Context) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let package = self.guest.as_ref().map(|g| g.package.dir.clone());
        let mut open = true;
        egui::SidePanel::right("editor")
            .default_width(300.0)
            .show(ctx, |ui| open = editor.panel(ui, package.as_deref()));
        if let Some(g) = &self.guest {
            editor.flush(g);
        }
        if !open {
            self.set_editing(false);
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
        self.loading = None;
        self.prompts.clear();
        self.verdict = None;
        self.editor = None;
        self.status = Status::Empty;
    }

    /// Resolve the address on a background thread: remote packages may
    /// need downloading and building.
    fn start(&mut self, ctx: &egui::Context, store: &Arc<Store>, trust: &Arc<Trust>) {
        self.started = true;
        if self.address.is_empty() {
            return;
        }
        let address = expand_example(&self.address);
        let (tx, rx) = mpsc::channel();
        let (store, trust, ctx) = (store.clone(), trust.clone(), ctx.clone());
        std::thread::spawn(move || {
            let progress = |l: &str| {
                let _ = tx.send(Loading::Progress(l.to_string()));
                ctx.request_repaint();
            };
            let result = store
                .open(&address, &progress)
                .map(|r| {
                    let verdict = r.subject.as_ref().map(|s| trust.check(s, &progress));
                    (r, verdict)
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Loading::Done(Box::new(result)));
            ctx.request_repaint();
        });
        self.loading = Some(rx);
        self.status = Status::Loading("Opening…".into());
    }

    fn spawn(&mut self, ctx: &egui::Context, resolved: Resolved) {
        let wake_ctx = ctx.clone();
        // grants belong to where the package came from, not just its name
        let identity = match &resolved.origin {
            Origin::Remote { index_url, .. } => Some(index_url.clone()),
            Origin::Local => None,
        };
        let opts = SpawnOptions {
            sandbox: self.sandbox,
            params: resolved.params,
            identity,
            // trust can grant what the package declared; it never turns
            // off the sandbox
            permissions: if self.allow_declared
                || self.verdict.as_ref().is_some_and(|v| v.auto_grant)
            {
                underweb_core::PermissionPolicy::AllowDeclared
            } else {
                underweb_core::PermissionPolicy::Ask
            },
            wake: Some(Arc::new(move || wake_ctx.request_repaint())),
            ..Default::default()
        };
        match Guest::spawn(&resolved.dir, opts) {
            Ok(g) => {
                let from = match &resolved.origin {
                    Origin::Local => resolved.dir.display().to_string(),
                    Origin::Remote {
                        index_url,
                        version,
                        built_from_source,
                    } => format!(
                        "{version} from {index_url}{}",
                        if *built_from_source {
                            ", built from source"
                        } else {
                            ""
                        }
                    ),
                };
                self.log.push(format!("── {} ({from})", g.package.name()));
                self.guest = Some(g);
                self.status = Status::Starting;
            }
            Err(e) => self.status = Status::Failed(format!("{e:#}")),
        }
    }

    /// Drain runner events.
    pub fn poll(&mut self, ctx: &egui::Context, store: &Arc<Store>, trust: &Arc<Trust>) {
        if !self.started {
            self.start(ctx, store, trust);
        }
        if let Some(rx) = &self.loading {
            let mut done = None;
            while let Ok(m) = rx.try_recv() {
                match m {
                    Loading::Progress(l) => {
                        self.log.push(l.clone());
                        self.status = Status::Loading(l);
                    }
                    Loading::Done(r) => done = Some(*r),
                }
            }
            match done {
                Some(Ok((resolved, verdict))) => {
                    self.loading = None;
                    if let Some(v) = &verdict {
                        self.log.push(v.summary());
                        self.log.extend(
                            v.rejected
                                .iter()
                                .map(|r| format!("ignored a statement: {r}")),
                        );
                    }
                    self.verdict = verdict;
                    self.spawn(ctx, resolved);
                }
                Some(Err(e)) => {
                    self.loading = None;
                    self.log.push(format!("error: {e}"));
                    self.status = Status::Failed(e);
                }
                None => {}
            }
        }
        let Some(guest) = self.guest.as_mut() else {
            return;
        };
        // a busy page (video) never runs out of events: handle what's
        // there, at most one frame, and leave the rest for the next repaint
        let mut handled = 0;
        let mut got_frame = false;
        while handled < 256 && !got_frame {
            let Some(ev) = guest.try_event() else { break };
            handled += 1;
            got_frame = matches!(ev, GuestEvent::Frame(_));
            match ev {
                GuestEvent::Started { sandboxed } => {
                    self.status = Status::Running { sandboxed };
                    if std::mem::take(&mut self.edit_on_start) {
                        if let Err(e) = guest.set_edit_mode(true) {
                            self.log.push(format!("editor: {e:#}"));
                        } else {
                            let mut ed = Editor::new(guest.package.name());
                            ed.auto_select = true;
                            self.editor = Some(ed);
                        }
                    }
                }
                GuestEvent::Scene(doc) => {
                    if let Some(e) = self.editor.as_mut() {
                        e.receive(doc);
                    }
                }
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
                GuestEvent::PermissionRequest {
                    id, description, ..
                } => {
                    self.log.push(format!("asks to {description}"));
                    self.prompts.push((id, description));
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
        if got_frame || handled == 256 {
            ctx.request_repaint(); // more may be waiting
        }
    }

    /// The oldest pending permission request, as a bar above the page.
    pub fn prompt_bar(&mut self, ctx: &egui::Context) {
        let Some((id, description)) = self.prompts.first().cloned() else {
            return;
        };
        let name = self
            .guest
            .as_ref()
            .map_or("This page".to_string(), |g| g.package.name().to_string());
        egui::TopBottomPanel::top("permission").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{name} wants to {description}.")).strong());
                let mut answer = None;
                if ui.button("Allow once").clicked() {
                    answer = Some(Decision::AllowOnce);
                }
                if ui.button("Always allow").clicked() {
                    answer = Some(Decision::AllowAlways);
                }
                if ui.button("Deny").clicked() {
                    answer = Some(Decision::Deny);
                }
                if let Some(d) = answer {
                    if let Some(g) = &self.guest {
                        g.answer(id, d);
                    }
                    self.log.push(format!("{d:?}: {description}"));
                    self.prompts.remove(0);
                }
                if self.prompts.len() > 1 {
                    ui.weak(format!("({} more)", self.prompts.len() - 1));
                }
            });
        });
    }

    pub fn has_sound(&self) -> bool {
        self.guest.as_ref().is_some_and(|g| g.has_sound())
    }

    /// A speaker that mutes the tab, shown while it makes sound.
    pub fn sound_button(&mut self, ui: &mut egui::Ui) {
        let Some(g) = &self.guest else { return };
        if !g.has_sound() {
            return;
        }
        let muted = g.is_muted();
        let (icon, tip) = if muted {
            ("🔇", "Unmute this tab")
        } else {
            ("🔊", "Mute this tab")
        };
        if ui.button(icon).on_hover_text(tip).clicked() {
            g.set_muted(!muted);
        }
    }

    /// One line about who published and vouches for this package.
    pub fn trust_label(&self, ui: &mut egui::Ui) {
        let Some(v) = &self.verdict else { return };
        let bad = !v.warnings.is_empty() || v.publisher_problem.is_some();
        let color = if bad {
            egui::Color32::RED
        } else if !v.vouches.is_empty() {
            egui::Color32::from_rgb(30, 140, 60)
        } else {
            egui::Color32::GRAY
        };
        ui.label(egui::RichText::new(v.summary()).color(color)).on_hover_text(if v.auto_grant {
            "People you follow vouch for this exact version, so its declared permissions were granted without asking."
        } else {
            "Its declared permissions will be asked for."
        });
    }

    /// What the package declared and what was decided.
    pub fn permissions_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(v) = &self.verdict {
            ui.strong("Trust");
            ui.label(v.summary());
            for (s, level) in &v.vouches {
                ui.label(format!("  ✔ {} ({}) — {level:?}", s.name, s.network));
            }
            for (s, note) in &v.warnings {
                ui.colored_label(
                    egui::Color32::RED,
                    format!("  ⚠ {} ({}): {note}", s.name, s.network),
                );
            }
            for r in &v.rejected {
                ui.weak(format!("  ignored: {r}"));
            }
            ui.separator();
        }
        let Some(g) = &self.guest else {
            ui.weak("No package open.");
            return;
        };
        let perms = g.permissions();
        if perms.is_empty() {
            ui.label(format!(
                "{} asks for no permissions: no network access.",
                g.package.name()
            ));
            return;
        }
        ui.label(format!("{} may ask to:", g.package.name()));
        egui::Grid::new("perms").striped(true).show(ui, |ui| {
            for (p, state) in perms {
                ui.label(p.describe());
                ui.label(match state {
                    GrantState::Undecided => "not asked yet",
                    GrantState::AllowedOnce => "allowed until closed",
                    GrantState::AllowedAlways => "always allowed",
                    GrantState::Denied => "denied",
                });
                if state != GrantState::Undecided && ui.small_button("Forget").clicked() {
                    g.forget(&p);
                }
                ui.end_row();
            }
        });
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
        if self.configured != Some(size) && size.0 > 0 && size.1 > 0 {
            match guest.resize(size.0, size.1, ppp as f64) {
                Ok(true) => self.configured = Some(size),
                // the page is still drawing the last size: ask again soon
                Ok(false) => ui
                    .ctx()
                    .request_repaint_after(std::time::Duration::from_millis(16)),
                Err(_) => {}
            }
        }

        if let Some(tex) = &self.texture {
            // At its own size from the top-left, never stretched: while the
            // window is dragged, shapes stay put and new space stays blank
            // until the page has drawn at the new size.
            ui.painter().rect_filled(rect, 0.0, egui::Color32::WHITE);
            let [tw, th] = tex.size();
            let frame = egui::vec2(tw as f32, th as f32) / ppp;
            let shown = frame.min(rect.size());
            let uv = egui::Rect::from_min_max(
                egui::pos2(0.0, 0.0),
                egui::pos2(shown.x / frame.x, shown.y / frame.y),
            );
            let at = egui::Rect::from_min_size(rect.min, shown);
            ui.painter().image(tex.id(), at, uv, egui::Color32::WHITE);
        }

        // while editing, the editor has the pointer
        if let Some(editor) = self.editor.as_mut() {
            editor.canvas(ui, rect);
            editor.flush(guest);
        }
        // input, in page coordinates
        let events = if self.editor.is_some() {
            Vec::new()
        } else {
            ui.input(|i| i.events.clone())
        };
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

/// Bare names of the bundled examples (`hello-ts`) open them, so the
/// browser is easy to try from a checkout.
fn expand_example(address: &str) -> String {
    if address.contains("://") {
        return address.to_string();
    }
    let (path, query) = address
        .split_once('?')
        .map_or((address, None), |(p, q)| (p, Some(q)));
    let candidates = [
        PathBuf::from(path),
        PathBuf::from("examples").join(path),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(path),
    ];
    match candidates
        .into_iter()
        .find(|p| p.join("underweb.toml").exists())
    {
        Some(p) => match query {
            Some(q) => format!("{}?{q}", p.display()),
            None => p.display().to_string(),
        },
        None => address.to_string(),
    }
}
