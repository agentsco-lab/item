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

// The first setup's circle (setup.rs), drawn over the left panel: a disc,
// or a ring `thickness` of its radius wide, lit in `accent` clockwise from
// the top as far as `progress`, cut into `segments` when there are any.
// `centre` and `radius` in the area's logical px (smithay's `size`), as
// they move smoothly.
uniform vec2 centre;
uniform float radius;
uniform float thickness;
uniform float progress;
uniform float segments;
uniform vec4 base;
uniform vec4 accent;

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
    float a = atan(p.x, -p.y) / 6.2831853;
    if (a < 0.0) a += 1.0;
    if (segments > 0.5) {
        float s = fract(a * segments);
        ring *= smoothstep(0.0, 0.03, s) * (1.0 - smoothstep(0.97, 1.0, s));
    }
    float lit = 1.0 - smoothstep(progress - 0.004, progress, a);
    vec4 color = mix(base, accent, lit);
    gl_FragColor = color * ring * alpha;
}
