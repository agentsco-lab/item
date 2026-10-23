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

// The turn (output.rs, draft 42): one physical panel's content turned about
// the panel's centre by `angle` (radians, as drawn) and scaled by `scale`,
// sampled from a frame-sized texture (`texsize` px) - the snapshot before
// the turn, or the live frame. `panel`: the panel's x, y, width, height in
// the texture. Outside the panel's own pixels nothing: the other panel and
// the hinge never show through.
uniform vec2 texsize;
uniform vec4 panel;
uniform float angle;
uniform float scale;

void main() {
    vec2 q = v_coords * texsize;
    vec2 c = panel.xy + panel.zw * 0.5;
    vec2 d = q - c;
    // Drawn turned by `angle`: the source is found by the turn back.
    float s = sin(-angle);
    float co = cos(-angle);
    vec2 p = c + vec2(co * d.x - s * d.y, s * d.x + co * d.y) / scale;
    if (p.x < panel.x || p.y < panel.y || p.x > panel.x + panel.z || p.y > panel.y + panel.w) {
        gl_FragColor = vec4(0.0);
        return;
    }
    gl_FragColor = texture2D(tex, p / texsize) * alpha;
}
