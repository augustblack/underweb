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
