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

// A cover of the flow (flow.rs, output.rs): the window's picture as a
// plane turned by `tilt` about its vertical axis, seen from the front with
// the eye `eye` cover widths away. The element is the plane's bounding
// box; `xrange` is the box's left and right in cover widths from the
// plane's middle, `yscale` its height over the cover's. `mirror` 1 draws the
// reflection: the plane upside down, fading from `fade` at the cover's
// foot to nothing by half way.
uniform float tilt;
uniform float eye;
uniform vec2 xrange;
uniform float yscale;
uniform float mirror;
uniform float fade;
// The corners' radius in cover units (x and y).
uniform vec2 corner;

void main() {
    // Where on the screen, in cover units about the plane's middle.
    float x = xrange.x + v_coords.x * (xrange.y - xrange.x);
    float y = (v_coords.y - 0.5) * yscale;
    // The plane's point s (along it, -0.5..0.5) projecting to x: s turned
    // sits at (s cos t, s sin t), and x = s cos t * eye / (eye - s sin t).
    float c = cos(tilt);
    float sn = sin(tilt);
    float s = x * eye / (c * eye + x * sn);
    float f = eye / (eye - s * sn);
    float v = y / f + 0.5;
    if (abs(s) > 0.5 || v < 0.0 || v > 1.0) {
        gl_FragColor = vec4(0.0);
        return;
    }
    float u = s + 0.5;
    float a = alpha;
    // Rounded corners: outside the radius at a corner, nothing; soft at the edge.
    vec2 d = max(vec2(abs(u - 0.5), abs(v - 0.5)) - (vec2(0.5) - corner), vec2(0.0));
    float dist = length(d / max(corner, vec2(0.0001)));
    if (dist > 1.0) {
        gl_FragColor = vec4(0.0);
        return;
    }
    a *= smoothstep(1.0, 0.92, dist);
    if (mirror > 0.5) {
        v = 1.0 - v;
        a *= fade * max(0.0, 1.0 - v * 2.0);
    }
    gl_FragColor = texture2D(tex, vec2(u, v)) * a;
}
