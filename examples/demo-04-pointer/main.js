// demos/04shape.vala, ported: a rotated shape reports the pointer, both
// where it is in the window and where that is in the shape's own
// (unrotated) coordinates. The old over/out/drag signals are made here
// from move, press and release.
import { Layout, Shape, window } from "underweb";

let log, box;
let over = false;
let dragging = false;

export function activate() {
  window.title = "Pointer";
  const layout = new Layout();
  window.addLayout(layout);

  log = new Shape().rectangle(0, 0, 700, 34).setFillColor(1, 0.5, 0.5, 0.5);
  log.setStrokeColor(0, 0, 0, 0);
  log.name = "log";
  log.text = "Move over the green shape, press and drag.";
  layout.append(log);

  box = new Shape()
    .rectangle(0, 0, 300, 200)
    .setFillColor(0, 1, 0, 0.5)
    .setStrokeColor(0, 0, 1, 0.5)
    .translate(200, 100)
    .rotate((10 * Math.PI) / 180);
  box.name = "box";
  box.font = "Sans 13";
  box.text =
    "This shape is turned by 10 degrees. Its events come with the pointer in window " +
    "coordinates; shape.toLocal() turns them into the shape's own, where its corner is (0, 0).";
  layout.append(box);
}

export function pointer(ev) {
  const inside = ev.shape === box;
  let what = ev.kind === "move" && dragging ? "drag" : ev.kind;
  if (inside !== over) {
    over = inside;
    what = inside ? "over" : "out";
    box.setFillColor(0, 1, 0, inside ? 0.8 : 0.5);
  }
  if (ev.kind === "press" && inside) dragging = true;
  if (ev.kind === "release") dragging = false;
  if (!inside && !dragging && what !== "out") return;
  const [x, y] = box.toLocal(ev.x, ev.y);
  log.text = `${what}  window (${ev.x.toFixed(0)}, ${ev.y.toFixed(0)})  shape (${x.toFixed(1)}, ${y.toFixed(1)})`;
}
