/* camera-warm: the lock screen's camera peek (compositor/src/camera.rs),
 * kept warm. Starting gst-launch for each peek cost ~0.66 s before the
 * camera did anything - loading gst-droid and droidmedia through libhybris -
 * so this stays running with them loaded, and takes one-byte orders on
 * stdin:
 *   'o' opens the camera (READY: the HAL's device open) and runs it a moment
 *       unseen, as the HAL's first start after an open is slow - measured
 *       2026-10-02, a first frame ~0.29 s after 'p' this way, ~0.67 s after
 *       a bare open, ~0.56 s from closed, ~1.2 s with a new gst-launch;
 *   'p' plays: MARK, then NV21 640x480 frames on stdout - at once if it
 *       comes while the camera still runs unseen;
 *   's' stops (back to open), 'c' closes it (NULL: the device free).
 * The frame's size: argv 1 and 2 (640 480 by default; item's fold camera,
 * foldcam.rs, asks 1280 960).
 * EOF ends it. The pipeline writes into a pipe of its own; a thread passes
 * whole frames on to stdout while one is wanted and drops them otherwise.
 * Built on the phone against the system's libgstreamer, no headers needed:
 *   gcc -O2 -o camera-warm camera-warm.c -lpthread -l:libgstreamer-1.0.so.0 -l:libgobject-2.0.so.0
 */
#include <poll.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

typedef void GstElement;
typedef struct { unsigned domain; int code; char *message; } GError;
enum { NUL = 1, READY = 2, PAUSED = 3, PLAYING = 4 };

extern void gst_init(int *argc, char ***argv);
extern GstElement *gst_parse_launch(const char *desc, GError **err);
extern int gst_element_set_state(GstElement *e, int state);
extern int gst_element_get_state(GstElement *e, int *state, int *pending, unsigned long long timeout);

/* The frame's bytes, NV21: width x height x 1.5. */
static size_t FRAME = 640 * 480 * 3 / 2;
/* How long the camera runs unseen after an open. */
#define WARM_MS 1500

/* Written before each play's frames: a frame cut short by a stop is skipped
 * up to it. */
static const char MARK[16] = "camera-warm:play";

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static int wanted;

static void *pass(void *arg) {
    int in = *(int *)arg;
    char *frame = malloc(FRAME);
    if (!frame)
        return NULL;
    for (;;) {
        size_t got = 0;
        while (got < FRAME) {
            ssize_t k = read(in, frame + got, FRAME - got);
            if (k <= 0)
                return NULL;
            got += k;
        }
        pthread_mutex_lock(&lock);
        if (wanted && write(1, frame, FRAME) != (ssize_t)FRAME)
            wanted = 0;
        pthread_mutex_unlock(&lock);
    }
}

static void want(int on) {
    pthread_mutex_lock(&lock);
    if (on && !wanted && write(1, MARK, sizeof MARK) != sizeof MARK)
        on = 0;
    wanted = on;
    pthread_mutex_unlock(&lock);
}

static void down(GstElement *p, int state) {
    gst_element_set_state(p, state);
    gst_element_get_state(p, NULL, NULL, 5000000000ULL);
}

static long long now_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000LL + t.tv_nsec / 1000000;
}

int main(int argc, char **argv) {
    gst_init(&argc, &argv);
    int width = argc > 2 ? atoi(argv[1]) : 640, height = argc > 2 ? atoi(argv[2]) : 480;
    if (width <= 0 || height <= 0)
        width = 640, height = 480;
    FRAME = (size_t)width * height * 3 / 2;
    int fds[2];
    if (pipe(fds))
        return 1;
    char desc[160];
    snprintf(desc, sizeof desc,
             "droidcamsrc ! video/x-raw,format=NV21,width=%d,height=%d ! fdsink fd=%d sync=false", width, height, fds[1]);
    GError *err = NULL;
    GstElement *p = gst_parse_launch(desc, &err);
    if (!p) {
        fprintf(stderr, "camera-warm: %s\n", err ? err->message : "no pipeline");
        return 1;
    }
    pthread_t t;
    pthread_create(&t, NULL, pass, &fds[0]);
    /* Loaded: droidcamsrc's plugin and droidmedia come in with the pipeline;
     * the device opened and closed once to have the HAL up. */
    down(p, READY);
    down(p, NUL);
    fprintf(stderr, "camera-warm: loaded\n");
    /* When the unseen run after an open ends; 0 none. */
    long long warm_until = 0;
    for (;;) {
        int timeout = warm_until ? (int)(warm_until - now_ms()) : -1;
        if (warm_until && timeout <= 0) {
            warm_until = 0;
            down(p, READY);
            fprintf(stderr, "camera-warm: open\n");
            continue;
        }
        struct pollfd in = { 0, POLLIN, 0 };
        if (poll(&in, 1, timeout) == 0)
            continue;
        char c;
        if (read(0, &c, 1) != 1)
            break;
        if (c == 'o') {
            down(p, READY);
            gst_element_set_state(p, PLAYING);
            warm_until = now_ms() + WARM_MS;
        } else if (c == 'p') {
            warm_until = 0;
            want(1);
            gst_element_set_state(p, PLAYING);
        } else if (c == 's' || c == 'c') {
            warm_until = 0;
            want(0);
            down(p, c == 's' ? READY : NUL);
        }
    }
    want(0);
    down(p, NUL);
    return 0;
}
