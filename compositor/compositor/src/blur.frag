#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

// Texture coordinates a pixel apart need more than mediump's 10 bits.
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

// One pass of the dual Kawase blur (glass.rs): down to half the size (five
// taps), or up to twice it (eight); `pixel` is half a texel of the texture
// read, in its coordinates.
uniform vec2 pixel;
uniform float up;

void main() {
    vec2 uv = v_coords;
    vec2 h = pixel;
    vec4 sum;
    if (up < 0.5) {
        sum = texture2D(tex, uv) * 4.0;
        sum += texture2D(tex, uv - h);
        sum += texture2D(tex, uv + h);
        sum += texture2D(tex, uv + vec2(h.x, -h.y));
        sum += texture2D(tex, uv - vec2(h.x, -h.y));
        sum /= 8.0;
    } else {
        sum = texture2D(tex, uv + vec2(-h.x * 2.0, 0.0));
        sum += texture2D(tex, uv + vec2(-h.x, h.y)) * 2.0;
        sum += texture2D(tex, uv + vec2(0.0, h.y * 2.0));
        sum += texture2D(tex, uv + vec2(h.x, h.y)) * 2.0;
        sum += texture2D(tex, uv + vec2(h.x * 2.0, 0.0));
        sum += texture2D(tex, uv + vec2(h.x, -h.y)) * 2.0;
        sum += texture2D(tex, uv + vec2(0.0, -h.y * 2.0));
        sum += texture2D(tex, uv + vec2(-h.x, -h.y)) * 2.0;
        sum /= 12.0;
    }
    gl_FragColor = sum * alpha;
}
