// demos/12videoencode.vala, ported. The page shows a source and sends it,
// as Ogg Theora, to an Icecast server; the browser makes the connection
// (net.tcp.connect in underweb.toml) and asks first. A second shape plays
// the stream back from the server once it is up.
//
// Parameters: url (default http://localhost:8000/underweb.ogg), password,
// source ("camera", or a test pattern: test:ball, test:smpte, ...).
import { Layout, Shape, video, window } from "underweb";

const url = window.param("url") ?? "http://localhost:8000/underweb.ogg";
const password = window.param("password") ?? "hackme";
const source = window.param("source") ?? "test:ball";

let status;

export function activate() {
  window.title = "Broadcast";
  const layout = new Layout();
  window.addLayout(layout);

  const local = new Shape().rectangle(0, 0, 400, 300).translate(30, 60).scale(0.9);
  local.name = "sending";
  layout.append(local);
  status = new Shape().rectangle(30, 10, 700, 40).setStrokeColor(0, 0, 0, 0);
  status.text = `Sending ${source} to ${url}...`;
  layout.append(status);

  const out = video.broadcast(source, url, { password });
  local.video = out;
  out.onerror = (m) => (status.text = `Not sending: ${m}`);
  out.onbroadcasting = () => {
    status.text = `On the air at ${url}. The right side plays it back from the server.`;
    // what listeners get: the stream, from the server (a fetch permission)
    const back = new Shape().circle(570, 200, 140);
    back.name = "received";
    back.video = video.open(url);
    layout.append(back);
  };
}
