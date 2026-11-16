// Prints each online display's gamma table: its id, its size, a hash of the
// whole table, and the red, green and blue at the top entry and at the middle.
// Read-only: it calls no CGSet* and no CGDisplayRestore*.
// usage: tabledump
// Build it into $TMPDIR, where flux-compare.sh looks for it:
//   clang -O1 -framework ApplicationServices tabledump.c -o "$TMPDIR/tabledump"
#include <ApplicationServices/ApplicationServices.h>
#include <stdio.h>

static unsigned long long fnv(const CGGammaValue *v, uint32_t n, unsigned long long h) {
    const unsigned char *p = (const unsigned char *)v;
    for (size_t i = 0; i < n * sizeof(CGGammaValue); i++) h = (h ^ p[i]) * 1099511628211ULL;
    return h;
}

int main(void) {
    CGDirectDisplayID ids[16];
    uint32_t count = 0;
    if (CGGetOnlineDisplayList(16, ids, &count) != kCGErrorSuccess) return 1;
    for (uint32_t d = 0; d < count; d++) {
        uint32_t cap = CGDisplayGammaTableCapacity(ids[d]);
        if (cap == 0 || cap > 4096) cap = 4096;
        CGGammaValue r[4096], g[4096], b[4096];
        uint32_t n = 0;
        if (CGGetDisplayTransferByTable(ids[d], cap, r, g, b, &n) != kCGErrorSuccess || n == 0) {
            printf("display=%u failed\n", ids[d]);
            continue;
        }
        unsigned long long h = 1469598103934665603ULL;
        h = fnv(r, n, h);
        h = fnv(g, n, h);
        h = fnv(b, n, h);
        uint32_t m = n / 2;
        printf("display=%u main=%d n=%u hash=%016llx top=%.4f,%.4f,%.4f mid=%.4f,%.4f,%.4f\n", ids[d],
               CGDisplayIsMain(ids[d]) ? 1 : 0, n, h, r[n - 1], g[n - 1], b[n - 1], r[m], g[m], b[m]);
    }
    return 0;
}
