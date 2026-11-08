// demos/05animation.vala, ported. The old demo redrew from a 10 ms timer;
// here the browser calls update(dt) once per display frame while the page
// says it is animating.
import { Layout, Shape, window } from "underweb";

let s: Shape;
let t = 0;

function curve(o: number): void {
  s.clearPath().moveTo(0, 0).lineTo(600, 0).curveTo(300, 300 * o, 300, -300 * o, 0, 0);
}

export function activate(): void {
  window.title = "Animation";
  const layout = new Layout();
  window.addLayout(layout);
  s = new Shape().setFillColor(0, 1, 0, 0.1).setStrokeColor(0, 0, 1, 0.5).translate(100, 200);
  s.name = "curve";
  s.strokeWidth = 2;
  curve(1);
  layout.append(s);
  window.animating = true;
}

export function update(dt: number): void {
  t += dt;
  curve(Math.sin(2 * Math.PI * 0.4 * t));
}
