#version 450
// Composites the chrome over the stretched picture.
//
// Only a composite: `blit.frag` already did the stretch, and it stays the one place that
// knows how. Keeping the chrome in its own pass also keeps it out of the colour cube — a
// graded HUD would read the wrong values back to an operator.
layout(location = 0) in vec2 vUv;
layout(location = 0) out vec4 oColor;

layout(set = 0, binding = 0) uniform sampler2D uPicture;
layout(set = 0, binding = 1) uniform sampler2D uOverlay;

layout(push_constant) uniform PC {
    float opacity;
    float _pad0;
} pc;

void main() {
    vec3 picture = texture(uPicture, vUv).rgb;
    vec4 chrome = texture(uOverlay, vUv);
    // Straight alpha, not premultiplied: the rasteriser writes colours as an operator
    // would name them. `opacity` fades the chrome only — dimming the picture with it
    // would be a lie about exposure.
    oColor = vec4(mix(picture, chrome.rgb, chrome.a * pc.opacity), 1.0);
}
