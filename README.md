# Underweb

A browser for **packages** instead of web pages. Packages are written in
JavaScript, TypeScript, Rust or C, run natively (no wasm), and draw on a
vector canvas built on cairo and pango. Every package runs in its own
sandboxed process.

This is a Rust rewrite of the 2011 Vala project. See
[docs/ROADMAP.md](docs/ROADMAP.md) for the design and phases.

## Status: milestone 1

Working on macOS:

- the browser: tabs, an address bar, a sandbox badge, a log panel
- one `underweb-runner` process per tab, sandboxed before any package code
  loads (macOS: `sandbox_init`; Linux: Landlock + seccomp, tested in CI only)
- JS and TS packages in V8 (deno_core), with TypeScript transpiled by
  deno_ast. They get the Underweb API and nothing else: no Deno, Node or
  fetch.
- Rust and C packages as prebuilt shared libraries behind a C ABI
  ([include/underweb.h](include/underweb.h)), with optional raw cairo
  drawing
- layouts and shapes (paths, fill, stroke, pango text), hit testing, pointer
  events, animation driven by the display's frame rate, and HiDPI
- shared-memory frames, so a crashing package only takes its own tab down

Not yet: package archives, `uw://` addresses, signing (phase 2);
permissions and networking (3); trust (4); CEF web pages (5); audio and Pd
(6); video (7); the layout editor (8); Windows (9).

## Build and run

Needs Rust ≥ 1.91 and cairo and pango:

```sh
brew install cairo pango pkgconf                         # macOS
sudo apt install libcairo2-dev libpango1.0-dev pkg-config \
  libxkbcommon-dev libwayland-dev                         # Debian/Ubuntu

cargo xtask examples          # builds the runner and the native example packages
cargo run -p underweb-app -- hello-js hello-ts hello-rust hello-c
```

Addresses are package directories, optionally with parameters
(`examples/probe-rust?crash=1`). `--no-sandbox` turns the sandbox off for
debugging, and the badge turns red.

`cargo xtask test` builds everything and runs all tests, including the
sandbox tests: a probe package tries to read your files, write outside its
folder, open a TCP connection and run `/bin/sh`, and every attempt must
fail. The same probe with the sandbox off must succeed.

## Writing a package

Every package is a directory with an `underweb.toml`:

```toml
[package]
name = "my-page"
version = "0.1.0"
kind = "ts"          # js | ts | rust | c
entry = "main.ts"    # js/ts only; native packages list [targets.<triple>]
```

```ts
import { Layout, Shape, window } from "underweb";   // types: sdk/js/underweb.d.ts

export function activate() {
  const layout = new Layout();
  window.addLayout(layout);
  const s = new Shape().rectangle(100, 100, 300, 200).setFillColor(1, 0, 0);
  s.text = "Hello";
  layout.append(s);
}
export function update(dt: number) {}   // every frame while window.animating
export function pointer(ev) {}          // ev.shape is the shape under the pointer
```

See `examples/` for the same idea in Rust (the `underweb` crate and
`export_page!`) and in C.

## Layout

| Path | What |
|---|---|
| `apps/underweb-app` | the browser (eframe/egui on wgpu) |
| `crates/underweb-core` | the engine as a library: spawns runners, receives frames |
| `crates/underweb-runner` | the per-package process: V8 or native loader, frame loop |
| `crates/underweb-guest` | the page API (scene of layouts and shapes) and its cairo/pango renderer |
| `crates/underweb-sandbox` | per-OS lockdown |
| `crates/underweb-ipc` | socket messages with fd passing, shared memory |
| `crates/underweb-abi` | the C ABI, source of `include/underweb.h` |
| `crates/underweb` | SDK for Rust packages |
| `crates/underweb-pkg` | `underweb.toml` |
| `sdk/js` | TypeScript types for JS/TS packages |
| `xtask` | `cargo xtask examples / header / test` |

## License

GPL-3.0-or-later, like the original (to be confirmed before a release).
