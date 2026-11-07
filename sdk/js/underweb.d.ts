// Types for the Underweb page API (JavaScript and TypeScript packages).
// The runner provides the module; there are no Deno, Node or DOM APIs.

declare module "underweb" {
  export class Layout {
    constructor();
    append(shape: Shape): this;
  }

  export class Shape {
    constructor();
    name: string;
    clearPath(): this;
    rectangle(x: number, y: number, w: number, h: number): this;
    moveTo(x: number, y: number): this;
    lineTo(x: number, y: number): this;
    curveTo(x1: number, y1: number, x2: number, y2: number, x3: number, y3: number): this;
    closePath(): this;
    setFillColor(r: number, g: number, b: number, a?: number): this;
    setStrokeColor(r: number, g: number, b: number, a?: number): this;
    setTextColor(r: number, g: number, b: number, a?: number): this;
    set strokeWidth(w: number);
    set text(t: string);
    /** A pango font description, e.g. "Serif Bold 18". */
    set font(f: string);
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
  function deactivate(): void;
}
