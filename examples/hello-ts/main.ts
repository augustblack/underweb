import { Layout, Shape, window } from "underweb";
import type { PointerEvent } from "underweb";

interface Wave {
  shape: Shape;
  phase: number;
}

const waves: Wave[] = [];
let time = 0;

function draw(w: Wave): void {
  const y = (offset: number): number => 200 + 120 * Math.sin(time * 2 + w.phase + offset);
  w.shape.clearPath().moveTo(50, 200).curveTo(250, y(Math.PI / 2), 450, y(-Math.PI / 2), 650, 200);
}

export function activate(): void {
  window.title = "TS animation";
  const layout = new Layout();
  window.addLayout(layout);

  const label = new Shape().rectangle(50, 360, 600, 70).setFillColor(0.95, 0.95, 0.8, 1);
  label.text = "TypeScript, transpiled in the runner and run in V8. Click to add a wave.";
  layout.append(label);

  addWave(layout, 0);
  window.animating = true;
  currentLayout = layout;
}

let currentLayout: Layout;

function addWave(layout: Layout, phase: number): void {
  const shape = new Shape().setStrokeColor(Math.random(), 0.2, 0.8, 1);
  shape.strokeWidth = 4;
  const w: Wave = { shape, phase };
  draw(w);
  waves.push(w);
  layout.append(shape);
}

export function update(dt: number): void {
  time += dt;
  waves.forEach(draw);
}

export function pointer(ev: PointerEvent): void {
  if (ev.kind === "press") {
    addWave(currentLayout, waves.length * 0.7);
  }
}
