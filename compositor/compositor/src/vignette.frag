//_DEFINES_

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// A panel's wallpaper darkening toward its edges, with a picture on each
// panel (walls.rs): an ellipse the panel's shape, clear in the middle, the
// dark rising from about half way out to the corners. `strength` 0 to 1.
uniform float strength;

void main() {
    vec2 q = (v_coords - 0.5) * 2.0;
    float r = length(q * vec2(1.0, 0.92));
    float a = strength * 0.82 * smoothstep(0.45, 1.42, r);
    if (a < 0.004) {
        discard;
    }
    gl_FragColor = vec4(0.0, 0.0, 0.0, a) * alpha;
}
