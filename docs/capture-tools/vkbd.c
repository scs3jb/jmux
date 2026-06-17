// Minimal virtual-keyboard driver (zwp_virtual_keyboard_v1).
// Uploads a US xkb keymap, then reads a script from stdin:
//   t <char>   type a printable char (a-z, 0-9, space)
//   k <code>   press+release a raw evdev keycode (Enter=28, Down=108, Esc=1, Backspace=14)
//   w <ms>     wait
//   q          quit
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>
#include <xkbcommon/xkbcommon.h>
#include <wayland-client.h>
#include "virtual-keyboard-unstable-v1-client-protocol.h"

static struct wl_seat *seat = NULL;
static struct zwp_virtual_keyboard_manager_v1 *mgr = NULL;
static struct zwp_virtual_keyboard_v1 *kbd = NULL;
static uint32_t t = 2000;

// evdev keycodes for a-z (index 0='a')
static const int LETTER[26] = {30,48,46,32,18,33,34,35,23,36,37,38,50,49,24,25,16,19,31,20,22,47,17,45,21,44};
static const int DIGIT[10]  = {11,2,3,4,5,6,7,8,9,10}; // 0-9

static void reg_global(void *d, struct wl_registry *r, uint32_t name,
                       const char *iface, uint32_t ver) {
    if (!strcmp(iface, wl_seat_interface.name))
        seat = wl_registry_bind(r, name, &wl_seat_interface, ver < 5 ? ver : 5);
    else if (!strcmp(iface, zwp_virtual_keyboard_manager_v1_interface.name))
        mgr = wl_registry_bind(r, name, &zwp_virtual_keyboard_manager_v1_interface, 1);
}
static void reg_remove(void *d, struct wl_registry *r, uint32_t name) {}
static const struct wl_registry_listener reg_l = { reg_global, reg_remove };

static struct wl_display *dpy;
static void tap(int code){
    zwp_virtual_keyboard_v1_key(kbd, t, code, 1); t += 8;
    zwp_virtual_keyboard_v1_key(kbd, t, code, 0); t += 8;
    wl_display_flush(dpy);
}
static int code_for(char c){
    if (c >= 'a' && c <= 'z') return LETTER[c-'a'];
    if (c >= '0' && c <= '9') return DIGIT[c-'0'];
    if (c == ' ') return 57;
    return -1;
}

int main(void){
    dpy = wl_display_connect(NULL);
    if (!dpy) { fprintf(stderr, "vkbd: no display\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(dpy);
    wl_registry_add_listener(reg, &reg_l, NULL);
    wl_display_roundtrip(dpy);
    if (!mgr) { fprintf(stderr, "vkbd: no virtual_keyboard_manager\n"); return 2; }
    kbd = zwp_virtual_keyboard_manager_v1_create_virtual_keyboard(mgr, seat);

    // build a default US keymap and upload it
    struct xkb_context *ctx = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    struct xkb_rule_names names = { .layout = "us" };
    struct xkb_keymap *km = xkb_keymap_new_from_names(ctx, &names, XKB_KEYMAP_COMPILE_NO_FLAGS);
    char *str = xkb_keymap_get_as_string(km, XKB_KEYMAP_FORMAT_TEXT_V1);
    size_t len = strlen(str) + 1;
    int fd = memfd_create("keymap", MFD_CLOEXEC);
    if (ftruncate(fd, len) < 0) { perror("ftruncate"); return 3; }
    void *map = mmap(NULL, len, PROT_READ|PROT_WRITE, MAP_SHARED, fd, 0);
    memcpy(map, str, len);
    munmap(map, len);
    zwp_virtual_keyboard_v1_keymap(kbd, WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, fd, len);
    wl_display_roundtrip(dpy);
    close(fd);

    char line[256];
    while (fgets(line, sizeof line, stdin)) {
        int ms, code;
        if (line[0] == 't' && line[1] == ' ') {
            int c = code_for(line[2]);
            if (c > 0) tap(c);
        } else if (sscanf(line, "k %d", &code) == 1) {
            tap(code);
        } else if (sscanf(line, "w %d", &ms) == 1) {
            wl_display_flush(dpy); usleep(ms*1000); wl_display_dispatch_pending(dpy);
        } else if (line[0] == 'q') break;
    }
    wl_display_roundtrip(dpy);
    return 0;
}
