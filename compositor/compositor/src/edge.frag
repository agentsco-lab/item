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

// A panel's edges going soft into the dark of the bezel: black outside a
// rectangle with corners rounded `corner` logical px, strongest at its edge
// and gone `soft` px in.
uniform float soft;
uniform float strength;
uniform float corner;

void main() {
    vec2 p = v_coords * size;
    // How far inside the rounded rectangle.
    vec2 q = abs(p - size * 0.5) - (size * 0.5 - vec2(corner));
    float inside = corner - (length(max(q, 0.0)) + min(max(q.x, q.y), 0.0));
    float k = smoothstep(0.0, soft, inside);
    float a = (1.0 - k) * strength;
    if (a < 0.004) {
        discard;
    }
    gl_FragColor = vec4(0.0, 0.0, 0.0, a) * alpha;
}
