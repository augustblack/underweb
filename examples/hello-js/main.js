import { Layout, Shape, window } from "underweb";

let box;

export function activate() {
  window.title = "JS shape";
  const layout = new Layout();
  window.addLayout(layout);

  box = new Shape()
    .rectangle(100, 100, 300, 200)
    .setFillColor(1, 0, 0, 1)
    .setStrokeColor(0, 0, 1, 0.5)
    .setTextColor(0, 0, 0, 1);
  box.strokeWidth = 1;
  box.text = "Hello, and welcome to the Underweb.\n\n" +
    "This is JavaScript speaking, from V8 in a sandboxed process. Click me.";
  layout.append(box);
}

export function pointer(ev) {
  if (ev.kind === "press" && ev.shape === box) {
    box.setFillColor(Math.random(), Math.random(), Math.random(), 1);
    console.log("clicked at", ev.x, ev.y);
  }
}
