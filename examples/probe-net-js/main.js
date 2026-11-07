// Exercises storage, TCP, UDP and fetch, logging one "PROBE" line per
// step. Ports come from ?tcp=&udp=&http= parameters.
import { net, storage, window } from "underweb";

const log = (...a) => window.log("PROBE", ...a);
const text = (b) => new TextDecoderLite().decode(b);

// deno_core has no TextDecoder; the bytes here are ASCII
class TextDecoderLite {
  decode(b) { return String.fromCharCode(...b); }
}

async function step(name, f) {
  try {
    await f();
  } catch (e) {
    log(name, "error", e.message);
  }
}

export async function activate() {
  window.title = "net probe (js)";
  const n = Number(storage.get("count") ?? "0") + 1;
  storage.set("count", String(n));
  log("storage", n, storage.keys().join(","));

  await step("tcp", async () => {
    const c = await net.connect("127.0.0.1", Number(window.param("tcp")));
    const reply = new Promise((resolve) => (c.ondata = resolve));
    c.send("hello");
    log("tcp", text(await reply));
    c.close();
  });

  await step("udp", async () => {
    const u = await net.udp();
    const reply = new Promise((resolve, reject) => {
      u.ondata = (d) => resolve(text(d));
      u.onerror = reject;
    });
    u.send("127.0.0.1", Number(window.param("udp")), "ping");
    log("udp", await reply);
    u.close();
  });

  await step("fetch", async () => {
    const r = await net.fetch(`http://127.0.0.1:${window.param("http")}/hello.txt`);
    log("fetch", r.status, await r.text());
  });

  await step("undeclared", async () => {
    await net.connect("127.0.0.2", 1);
    log("undeclared", "allowed");
  });

  log("done");
}
