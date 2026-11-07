import { Layout, Shape, net, storage, window } from "underweb";
import type { PointerEvent } from "underweb";

let box: Shape;
let button: Shape;

export function activate(): void {
  window.title = "fetch demo";
  const layout = new Layout();
  window.addLayout(layout);

  const visits = Number(storage.get("visits") ?? "0") + 1;
  storage.set("visits", String(visits));

  box = new Shape().rectangle(60, 60, 600, 160).setFillColor(0.97, 0.97, 1);
  box.text = `This page may load data from api.github.com, if you allow it.\n\nVisit number ${visits} (kept in the page's private storage).`;
  layout.append(box);

  button = new Shape().rectangle(60, 250, 220, 50).setFillColor(0.2, 0.5, 0.9);
  button.setTextColor(1, 1, 1);
  button.text = "Fetch another";
  layout.append(button);
  load();
}

export function pointer(ev: PointerEvent): void {
  if (ev.kind === "press" && ev.shape === button) load();
}

async function load(): Promise<void> {
  box.text = "Fetching… (the browser asks you first)";
  try {
    const r = await net.fetch("https://api.github.com/zen");
    box.text = r.ok ? `“${await r.text()}”` : `The server said ${r.status}.`;
  } catch (e) {
    box.text = `Not allowed or failed:\n${(e as Error).message}`;
  }
}
