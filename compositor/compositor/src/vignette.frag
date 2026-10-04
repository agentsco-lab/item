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
// panel (walls.rs): an ellipse the panel's shape, clear in the middle.
// `strength` how dark, `reach` how far in (0 the corners only, 1 from near
// the middle), `soft` how gradual. With `glow` it is the accent (`colour`)
// instead of the dark. And the hinge: folded like a book (`book`, 0 flat to
// 1 at 90 degrees) the panel's edge by the hinge (`inner`: 1 its right, -1
// its left) goes into shadow, as the inside of an open book does.
uniform float strength;
uniform float reach;
uniform float soft;
uniform float glow;
uniform vec3 colour;
uniform float book;
uniform float inner;

void main() {
    vec2 q = (v_coords - 0.5) * 2.0;
    float r = length(q * vec2(1.0, 0.92));
    float start = mix(1.3, 0.12, reach);
    float width = mix(0.1, 1.15, soft);
    float av = strength * smoothstep(start, start + width, r);
    if (glow > 0.5) {
        av *= 0.7;
    }
    // 1 at the hinge's edge, 0 across the panel.
    float x = inner > 0.0 ? v_coords.x : 1.0 - v_coords.x;
    float hz = book * smoothstep(0.5, 1.0, x) * (0.5 + 0.45 * strength);
    float a = av + hz - av * hz;
    if (a < 0.004) {
        discard;
    }
    vec3 c = glow > 0.5 ? colour : vec3(0.0);
    gl_FragColor = vec4(c * av * (1.0 - hz), a) * alpha;
}
