// demos/10server.vala and 10client.vala, ported into one page: it listens
// on a TCP port and connects to itself. The browser asks before either;
// the page never touches a socket itself (the browser does, and relays).
import { Layout, Shape, net, window } from "underweb";
import type { Connection, PointerEvent } from "underweb";

const PORT = Number(window.param("port") ?? 7007);
const text = new TextDecoder();

let serverLog: Shape, clientLog: Shape, button: Shape;
const serverLines: string[] = [];
const clientLines: string[] = [];
let client: Connection | null = null;
let sent = 0;

function panel(layout: Layout, x: number, title: string, color: number[]): Shape {
  const s = new Shape().rectangle(x, 70, 330, 300).setFillColor(color[0], color[1], color[2], 0.15);
  s.font = "Monospace 11";
  s.name = title;
  layout.append(s);
  const head = new Shape().rectangle(x, 30, 330, 40).setFillColor(color[0], color[1], color[2], 0.5);
  head.text = title;
  layout.append(head);
  return s;
}

function say(lines: string[], shape: Shape, line: string): void {
  lines.push(line);
  shape.text = lines.slice(-14).join("\n");
}

export async function activate(): Promise<void> {
  window.title = "TCP chat";
  const layout = new Layout();
  window.addLayout(layout);
  serverLog = panel(layout, 40, `server, port ${PORT}`, [0.9, 0.5, 0.1]);
  clientLog = panel(layout, 400, "client", [0.1, 0.5, 0.9]);
  button = new Shape().rectangle(400, 390, 200, 44).setFillColor(0.1, 0.5, 0.9);
  button.setTextColor(1, 1, 1);
  button.text = "Send a line";
  layout.append(button);

  try {
    const server = await net.listen(PORT);
    say(serverLines, serverLog, "listening");
    server.onconnection = (c) => {
      say(serverLines, serverLog, `connection from ${c.peer}`);
      c.ondata = (d) => {
        const line = text.decode(d);
        say(serverLines, serverLog, `got: ${line}`);
        c.send(`echo: ${line.split("").reverse().join("")}`);
      };
      c.onclose = () => say(serverLines, serverLog, "closed");
    };
  } catch (e) {
    say(serverLines, serverLog, `can't listen: ${(e as Error).message}`);
    return;
  }
  try {
    client = await net.connect("127.0.0.1", PORT);
    say(clientLines, clientLog, "connected");
    client.ondata = (d) => say(clientLines, clientLog, text.decode(d));
  } catch (e) {
    say(clientLines, clientLog, `can't connect: ${(e as Error).message}`);
  }
}

export function pointer(ev: PointerEvent): void {
  if (ev.kind === "press" && ev.shape === button && client) {
    const line = `hello #${++sent}`;
    say(clientLines, clientLog, `sent: ${line}`);
    client.send(line);
  }
}
