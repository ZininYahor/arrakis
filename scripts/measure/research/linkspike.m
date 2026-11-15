// RESEARCH CODE, NOT RUN BY ANYTHING: no script, test or workflow in this repository builds or runs it. It is
// kept so that the prototype's first reading can be repeated.
//
// What that reading found with it: that a `CADisplayLink` made from an `NSView` (macOS 14) ticks on a second thread's
// run loop at the panel's rate (118.9 to 120.7 a second on the built-in 120 Hz display), with the main
// thread pumping events and blocked in `usleep` as the hold blocks it; that a window hidden by `orderOut`
// gets 1 tick in a second and the link goes on when it comes back; that `invalidate` from main takes
// 0.04 ms; that making the link on the main thread takes 0.09 ms; and that the fallback, the link
// on the main run loop in a private mode, ticks only when that mode runs.
//
// Build (the deployment target is the app's, 14.0). No Xcode needed:
//   clang -O1 -fobjc-arc -mmacosx-version-min=14.0 -framework AppKit -framework QuartzCore \
//       linkspike.m -o "$TMPDIR/linkspike"
// Run it for what it opens: an empty 120 x 120 pt titled window on the main display, for about 10 s. It
// touches no display's table and is not Arrakis, but it is a real window on that display, so run it
// only when you mean to. It prints one line for each
// case: ticks, the time they took, the rate, how many came on the main thread, the link's `duration` and
// the span from `timestamp` to `targetTimestamp`.
//
// Does a CADisplayLink made from an NSView (macOS 14) tick on a second thread's run loop, and at what
// rate, with the main thread free, then blocked as the hold blocks it? And on the main run loop in a
// private mode, the fallback? An empty window, no display's tables touched.
#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#include <stdatomic.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>

static atomic_long ticks;
static atomic_long ticks_on_main;
static _Atomic double last_duration;
static _Atomic double last_span;

static double now_ms(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1e6; }

@interface Tick : NSObject
- (void)tick:(CADisplayLink *)link;
@end
@implementation Tick
- (void)tick:(CADisplayLink *)link {
    atomic_fetch_add(&ticks, 1);
    if ([NSThread isMainThread]) atomic_fetch_add(&ticks_on_main, 1);
    atomic_store(&last_duration, link.duration);
    atomic_store(&last_span, link.targetTimestamp - link.timestamp);
}
@end

static void pump(double seconds) {
    NSDate *until = [NSDate dateWithTimeIntervalSinceNow:seconds];
    while (until.timeIntervalSinceNow > 0) {
        NSEvent *event = [NSApp nextEventMatchingMask:NSEventMaskAny
                                            untilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]
                                               inMode:NSDefaultRunLoopMode
                                              dequeue:YES];
        if (event) [NSApp sendEvent:event];
    }
}

static void report(const char *name, double seconds, void (^body)(void)) {
    long before = atomic_load(&ticks);
    long before_main = atomic_load(&ticks_on_main);
    double start = now_ms();
    body();
    double took = (now_ms() - start) / 1000.0;
    long n = atomic_load(&ticks) - before;
    printf("%-34s %5ld ticks in %.2f s = %6.1f /s, on main %ld, duration %.2f ms, span %.2f ms\n", name, n, took,
           n / took, atomic_load(&ticks_on_main) - before_main, atomic_load(&last_duration) * 1000,
           atomic_load(&last_span) * 1000);
    fflush(stdout);
    (void)seconds;
}

int main(void) {
    @autoreleasepool {
        NSApplication *app = NSApplication.sharedApplication;
        [app setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [app finishLaunching];
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(60, 60, 120, 120)
                                                       styleMask:NSWindowStyleMaskTitled
                                                         backing:NSBackingStoreBuffered
                                                           defer:NO];
        window.releasedWhenClosed = NO;
        [window orderFrontRegardless];
        NSView *view = window.contentView;
        view.wantsLayer = YES;
        pump(0.3);
        printf("screen max fps %ld\n", (long)window.screen.maximumFramesPerSecond);

        Tick *target = [Tick new];
        double t0 = now_ms();
        CADisplayLink *link = [view displayLinkWithTarget:target selector:@selector(tick:)];
        printf("making the link on main: %.2f ms\n", now_ms() - t0);
        link.preferredFrameRateRange = CAFrameRateRangeMake(120, 120, 120);

        [NSThread detachNewThreadWithBlock:^{
            NSRunLoop *loop = NSRunLoop.currentRunLoop;
            [loop addPort:[NSPort port] forMode:NSDefaultRunLoopMode];
            [link addToRunLoop:loop forMode:NSDefaultRunLoopMode];
            [loop run];
        }];
        report("second thread, main pumps events", 2, ^{ pump(2.0); });
        report("second thread, main blocked", 2, ^{ usleep(2000000); });
        report("second thread, main pumps again", 1, ^{ pump(1.0); });
        report("second thread, window hidden", 1, ^{
            [window orderOut:nil];
            pump(1.0);
        });
        [window orderFrontRegardless];
        pump(0.3);
        report("second thread, window back", 1, ^{ pump(1.0); });
        double t1 = now_ms();
        [link invalidate];
        printf("invalidate from main: %.2f ms\n", now_ms() - t1);
        report("after invalidate", 1, ^{ pump(0.5); });

        CADisplayLink *fallback = [view displayLinkWithTarget:target selector:@selector(tick:)];
        fallback.preferredFrameRateRange = CAFrameRateRangeMake(120, 120, 120);
        CFStringRef mode = CFSTR("hold");
        [fallback addToRunLoop:NSRunLoop.mainRunLoop forMode:(__bridge NSRunLoopMode)mode];
        report("main run loop, private mode", 2, ^{
            double end = now_ms() + 2000;
            while (now_ms() < end) CFRunLoopRunInMode(mode, 0.05, true);
        });
        report("main run loop, default mode only", 1, ^{ pump(1.0); });
        [fallback invalidate];
        [window close];
    }
    return 0;
}
