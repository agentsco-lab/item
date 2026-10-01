#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// The lock screen's picture going away in a wave from where it was touched
// (door.rs): gone inside the wave, a soft glowing edge, untouched outside.
uniform vec2 size;      // the picture, logical px
uniform vec2 centre;    // where the wave starts, logical px
uniform float radius;   // how far it has come
uniform float edge;     // the width of its soft edge
uniform float glow;     // how bright the edge is

// All in hundreds of px: Adreno's mediump is 16 bit, and a squared distance
// in px (up to 2e6) overflows it.
void main() {
    vec4 color = texture2D(tex, v_coords);
    vec2 delta = (v_coords * size - centre) * 0.01;
    float d = length(delta);
    float r = radius * 0.01;
    float e = edge * 0.01;
    float keep = smoothstep(r - e, r, d);
    float band = (d - (r - e * 0.5)) / (e * 0.6);
    float light = glow * exp(-band * band) * (1.0 - keep * 0.5);
    gl_FragColor = (color * keep + vec4(light, light, light, light)) * alpha;
}
