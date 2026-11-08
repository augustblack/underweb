import { audio, window } from "underweb";

export function activate() {
  window.title = "audio probe";
  window.log("PROBE rate", audio.rate);
  const osc = window.param("osc");
  if (osc) audio.osc("sine", { freq: Number(osc), amp: 0.5, pan: -1 }); // left only
  const pd = window.param("pd");
  if (pd) {
    audio.pd.open("tone.pd");
    audio.pd.send("freq", Number(pd));
    audio.pd.subscribe("pong");
    audio.pd.onmessage = (r, v) => window.log("PROBE", r, v);
    audio.pd.send("ping", 41);
  }
  try {
    audio.pd.open("../../secret.pd");
  } catch (e) {
    window.log("PROBE escape refused");
  }
}
