#ifndef UNDERWEB_H
#define UNDERWEB_H

/* Generated from crates/underweb-abi by `cargo xtask header`. Do not edit. */

#include <stdbool.h>
#include <stdint.h>

// The ABI this header describes. Version 2 appended network and storage
// functions to [`UwHost`]; packages built for version 1 still load.
#define UW_ABI_VERSION 2

// The oldest ABI the runner still accepts.
#define UW_ABI_MIN_VERSION 1

#define UW_POINTER_MOVE 0

#define UW_POINTER_PRESS 1

#define UW_POINTER_RELEASE 2

// [`UwNetEvent::kind`]: a TCP connection or listener is up, or a UDP
// socket is open; `text` is the address.
#define UW_NET_CONNECTED 0

// Bytes arrived in `data`/`len`. For UDP, `text` is the sender.
#define UW_NET_DATA 1

#define UW_NET_CLOSED 2

// The request failed or was refused; `text` says why.
#define UW_NET_ERROR 3

// A listener accepted a connection, `new_handle`; `text` is the peer.
#define UW_NET_ACCEPTED 4

// A fetch finished: `status`, the body in `data`/`len`, and the headers
// in `text` as "name: value" lines.
#define UW_NET_RESPONSE 5

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
  // The package's private directory, the only place it may write.
  const char *(*data_dir)(void *ctx);
  // Network requests return a handle at once; the outcome arrives later
  // as `uw_net` events. Each needs a permission declared in
  // underweb.toml and granted by the user.
  uint32_t (*net_tcp_connect)(void *ctx, const char *host, uint16_t port);
  uint32_t (*net_tcp_listen)(void *ctx, uint16_t port);
  // Port 0 opens an ephemeral port for sending.
  uint32_t (*net_udp_open)(void *ctx, uint16_t port);
  void (*net_send)(void *ctx, uint32_t handle, const uint8_t *data, uintptr_t len);
  void (*net_udp_send)(void *ctx,
                       uint32_t handle,
                       const char *host,
                       uint16_t port,
                       const uint8_t *data,
                       uintptr_t len);
  void (*net_close)(void *ctx, uint32_t handle);
  // `headers`: "Name: value" lines separated by '\n', or NULL.
  uint32_t (*fetch)(void *ctx,
                    const char *method,
                    const char *url,
                    const char *headers,
                    const uint8_t *body,
                    uintptr_t len);
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

// A network event for a handle returned by one of the `net_*` or `fetch`
// functions. Pointers are valid only during the `uw_net` call.
typedef struct UwNetEvent {
  uint32_t kind;
  uint32_t handle;
  uint32_t new_handle;
  uint32_t status;
  const uint8_t *data;
  uintptr_t len;
  const char *text;
} UwNetEvent;

/* A native package exports these. Only uw_abi_version and uw_activate are
 * required. uw_draw gets a cairo_t* (include <cairo.h> to use it). */
uint32_t uw_abi_version(void);
void uw_activate(const UwHost *host);
void uw_update(const UwHost *host, double dt);
void uw_pointer(const UwHost *host, const UwPointerEvent *ev);
void uw_draw(const UwHost *host, void *cr, double w, double h);
void uw_deactivate(const UwHost *host);
void uw_net(const UwHost *host, const UwNetEvent *ev);

/* Convenience: call a host function with its context, e.g.
 *   UW(host, set_title, "hello");                                       */
#define UW(host, fn, ...) ((host)->fn((host)->ctx, ##__VA_ARGS__))

#endif /* UNDERWEB_H */
