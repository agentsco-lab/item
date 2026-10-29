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

//_DITHER_
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

// The keyboard's tile as frosted glass (glass.rs, kbd.rs): the screen
// under it blurred, under a dark tint, its top corners round. `tex` is the
// glass's atlas as pane.frag reads it: the screen sharp in its lower half,
// blurred in its upper, a logical px each, bottom-up. `tile`: the tile's
// x, y, width and height on screen; `round`: the corners' radius; `veil`:
// the tint's share (the keyboard's 0.62; a posture's turn veils the whole
// screen lightly).
uniform vec2 texl;
uniform vec2 src0;
uniform vec2 origin;
// The frame's turn (layout.rs): 0 none, 1 the right panel up, 2 the left.
uniform float turn;
uniform vec4 tile;
uniform float round;
uniform float veil;

const vec3 TINT = vec3(0.05, 0.06, 0.09);

// A point of the frame where it lies on the screen (the atlas is the
// screen as hwcomposer has it, the hinge vertical).
vec2 lying(vec2 q) {
    if (turn > 1.5) {
        return vec2(q.y, texl.y * 0.5 - q.x);
    }
    if (turn > 0.5) {
        return vec2(texl.x - q.y, q.x);
    }
    return q;
}

vec3 frosted(vec2 q) {
    q = lying(q);
    return texture2D(tex, vec2(q.x, texl.y - q.y) / texl).rgb;
}

void main() {
    vec2 q = origin + (v_coords * texl - src0);
    // The top corners: outside the round, nothing.
    vec2 p = q - tile.xy;
    float cover = 1.0;
    if (p.y < round) {
        float cx = p.x < round ? round - p.x : (p.x > tile.z - round ? p.x - (tile.z - round) : 0.0);
        float d = length(vec2(cx, round - p.y)) - round;
        cover = 1.0 - smoothstep(-1.0, 0.5, d);
    }
    // The tint added as the picture under it is lit (pane.frag): black
    // stays black.
    vec3 under = frosted(q);
    float lit = min(1.0, dot(under, vec3(0.299, 0.587, 0.114)) * 8.0);
    vec3 c = under * (1.0 - veil) + TINT * veil * lit;
    gl_FragColor = vec4(dither(c), 1.0) * alpha * cover;
}
