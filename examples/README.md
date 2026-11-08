# Examples

Run any of these by name: `cargo run -p underweb-app -- demo-06-image`
(after `cargo xtask examples`, which builds the native ones). Add `--edit`
to open one in the layout editor.

## The 2011 demos, ported

| Old demo | Package | Language | Shows |
|---|---|---|---|
| 01shape | `hello-js`, `hello-ts`, `hello-rust`, `hello-c` | all four | a shape with text, clicks |
| 02shape | `demo-02-text` | TS | Pango markup, many scripts |
| 03shape | `demo-03-shapes` | JS | text flowing inside curved, rotated shapes |
| 04shape | `demo-04-pointer` | JS | pointer events on a rotated shape, `toLocal` |
| 05animation | `demo-05-animation` | TS | `update(dt)` every display frame |
| 06image | `demo-06-image` | JS | PNG and JPEG pictures, scaled and clipped |
| 07videoplay | `hello-video` | JS | video inside shapes |
| 08draw_cb | `demo-08-draw` | Rust | raw cairo drawing over shapes |
| 09widgets | `demo-09-sliders` | JS | sliders made of shapes |
| 10server, 10client | `demo-10-chat` | TS | a TCP server and client, with permissions |
| 11audio | `hello-audio` | TS | oscillators and the scope |
| 12videoencode | `demo-12-broadcast` | JS | sending video to Icecast (`?url=…&password=…`) |
| 13gui | `demo-13-tree` | JS | a tree list made of shapes |
| 15puredata | `hello-pd` | JS | a Pure Data patch |

Not ported: 14browser (a WebKit page; that waits for the CEF phase) and
16underweb (the old browser itself).

## Others

`hello-net` fetches from GitHub (it asks first). `probe-*` packages are
for the tests: they try things a sandboxed package must not be able to do.
