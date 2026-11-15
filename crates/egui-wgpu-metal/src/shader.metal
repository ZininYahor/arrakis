// egui_glow-0.36.2/src/shader/{vertex,fragment}.glsl, in MSL. The noise takes GL's coordinates, which
// run from the bottom of the drawable.
#include <metal_stdlib>
using namespace metal;

struct Vertex { packed_float2 pos; packed_float2 uv; packed_uchar4 color; };
struct Out { float4 position [[position]]; float4 rgba; float2 tc; };
struct Pixel { float height; float dither; };

vertex Out vs(uint id [[vertex_id]],
              const device Vertex* vertices [[buffer(0)]],
              constant float2& screen [[buffer(1)]]) {
    Vertex in = vertices[id];
    uchar4 color = in.color;
    Out out;
    out.position = float4(2.0 * in.pos[0] / screen.x - 1.0,
                          1.0 - 2.0 * in.pos[1] / screen.y,
                          0.0, 1.0);
    out.rgba = float4(color) / 255.0;
    out.tc = float2(in.uv);
    return out;
}

fragment float4 fs(Out in [[stage_in]],
                   texture2d<float> image [[texture(0)]],
                   sampler images [[sampler(0)]],
                   constant Pixel& pixel [[buffer(0)]]) {
    float4 color = in.rgba * image.sample(images, in.tc);
    if (pixel.dither != 0.0) {
        float2 n = float2(in.position.x, pixel.height - in.position.y);
        float f = 0.06711056 * n.x + 0.00583715 * n.y;
        float noise = fract(52.9829189 * fract(f));
        noise = (noise - 0.5) * 0.95;
        color.rgb = color.rgb + noise / 255.0;
    }
    return color;
}
