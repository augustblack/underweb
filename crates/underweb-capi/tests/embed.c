/* Embeds the engine: opens a package, waits for its first frame. */
#include <stdio.h>
#include "underweb_engine.h"

int main(int argc, char **argv)
{
    char err[512];
    UwEngine *e = uw_engine_open(argv[1], true, err, sizeof err);
    if (!e) {
        fprintf(stderr, "open failed: %s\n", err);
        return 1;
    }
    uw_engine_configure(e, 320, 240, 1.0);
    UwEngineEvent ev;
    const char *title = "";
    int sandboxed = 0, ok = 0;
    while (uw_engine_next_event(e, 10000, &ev)) {
        if (ev.kind == UW_EVENT_STARTED) sandboxed = ev.flag;
        if (ev.kind == UW_EVENT_TITLE) printf("title %s\n", ev.text);
        if (ev.kind == UW_EVENT_FRAME) {
            const uint8_t *p = ev.rgba + 4 * (230 * ev.width + 110);   /* first circle */
            printf("frame %ux%u sandboxed=%d pixel=%u,%u,%u\n", ev.width, ev.height, sandboxed, p[0], p[1], p[2]);
            ok = 1;
            break;
        }
        if (ev.kind == UW_EVENT_ERROR || ev.kind == UW_EVENT_EXITED) break;
    }
    (void)title;
    uw_engine_close(e);
    return ok ? 0 : 1;
}
