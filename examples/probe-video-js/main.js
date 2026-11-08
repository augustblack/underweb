import { Layout, Shape, video, window } from "underweb";

export function activate() {
  window.title = "video probe";
  const layout = new Layout();
  window.addLayout(layout);
  const box = new Shape().rectangle(0, 0, 640, 360).setFillColor(1, 0, 1);
  layout.append(box);

  const src = window.param("src");
  const url = window.param("broadcast");
  const v = url ? video.broadcast(src, url, { password: "hackme" }) : video.open(src, { loop: true });
  v.onready = (e) => window.log(`PROBE ready ${e.width}x${e.height}`);
  v.onended = () => window.log("PROBE ended");
  v.onerror = (m) => window.log("PROBE error " + m);
  v.onbroadcasting = () => window.log("PROBE broadcasting");
  box.video = v;
  v.play();
}
