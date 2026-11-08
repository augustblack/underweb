//! The page's scene as plain data: what the layout editor sees and edits.
//!
//! The runner owns the live scene (`underweb-guest`). In edit mode it sends
//! a [`SceneDoc`] to the browser whenever it changes; the browser sends
//! back [`EditOp`]s. Exported code is generated from a `SceneDoc` too.

use serde::{Deserialize, Serialize};

pub type Id = u32;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rgba(pub f64, pub f64, pub f64, pub f64);

impl Rgba {
    pub const BLACK: Rgba = Rgba(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Rgba = Rgba(1.0, 1.0, 1.0, 1.0);
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PathOp {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    CurveTo(f64, f64, f64, f64, f64, f64),
    Rectangle(f64, f64, f64, f64),
    Close,
}

/// An affine transform, in cairo's order: a point (x, y) maps to
/// (xx·x + xy·y + x0, yx·x + yy·y + y0).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    pub xx: f64,
    pub yx: f64,
    pub xy: f64,
    pub yy: f64,
    pub x0: f64,
    pub y0: f64,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix {
        xx: 1.0,
        yx: 0.0,
        xy: 0.0,
        yy: 1.0,
        x0: 0.0,
        y0: 0.0,
    };

    pub fn is_identity(&self) -> bool {
        *self == Matrix::IDENTITY
    }

    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.xx * x + self.xy * y + self.x0,
            self.yx * x + self.yy * y + self.y0,
        )
    }

    /// `self` after `first`: applies `first`, then `self`.
    pub fn after(&self, first: &Matrix) -> Matrix {
        Matrix {
            xx: self.xx * first.xx + self.xy * first.yx,
            yx: self.yx * first.xx + self.yy * first.yx,
            xy: self.xx * first.xy + self.xy * first.yy,
            yy: self.yx * first.xy + self.yy * first.yy,
            x0: self.xx * first.x0 + self.xy * first.y0 + self.x0,
            y0: self.yx * first.x0 + self.yy * first.y0 + self.y0,
        }
    }

    pub fn invert(&self) -> Option<Matrix> {
        let det = self.xx * self.yy - self.xy * self.yx;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let (xx, xy, yx, yy) = (self.yy / det, -self.xy / det, -self.yx / det, self.xx / det);
        Some(Matrix {
            xx,
            yx,
            xy,
            yy,
            x0: -(xx * self.x0 + xy * self.y0),
            y0: -(yx * self.x0 + yy * self.y0),
        })
    }

    // like cairo_translate & co: the new step happens first, in the
    // shape's own coordinates

    pub fn translate(&self, tx: f64, ty: f64) -> Matrix {
        self.after(&Matrix {
            x0: tx,
            y0: ty,
            ..Matrix::IDENTITY
        })
    }

    pub fn scale(&self, sx: f64, sy: f64) -> Matrix {
        self.after(&Matrix {
            xx: sx,
            yy: sy,
            ..Matrix::IDENTITY
        })
    }

    /// Radians, clockwise on screen.
    pub fn rotate(&self, angle: f64) -> Matrix {
        let (s, c) = angle.sin_cos();
        self.after(&Matrix {
            xx: c,
            yx: s,
            xy: -s,
            yy: c,
            ..Matrix::IDENTITY
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShapeDoc {
    /// 0 for a shape the editor is about to create.
    pub id: Id,
    pub name: String,
    pub path: Vec<PathOp>,
    pub matrix: Matrix,
    pub fill: Option<Rgba>,
    pub stroke: Option<Rgba>,
    pub stroke_width: f64,
    pub text: Option<String>,
    pub text_color: Rgba,
    pub font: String,
    /// Text is Pango markup.
    pub markup: bool,
    /// Text flows word by word inside the path, instead of in its
    /// bounding box.
    pub flow: bool,
    /// Package file shown inside the path; "" for one made from bytes.
    pub image: Option<String>,
    /// The source of the video shown inside the path (read only).
    pub video: Option<String>,
}

impl Default for ShapeDoc {
    fn default() -> Self {
        ShapeDoc {
            id: 0,
            name: String::new(),
            path: Vec::new(),
            matrix: Matrix::IDENTITY,
            fill: None,
            stroke: Some(Rgba::BLACK),
            stroke_width: 1.0,
            text: None,
            text_color: Rgba::BLACK,
            font: "Sans 14".into(),
            markup: false,
            flow: false,
            image: None,
            video: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutDoc {
    pub id: Id,
    pub background: Option<Rgba>,
    pub shapes: Vec<ShapeDoc>,
}

/// The layouts shown in the window, bottom to top.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneDoc {
    pub title: String,
    pub layouts: Vec<LayoutDoc>,
}

impl SceneDoc {
    pub fn shape(&self, id: Id) -> Option<&ShapeDoc> {
        self.shapes().find(|s| s.id == id)
    }

    pub fn shape_mut(&mut self, id: Id) -> Option<&mut ShapeDoc> {
        self.layouts
            .iter_mut()
            .flat_map(|l| l.shapes.iter_mut())
            .find(|s| s.id == id)
    }

    /// Every shape, bottom to top.
    pub fn shapes(&self) -> impl DoubleEndedIterator<Item = &ShapeDoc> {
        self.layouts.iter().flat_map(|l| l.shapes.iter())
    }

    /// Topmost shape containing the window point.
    pub fn hit_test(&self, x: f64, y: f64) -> Option<Id> {
        self.shapes().rev().find(|s| s.contains(x, y)).map(|s| s.id)
    }
}

/// A change the layout editor makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EditOp {
    /// Replace a shape's properties (its `id` says which). A changed
    /// `image` source is loaded from the package.
    SetShape(ShapeDoc),
    /// Append a shape to `layout`, or to a new layout if 0.
    NewShape {
        layout: Id,
        shape: ShapeDoc,
    },
    /// Take a shape out of its layouts.
    Delete(Id),
    /// Move a shape one step up (towards the top) or down.
    Restack {
        id: Id,
        up: bool,
    },
    SetBackground {
        layout: Id,
        color: Option<Rgba>,
    },
    SetTitle(String),
}

// ---- geometry, for the editor (no cairo in the browser) -----------------

impl ShapeDoc {
    /// The outline as polygons in window coordinates, curves flattened.
    pub fn polygons(&self) -> Vec<Vec<(f64, f64)>> {
        flatten(&self.path)
            .into_iter()
            .map(|p| {
                p.into_iter()
                    .map(|(x, y)| self.matrix.apply(x, y))
                    .collect()
            })
            .collect()
    }

    /// Whether the window point is inside the fill (non-zero winding, like
    /// cairo) or within half the stroke width of the outline.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let Some(inv) = self.matrix.invert() else {
            return false;
        };
        let (px, py) = inv.apply(x, y);
        let polys = flatten(&self.path);
        let mut winding = 0i32;
        for poly in &polys {
            for i in 0..poly.len() {
                let (ax, ay) = poly[i];
                let (bx, by) = poly[(i + 1) % poly.len()];
                let side = (bx - ax) * (py - ay) - (px - ax) * (by - ay);
                if ay <= py && by > py && side > 0.0 {
                    winding += 1;
                } else if ay > py && by <= py && side < 0.0 {
                    winding -= 1;
                }
            }
        }
        if winding != 0 {
            return true;
        }
        let half = (self.stroke_width / 2.0).max(2.0);
        self.stroke.is_some()
            && polys.iter().any(|p| {
                p.windows(2)
                    .any(|w| dist_to_segment((px, py), w[0], w[1]) <= half)
            })
    }

    /// Bounding box of the outline in window coordinates.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let mut it = self.polygons().into_iter().flatten();
        let (x, y) = it.next()?;
        Some(it.fold((x, y, x, y), |(a, b, c, d), (x, y)| {
            (a.min(x), b.min(y), c.max(x), d.max(y))
        }))
    }
}

fn dist_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Subpaths as closed polygons in the shape's own coordinates.
pub fn flatten(path: &[PathOp]) -> Vec<Vec<(f64, f64)>> {
    let mut out = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut flush = |cur: &mut Vec<(f64, f64)>| {
        if cur.len() > 1 {
            out.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };
    for op in path {
        match *op {
            PathOp::MoveTo(x, y) => {
                flush(&mut cur);
                cur.push((x, y));
            }
            PathOp::LineTo(x, y) => cur.push((x, y)),
            PathOp::CurveTo(x1, y1, x2, y2, x3, y3) => {
                let (x0, y0) = cur.last().copied().unwrap_or((x1, y1));
                for i in 1..=16 {
                    let t = i as f64 / 16.0;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    cur.push((
                        a * x0 + b * x1 + c * x2 + d * x3,
                        a * y0 + b * y1 + c * y2 + d * y3,
                    ));
                }
            }
            PathOp::Rectangle(x, y, w, h) => {
                flush(&mut cur);
                cur.extend([(x, y), (x + w, y), (x + w, y + h), (x, y + h)]);
                flush(&mut cur);
            }
            PathOp::Close => {
                let start = cur.first().copied();
                flush(&mut cur);
                // like cairo: drawing continues from the subpath's start
                if let Some(p) = start {
                    cur.push(p);
                }
            }
        }
    }
    flush(&mut cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_compose_and_invert() {
        let m = Matrix::IDENTITY
            .translate(100.0, 50.0)
            .rotate(0.5)
            .scale(2.0, 3.0);
        let (x, y) = m.apply(4.0, -7.0);
        let back = m.invert().unwrap().apply(x, y);
        assert!((back.0 - 4.0).abs() < 1e-9 && (back.1 + 7.0).abs() < 1e-9);
        // translate happens last on screen: the origin lands at (100, 50)
        assert_eq!(m.apply(0.0, 0.0), (100.0, 50.0));
    }

    #[test]
    fn hit_testing_follows_the_matrix() {
        let mut s = ShapeDoc {
            path: vec![PathOp::Rectangle(0.0, 0.0, 100.0, 50.0)],
            stroke: None,
            ..Default::default()
        };
        assert!(s.contains(10.0, 10.0));
        s.matrix = Matrix::IDENTITY.translate(200.0, 0.0);
        assert!(!s.contains(10.0, 10.0));
        assert!(s.contains(210.0, 10.0));
        assert_eq!(s.bounds(), Some((200.0, 0.0, 300.0, 50.0)));
    }

    #[test]
    fn curves_and_triangles() {
        let tri = ShapeDoc {
            path: vec![
                PathOp::MoveTo(75.0, 0.0),
                PathOp::LineTo(150.0, 100.0),
                PathOp::LineTo(0.0, 100.0),
                PathOp::Close,
            ],
            stroke: None,
            ..Default::default()
        };
        assert!(tri.contains(75.0, 50.0));
        assert!(!tri.contains(10.0, 10.0));
        let blob = ShapeDoc {
            path: vec![
                PathOp::MoveTo(0.0, 0.0),
                PathOp::CurveTo(100.0, -100.0, 200.0, 100.0, 0.0, 100.0),
                PathOp::Close,
            ],
            ..Default::default()
        };
        assert!(blob.contains(50.0, 50.0));
        assert!(!blob.contains(-50.0, 50.0));
    }
}
