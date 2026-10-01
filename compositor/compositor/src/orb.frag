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

// The first setup's circle (setup.rs): a disc, or a ring `thickness` of its
// radius wide, lit in `accent` clockwise from the top as far as `progress`,
// cut into `segments` when there are any. Inside the ring, on the finger's
// step, a fingerprint of its own: ridges shown from the middle out as far as
// `print`, lit in `accent` as far as `lit` (ridge by ridge, each clockwise
// from the top), brighter for a moment after a touch (`flash`). `centre` and
// `radius` in the area's logical px (smithay's `size`).
uniform vec2 centre;
uniform float radius;
uniform float thickness;
uniform float progress;
uniform float segments;
uniform vec4 base;
uniform vec4 accent;
uniform float print;
uniform float lit;
uniform float flash;

const float TAU = 6.2831853;
const float RIDGES = 8.0;
// The print's size in the ring, and its ridges' half width (of a ridge's
// spacing).
const float PRINT = 0.8;
const float HALF = 0.16;

float hash(float n) {
    return fract(sin(n * 12.9898 + 4.1) * 43758.5453);
}

// How far `a` is from `c` round the circle, in turns.
float apart(float a, float c) {
    float d = abs(a - c);
    return min(d, 1.0 - d);
}

void main() {
    vec2 p = (v_coords * size - centre) / max(radius, 0.001);
    float d = length(p);
    if (d > 1.05) {
        discard;
    }
    float aa = 0.8 / max(radius, 1.0);
    float outer = 1.0 - smoothstep(1.0 - aa, 1.0, d);
    float inner_r = 1.0 - thickness;
    float ring = outer * smoothstep(inner_r - aa, inner_r, d);
    float a = atan(p.x, -p.y) / TAU;
    if (a < 0.0) a += 1.0;
    if (segments > 0.5) {
        float s = fract(a * segments);
        ring *= smoothstep(0.0, 0.03, s) * (1.0 - smoothstep(0.97, 1.0, s));
    }
    float on = 1.0 - smoothstep(progress - 0.004, progress, a);
    vec4 color = mix(base, accent, on) * ring;

    if (print > 0.001) {
        // A whorl a little taller than wide, its middle a little high, the
        // ridges not quite round.
        // Seen through the drop it is in: larger in the middle, where the
        // water is deepest.
        float dome = 1.0 - d * d;
        vec2 q = vec2(p.x * 1.1, p.y + 0.04) / PRINT * (1.0 - 0.14 * dome);
        float qa = atan(q.x, -q.y) / TAU;
        if (qa < 0.0) qa += 1.0;
        // Rounder in the middle, a little uneven further out.
        float r = length(q);
        float f = r + smoothstep(0.1, 0.6, r) * (0.03 * sin(qa * TAU * 2.0 + 0.9) + 0.015 * sin(qa * TAU * 3.0 + 2.3));
        // The core a small oval, not a ring.
        float n = f * RIDGES + 0.5;
        float i = floor(n) - 1.0;
        float px = RIDGES * 1.1 / (PRINT * max(radius, 1.0));
        float line = 1.0 - smoothstep(HALF - px, HALF + px, abs(fract(n) - 0.5));
        line *= step(0.0, i) * step(i, RIDGES - 1.0);
        // An edge round the ridge, in turns, a pixel wide.
        float e = px / (TAU * (i + 1.0));
        // The two outer ridges open at the bottom; one break in each of
        // the others but the two inmost, here and there.
        if (i >= RIDGES - 2.0) {
            float open = 0.045 + 0.03 * (i - RIDGES + 2.0);
            line *= smoothstep(open, open + e, apart(qa, 0.5));
        }
        if (i >= 2.0) {
            line *= smoothstep(0.018, 0.018 + e, apart(qa, 0.1 + 0.8 * hash(i)));
        }
        // Shown from the middle out; lit ridge by ridge, clockwise.
        float shown = clamp(print * (RIDGES + 1.0) - i, 0.0, 1.0);
        float k = lit * (RIDGES + 0.05) - i;
        float glow = clamp((k - qa) * 30.0, 0.0, 1.0);
        vec4 dim = vec4(0.22, 0.22, 0.22, 0.22);
        vec4 ridge = mix(dim, accent * (1.0 + 0.45 * flash), glow);
        // Fainter towards the drop's edge.
        float edge = 1.0 - smoothstep(0.82, 1.0, d);
        color += ridge * line * shown * edge * (1.0 - ring);
    }
    gl_FragColor = color * alpha;
}
