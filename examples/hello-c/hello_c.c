/* A C package: a row of circles, each a shape made of Bezier curves.
 * Clicking one turns it red. */
#include <stdio.h>
#include "underweb.h"

#define N 5
static uint32_t circles[N];

/* a circle from four Bezier quarter arcs */
static void circle(const UwHost *h, uint32_t s, double cx, double cy, double r)
{
    const double k = 0.5523 * r;
    UW(h, shape_move_to, s, cx + r, cy);
    UW(h, shape_curve_to, s, cx + r, cy + k, cx + k, cy + r, cx, cy + r);
    UW(h, shape_curve_to, s, cx - k, cy + r, cx - r, cy + k, cx - r, cy);
    UW(h, shape_curve_to, s, cx - r, cy - k, cx - k, cy - r, cx, cy - r);
    UW(h, shape_curve_to, s, cx + k, cy - r, cx + r, cy - k, cx + r, cy);
    UW(h, shape_close_path, s);
}

uint32_t uw_abi_version(void) { return UW_ABI_VERSION; }

void uw_activate(const UwHost *h)
{
    UW(h, set_title, "C circles");
    uint32_t layout = UW(h, layout_new);
    UW(h, add_layout, layout);

    uint32_t label = UW(h, shape_new);
    UW(h, shape_rectangle, label, 60, 60, 560, 60);
    UW(h, shape_set_fill_color, label, 0.9, 0.95, 1, 1);
    UW(h, shape_set_text, label, "Hello from C. Click a circle.");
    UW(h, layout_append, layout, label);

    for (int i = 0; i < N; i++) {
        circles[i] = UW(h, shape_new);
        circle(h, circles[i], 110 + i * 115, 230, 50);
        UW(h, shape_set_fill_color, circles[i], 0.2, 0.2 + 0.15 * i, 0.9, 1);
        UW(h, shape_set_stroke_width, circles[i], 3);
        UW(h, layout_append, layout, circles[i]);
    }
}

void uw_pointer(const UwHost *h, const UwPointerEvent *ev)
{
    if (ev->kind != UW_POINTER_PRESS)
        return;
    for (int i = 0; i < N; i++) {
        if (ev->shape == circles[i]) {
            char msg[64];
            UW(h, shape_set_fill_color, circles[i], 0.9, 0.1, 0.1, 1);
            snprintf(msg, sizeof msg, "circle %d clicked", i);
            UW(h, log, msg);
        }
    }
}
