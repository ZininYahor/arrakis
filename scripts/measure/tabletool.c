// Reads the main display's gamma table for <seconds> and prints how often it changed: the check that a
// close, or anything else, leaves the tint alone (close-under-tint.sh). It sleeps 2 ms after each read,
// and a pass took about 3.3 ms in all, the read and the sleep's overshoot included: 12 s gave about
// 3,600 reads. A read that fails is counted apart (failures=) and changes nothing else; the last
// line is reads=, changes= and failures=.
// Read-only: it calls no CGSet* and no CGDisplayRestore*.
// usage: tabletool <seconds>
// Build it into $TMPDIR, where the scripts look for it:
//   clang -O1 -framework ApplicationServices tabletool.c -o "$TMPDIR/tabletool"
#include <ApplicationServices/ApplicationServices.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

enum { CAP = 1024 };

// Reads the table; 0 when the read failed (or gave no entries), with *count 0.
static int read_table(CGGammaValue *r, CGGammaValue *g, CGGammaValue *b, uint32_t *count) {
    *count = 0;
    if (CGGetDisplayTransferByTable(CGMainDisplayID(), CAP, r, g, b, count) != kCGErrorSuccess) {
        *count = 0;
        return 0;
    }
    return *count > 0;
}

static double now(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec + t.tv_nsec / 1e9;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    double seconds = atof(argv[1]);
    static CGGammaValue r[CAP], g[CAP], b[CAP], lr[CAP], lg[CAP], lb[CAP];
    uint32_t last;
    if (!read_table(lr, lg, lb, &last)) {
        puts("no table");
        return 1;
    }
    printf("start: entries=%u last red=%.5f green=%.5f blue=%.5f\n", last, lr[last - 1], lg[last - 1], lb[last - 1]);
    double start = now();
    long reads = 0, changes = 0, failures = 0;
    while (now() - start < seconds) {
        uint32_t count;
        reads++;
        if (!read_table(r, g, b, &count)) {
            // A failed read says nothing about the table: count it apart and keep `last` as it was.
            failures++;
            usleep(2000);
            continue;
        }
        if (count != last || memcmp(r, lr, sizeof(CGGammaValue) * count) || memcmp(g, lg, sizeof(CGGammaValue) * count) ||
            memcmp(b, lb, sizeof(CGGammaValue) * count)) {
            changes++;
            printf("change at %.3f s: last red=%.5f green=%.5f blue=%.5f\n", now() - start, r[count - 1], g[count - 1], b[count - 1]);
            last = count;
            memcpy(lr, r, sizeof r);
            memcpy(lg, g, sizeof g);
            memcpy(lb, b, sizeof b);
        }
        usleep(2000);
    }
    printf("reads=%ld changes=%ld failures=%ld\n", reads, changes, failures);
    return 0;
}
