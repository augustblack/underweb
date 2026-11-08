// demos/15puredata.vala, ported. 440.pd has two oscillators (left and
// right) with receivers freqL, freqR, both, ampL and ampR.
import { Layout, Shape, audio, window } from "underweb";

let scope;

export function activate() {
  window.title = "Pure Data";
  const layout = new Layout();
  window.addLayout(layout);
  const label = new Shape().rectangle(50, 40, 600, 70).setFillColor(0.85, 0.95, 1);
  label.text = "440.pd from the 2011 demos. Mouse across: pitch of both oscillators. Up and down: left/right loudness.";
  layout.append(label);
  layout.append(new Shape().rectangle(50, 140, 600, 200).setFillColor(0.1, 0.1, 0.15));
  scope = new Shape().setStrokeColor(1, 0.8, 0.3);
  scope.strokeWidth = 2;
  layout.append(scope);

  if (!audio.rate) {
    label.text = "This browser has no audio output.";
    return;
  }
  audio.pd.open("440.pd");
  audio.pd.send("ampL", 0.5);
  audio.pd.send("ampR", 0.5);
  window.animating = true;
}

export function update() {
  const s = audio.scope(600);
  scope.clearPath().moveTo(50, 240);
  s.forEach((x, i) => scope.lineTo(50 + i, 240 - Math.max(-1, Math.min(1, x)) * 96));
}

export function pointer(ev) {
  if (ev.kind !== "move" || !audio.rate) return;
  audio.pd.send("both", 110 + (ev.x / Math.max(1, window.width)) * 770);
  const y = ev.y / Math.max(1, window.height);
  audio.pd.send("ampL", 1 - y);
  audio.pd.send("ampR", y);
}
