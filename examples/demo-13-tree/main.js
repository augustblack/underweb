// demos/13gui.vala, ported. The old demo was a GTK TreeView; this one is
// drawn with shapes and laid out again whenever a row opens or closes.
import { Layout, Shape, window } from "underweb";

const tree = {
  name: "All Products",
  open: true,
  children: [
    {
      name: "Books",
      open: true,
      children: [
        ["Moby Dick", "$10.36"],
        ["Heart of Darkness", "$4.99"],
        ["Ulysses", "$26.09"],
        ["Effective Vala", "$38.99"],
      ],
    },
    {
      name: "Films",
      open: true,
      children: [
        ["Amores Perros", "$7.99"],
        ["Twin Peaks", "$14.99"],
        ["Vertigo", "$20.49"],
      ],
    },
  ],
};

const ROW = 30, X = 40, Y = 40;
let layout;
const rows = []; // shapes in display order, reused
const hits = new Map(); // row shape -> node it toggles

function row(i) {
  while (rows.length <= i) {
    const name = new Shape().setStrokeColor(0, 0, 0, 0);
    const price = new Shape().setStrokeColor(0, 0, 0, 0);
    name.font = price.font = "Sans 11";
    layout.append(name).append(price);
    rows.push([name, price]);
  }
  return rows[i];
}

function render() {
  const lines = [];
  const walk = (node, depth) => {
    if (Array.isArray(node)) {
      lines.push({ depth, label: node[0], price: node[1] });
      return;
    }
    lines.push({ depth, label: `${node.open ? "▾" : "▸"} ${node.name}`, node });
    if (node.open) node.children.forEach((c) => walk(c, depth + 1));
  };
  walk(tree, 0);
  hits.clear();
  lines.forEach((l, i) => {
    const [name, price] = row(i);
    const y = Y + ROW + i * ROW;
    name.clearPath().rectangle(X, y, 360, ROW).setFillColor(i % 2 ? 0.96 : 0.9, 0.93, 1);
    name.text = `${"    ".repeat(l.depth)}${l.label}`;
    price.clearPath().rectangle(X + 360, y, 140, ROW).setFillColor(i % 2 ? 0.96 : 0.9, 0.93, 1);
    price.text = l.price ?? "";
    if (l.node) hits.set(name, l.node);
  });
  // rows left over from a longer list
  for (let i = lines.length; i < rows.length; i++) {
    for (const s of rows[i]) {
      s.clearPath();
      s.text = "";
    }
  }
}

export function activate() {
  window.title = "Tree";
  layout = new Layout();
  window.addLayout(layout);
  const head = new Shape().rectangle(X, Y, 360, ROW).setFillColor(0.3, 0.4, 0.6);
  head.setTextColor(1, 1, 1);
  head.text = "Product";
  const price = new Shape().rectangle(X + 360, Y, 140, ROW).setFillColor(0.3, 0.4, 0.6);
  price.setTextColor(1, 1, 1);
  price.text = "Price";
  layout.append(head).append(price);
  for (const s of [head, price]) s.font = "Sans Bold 11";
  render();
}

export function pointer(ev) {
  const node = ev.kind === "press" && hits.get(ev.shape);
  if (node) {
    node.open = !node.open;
    render();
  }
}
