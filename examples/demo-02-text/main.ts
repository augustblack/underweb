// demos/02shape.vala, ported: a shape whose text is Pango markup, in
// several languages and scripts ("I can eat glass, it doesn't hurt me").
import { Layout, Shape, window } from "underweb";

const glass = [
  "Hello there.",
  '<span lang="is" foreground="#0000ff">Ég get etið gler án þess að meiða mig.</span>',
  '<span foreground="#ff0000" background="#000000" lang="ar">أنا قادر على أكل الزجاج و هذا لا يؤلمني.</span>',
  '<span lang="hi">मैं काँच खा सकता हूँ, मुझे उस से कोई पीडा नहीं होती.</span>',
  "",
  '<span lang="yi">איך קען עסן גלאָז און עס טוט מיר נישט װײ</span>',
  '<span lang="zh">我能吞下玻璃而不伤身体。</span>',
  '<span lang="ja">私はガラスを食べられます。それは私を傷つけません。</span>',
  '<span lang="ko">나는 유리를 먹을 수 있어요. 그래도 아프지 않아요</span>',
  '<span lang="th">ฉันกินกระจกได้ แต่มันไม่ทำให้ฉันเจ็บ</span>',
  "",
  "<b>Bold</b>, <i>italic</i>, <tt>monospace</tt>, <s>struck</s>, H<sub>2</sub>O and <big>big</big>.",
].join("\n");

export function activate(): void {
  window.title = "Markup";
  const layout = new Layout();
  window.addLayout(layout);

  const s = new Shape()
    .rectangle(100, 60, 420, 400)
    .setFillColor(1, 0, 0, 0.5)
    .setStrokeColor(0, 0, 1, 0.5)
    .setTextColor(0, 0, 0, 0.7);
  s.name = "My text shape";
  s.markup = true;
  s.text = glass;
  layout.append(s);
}
