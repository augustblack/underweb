#ifndef UNDERWEB_ENGINE_H
#define UNDERWEB_ENGINE_H

/* Generated from crates/underweb-capi by `cargo xtask header`. Do not edit. */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define UW_EVENT_STARTED 1

#define UW_EVENT_TITLE 2

#define UW_EVENT_FRAME 3

#define UW_EVENT_ANIMATING 4

#define UW_EVENT_LOG 5

#define UW_EVENT_ERROR 6

#define UW_EVENT_EXITED 7

// An open package.
typedef struct UwEngine UwEngine;

// One event. Pointers stay valid until the next call on the engine.
typedef struct UwEngineEvent {
  // `UW_EVENT_*`.
  uint32_t kind;
  // STARTED: sandboxed. ANIMATING: on.
  bool flag;
  // FRAME: size in pixels; `rgba` is premultiplied RGBA, rows packed.
  uint32_t width;
  uint32_t height;
  const uint8_t *rgba;
  // TITLE, LOG, ERROR: the text.
  const char *text;
} UwEngineEvent;

// Resolve an address (downloading and building if needed; this can take
// a while) and start its runner. Returns NULL on failure, with a message
// in `err`.
//
// # Safety
// `address` must be a NUL-terminated string; `err` NULL or `err_len`
// writable bytes.
struct UwEngine *uw_engine_open(const char *address, bool sandbox, char *err, uintptr_t err_len);

// Size the page in physical pixels. Returns 0 on success.
//
// # Safety
// `e` must come from `uw_engine_open` and not be closed.
int32_t uw_engine_configure(struct UwEngine *e, uint32_t width, uint32_t height, double scale);

// Pointer event in logical pixels; `kind` is `UW_POINTER_*` from
// underweb.h. Returns 0 on success.
//
// # Safety
// As for `uw_engine_configure`.
int32_t uw_engine_pointer(struct UwEngine *e, uint32_t kind, uint32_t button, double x, double y);

// Advance an animating page by `dt` seconds. Returns 0 on success.
//
// # Safety
// As for `uw_engine_configure`.
int32_t uw_engine_tick(struct UwEngine *e, double dt);

// Wait up to `timeout_ms` for an event. Returns false on timeout.
//
// # Safety
// As for `uw_engine_configure`; `out` must be writable.
bool uw_engine_next_event(struct UwEngine *e, uint32_t timeout_ms, struct UwEngineEvent *out);

// Stop the package and free the engine.
//
// # Safety
// `e` must come from `uw_engine_open`; it is invalid afterwards.
void uw_engine_close(struct UwEngine *e);

#endif /* UNDERWEB_ENGINE_H */
