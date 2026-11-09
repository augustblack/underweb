// Send the live camera to an Icecast server, as Ogg Theora, and watch it
// come back. Press "Go live" to start and "Stop" to end. The preview is
// mirrored (as a mirror is what people expect of themselves); listeners
// get the camera as it is. The browser asks before connecting
// (net.tcp.connect in underweb.toml) and before using the camera.
//
// Parameters:
//   url       where to send, http://host:port/mount.ogg
//             (default http://127.0.0.1:8000/camera.ogg: a local server,
//             so nothing goes out unless you say so)
//   password  the server's source password (default "hackme")
//   source    "camera", or a test pattern: test:ball, test:smpte, ...
//   live      1 to go live at once
import { Layout, Shape, video, window } from "underweb";

const url = window.param("url") ?? "http://127.0.0.1:8000/camera.ogg";
const password = window.param("password") ?? "hackme";
const source = window.param("source") ?? "camera";

// idle: the preview only; starting: connecting; live: on the air
let state = "idle";
let preview, sending, back;
let view, button, dot, status, watch, note;
let size = [800, 600];
let message = "Off the air. Press Go live to send the camera.";

export function activate() {
  window.title = "Camera broadcast";
  const layout = new Layout().setBackground(0.1, 0.1, 0.12);
  window.addLayout(layout);

  view = new Shape().setFillColor(0, 0, 0).setStrokeColor(0, 0, 0, 0);
  view.name = "camera";
  layout.append(view);
  watch = new Shape().setFillColor(0.2, 0.2, 0.22).setStrokeColor(1, 1, 1);
  watch.strokeWidth = 3;
  watch.name = "from the server";
  layout.append(watch);

  button = new Shape().setStrokeColor(0, 0, 0, 0).setTextColor(1, 1, 1);
  button.name = "go live";
  button.font = "Sans Bold 15";
  layout.append(button);
  dot = new Shape().setStrokeColor(0, 0, 0, 0);
  layout.append(dot);
  status = new Shape().setFillColor(0, 0, 0, 0).setStrokeColor(0, 0, 0, 0)
    .setTextColor(0.9, 0.9, 0.9);
  status.font = "Sans 13";
  layout.append(status);
  note = new Shape().setFillColor(0, 0, 0, 0).setStrokeColor(0, 0, 0, 0)
    .setTextColor(0.6, 0.6, 0.65);
  note.font = "Sans 11";
  note.text = `Sends ${source} to ${url}. The preview is mirrored; listeners see the camera as it is.`;
  layout.append(note);

  showPreview();
  say(message);
  arrange();
  if (window.param("live") === "1") goLive();
}

export function resize(w, h) {
  size = [w, h];
  arrange();
}

// ---- states --------------------------------------------------------------

function showPreview() {
  preview = video.open(source).play();
  preview.onerror = (m) => say(`No picture from ${source}: ${m}`);
  view.video = preview;
}

function goLive() {
  // one camera at a time: the broadcast shows what it sends
  preview?.close();
  preview = null;
  state = "starting";
  say(`Connecting to ${url}...`);
  sending = video.broadcast(source, url, { password }).play();
  view.video = sending;
  sending.onbroadcasting = () => {
    state = "live";
    say(`On the air at ${url}`);
    window.log("broadcasting");
    // what listeners get, played back from the server
    back = video.open(url).play();
    back.onerror = (m) => say(`On the air; playback from the server failed: ${m}`);
    watch.video = back;
    arrange();
  };
  sending.onerror = (m) => {
    window.log(`error ${m}`);
    stop();
    say(`Not sending: ${m}`);
  };
  arrange();
}

function stop() {
  sending?.close();
  back?.close();
  sending = back = null;
  watch.video = null;
  state = "idle";
  say("Off the air.");
  showPreview();
  arrange();
}

function say(text) {
  message = text;
  status.text = text;
}

// ---- layout ----------------------------------------------------------------

function arrange() {
  const [w, h] = size;
  const pad = 20;
  // the camera: 4:3, as large as fits beside the playback circle
  const vw = Math.max(160, Math.min(w * 0.62, (h - 140) * 4 / 3));
  const vh = vw * 3 / 4;
  // drawn right to left: a mirror
  view.clearPath().resetTransform().rectangle(0, 0, vw, vh).translate(pad + vw, pad).scale(-1, 1);

  const r = Math.max(50, Math.min(w - vw - 3 * pad, vh) / 2);
  watch.clearPath().circle(pad + vw + pad + r, pad + vh / 2, r);

  const y = pad + vh + pad;
  const live = state !== "idle";
  button.clearPath().rectangle(pad, y, 150, 44)
    .setFillColor(...(live ? [0.35, 0.35, 0.4] : [0.8, 0.15, 0.15]));
  button.text = live ? "■ Stop" : "● Go live";
  dot.clearPath().circle(pad + 175, y + 22, 9)
    .setFillColor(...(state === "live" ? [1, 0.1, 0.1] : state === "starting" ? [1, 0.7, 0.1] : [0.4, 0.4, 0.4]));
  status.clearPath().rectangle(pad + 192, y + 4, w - pad * 2 - 192, 40);
  note.clearPath().rectangle(pad, y + 56, w - 2 * pad, 40);
}

export function pointer(ev) {
  if (ev.kind === "press" && ev.shape === button) {
    state === "idle" ? goLive() : stop();
  }
}
