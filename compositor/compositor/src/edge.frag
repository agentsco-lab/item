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

// A panel's outer edges going soft into the dark of the bezel: black
// outside a rectangle with corners rounded `corner` logical px, strongest at
// its edge and gone `soft` px in. The edge by the hinge (`inner`: 1 its
// right, -1 its left) stays as it is: the rectangle goes on past it.
uniform float soft;
uniform float strength;
uniform float corner;
uniform float inner;

void main() {
    vec2 p = v_coords * size;
    float extra = soft + corner + 2.0;
    vec2 lo = vec2(inner < 0.0 ? -extra : 0.0, 0.0);
    vec2 hi = vec2(size.x + (inner > 0.0 ? extra : 0.0), size.y);
    // How far inside the rounded rectangle.
    vec2 q = abs(p - (lo + hi) * 0.5) - ((hi - lo) * 0.5 - vec2(corner));
    float inside = corner - (length(max(q, 0.0)) + min(max(q.x, q.y), 0.0));
    float k = smoothstep(0.0, soft, inside);
    float a = (1.0 - k) * strength;
    if (a < 0.004) {
        discard;
    }
    gl_FragColor = vec4(0.0, 0.0, 0.0, a) * alpha;
}
