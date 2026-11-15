// RESEARCH CODE, NOT RUN BY ANYTHING: no script, test or workflow in this repository builds or runs it. It is
// kept so that the prototype's first reading can be repeated.
//
// What that reading found with it: how long the prototype's shader start takes, in two parts. `newLibraryWithSource`
// is the source to AIR (about 90 ms of about 105 to 118) and `newRenderPipelineStateWithDescriptor` is AIR
// to machine code (13 to 21 ms for new AIR, 0.7 to 0.8 ms for AIR the system has already compiled). Run
// again, or in a binary with a new name or UUID, both read 0.5 to 1.2 ms: the system's cache follows the
// shader's text and not the binary.
//
// Build (no Xcode needed; the Command Line Tools' clang is enough). It touches no window and no display:
//   clang -O1 -fobjc-arc -framework Foundation -framework Metal split.m -o "$TMPDIR/split"
// Run it on a file that holds the shader's text, whose functions are named `vs` and `fs`, as
// crates/egui-wgpu-metal/src/shader.metal's are:
//   "$TMPDIR/split" crates/egui-wgpu-metal/src/shader.metal
// It prints one line: device, library, pipeline and their sum, in ms.
//
// Times the prototype's shader start in two parts: source to AIR (the library), AIR to machine code
// (the pipeline). No window, no display touched. Same calls and same descriptor as the prototype's
// `start_gpu`.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdio.h>
#include <time.h>

static double now_ms(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1e6; }

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc < 2) return 2;
        NSString *src = [NSString stringWithContentsOfFile:[NSString stringWithUTF8String:argv[1]]
                                                  encoding:NSUTF8StringEncoding
                                                     error:nil];
        if (!src) return 3;
        double t0 = now_ms();
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        id<MTLCommandQueue> queue = [device newCommandQueue];
        double t1 = now_ms();
        NSError *error = nil;
        id<MTLLibrary> library = [device newLibraryWithSource:src options:nil error:&error];
        if (!library) {
            fprintf(stderr, "%s\n", error.description.UTF8String);
            return 1;
        }
        id<MTLFunction> vs = [library newFunctionWithName:@"vs"];
        id<MTLFunction> fs = [library newFunctionWithName:@"fs"];
        double t2 = now_ms();
        MTLRenderPipelineDescriptor *desc = [MTLRenderPipelineDescriptor new];
        desc.vertexFunction = vs;
        desc.fragmentFunction = fs;
        MTLRenderPipelineColorAttachmentDescriptor *color = desc.colorAttachments[0];
        color.pixelFormat = MTLPixelFormatBGRA8Unorm;
        color.blendingEnabled = YES;
        color.rgbBlendOperation = MTLBlendOperationAdd;
        color.alphaBlendOperation = MTLBlendOperationAdd;
        color.sourceRGBBlendFactor = MTLBlendFactorOne;
        color.destinationRGBBlendFactor = MTLBlendFactorOneMinusSourceAlpha;
        color.sourceAlphaBlendFactor = MTLBlendFactorOneMinusDestinationAlpha;
        color.destinationAlphaBlendFactor = MTLBlendFactorOne;
        id<MTLRenderPipelineState> pipeline = [device newRenderPipelineStateWithDescriptor:desc error:&error];
        double t3 = now_ms();
        if (!pipeline) {
            fprintf(stderr, "%s\n", error.description.UTF8String);
            return 1;
        }
        (void)queue;
        printf("device %.1f library %.1f pipeline %.1f sum %.1f\n", t1 - t0, t2 - t1, t3 - t2, t3 - t0);
    }
    return 0;
}
