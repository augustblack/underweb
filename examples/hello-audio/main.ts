// demos/11audio.vala, ported: move the mouse to play; x is pitch and
// pan, y is loudness. The scope shows what you hear.
import { Layout, Shape, audio, window } from "underweb";
import type { PointerEvent } from "underweb";

const W = 600, H = 200, X = 50, Y = 140;
let scope: Shape;
let osc: ReturnType<typeof audio.osc>;

export function activate(): void {
  window.title = "audio";
  const layout = new Layout();
  window.addLayout(layout);

  const label = new Shape().rectangle(X, 40, W, 70).setFillColor(1, 0.85, 1);
  label.text = audio.rate
    ? `Move the mouse over the page: across for pitch and pan, up and down for loudness. (${audio.rate} Hz)`
    : "This browser has no audio output.";
  layout.append(label);

  const frame = new Shape().rectangle(X, Y, W, H).setFillColor(0.1, 0.1, 0.15);
  layout.append(frame);
  scope = new Shape().setStrokeColor(0.4, 1, 0.6);
  scope.strokeWidth = 2;
  layout.append(scope);

  if (audio.rate) {
    osc = audio.osc("square", { freq: 220, amp: 0.15 });
    window.animating = true;
  }
}

export function update(): void {
  const s = audio.scope(W);
  scope.clearPath().moveTo(X, Y + H / 2);
  for (let i = 0; i < s.length; i++) {
    scope.lineTo(X + i, Y + H / 2 - Math.max(-1, Math.min(1, s[i])) * (H / 2 - 4));
  }
}

export function pointer(ev: PointerEvent): void {
  if (!osc || ev.kind !== "move") return;
  const w = Math.max(1, window.width), h = Math.max(1, window.height);
  osc.freq = 55 * Math.pow(2, (ev.x / w) * 5); // 55 Hz to 1760 Hz
  osc.pan = (ev.x / w) * 2 - 1;
  osc.amp = 0.3 * (1 - ev.y / h);
}
