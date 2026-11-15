// Prints the window number of a process's on-screen window at the normal level, for
// `screencapture -l<id>` (tear-captures.sh). Exits 1 when the process has none.
// Read-only: it lists windows and changes nothing.
// usage: winid <pid>
// Build it into $TMPDIR, where the scripts look for it:
//   clang -O1 -framework CoreGraphics -framework CoreFoundation winid.c -o "$TMPDIR/winid"
#include <CoreFoundation/CoreFoundation.h>
#include <CoreGraphics/CoreGraphics.h>
#include <stdio.h>
#include <stdlib.h>

static long number(CFDictionaryRef window, CFStringRef key) {
    long value = -1;
    CFNumberRef found = CFDictionaryGetValue(window, key);
    if (found) CFNumberGetValue(found, kCFNumberLongType, &value);
    return value;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    long pid = atol(argv[1]);
    CFArrayRef windows = CGWindowListCopyWindowInfo(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID);
    if (!windows) return 1;
    for (CFIndex i = 0; i < CFArrayGetCount(windows); i++) {
        CFDictionaryRef window = CFArrayGetValueAtIndex(windows, i);
        if (number(window, kCGWindowOwnerPID) == pid && number(window, kCGWindowLayer) == 0) {
            printf("%ld\n", number(window, kCGWindowNumber));
            return 0;
        }
    }
    return 1;
}
