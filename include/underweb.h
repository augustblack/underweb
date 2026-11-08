#ifndef UNDERWEB_H
#define UNDERWEB_H

/* Generated from crates/underweb-abi by `cargo xtask header`. Do not edit. */

#include <stdbool.h>
#include <stdint.h>

// The ABI this header describes. Version 2 appended network and storage
// functions to [`UwHost`], version 3 audio, version 4 video; packages
// built for older versions still load.
#define UW_ABI_VERSION 4

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

// [`UwMediaEvent::kind`]: frames are coming (`width`, `height`).
#define UW_MEDIA_READY 0

#define UW_MEDIA_ENDED 1

// A broadcast is being sent.
#define UW_MEDIA_BROADCASTING 2

// `text` says what went wrong (including refused permissions).
#define UW_MEDIA_ERROR 3

// Oscillator waves for `osc_new`.
#define UW_WAVE_SINE 0

#define UW_WAVE_SAW 1

#define UW_WAVE_SQUARE 2

#define UW_WAVE_TRIANGLE 3

#define UW_WAVE_NOISE 4

// [`UwPdEvent::kind`]
#define UW_PD_BANG 0

#define UW_PD_FLOAT 1

#define UW_PD_SYMBOL 2

// The floats of a list (symbols in it are left out).
#define UW_PD_LIST 3

typedef struct Option_UwAudioFn Option_UwAudioFn;

typedef struct UwMediaInfo {
  uint32_t width;
  uint32_t height;
  // Seconds.
  double position;
  // Seconds; 0 for live sources.
  double duration;
} UwMediaInfo;

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
  // The browser's sample rate; 0 if it has no audio.
  uint32_t (*audio_rate)(void *ctx);
  // Start sound (oscillators, Pd and `audio_callback` start it too).
  bool (*audio_start)(void *ctx);
  void (*audio_stop)(void *ctx);
  void (*audio_volume)(void *ctx, float volume);
  // `wave` is a `UW_WAVE_*`; `pan` from -1 (left) to 1. Returns 0 if
  // none is free.
  uint32_t (*osc_new)(void *ctx, uint32_t wave, float freq, float amp, float pan);
  void (*osc_set)(void *ctx, uint32_t id, float freq, float amp, float pan);
  void (*osc_free)(void *ctx, uint32_t id);
  // Copy up to `count` of the latest frames played (mono) to `out`.
  uintptr_t (*audio_scope)(void *ctx, float *out, uintptr_t count);
  // Mix your own audio in; `None` removes it.
  void (*audio_callback)(void *ctx, struct Option_UwAudioFn callback, void *user);
  // Open a Pd patch from the package (relative path); starts sound.
  bool (*pd_open)(void *ctx, const char *path);
  void (*pd_bang)(void *ctx, const char *receiver);
  void (*pd_float)(void *ctx, const char *receiver, float value);
  void (*pd_symbol)(void *ctx, const char *receiver, const char *symbol);
  void (*pd_list)(void *ctx, const char *receiver, const float *values, uintptr_t count);
  // Receive what Pd sends to `[s receiver]` in `uw_pd`.
  void (*pd_subscribe)(void *ctx, const char *receiver);
  // A package file, an http(s) URL, "camera", or "test:<pattern>".
  // Returns an id; events arrive in `uw_media`.
  uint32_t (*media_open)(void *ctx, const char *source, bool looping);
  // Like `media_open`, and send it to an Icecast server
  // ("http://host:port/mount.ogg").
  uint32_t (*media_broadcast)(void *ctx, const char *source, const char *url, const char *password);
  void (*media_play)(void *ctx, uint32_t id);
  void (*media_pause)(void *ctx, uint32_t id);
  void (*media_seek)(void *ctx, uint32_t id, double seconds);
  void (*media_close)(void *ctx, uint32_t id);
  // False until the first frame.
  bool (*media_info)(void *ctx, uint32_t id, struct UwMediaInfo *out);
  // Show video `id` inside the shape (0: none).
  void (*shape_set_video)(void *ctx, uint32_t shape, uint32_t video);
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

// Something Pd sent to a receiver the package subscribed to. Pointers
// are valid only during the `uw_pd` call.
typedef struct UwPdEvent {
  const char *receiver;
  uint32_t kind;
  float value;
  const char *symbol;
  const float *list;
  uintptr_t len;
} UwPdEvent;

typedef struct UwMediaEvent {
  uint32_t id;
  uint32_t kind;
  uint32_t width;
  uint32_t height;
  const char *text;
} UwMediaEvent;

/* A native package exports these. Only uw_abi_version and uw_activate are
 * required. uw_draw gets a cairo_t* (include <cairo.h> to use it). */
uint32_t uw_abi_version(void);
void uw_activate(const UwHost *host);
void uw_update(const UwHost *host, double dt);
void uw_pointer(const UwHost *host, const UwPointerEvent *ev);
void uw_draw(const UwHost *host, void *cr, double w, double h);
void uw_deactivate(const UwHost *host);
void uw_net(const UwHost *host, const UwNetEvent *ev);
void uw_pd(const UwHost *host, const UwPdEvent *ev);
void uw_media(const UwHost *host, const UwMediaEvent *ev);

/* Convenience: call a host function with its context, e.g.
 *   UW(host, set_title, "hello");                                       */
#define UW(host, fn, ...) ((host)->fn((host)->ctx, ##__VA_ARGS__))

#endif /* UNDERWEB_H */
