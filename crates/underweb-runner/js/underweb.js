// The Underweb page API for JavaScript and TypeScript packages.
// Packages import it as `import { Layout, Shape, window } from "underweb"`.
// Types: sdk/js/underweb.d.ts.

const core = Deno.core;
const ops = core.ops;

const shapes = new Map();

class Layout {
  constructor() {
    this.id = ops.op_uw_layout_new();
  }
  append(shape) {
    ops.op_uw_layout_append(this.id, shape.id);
    return this;
  }
}

class Shape {
  constructor() {
    this.id = ops.op_uw_shape_new();
    this.name = "";
    shapes.set(this.id, this);
  }
  clearPath() { ops.op_uw_shape_clear_path(this.id); return this; }
  rectangle(x, y, w, h) { ops.op_uw_shape_rectangle(this.id, x, y, w, h); return this; }
  moveTo(x, y) { ops.op_uw_shape_move_to(this.id, x, y); return this; }
  lineTo(x, y) { ops.op_uw_shape_line_to(this.id, x, y); return this; }
  curveTo(x1, y1, x2, y2, x3, y3) {
    ops.op_uw_shape_curve_to(this.id, x1, y1, x2, y2, x3, y3);
    return this;
  }
  closePath() { ops.op_uw_shape_close_path(this.id); return this; }
  setFillColor(r, g, b, a = 1) { ops.op_uw_shape_set_fill_color(this.id, r, g, b, a); return this; }
  setStrokeColor(r, g, b, a = 1) { ops.op_uw_shape_set_stroke_color(this.id, r, g, b, a); return this; }
  setTextColor(r, g, b, a = 1) { ops.op_uw_shape_set_text_color(this.id, r, g, b, a); return this; }
  set strokeWidth(w) { ops.op_uw_shape_set_stroke_width(this.id, w); }
  set text(t) { ops.op_uw_shape_set_text(this.id, String(t)); }
  set font(f) { ops.op_uw_shape_set_font(this.id, String(f)); }
}

const window = {
  set title(t) { ops.op_uw_set_title(String(t)); },
  addLayout(layout) { ops.op_uw_add_layout(layout.id); },
  set animating(on) { ops.op_uw_set_animating(!!on); },
  redraw() { ops.op_uw_request_redraw(); },
  get width() { return ops.op_uw_width(); },
  get height() { return ops.op_uw_height(); },
  param(key) { return ops.op_uw_param(String(key)); },
  log(...args) { ops.op_uw_log(args.map(String).join(" ")); },
};

// ---- network ------------------------------------------------------------
// Every request goes to the browser, which checks it against the
// permissions in underweb.toml and asks the user. Requests resolve or
// reject when the browser answers.

const bytes = (d) =>
  d instanceof Uint8Array ? d : typeof d === "string" ? core.encode(d) : new Uint8Array(d);

const pending = new Map(); // handle -> { resolve, reject }
const handles = new Map(); // handle -> Connection | Listener | UdpSocket

function waitFor(handle, value) {
  handles.set(handle, value);
  return new Promise((resolve, reject) => {
    pending.set(handle, { resolve: (v) => resolve(v ?? value), reject });
  });
}

class Connection {
  constructor(handle, peer = "") {
    this.handle = handle;
    this.peer = peer;
    this.ondata = null; // (bytes: Uint8Array) => void
    this.onclose = null;
    this.onerror = null;
  }
  send(data) { ops.op_uw_net_send(this.handle, bytes(data)); }
  close() { ops.op_uw_net_close(this.handle); handles.delete(this.handle); }
}

class Listener {
  constructor(handle) {
    this.handle = handle;
    this.onconnection = null; // (conn: Connection) => void
  }
  close() { ops.op_uw_net_close(this.handle); handles.delete(this.handle); }
}

class UdpSocket {
  constructor(handle) {
    this.handle = handle;
    this.address = "";
    this.ondata = null; // (bytes, from: "ip:port") => void
    this.onerror = null;
  }
  send(host, port, data) { ops.op_uw_net_udp_send(this.handle, String(host), port, bytes(data)); }
  close() { ops.op_uw_net_close(this.handle); handles.delete(this.handle); }
}

class Response {
  constructor(status, headerText, body) {
    this.status = status;
    this.ok = status >= 200 && status < 300;
    this.headers = {};
    for (const line of headerText.split("\n")) {
      const i = line.indexOf(":");
      if (i > 0) this.headers[line.slice(0, i).trim().toLowerCase()] = line.slice(i + 1).trim();
    }
    this.body = body;
  }
  bytes() { return Promise.resolve(this.body); }
  text() { return Promise.resolve(core.decode(this.body)); }
  async json() { return JSON.parse(await this.text()); }
}

const net = {
  /** Open a TCP connection. */
  connect(host, port) {
    const h = ops.op_uw_net_tcp_connect(String(host), port);
    return waitFor(h, new Connection(h));
  },
  /** Accept TCP connections on `port`. */
  listen(port) {
    const h = ops.op_uw_net_tcp_listen(port);
    return waitFor(h, new Listener(h));
  },
  /** A UDP socket on `port`, or an ephemeral port for sending (0). */
  udp(port = 0) {
    const h = ops.op_uw_net_udp_open(port);
    return waitFor(h, new UdpSocket(h));
  },
  /** HTTP(S) request; resolves to a Response. Redirects are not followed. */
  fetch(url, { method = "GET", headers = {}, body = "" } = {}) {
    const h = ops.op_uw_fetch(method, String(url), Object.entries(headers).map(([k, v]) => [k, String(v)]), bytes(body));
    return new Promise((resolve, reject) => pending.set(h, { resolve, reject }));
  },
};

function netEvents() {
  let ev;
  while ((ev = ops.op_uw_net_next()) !== null) {
    const data = ops.op_uw_net_data();
    const obj = handles.get(ev.handle);
    const wait = pending.get(ev.handle);
    switch (ev.kind) {
      case "connected":
        if (obj instanceof UdpSocket) obj.address = ev.text;
        if (obj instanceof Connection) obj.peer = ev.text;
        if (wait) { pending.delete(ev.handle); wait.resolve(); }
        break;
      case "response":
        if (wait) { pending.delete(ev.handle); wait.resolve(new Response(ev.status, ev.text, data)); }
        break;
      case "error":
        if (wait) {
          pending.delete(ev.handle);
          handles.delete(ev.handle);
          wait.reject(new Error(ev.text));
        } else {
          obj?.onerror?.(new Error(ev.text));
        }
        break;
      case "data":
        obj?.ondata?.(data, ev.text);
        break;
      case "closed":
        handles.delete(ev.handle);
        obj?.onclose?.();
        break;
      case "accepted": {
        const conn = new Connection(ev.new_handle, ev.text);
        handles.set(ev.new_handle, conn);
        obj?.onconnection?.(conn);
        break;
      }
    }
  }
}

// ---- storage: strings kept in the package's private directory -----------

const storage = {
  get: (key) => ops.op_uw_storage_get(String(key)),
  set: (key, value) => ops.op_uw_storage_set(String(key), String(value)),
  remove: (key) => ops.op_uw_storage_remove(String(key)),
  keys: () => ops.op_uw_storage_keys(),
};

// ---- audio ----------------------------------------------------------------
// Sound is rendered in this package's process, off the page's thread, and
// played by the browser. Oscillators and Pure Data patches are controlled
// from here; nothing here runs per sample.

class Osc {
  #f; #a; #p;
  constructor(wave = "sine", { freq = 440, amp = 0.2, pan = 0 } = {}) {
    this.#f = freq; this.#a = amp; this.#p = pan;
    this.id = ops.op_uw_osc_new(wave, freq, amp, pan);
  }
  #set() { ops.op_uw_osc_set(this.id, this.#f, this.#a, this.#p); }
  get freq() { return this.#f; }
  set freq(v) { this.#f = v; this.#set(); }
  get amp() { return this.#a; }
  set amp(v) { this.#a = v; this.#set(); }
  get pan() { return this.#p; }
  set pan(v) { this.#p = v; this.#set(); }
  stop() { ops.op_uw_osc_free(this.id); }
}

const pd = {
  /** Open a patch shipped with the package; starts sound. */
  open(path) { ops.op_uw_pd_open(String(path)); },
  /** Send to [r receiver]: a number, a string, an array, or nothing (bang). */
  send(receiver, value = null) { ops.op_uw_pd_send(String(receiver), value ?? null); },
  bang(receiver) { ops.op_uw_pd_send(String(receiver), null); },
  /** Get what Pd sends to [s receiver] in onmessage. */
  subscribe(receiver) { ops.op_uw_pd_subscribe(String(receiver)); },
  onmessage: null, // (receiver, value) => void
};

const audio = {
  /** The browser's sample rate (0: no audio). */
  get rate() { return ops.op_uw_audio_rate(); },
  start() { ops.op_uw_audio_start(); },
  stop() { ops.op_uw_audio_stop(); },
  set volume(v) { ops.op_uw_audio_volume(v); },
  /** "sine" | "saw" | "square" | "triangle" | "noise" */
  osc(wave, opts) { return new Osc(wave, opts); },
  /** The last `frames` frames played, mono, for drawing. */
  scope(frames = 512) {
    const b = ops.op_uw_audio_scope(frames);
    return new Float32Array(b.buffer, b.byteOffset, b.byteLength / 4);
  },
  pd,
};

function pdMessages() {
  for (const [receiver, value] of ops.op_uw_pd_next()) {
    pd.onmessage?.(receiver, value);
  }
}

globalThis.console = {
  log: window.log,
  info: window.log,
  warn: window.log,
  error: window.log,
};

// Called by the runner for each event. `name` is a page export.
function dispatch(name, arg) {
  const page = globalThis.__uw_page;
  const fn = page && page[name];
  if (typeof fn !== "function") return;
  if (name === "pointer") {
    arg.shape = shapes.get(arg.shape) ?? null;
  }
  fn(arg);
}

// an unhandled rejection (say, a refused permission nobody caught) is
// logged instead of stopping the page
core.setUnhandledPromiseRejectionHandler((_promise, reason) => {
  window.log("uncaught (in promise):", reason?.message ?? reason);
  return true;
});

const api = { Layout, Shape, window, net, storage, audio, dispatch, netEvents, pdMessages };
Object.defineProperty(globalThis, "__uw", { value: api });

// Packages get no Deno internals: only the API above.
delete globalThis.Deno;
