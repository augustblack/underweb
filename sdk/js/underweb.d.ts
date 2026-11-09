// Types for the Underweb page API (JavaScript and TypeScript packages).
// The runner provides the module; there are no Deno, Node or DOM APIs.

declare module "underweb" {
  export class Layout {
    constructor();
    append(shape: Shape): this;
    /** Paint the window under this layout's shapes; no arguments for none. */
    setBackground(r?: number, g?: number, b?: number, a?: number): this;
  }

  /**
   * An affine transform, as cairo_matrix_t: (x, y) lands on
   * (xx*x + xy*y + x0, yx*x + yy*y + y0). Methods return new matrices; like
   * cairo, each step happens in the coordinates the earlier ones made.
   */
  export class Matrix {
    constructor(xx?: number, yx?: number, xy?: number, yy?: number, x0?: number, y0?: number);
    readonly xx: number;
    readonly yx: number;
    readonly xy: number;
    readonly yy: number;
    readonly x0: number;
    readonly y0: number;
    multiply(first: Matrix): Matrix;
    translate(x: number, y: number): Matrix;
    scale(sx: number, sy?: number): Matrix;
    /** Radians, clockwise on screen. */
    rotate(angle: number): Matrix;
    apply(x: number, y: number): [number, number];
    invert(): Matrix | null;
  }

  /** A decoded picture (PNG, JPEG or GIF). */
  export class Image {
    readonly id: number;
    readonly width: number;
    readonly height: number;
    /** A file in the package; throws if it can't be read or decoded. */
    static open(path: string): Image;
    static fromBytes(data: Uint8Array | ArrayBuffer): Image;
  }

  export class Shape {
    constructor();
    /** Shown in the layout editor. */
    name: string;
    clearPath(): this;
    rectangle(x: number, y: number, w: number, h: number): this;
    moveTo(x: number, y: number): this;
    lineTo(x: number, y: number): this;
    curveTo(x1: number, y1: number, x2: number, y2: number, x3: number, y3: number): this;
    closePath(): this;
    /** A circle made of four Bezier arcs. */
    circle(cx: number, cy: number, r: number): this;
    setFillColor(r: number, g: number, b: number, a?: number): this;
    setStrokeColor(r: number, g: number, b: number, a?: number): this;
    setTextColor(r: number, g: number, b: number, a?: number): this;
    set strokeWidth(w: number);
    set text(t: string);
    /** A pango font description, e.g. "Serif Bold 18". */
    set font(f: string);
    /** The text is Pango markup (<b>, <span foreground="red">, ...). */
    markup: boolean;
    /** The text flows word by word inside the outline, not its box. */
    flow: boolean;
    /** Drawn at the shape's origin, clipped to its outline. A shape with
     * no outline gets the image's rectangle. */
    image: Image | null;
    /** From the shape's own coordinates to the window's. */
    matrix: Matrix;
    translate(x: number, y: number): this;
    scale(sx: number, sy?: number): this;
    rotate(angle: number): this;
    resetTransform(): this;
    /** A window point in the shape's own coordinates. */
    toLocal(x: number, y: number): [number, number];
    /** Show a video inside the outline; null to stop. */
    set video(v: Video | null);
    /**
     * Show the video through an effect; null for none. `amount` is the
     * block size for "pixelate" (default 12) and the levels per channel
     * for "posterize" (default 4).
     */
    setVideoFilter(
      name: "grayscale" | "invert" | "pixelate" | "posterize" | null,
      amount?: number,
    ): this;
  }

  export interface PointerEvent {
    kind: "move" | "press" | "release";
    button: number;
    x: number;
    y: number;
    /** Topmost shape under the pointer. */
    shape: Shape | null;
  }

  export class Connection {
    readonly peer: string;
    ondata: ((data: Uint8Array) => void) | null;
    onclose: (() => void) | null;
    onerror: ((e: Error) => void) | null;
    send(data: string | Uint8Array): void;
    close(): void;
  }

  export class Listener {
    onconnection: ((conn: Connection) => void) | null;
    close(): void;
  }

  export class UdpSocket {
    readonly address: string;
    ondata: ((data: Uint8Array, from: string) => void) | null;
    onerror: ((e: Error) => void) | null;
    send(host: string, port: number, data: string | Uint8Array): void;
    close(): void;
  }

  export class Response {
    readonly status: number;
    readonly ok: boolean;
    /** Lower-case header names. */
    readonly headers: Record<string, string>;
    bytes(): Promise<Uint8Array>;
    text(): Promise<string>;
    json(): Promise<unknown>;
  }

  /**
   * Networking through the browser. Each call needs a permission declared
   * in underweb.toml and granted by the user; refused or failed requests
   * reject.
   */
  export const net: {
    connect(host: string, port: number): Promise<Connection>;
    listen(port: number): Promise<Listener>;
    /** Port 0 (the default): an ephemeral port for sending. */
    udp(port?: number): Promise<UdpSocket>;
    /** Redirects are not followed. */
    fetch(
      url: string,
      init?: { method?: string; headers?: Record<string, string>; body?: string | Uint8Array },
    ): Promise<Response>;
  };

  /** Strings kept in the package's private storage. */
  export const storage: {
    get(key: string): string | null;
    set(key: string, value: string): void;
    remove(key: string): void;
    keys(): string[];
  };

  export type Wave = "sine" | "saw" | "square" | "triangle" | "noise";
  export type PdValue = null | number | string | (number | string)[];

  export class Osc {
    freq: number;
    amp: number;
    /** -1 (left) to 1 (right). */
    pan: number;
    stop(): void;
  }

  /** Sound, rendered in the package's process and played by the browser. */
  export const audio: {
    /** The browser's sample rate (0: no audio). */
    readonly rate: number;
    start(): void;
    stop(): void;
    set volume(v: number);
    osc(wave?: Wave, opts?: { freq?: number; amp?: number; pan?: number }): Osc;
    /** The last `frames` frames played, mono, for drawing. */
    scope(frames?: number): Float32Array;
    pd: {
      /** Open a patch shipped with the package; starts sound. */
      open(path: string): void;
      /** To [r receiver]; null or nothing is a bang. */
      send(receiver: string, value?: PdValue): void;
      bang(receiver: string): void;
      /** Get what Pd sends to [s receiver] in onmessage. */
      subscribe(receiver: string): void;
      onmessage: ((receiver: string, value: PdValue) => void) | null;
    };
  };

  export class Video {
    readonly id: number;
    onready: ((info: { width: number; height: number; duration: number }) => void) | null;
    onended: (() => void) | null;
    onerror: ((message: string) => void) | null;
    onbroadcasting: (() => void) | null;
    play(): this;
    pause(): this;
    seek(seconds: number): this;
    close(): void;
    readonly width: number;
    readonly height: number;
    /** Seconds. */
    readonly position: number;
    /** Seconds; 0 for live sources. */
    readonly duration: number;
  }

  export const video: {
    /**
     * A package file ("clip.webm"), an http(s) URL (needs a fetch
     * permission), "camera" (needs the camera permission), or
     * "test:<pattern>" (smpte, ball, snow, ...).
     */
    open(source: string, opts?: { loop?: boolean }): Video;
    /** Show `source` and send it as Ogg Theora to an Icecast server,
     * "http://host:port/mount.ogg" (needs net.tcp.connect). */
    broadcast(source: string, url: string, opts?: { password?: string }): Video;
  };

  export const window: {
    set title(t: string);
    addLayout(layout: Layout): void;
    /** While true, the page's `update(dt)` runs every display frame. */
    set animating(on: boolean);
    redraw(): void;
    readonly width: number;
    readonly height: number;
    /** A `?key=value` parameter from the package URL. */
    param(key: string): string | null;
    log(...args: unknown[]): void;
  };
}

/** Page modules may export any of these. */
declare namespace UnderwebPage {
  function activate(): void;
  function update(dt: number): void;
  function pointer(ev: import("underweb").PointerEvent): void;
  /** The window's size in logical pixels: once at start, then on every
   * change (also in `window.width` / `window.height`). */
  function resize(width: number, height: number): void;
  function deactivate(): void;
}
