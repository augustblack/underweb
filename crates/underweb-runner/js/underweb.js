// The Underweb page API for JavaScript and TypeScript packages.
// Packages import it as `import { Layout, Shape, window } from "underweb"`.
// Types: sdk/js/underweb.d.ts.

const ops = Deno.core.ops;

const shapes = new Map();

class Layout {
  constructor() {
    this.id = ops.op_uw_layout_new();
  }
  append(shape) {
    ops.op_uw_layout_append(this.id, shape.id);
    return this;
  }
}

class Shape {
  constructor() {
    this.id = ops.op_uw_shape_new();
    this.name = "";
    shapes.set(this.id, this);
  }
  clearPath() { ops.op_uw_shape_clear_path(this.id); return this; }
  rectangle(x, y, w, h) { ops.op_uw_shape_rectangle(this.id, x, y, w, h); return this; }
  moveTo(x, y) { ops.op_uw_shape_move_to(this.id, x, y); return this; }
  lineTo(x, y) { ops.op_uw_shape_line_to(this.id, x, y); return this; }
  curveTo(x1, y1, x2, y2, x3, y3) {
    ops.op_uw_shape_curve_to(this.id, x1, y1, x2, y2, x3, y3);
    return this;
  }
  closePath() { ops.op_uw_shape_close_path(this.id); return this; }
  setFillColor(r, g, b, a = 1) { ops.op_uw_shape_set_fill_color(this.id, r, g, b, a); return this; }
  setStrokeColor(r, g, b, a = 1) { ops.op_uw_shape_set_stroke_color(this.id, r, g, b, a); return this; }
  setTextColor(r, g, b, a = 1) { ops.op_uw_shape_set_text_color(this.id, r, g, b, a); return this; }
  set strokeWidth(w) { ops.op_uw_shape_set_stroke_width(this.id, w); }
  set text(t) { ops.op_uw_shape_set_text(this.id, String(t)); }
  set font(f) { ops.op_uw_shape_set_font(this.id, String(f)); }
}

const window = {
  set title(t) { ops.op_uw_set_title(String(t)); },
  addLayout(layout) { ops.op_uw_add_layout(layout.id); },
  set animating(on) { ops.op_uw_set_animating(!!on); },
  redraw() { ops.op_uw_request_redraw(); },
  get width() { return ops.op_uw_width(); },
  get height() { return ops.op_uw_height(); },
  param(key) { return ops.op_uw_param(String(key)); },
  log(...args) { ops.op_uw_log(args.map(String).join(" ")); },
};

globalThis.console = {
  log: window.log,
  info: window.log,
  warn: window.log,
  error: window.log,
};

// Called by the runner for each event. `name` is a page export.
function dispatch(name, arg) {
  const page = globalThis.__uw_page;
  const fn = page && page[name];
  if (typeof fn !== "function") return;
  if (name === "pointer") {
    arg.shape = shapes.get(arg.shape) ?? null;
  }
  fn(arg);
}

const api = { Layout, Shape, window, dispatch };
Object.defineProperty(globalThis, "__uw", { value: api });

// Packages get no Deno internals: only the API above.
delete globalThis.Deno;
