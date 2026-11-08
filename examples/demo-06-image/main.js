// demos/06image.vala, ported. Pictures are package files (the old demo
// loaded some from the web; with a fetch permission, Image.fromBytes does
// that). An image sits at its shape's origin, clipped to the outline; a
// shape with no outline takes the picture's rectangle.
import { Image, Layout, Shape, window } from "underweb";

export function activate() {
  window.title = "Image Example";
  const layout = new Layout().setBackground(1, 0.9, 1);
  window.addLayout(layout);

  const void_ = new Shape().setStrokeColor(0, 0, 0, 0);
  void_.name = "void";
  void_.image = Image.open("void.png");
  layout.append(void_);

  const small = new Shape().setStrokeColor(0, 0, 0, 0).translate(340, 0).scale(0.6);
  small.name = "leap, small";
  small.image = Image.open("leap.jpg");
  layout.append(small);

  const dot = new Shape().circle(255, 265, 60).setFillColor(1, 0, 0, 0.4).setStrokeColor(0, 0, 0, 0.3);
  dot.name = "dot";
  dot.strokeWidth = 2;
  layout.append(dot);

  // the picture shows through a hand-drawn outline, with text flowing in it
  const leap = new Shape()
    .moveTo(76.376571, 132.219565)
    .lineTo(342.910245, 63.379742)
    .curveTo(342.910245, 63.379742, 377.78318, 161.314802, 291.721658, 269.899211)
    .curveTo(214.056217, 352.860024, 78.141694, 405.813734, 78.141694, 405.813734)
    .closePath()
    .setStrokeColor(0, 0, 0, 1)
    .setTextColor(1, 1, 1, 0.8)
    .translate(560, 200)
    .scale(0.8);
  leap.name = "leap";
  leap.strokeWidth = 2;
  leap.font = "Sans Bold 22";
  leap.flow = true;
  leap.image = Image.open("leap.jpg");
  leap.text = "Leap into the void. ".repeat(6);
  layout.append(leap);
}
