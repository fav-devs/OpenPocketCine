#version 450
// Planar 8-bit 4:2:0 to RGB at the source raster.
//
// On Android this pass is a VkSamplerYcbcrConversion over the decoder's
// AHardwareBuffer. Software decode hands over three separate planes instead, so the
// conversion is written out here — the same BT.709 limited-range matrix the sampler
// conversion applies. Everything after this point is the shared pipeline.
layout(location = 0) in vec2 vUv;
layout(location = 0) out vec4 oColor;

layout(set = 0, binding = 0) uniform sampler2D uLuma;
layout(set = 0, binding = 1) uniform sampler2D uChromaBlue;
layout(set = 0, binding = 2) uniform sampler2D uChromaRed;

layout(push_constant) uniform PC {
    vec2 sourceSize;
    float fullRange;
    float _pad0;
} pc;

void main() {
    float y = texture(uLuma, vUv).r;
    float cb = texture(uChromaBlue, vUv).r - 0.5;
    float cr = texture(uChromaRed, vUv).r - 0.5;

    // Limited range puts luma on 16..235 and chroma on 16..240 of 8-bit.
    if (pc.fullRange < 0.5) {
        y = (y - 16.0 / 255.0) * (255.0 / 219.0);
        cb *= 255.0 / 224.0;
        cr *= 255.0 / 224.0;
    }

    vec3 rgb = vec3(
        y + 1.5748 * cr,
        y - 0.1873 * cb - 0.4681 * cr,
        y + 1.8556 * cb);
    oColor = vec4(clamp(rgb, 0.0, 1.0), 1.0);
}
