import { window } from "underweb";

const report = (name, allowed, detail = "") =>
  window.log(`PROBE ${name} ${allowed ? "allowed" : "denied"} ${detail}`);

export async function activate() {
  window.title = "sandbox probe (js)";
  for (const name of ["Deno", "process", "require", "fetch", "XMLHttpRequest", "WebSocket", "Bun"]) {
    report(`global_${name}`, typeof globalThis[name] !== "undefined", typeof globalThis[name]);
  }
  const secret = window.param("secret");
  if (secret) {
    try {
      await import("file://" + secret);
      report("import_outside", true);
    } catch (e) {
      report("import_outside", false, String(e).split("\n")[0]);
    }
  }
  try {
    await import("https://example.com/x.js");
    report("import_remote", true);
  } catch (e) {
    report("import_remote", false, String(e).split("\n")[0]);
  }
  window.log("PROBE done");
}
