// demos/03shape.vala, ported. The three shapes were drawn in underweb's
// layout editor; each has text that flows inside its outline.
import { Layout, Matrix, Shape, window } from "underweb";

const hello = "Hello, and welcome to the Underweb. ";

export function activate() {
  window.title = "Shapes";
  const layout = new Layout();
  window.addLayout(layout);

  const red = new Shape()
    .setFillColor(1, 0, 0, 0.5)
    .setTextColor(0, 0, 0, 0.7)
    .setStrokeColor(0, 0, 1, 0.5)
    .moveTo(319, 12)
    .curveTo(404.179932, 13.518524, 479.994019, 64.823013, 462, 110)
    .lineTo(351, 268)
    .lineTo(144, 200)
    .curveTo(144, 200, 157, 18, 319, 12)
    .closePath();
  red.name = "red";
  red.flow = true;
  red.text = hello.repeat(4);
  layout.append(red);

  const green = new Shape()
    .setFillColor(0.073518, 1, 0, 0.462745)
    .setStrokeColor(0, 0, 0, 1)
    .moveTo(135.445099, -113.084386)
    .curveTo(323.668922, 60.874783, 59.992598, 193.855217, 59.992598, 193.855217)
    .lineTo(-62.017593, 47.057692)
    .curveTo(-125.218394, -69.774972, 20.859019, -206.716976, 135.445099, -113.084386)
    .closePath();
  green.name = "green";
  green.matrix = new Matrix(0.260163, 0.965565, -0.965565, 0.260163, 528.571922, 161.63934);
  green.flow = true;
  green.text = hello.repeat(4);
  layout.append(green);

  const blue = new Shape()
    .setFillColor(0.062486, 0, 1, 0.396078)
    .setStrokeColor(0, 0, 0, 1)
    .moveTo(-53.489502, -67.130676)
    .lineTo(151.510498, -0.130676)
    .curveTo(151.510498, -0.130676, 246.510498, 100.869324, 341.510498, 98.869324)
    .lineTo(-51.489502, 168.869324)
    .closePath();
  blue.name = "blue";
  blue.translate(199.489502, 273.130676);
  blue.flow = true;
  blue.text = "Hello and welcome to the Underweb. This is just a placement text for visual layout only.";
  layout.append(blue);
}
