// demos/07videoplay.vala, ported: one clip shown in two shapes (a
// rectangle and a curved blob, clipped to its path), and a live test
// pattern in a third. Click a shape to pause or play its video.
import { Layout, Shape, video, window } from "underweb";

let clip, bars, status, playing = true;

export function activate() {
  window.title = "video";
  const layout = new Layout();
  window.addLayout(layout);

  clip = video.open("ball.webm", { loop: true }).play();
  bars = video.open("test:smpte").play();

  const rect = new Shape().rectangle(40, 40, 320, 240).setFillColor(0, 0, 0);
  rect.strokeWidth = 3;
  rect.video = clip;
  layout.append(rect);

  // a blob made of curves: the video is clipped to it
  const blob = new Shape()
    .moveTo(420, 160)
    .curveTo(420, 40, 640, 20, 660, 140)
    .curveTo(680, 260, 560, 320, 480, 280)
    .curveTo(400, 240, 420, 200, 420, 160)
    .closePath()
    .setStrokeColor(0.8, 0.2, 0.5);
  blob.strokeWidth = 4;
  blob.video = clip;
  layout.append(blob);

  const bubble = new Shape();
  circle(bubble, 200, 420, 90);
  bubble.video = bars;
  layout.append(bubble);

  status = new Shape().rectangle(320, 360, 360, 110).setFillColor(0.95, 0.95, 0.95);
  layout.append(status);
  window.animating = true;
  clip.onerror = (m) => (status.text = "video error: " + m);
}

function circle(s, cx, cy, r) {
  const k = 0.5523 * r;
  s.moveTo(cx + r, cy)
    .curveTo(cx + r, cy + k, cx + k, cy + r, cx, cy + r)
    .curveTo(cx - k, cy + r, cx - r, cy + k, cx - r, cy)
    .curveTo(cx - r, cy - k, cx - k, cy - r, cx, cy - r)
    .curveTo(cx + k, cy - r, cx + r, cy - k, cx + r, cy)
    .closePath();
}

export function update() {
  status.text = `ball.webm ${clip.width}×${clip.height}, ${clip.position.toFixed(1)} of ${clip.duration.toFixed(1)} s` +
    `\nsmpte test pattern ${bars.width}×${bars.height} (live)\nclick to ${playing ? "pause" : "play"}`;
}

export function pointer(ev) {
  if (ev.kind !== "press") return;
  playing = !playing;
  for (const v of [clip, bars]) playing ? v.play() : v.pause();
}
