#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
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

// A shade's sheet as a pane of glass (glass.rs, output.rs) sliding over the
// screen, which stands still under it. `tex` is the glass's atlas, `texl`
// px: the screen sharp in its lower half, blurred in its upper, both at
// half the output's size (a logical px each) and bottom-up as the canvas
// they came from. The element's part of it starts at `src0`; its top left
// is `origin` on screen. `sheet`: the panel's x and width, and the sheet's
// height - its bottom edge.
//
// Plain clear glass: the screen under it a little soft, under an even tint,
// nothing drawn along its edge or across it.
uniform vec2 texl;
uniform vec2 src0;
uniform vec2 origin;
uniform vec4 sheet;

const float TINT_A = 0.5;
const vec3 TINT = vec3(0.04, 0.05, 0.08);

vec3 sharp(vec2 q) {
    return texture2D(tex, vec2(q.x, texl.y * 0.5 - q.y) / texl).rgb;
}

vec3 frosted(vec2 q) {
    return texture2D(tex, vec2(q.x, texl.y - q.y) / texl).rgb;
}

void main() {
    vec2 q = origin + (v_coords * texl - src0);
    if (q.y > sheet.z) {
        gl_FragColor = vec4(0.0);
        return;
    }
    // Clear, a touch of the blurred picture so what is on the sheet reads.
    vec3 c = mix(sharp(q), frosted(q), 0.25);
    gl_FragColor = vec4(c * (1.0 - TINT_A) + TINT * TINT_A, 1.0) * alpha;
}
