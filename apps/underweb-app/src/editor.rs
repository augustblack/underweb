//! The layout editor: select, drag and reshape a page's shapes, change
//! their colours and text, and export the result as a new package.
//!
//! The page keeps running in its sandboxed process. The editor works on a
//! copy of its scene ([`SceneDoc`]) and sends each change back as an
//! [`EditOp`]; the page redraws itself. While the editor is open the page
//! gets no pointer events.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use underweb_core::Guest;
use underweb_core::scene::{EditOp, Id, Matrix, PathOp, Rgba, SceneDoc, ShapeDoc};
use underweb_export::Lang;

/// How close (in points) the pointer must be to grab a handle.
const GRAB: f32 = 7.0;
const ACCENT: Color32 = Color32::from_rgb(20, 110, 230);

enum Drag {
    /// Moving the whole shape; the last pointer position.
    Shape { id: Id, last: Pos2 },
    /// Moving one point: path op `op`, handle `k` of it.
    Point { id: Id, op: usize, k: usize },
}

pub struct Editor {
    pub doc: SceneDoc,
    pub selected: Option<Id>,
    drag: Option<Drag>,
    /// A shape changed locally, to send at the end of the frame.
    changed: Option<Id>,
    /// Edits waiting to go to the page.
    outbox: Vec<EditOp>,
    lang: Lang,
    export_dir: String,
    image_source: String,
    message: String,
    /// Shapes added so far, to place new ones a little apart.
    added: u32,
    /// Select the top shape when the scene arrives (`--edit`).
    pub auto_select: bool,
}

impl Editor {
    pub fn new(package_name: &str) -> Editor {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or(PathBuf::from("."), PathBuf::from);
        Editor {
            doc: SceneDoc::default(),
            selected: None,
            drag: None,
            changed: None,
            outbox: Vec::new(),
            lang: Lang::Js,
            export_dir: home
                .join("Underweb")
                .join(format!("{package_name}-edited"))
                .display()
                .to_string(),
            image_source: String::new(),
            message: String::new(),
            added: 0,
            auto_select: false,
        }
    }

    /// The page's scene changed (or we hear it for the first time).
    pub fn receive(&mut self, mut doc: SceneDoc) {
        // while dragging, our copy of that shape is newer than the page's
        let dragged = match self.drag {
            Some(Drag::Shape { id, .. } | Drag::Point { id, .. }) => Some(id),
            None => None,
        };
        if let Some(id) = dragged
            && let (Some(mine), Some(theirs)) = (self.doc.shape(id).cloned(), doc.shape_mut(id))
        {
            *theirs = mine;
        }
        if self.selected.is_some_and(|id| doc.shape(id).is_none()) {
            self.selected = None;
        }
        if self.auto_select && self.selected.is_none() {
            self.selected = doc.shapes().last().map(|s| s.id);
            self.auto_select = self.selected.is_none();
        }
        self.doc = doc;
    }

    /// Send what changed this frame.
    pub fn flush(&mut self, guest: &Guest) {
        if let Some(id) = self.changed.take()
            && let Some(s) = self.doc.shape(id)
        {
            self.outbox.push(EditOp::SetShape(s.clone()));
        }
        for op in self.outbox.drain(..) {
            let _ = guest.edit(op);
        }
    }

    fn shape_mut(&mut self, id: Id) -> Option<&mut ShapeDoc> {
        self.changed = Some(id);
        self.doc.shape_mut(id)
    }

    // ---- the page area ----------------------------------------------------

    /// Outlines and handles over the page, and the pointer. `rect` is where
    /// the page is drawn; page coordinates are points from its corner.
    pub fn canvas(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let to_screen = |(x, y): (f64, f64)| rect.min + Vec2::new(x as f32, y as f32);
        let painter = ui.painter_at(rect);

        // every shape faintly, so invisible ones can be found
        for s in self.doc.shapes() {
            for poly in s.polygons() {
                let pts: Vec<Pos2> = poly.into_iter().map(to_screen).collect();
                painter.add(egui::Shape::closed_line(
                    pts,
                    Stroke::new(1.0, Color32::from_black_alpha(60)),
                ));
            }
        }
        if let Some(s) = self.selected.and_then(|id| self.doc.shape(id)) {
            for poly in s.polygons() {
                let pts: Vec<Pos2> = poly.into_iter().map(to_screen).collect();
                painter.add(egui::Shape::closed_line(pts, Stroke::new(2.0, ACCENT)));
            }
            for h in handles(s) {
                let p = to_screen(s.matrix.apply(h.pos.0, h.pos.1));
                if let Some(anchor) = h.anchor {
                    let a = to_screen(s.matrix.apply(anchor.0, anchor.1));
                    painter.line_segment([a, p], Stroke::new(1.0, ACCENT));
                    painter.circle_filled(p, 4.0, Color32::WHITE);
                    painter.circle_stroke(p, 4.0, Stroke::new(1.5, ACCENT));
                } else {
                    let r = Rect::from_center_size(p, Vec2::splat(8.0));
                    painter.rect_filled(r, 0.0, Color32::WHITE);
                    painter.rect_stroke(r, 0.0, Stroke::new(1.5, ACCENT), egui::StrokeKind::Middle);
                }
            }
        }

        let to_page = |p: Pos2| ((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64);
        let events = ui.input(|i| i.events.clone());
        for e in events {
            match e {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } if rect.contains(pos) => self.press(pos, to_page(pos), rect),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    ..
                } if rect.contains(pos) => self.right_click(pos, to_page(pos), rect),
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } => self.drag = None,
                egui::Event::PointerMoved(pos) => self.drag_to(pos, to_page(pos)),
                _ => {}
            }
        }
        if !ui.ctx().wants_keyboard_input()
            && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
            && let Some(id) = self.selected.take()
        {
            self.outbox.push(EditOp::Delete(id));
        }
    }

    /// The handle of the selected shape under the screen point.
    fn handle_at(&self, screen: Pos2, rect: Rect) -> Option<(Id, Handle)> {
        let id = self.selected?;
        let s = self.doc.shape(id)?;
        handles(s)
            .into_iter()
            .map(|h| {
                let (x, y) = s.matrix.apply(h.pos.0, h.pos.1);
                let d = (rect.min + Vec2::new(x as f32, y as f32)).distance(screen);
                (d, h)
            })
            .filter(|(d, _)| *d <= GRAB)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, h)| (id, h))
    }

    fn press(&mut self, screen: Pos2, page: (f64, f64), rect: Rect) {
        if let Some((id, h)) = self.handle_at(screen, rect) {
            self.drag = Some(Drag::Point {
                id,
                op: h.op,
                k: h.k,
            });
            return;
        }
        self.selected = self.doc.hit_test(page.0, page.1);
        self.drag = self.selected.map(|id| Drag::Shape { id, last: screen });
    }

    fn drag_to(&mut self, screen: Pos2, page: (f64, f64)) {
        match self.drag {
            Some(Drag::Shape { id, last }) => {
                let d = screen - last;
                if let Some(s) = self.shape_mut(id) {
                    s.matrix.x0 += d.x as f64;
                    s.matrix.y0 += d.y as f64;
                }
                self.drag = Some(Drag::Shape { id, last: screen });
            }
            Some(Drag::Point { id, op, k }) => {
                if let Some(s) = self.shape_mut(id)
                    && let Some(inv) = s.matrix.invert()
                {
                    let local = inv.apply(page.0, page.1);
                    set_handle(&mut s.path, op, k, local);
                }
            }
            None => {}
        }
    }

    /// On a point: remove it. Elsewhere: add a corner there, after the
    /// nearest point of the selected shape.
    fn right_click(&mut self, screen: Pos2, page: (f64, f64), rect: Rect) {
        if let Some((id, h)) = self.handle_at(screen, rect) {
            if let Some(s) = self.shape_mut(id)
                && matches!(
                    s.path.get(h.op),
                    Some(PathOp::LineTo(..) | PathOp::CurveTo(..))
                )
            {
                s.path.remove(h.op);
            }
            return;
        }
        let Some(id) = self.selected else { return };
        if let Some(s) = self.shape_mut(id)
            && let Some(inv) = s.matrix.invert()
        {
            let (x, y) = inv.apply(page.0, page.1);
            let after = handles(s)
                .into_iter()
                .filter(|h| h.anchor.is_none() && !matches!(s.path[h.op], PathOp::Rectangle(..)))
                .min_by(|a, b| {
                    let d = |h: &Handle| (h.pos.0 - x).powi(2) + (h.pos.1 - y).powi(2);
                    d(a).total_cmp(&d(b))
                });
            if let Some(h) = after {
                s.path.insert(h.op + 1, PathOp::LineTo(x, y));
            }
        }
    }

    // ---- the side panel ---------------------------------------------------

    /// Returns false when the user closes the editor.
    pub fn panel(&mut self, ui: &mut egui::Ui, package: Option<&Path>) -> bool {
        let mut open = true;
        ui.horizontal(|ui| {
            ui.heading("Layout editor");
            if ui.button("Done").clicked() {
                open = false;
            }
        });
        ui.weak("Click a shape to select it, drag it or its points. Right-click adds or removes a point. The page doesn't get clicks while you edit.");
        ui.separator();

        ui.horizontal_wrapped(|ui| {
            ui.label("Add:");
            for (label, path) in [
                ("Rectangle", rectangle()),
                ("Triangle", triangle()),
                ("Circle", circle()),
            ] {
                if ui.button(label).clicked() {
                    self.add(ShapeDoc {
                        path,
                        fill: Some(Rgba(0.55, 0.75, 1.0, 1.0)),
                        ..Default::default()
                    });
                }
            }
            if ui.button("Text").clicked() {
                self.add(ShapeDoc {
                    path: vec![PathOp::Rectangle(0.0, 0.0, 260.0, 90.0)],
                    stroke: None,
                    text: Some("Text".into()),
                    ..Default::default()
                });
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.image_source)
                    .hint_text("picture.png (a package file)")
                    .desired_width(170.0),
            );
            if ui.button("Add image").clicked() && !self.image_source.trim().is_empty() {
                self.add(ShapeDoc {
                    stroke: None,
                    image: Some(self.image_source.trim().to_string()),
                    ..Default::default()
                });
            }
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("shapes")
            .max_height(150.0)
            .show(ui, |ui| {
                if self.doc.layouts.is_empty() {
                    ui.weak("The page has no shapes yet.");
                }
                // top first, like a layers list
                let rows: Vec<(Id, String)> = self
                    .doc
                    .shapes()
                    .rev()
                    .map(|s| (s.id, shape_label(s)))
                    .collect();
                for (id, label) in rows {
                    if ui
                        .selectable_label(self.selected == Some(id), label)
                        .clicked()
                    {
                        self.selected = Some(id);
                    }
                }
            });
        ui.separator();

        egui::ScrollArea::vertical()
            .id_salt("props")
            .show(ui, |ui| {
                if let Some(id) = self.selected {
                    self.properties(ui, id);
                    ui.separator();
                }
                self.background(ui);
                ui.separator();
                self.export_ui(ui, package);
            });
        open
    }

    fn add(&mut self, mut shape: ShapeDoc) {
        let offset = 40.0 + 24.0 * (self.added % 8) as f64;
        self.added += 1;
        shape.matrix = Matrix::IDENTITY.translate(offset, offset);
        let layout = self.doc.layouts.last().map_or(0, |l| l.id);
        self.outbox.push(EditOp::NewShape { layout, shape });
        self.message.clear();
    }

    fn properties(&mut self, ui: &mut egui::Ui, id: Id) {
        let Some(mut s) = self.doc.shape(id).cloned() else {
            return;
        };
        let before = s.clone();
        egui::Grid::new("props").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut s.name);
            ui.end_row();

            ui.label("Fill");
            optional_color(ui, &mut s.fill, Rgba(0.55, 0.75, 1.0, 1.0));
            ui.end_row();

            ui.label("Outline");
            ui.horizontal(|ui| {
                optional_color(ui, &mut s.stroke, Rgba::BLACK);
                if s.stroke.is_some() {
                    ui.add(
                        egui::DragValue::new(&mut s.stroke_width)
                            .range(0.0..=100.0)
                            .speed(0.2),
                    );
                }
            });
            ui.end_row();

            ui.label("Text color");
            color(ui, &mut s.text_color);
            ui.end_row();

            ui.label("Font");
            ui.text_edit_singleline(&mut s.font);
            ui.end_row();

            ui.label("");
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.markup, "Markup").on_hover_text(
                    "Pango markup: <b>bold</b>, <span foreground=\"red\">...</span>",
                );
                ui.checkbox(&mut s.flow, "Flow inside the shape");
            });
            ui.end_row();

            ui.label("Image");
            let mut src = s.image.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut src).hint_text("package file"))
                .lost_focus()
            {
                s.image = (!src.trim().is_empty()).then(|| src.trim().to_string());
            }
            ui.end_row();

            if let Some(v) = &s.video {
                ui.label("Video");
                ui.weak(if v.is_empty() { "(unknown source)" } else { v });
                ui.end_row();
            }

            ui.label("Position");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut s.matrix.x0)
                        .prefix("x ")
                        .speed(1.0),
                );
                ui.add(
                    egui::DragValue::new(&mut s.matrix.y0)
                        .prefix("y ")
                        .speed(1.0),
                );
            });
            ui.end_row();
        });

        let mut text = s.text.clone().unwrap_or_default();
        ui.label("Text");
        if ui
            .add(
                egui::TextEdit::multiline(&mut text)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            s.text = (!text.is_empty()).then_some(text);
        }

        ui.horizontal_wrapped(|ui| {
            let around = |s: &ShapeDoc, step: Matrix| {
                let (cx, cy) = s
                    .bounds()
                    .map_or((0.0, 0.0), |(a, b, c, d)| ((a + c) / 2.0, (b + d) / 2.0));
                // `step` about the shape's centre, in window coordinates
                let about = Matrix::IDENTITY
                    .translate(cx, cy)
                    .after(&step)
                    .translate(-cx, -cy);
                about.after(&s.matrix)
            };
            if ui.button("⟲ 15°").clicked() {
                s.matrix = around(&s, Matrix::IDENTITY.rotate(-15f64.to_radians()));
            }
            if ui.button("⟳ 15°").clicked() {
                s.matrix = around(&s, Matrix::IDENTITY.rotate(15f64.to_radians()));
            }
            if ui.button("Bigger").clicked() {
                s.matrix = around(&s, Matrix::IDENTITY.scale(1.1, 1.1));
            }
            if ui.button("Smaller").clicked() {
                s.matrix = around(&s, Matrix::IDENTITY.scale(1.0 / 1.1, 1.0 / 1.1));
            }
            if ui
                .button("Straighten")
                .on_hover_text("Keep the position, drop rotation and scaling")
                .clicked()
            {
                s.matrix = Matrix::IDENTITY.translate(s.matrix.x0, s.matrix.y0);
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Raise").clicked() {
                self.outbox.push(EditOp::Restack { id, up: true });
            }
            if ui.button("Lower").clicked() {
                self.outbox.push(EditOp::Restack { id, up: false });
            }
            if ui.button("Delete").clicked() {
                self.outbox.push(EditOp::Delete(id));
                self.selected = None;
            }
        });

        if s != before
            && let Some(mine) = self.shape_mut(id)
        {
            *mine = s;
        }
    }

    fn background(&mut self, ui: &mut egui::Ui) {
        // the layout of the selected shape, or the bottom one
        let layout = self
            .selected
            .and_then(|id| {
                self.doc
                    .layouts
                    .iter()
                    .find(|l| l.shapes.iter().any(|s| s.id == id))
            })
            .or(self.doc.layouts.first());
        let Some(layout) = layout else { return };
        let (id, mut bg) = (layout.id, layout.background);
        ui.horizontal(|ui| {
            ui.label("Background");
            if optional_color(ui, &mut bg, Rgba::WHITE) {
                self.outbox.push(EditOp::SetBackground {
                    layout: id,
                    color: bg,
                });
            }
        });
    }

    fn export_ui(&mut self, ui: &mut egui::Ui, package: Option<&Path>) {
        ui.strong("Export");
        ui.weak(
            "Writes the scene as a new package. Code (clicks, animation) stays in the original.",
        );
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("lang")
                .selected_text(self.lang.label())
                .show_ui(ui, |ui| {
                    for l in Lang::ALL {
                        ui.selectable_value(&mut self.lang, l, l.label());
                    }
                });
            if ui.button("Copy code").clicked() {
                ui.ctx()
                    .copy_text(underweb_export::source(&self.doc, self.lang));
                self.message = format!("{} code copied.", self.lang.label());
            }
        });
        ui.add(egui::TextEdit::singleline(&mut self.export_dir).desired_width(f32::INFINITY));
        if ui.button("Export package").clicked() {
            self.message = self.export(package);
        }
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
    }

    fn export(&self, package: Option<&Path>) -> String {
        let out = PathBuf::from(self.export_dir.trim());
        let name = out
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase().replace(' ', "-"))
            .filter(|n| underweb_export::valid_name(n))
            .unwrap_or_else(|| "exported".into());
        match underweb_export::write(&self.doc, self.lang, &name, package, &out) {
            Ok(notes) if notes.is_empty() => {
                format!("Wrote {}. Open it in a new tab.", out.display())
            }
            Ok(notes) => format!("Wrote {}.\n{}", out.display(), notes.join("\n")),
            Err(e) => format!("Export failed: {e:#}"),
        }
    }
}

// ---- handles ----------------------------------------------------------------

struct Handle {
    op: usize,
    k: usize,
    /// In the shape's own coordinates.
    pos: (f64, f64),
    /// For a curve's control point: the point it pulls from.
    anchor: Option<(f64, f64)>,
}

fn handles(s: &ShapeDoc) -> Vec<Handle> {
    let mut out = Vec::new();
    let mut prev = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    for (op, p) in s.path.iter().enumerate() {
        let h = |k, pos, anchor| Handle { op, k, pos, anchor };
        match *p {
            PathOp::MoveTo(x, y) => {
                out.push(h(0, (x, y), None));
                prev = (x, y);
                start = prev;
            }
            PathOp::LineTo(x, y) => {
                out.push(h(0, (x, y), None));
                prev = (x, y);
            }
            PathOp::CurveTo(x1, y1, x2, y2, x3, y3) => {
                out.push(h(0, (x1, y1), Some(prev)));
                out.push(h(1, (x2, y2), Some((x3, y3))));
                out.push(h(2, (x3, y3), None));
                prev = (x3, y3);
            }
            PathOp::Rectangle(x, y, w, hh) => {
                for (k, c) in [(x, y), (x + w, y), (x + w, y + hh), (x, y + hh)]
                    .into_iter()
                    .enumerate()
                {
                    out.push(h(k, c, None));
                }
            }
            PathOp::Close => prev = start,
        }
    }
    out
}

fn set_handle(path: &mut [PathOp], op: usize, k: usize, (x, y): (f64, f64)) {
    let Some(p) = path.get_mut(op) else { return };
    match p {
        PathOp::MoveTo(px, py) | PathOp::LineTo(px, py) => (*px, *py) = (x, y),
        PathOp::CurveTo(x1, y1, x2, y2, x3, y3) => match k {
            0 => (*x1, *y1) = (x, y),
            1 => (*x2, *y2) = (x, y),
            _ => {
                // the end point carries its control point along
                (*x2, *y2) = (*x2 + x - *x3, *y2 + y - *y3);
                (*x3, *y3) = (x, y);
            }
        },
        PathOp::Rectangle(rx, ry, w, h) => {
            // the opposite corner stays put
            let (ox, oy) = match k {
                0 => (*rx + *w, *ry + *h),
                1 => (*rx, *ry + *h),
                2 => (*rx, *ry),
                _ => (*rx + *w, *ry),
            };
            (*rx, *ry, *w, *h) = (x.min(ox), y.min(oy), (x - ox).abs(), (y - oy).abs());
        }
        PathOp::Close => {}
    }
}

fn rectangle() -> Vec<PathOp> {
    vec![
        PathOp::MoveTo(0.0, 0.0),
        PathOp::LineTo(150.0, 0.0),
        PathOp::LineTo(150.0, 100.0),
        PathOp::LineTo(0.0, 100.0),
        PathOp::Close,
    ]
}

fn triangle() -> Vec<PathOp> {
    vec![
        PathOp::MoveTo(75.0, 0.0),
        PathOp::LineTo(150.0, 100.0),
        PathOp::LineTo(0.0, 100.0),
        PathOp::Close,
    ]
}

fn circle() -> Vec<PathOp> {
    let (r, c) = (75.0, 75.0);
    let k = 0.5523 * r;
    vec![
        PathOp::MoveTo(c + r, c),
        PathOp::CurveTo(c + r, c + k, c + k, c + r, c, c + r),
        PathOp::CurveTo(c - k, c + r, c - r, c + k, c - r, c),
        PathOp::CurveTo(c - r, c - k, c - k, c - r, c, c - r),
        PathOp::CurveTo(c + k, c - r, c + r, c - k, c + r, c),
        PathOp::Close,
    ]
}

fn shape_label(s: &ShapeDoc) -> String {
    if !s.name.is_empty() {
        return s.name.clone();
    }
    let what = if s.video.is_some() {
        "video"
    } else if s.image.is_some() {
        "image"
    } else if s.text.is_some() {
        "text"
    } else {
        "shape"
    };
    format!("{what} {}", s.id)
}

fn color(ui: &mut egui::Ui, c: &mut Rgba) -> bool {
    let mut v = [c.0 as f32, c.1 as f32, c.2 as f32, c.3 as f32];
    let changed = ui.color_edit_button_rgba_unmultiplied(&mut v).changed();
    if changed {
        *c = Rgba(v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64);
    }
    changed
}

/// A checkbox to have the colour at all, and the colour.
fn optional_color(ui: &mut egui::Ui, c: &mut Option<Rgba>, default: Rgba) -> bool {
    let mut on = c.is_some();
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.checkbox(&mut on, "").changed() {
            *c = on.then_some(c.unwrap_or(default));
            changed = true;
        }
        if let Some(col) = c.as_mut() {
            changed |= color(ui, col);
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use underweb_core::scene::LayoutDoc;

    #[test]
    fn click_selects_drag_moves_and_reshapes() {
        let mut e = Editor::new("test");
        e.receive(SceneDoc {
            title: String::new(),
            layouts: vec![LayoutDoc {
                id: 1,
                background: None,
                shapes: vec![ShapeDoc {
                    id: 7,
                    path: rectangle(),
                    stroke: None,
                    ..Default::default()
                }],
            }],
        });
        // the page is drawn at (100, 50) on screen
        let rect = Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(800.0, 600.0));
        let at = |x: f32, y: f32| (Pos2::new(100.0 + x, 50.0 + y), (x as f64, y as f64));

        let (s, p) = at(20.0, 20.0);
        e.press(s, p, rect);
        assert_eq!(e.selected, Some(7));
        let (s, p) = at(50.0, 30.0);
        e.drag_to(s, p);
        assert_eq!(
            e.doc.shape(7).unwrap().matrix,
            Matrix::IDENTITY.translate(30.0, 10.0)
        );
        e.drag = None;

        // the corner that was at (150, 0) is now at (180, 10)
        let (s, p) = at(181.0, 11.0);
        e.press(s, p, rect);
        assert!(matches!(e.drag, Some(Drag::Point { op: 1, k: 0, .. })));
        let (s, p) = at(200.0, 10.0);
        e.drag_to(s, p);
        assert_eq!(e.doc.shape(7).unwrap().path[1], PathOp::LineTo(170.0, 0.0));
        e.drag = None;

        // right-click on a corner removes it; elsewhere adds one
        let (s, p) = at(200.0, 10.0);
        e.right_click(s, p, rect);
        assert_eq!(e.doc.shape(7).unwrap().path.len(), 4);
        let (s, p) = at(250.0, 200.0);
        e.right_click(s, p, rect);
        assert_eq!(e.doc.shape(7).unwrap().path.len(), 5);

        // clicking empty space clears the selection
        let (s, p) = at(700.0, 500.0);
        e.press(s, p, rect);
        assert_eq!(e.selected, None);
        assert_eq!(
            e.changed,
            Some(7),
            "the shape is sent once, at the end of the frame"
        );
    }

    #[test]
    fn dragging_handles_reshapes_paths() {
        let mut path = vec![PathOp::Rectangle(10.0, 10.0, 100.0, 50.0)];
        set_handle(&mut path, 0, 2, (200.0, 100.0));
        assert_eq!(path[0], PathOp::Rectangle(10.0, 10.0, 190.0, 90.0));
        // dragged past the opposite corner: still a proper rectangle
        set_handle(&mut path, 0, 0, (300.0, 0.0));
        assert_eq!(path[0], PathOp::Rectangle(200.0, 0.0, 100.0, 100.0));

        let mut curve = circle();
        set_handle(&mut curve, 1, 2, (0.0, 0.0));
        let PathOp::CurveTo(_, _, x2, y2, x3, y3) = curve[1] else {
            panic!()
        };
        assert_eq!((x3, y3), (0.0, 0.0));
        // the control point moved with its end point, by (-75, -150)
        assert!((x2 - 0.5523 * 75.0).abs() < 1e-9 && y2.abs() < 1e-9);
    }

    #[test]
    fn every_point_has_a_handle() {
        let s = ShapeDoc {
            path: circle(),
            ..Default::default()
        };
        // a move-to and four curves of three handles each
        assert_eq!(handles(&s).len(), 1 + 4 * 3);
    }
}
