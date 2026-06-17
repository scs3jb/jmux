// Minimal wlr-virtual-pointer driver.
// Reads a script from stdin, one command per line, keeping a single virtual
// pointer alive for the whole sequence (required for drag = press across moves):
//   m X Y   absolute motion to (X,Y)
//   d       left button down
//   u       left button up
//   w MS    wait MS milliseconds
//   q       quit
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <wayland-client.h>
#include "wlr-virtual-pointer-unstable-v1-client-protocol.h"

#define BTN_LEFT 0x110
#define EXT_W 1600
#define EXT_H 1000

static struct wl_seat *seat = NULL;
static struct zwlr_virtual_pointer_manager_v1 *mgr = NULL;
static struct zwlr_virtual_pointer_v1 *ptr = NULL;
static uint32_t t = 1000;

static void reg_global(void *d, struct wl_registry *r, uint32_t name,
                       const char *iface, uint32_t ver) {
    if (!strcmp(iface, wl_seat_interface.name))
        seat = wl_registry_bind(r, name, &wl_seat_interface, ver < 5 ? ver : 5);
    else if (!strcmp(iface, zwlr_virtual_pointer_manager_v1_interface.name))
        mgr = wl_registry_bind(r, name, &zwlr_virtual_pointer_manager_v1_interface, ver < 2 ? ver : 2);
}
static void reg_remove(void *d, struct wl_registry *r, uint32_t name) {}
static const struct wl_registry_listener reg_l = { reg_global, reg_remove };

int main(void) {
    struct wl_display *dpy = wl_display_connect(NULL);
    if (!dpy) { fprintf(stderr, "vptr: no display\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(dpy);
    wl_registry_add_listener(reg, &reg_l, NULL);
    wl_display_roundtrip(dpy);
    if (!mgr) { fprintf(stderr, "vptr: no virtual_pointer_manager (compositor lacks support)\n"); return 2; }
    ptr = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(mgr, seat);
    wl_display_roundtrip(dpy);

    char line[256];
    while (fgets(line, sizeof line, stdin)) {
        int x, y, ms;
        if (sscanf(line, "m %d %d", &x, &y) == 2) {
            zwlr_virtual_pointer_v1_motion_absolute(ptr, t, x, y, EXT_W, EXT_H);
            zwlr_virtual_pointer_v1_frame(ptr);
            t += 10; wl_display_flush(dpy);
        } else if (line[0] == 'd') {
            zwlr_virtual_pointer_v1_button(ptr, t, BTN_LEFT, 1);
            zwlr_virtual_pointer_v1_frame(ptr);
            t += 10; wl_display_flush(dpy);
        } else if (line[0] == 'u') {
            zwlr_virtual_pointer_v1_button(ptr, t, BTN_LEFT, 0);
            zwlr_virtual_pointer_v1_frame(ptr);
            t += 10; wl_display_flush(dpy);
        } else if (sscanf(line, "w %d", &ms) == 1) {
            wl_display_flush(dpy);
            usleep(ms * 1000);
            wl_display_dispatch_pending(dpy);
        } else if (line[0] == 'q') {
            break;
        }
    }
    wl_display_roundtrip(dpy);
    return 0;
}
