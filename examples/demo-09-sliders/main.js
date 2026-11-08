// demos/09widgets.vala, ported. The old demo put GTK sliders on the page;
// pages are drawn with shapes now, so the sliders are shapes too: a track
// and a knob each, dragged with the pointer.
import { Layout, Shape, window } from "underweb";

const TOP = 120, HEIGHT = 200, WIDTH = 18;
const sliders = [];
let target, layout, label, active = null;
const fill = [0.2, 0.6, 1, 0.8];
const background = [1, 1, 1, 1];

class Slider {
  constructor(x, name, values, i, color) {
    Object.assign(this, { x, values, i });
    this.track = new Shape().rectangle(x, TOP, WIDTH, HEIGHT).setFillColor(...color, 0.25);
    this.track.setStrokeColor(0, 0, 0, 0.4);
    this.track.name = `${name} track`;
    this.knob = new Shape().setFillColor(...color, 1).setStrokeColor(0, 0, 0, 1);
    this.knob.name = `${name} knob`;
    this.place();
  }
  get value() { return this.values[this.i]; }
  place() {
    const y = TOP + (1 - this.value) * HEIGHT;
    this.knob.clearPath().rectangle(this.x - 6, y - 7, WIDTH + 12, 14);
  }
  hits(shape) { return shape === this.track || shape === this.knob; }
  drag(y) {
    this.values[this.i] = Math.min(1, Math.max(0, 1 - (y - TOP) / HEIGHT));
    this.place();
  }
}

function group(x, name, values) {
  const colors = [[1, 0, 0], [0, 0.7, 0], [0, 0, 1], [0.4, 0.4, 0.4]];
  "RGBA".split("").forEach((c, i) => sliders.push(new Slider(x + i * 34, `${name} ${c}`, values, i, colors[i])));
  const caption = new Shape().rectangle(x - 10, TOP + HEIGHT + 16, 150, 34).setStrokeColor(0, 0, 0, 0);
  caption.text = name;
  layout.append(caption);
}

function update() {
  target.setFillColor(...fill);
  layout.setBackground(...background);
  const f = (v) => v.map((x) => x.toFixed(2)).join(", ");
  label.text = `shape: rgba(${f(fill)})\nbackground: rgba(${f(background)})`;
}

export function activate() {
  window.title = "Sliders";
  layout = new Layout();
  window.addLayout(layout);

  target = new Shape().circle(560, 220, 110).setStrokeColor(0, 0, 0, 1);
  target.name = "target";
  target.strokeWidth = 2;
  layout.append(target);
  label = new Shape().rectangle(420, 360, 420, 70).setStrokeColor(0, 0, 0, 0);
  label.font = "Monospace 11";
  layout.append(label);

  group(60, "shape", fill);
  group(240, "background", background);
  for (const s of sliders) layout.append(s.track).append(s.knob);
  update();
}

export function pointer(ev) {
  if (ev.kind === "press") active = sliders.find((s) => s.hits(ev.shape)) ?? null;
  if (ev.kind === "release") active = null;
  if (active && ev.kind !== "release") {
    active.drag(ev.y);
    update();
  }
}
