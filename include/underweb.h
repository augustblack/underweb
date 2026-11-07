#ifndef UNDERWEB_H
#define UNDERWEB_H

/* Generated from crates/underweb-abi by `cargo xtask header`. Do not edit. */

#include <stdbool.h>
#include <stdint.h>

// Bumped on any incompatible change to [`UwHost`] or the exports.
#define UW_ABI_VERSION 1

#define UW_POINTER_MOVE 0

#define UW_POINTER_PRESS 1

#define UW_POINTER_RELEASE 2

// The function table the runner hands to a native package.
// `ctx` must be passed back as the first argument of every call.
typedef struct UwHost {
  uint32_t abi_version;
  void *ctx;
  void (*set_title)(void *ctx, const char *title);
  void (*add_layout)(void *ctx, uint32_t layout);
  // Ask for `uw_update` every frame (animation) or only on events.
  void (*set_animating)(void *ctx, bool on);
  void (*request_redraw)(void *ctx);
  double (*width)(void *ctx);
  double (*height)(void *ctx);
  // Value of a `?key=value` parameter from the package URL, or NULL.
  // The string stays valid until the next call to `param`.
  const char *(*param)(void *ctx, const char *key);
  void (*log)(void *ctx, const char *message);
  uint32_t (*layout_new)(void *ctx);
  void (*layout_append)(void *ctx, uint32_t layout, uint32_t shape);
  uint32_t (*shape_new)(void *ctx);
  void (*shape_clear_path)(void *ctx, uint32_t shape);
  void (*shape_rectangle)(void *ctx, uint32_t shape, double x, double y, double w, double h);
  void (*shape_move_to)(void *ctx, uint32_t shape, double x, double y);
  void (*shape_line_to)(void *ctx, uint32_t shape, double x, double y);
  void (*shape_curve_to)(void *ctx,
                         uint32_t shape,
                         double x1,
                         double y1,
                         double x2,
                         double y2,
                         double x3,
                         double y3);
  void (*shape_close_path)(void *ctx, uint32_t shape);
  void (*shape_set_fill_color)(void *ctx, uint32_t shape, double r, double g, double b, double a);
  void (*shape_set_stroke_color)(void *ctx, uint32_t shape, double r, double g, double b, double a);
  void (*shape_set_stroke_width)(void *ctx, uint32_t shape, double width);
  void (*shape_set_text)(void *ctx, uint32_t shape, const char *text);
  void (*shape_set_text_color)(void *ctx, uint32_t shape, double r, double g, double b, double a);
  // Pango font description, e.g. "Sans Bold 14".
  void (*shape_set_font)(void *ctx, uint32_t shape, const char *font);
} UwHost;

typedef struct UwPointerEvent {
  // `UW_POINTER_MOVE`, `UW_POINTER_PRESS` or `UW_POINTER_RELEASE`.
  uint32_t kind;
  // Mouse button, 1 = primary. 0 for moves.
  uint32_t button;
  // Position in logical pixels.
  double x;
  double y;
  // Topmost shape under the pointer, 0 if none.
  uint32_t shape;
} UwPointerEvent;

/* A native package exports these. Only uw_abi_version and uw_activate are
 * required. uw_draw gets a cairo_t* (include <cairo.h> to use it). */
uint32_t uw_abi_version(void);
void uw_activate(const UwHost *host);
void uw_update(const UwHost *host, double dt);
void uw_pointer(const UwHost *host, const UwPointerEvent *ev);
void uw_draw(const UwHost *host, void *cr, double w, double h);
void uw_deactivate(const UwHost *host);

/* Convenience: call a host function with its context, e.g.
 *   UW(host, set_title, "hello");                                       */
#define UW(host, fn, ...) ((host)->fn((host)->ctx, ##__VA_ARGS__))

#endif /* UNDERWEB_H */
