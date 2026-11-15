// Two small jobs for the scripts that drive the real app (real-cycles.sh, delivery.sh):
//   axtool fp <pid>       prints the process's phys_footprint in MiB (2^20 B), the number Activity Monitor's
//                         Memory column is close to. Any process of the same user; no root.
//   axtool click <x> <y>  posts a left click at a global point, to press the menu-bar item. It needs the
//                         terminal to be allowed to control the computer (System Settings, Privacy & Security,
//                         Accessibility).
// Build it into $TMPDIR, where the scripts look for it:
//   clang -O1 -framework ApplicationServices axtool.c -o "$TMPDIR/axtool"
#include <ApplicationServices/ApplicationServices.h>
#include <libproc.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc == 3 && !strcmp(argv[1], "fp")) {
        struct rusage_info_v4 info;
        if (proc_pid_rusage(atoi(argv[2]), RUSAGE_INFO_V4, (rusage_info_t *)&info)) return 1;
        printf("%.2f\n", info.ri_phys_footprint / 1048576.0);
        return 0;
    }
    if (argc == 4 && !strcmp(argv[1], "click")) {
        CGPoint point = {atof(argv[2]), atof(argv[3])};
        CGEventType types[] = {kCGEventMouseMoved, kCGEventLeftMouseDown, kCGEventLeftMouseUp};
        for (int i = 0; i < 3; i++) {
            CGEventRef event = CGEventCreateMouseEvent(NULL, types[i], point, kCGMouseButtonLeft);
            CGEventPost(kCGHIDEventTap, event);
            CFRelease(event);
            usleep(60000);
        }
        return 0;
    }
    return 2;
}
