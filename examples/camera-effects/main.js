// The live camera inside shapes, with effects. Some effects are only
// geometry (the same video in several transformed shapes: mirror, tiles,
// kaleidoscope); the others are video filters (grayscale, invert,
// pixelate, posterize). Drag the small circle around; it always shows the
// plain camera. The layout follows the window's size (resize).
//
// ?source=test:ball (or any video source) stands in for the camera;
// ?effect=kaleidoscope (say) starts with that effect.
import { Layout, Shape, video, window } from "underweb";

const source = window.param("source") ?? "camera";
const EFFECTS = [
  "normal", "mirror", "tiles", "kaleidoscope",
  "grayscale", "invert", "pixelate", "posterize",
];
// the most shapes an effect uses (kaleidoscope)
const VIEWS = 6;

let cam, layout, status, freeze, frozen = false;
let effect = EFFECTS.includes(window.param("effect")) ? window.param("effect") : "normal";
let views = [], buttons = [], bubble;
let size = [800, 600];
let bubbleAt = null; // the circle's centre, once dragged
let drag = null;     // [dx, dy] from the pointer to the circle's centre

export function activate() {
  window.title = "Camera effects";
  layout = new Layout().setBackground(0.12, 0.12, 0.14);
  window.addLayout(layout);

  cam = video.open(source).play();
  cam.onerror = (m) => {
    status.text = `No picture from ${source}: ${m}`;
    window.log(`error ${m}`);
  };
  cam.onready = (info) => window.log(`ready ${info.width}x${info.height}`);

  for (let i = 0; i < VIEWS; i++) {
    const v = new Shape().setFillColor(0, 0, 0, 0).setStrokeColor(0, 0, 0, 0);
    v.name = `view ${i}`;
    views.push(v);
    layout.append(v);
  }
  for (const name of EFFECTS) {
    const b = new Shape().setTextColor(1, 1, 1).setStrokeColor(0, 0, 0, 0);
    b.name = name;
    b.font = "Sans 13";
    b.text = name;
    buttons.push(b);
    layout.append(b);
  }
  freeze = new Shape().setTextColor(1, 1, 1).setStrokeColor(0, 0, 0, 0);
  freeze.name = "freeze";
  freeze.font = "Sans Bold 13";
  layout.append(freeze);

  bubble = new Shape().setFillColor(0, 0, 0).setStrokeColor(1, 1, 1);
  bubble.strokeWidth = 3;
  bubble.name = "plain camera";
  bubble.video = cam;
  layout.append(bubble);

  status = new Shape()
    .setTextColor(0.85, 0.85, 0.85)
    .setFillColor(0, 0, 0, 0)
    .setStrokeColor(0, 0, 0, 0);
  status.font = "Sans 12";
  layout.append(status);

  window.animating = true;
  arrange();
}

export function resize(w, h) {
  size = [w, h];
  arrange();
}

// ---- layout ------------------------------------------------------------

const PAD = 16, BUTTON_W = 130, BUTTON_H = 34;

/** Where the picture goes: a 4:3 box left of the buttons. */
function stage() {
  const [w, h] = size;
  const maxW = Math.max(120, w - BUTTON_W - 3 * PAD);
  const maxH = Math.max(90, h - 2 * PAD - 30);
  const sw = Math.min(maxW, maxH * 4 / 3);
  const sh = sw * 3 / 4;
  return { x: PAD, y: PAD + 30, w: sw, h: sh };
}

function arrange() {
  if (!status) return;
  const s = stage();
  status.clearPath().rectangle(PAD, 4, size[0] - 2 * PAD, 26);

  const bx = s.x + s.w + PAD;
  buttons.forEach((b, i) => {
    b.clearPath().rectangle(bx, s.y + i * (BUTTON_H + 6), BUTTON_W, BUTTON_H);
    b.setFillColor(...(b.name === effect ? [0.85, 0.3, 0.25] : [0.25, 0.25, 0.3]));
  });
  freeze
    .clearPath()
    .rectangle(bx, s.y + EFFECTS.length * (BUTTON_H + 6) + 12, BUTTON_W, BUTTON_H)
    .setFillColor(...(frozen ? [0.2, 0.5, 0.85] : [0.25, 0.25, 0.3]));
  freeze.text = frozen ? "▶ live" : "❚❚ freeze";

  applyEffect(s);

  const r = Math.max(30, s.h / 6);
  const [cx, cy] = bubbleAt ?? [s.x + s.w - r - 10, s.y + s.h - r - 10];
  bubble.clearPath().circle(cx, cy, r);
}

/** Point the view shapes at the camera the way `effect` wants. */
function applyEffect(s) {
  for (const v of views) {
    v.clearPath().resetTransform().setVideoFilter(null);
    v.video = null;
  }
  const show = (v) => (v.video = cam);
  const main = views[0];
  switch (effect) {
    case "mirror":
      // drawn right to left: x -> s.x + s.w - x
      main.rectangle(0, 0, s.w, s.h).translate(s.x + s.w, s.y).scale(-1, 1);
      show(main);
      break;
    case "tiles": {
      // 2x2, each tile a mirror image of its neighbours
      const [tw, th] = [s.w / 2, s.h / 2];
      for (let i = 0; i < 4; i++) {
        const [col, row] = [i % 2, Math.floor(i / 2)];
        const t = views[i].rectangle(0, 0, tw, th);
        t.translate(s.x + tw * (col + (col ? 1 : 0)), s.y + th * (row + (row ? 1 : 0)));
        t.scale(col ? -1 : 1, row ? -1 : 1);
        show(t);
      }
      break;
    }
    case "kaleidoscope": {
      // six wedges around the centre, every other one mirrored
      const r = Math.min(s.w, s.h) / 2;
      const a = Math.PI / 3;
      for (let i = 0; i < 6; i++) {
        const w = views[i]
          .moveTo(0, 0)
          .lineTo(r, 0)
          .lineTo(r * Math.cos(a), r * Math.sin(a))
          .closePath();
        // a mirrored wedge: reflect, then turn to the far edge of its slot
        w.translate(s.x + s.w / 2, s.y + s.h / 2);
        if (i % 2) w.rotate((i + 1) * a).scale(1, -1);
        else w.rotate(i * a);
        show(w);
      }
      break;
    }
    default:
      // normal, or one of the video filters
      main.rectangle(s.x, s.y, s.w, s.h);
      show(main);
      if (effect !== "normal") main.setVideoFilter(effect);
  }
}

// ---- input -------------------------------------------------------------

export function pointer(ev) {
  if (ev.kind === "press") {
    if (ev.shape === bubble) {
      const [cx, cy] = bubbleCentre();
      drag = [ev.x - cx, ev.y - cy];
    } else if (ev.shape === freeze) {
      frozen = !frozen;
      frozen ? cam.pause() : cam.play();
      window.log(frozen ? "frozen" : "live");
      arrange();
    } else if (ev.shape && EFFECTS.includes(ev.shape.name)) {
      effect = ev.shape.name;
      window.log(`effect ${effect}`);
      arrange();
    }
  } else if (ev.kind === "move" && drag) {
    bubbleAt = [ev.x - drag[0], ev.y - drag[1]];
    arrange();
  } else if (ev.kind === "release") {
    drag = null;
  }
}

function bubbleCentre() {
  const s = stage();
  const r = Math.max(30, s.h / 6);
  return bubbleAt ?? [s.x + s.w - r - 10, s.y + s.h - r - 10];
}

export function update() {
  const what = cam.width ? `${cam.width}×${cam.height}` : "waiting for a picture";
  status.text = `${source} · ${what} · ${effect}${frozen ? " · frozen" : ""}`;
}
