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

// The first setup's table (setup.rs), as item/grid draws its own and the
// recovery its screen: dark paper, a grid of squares over both panels as
// one (the lines go on across the hinge). Drawn once while it is still (the
// lamp's light over it is light.frag's, each frame, round the lamp alone).
//
// Drawn over the wallpaper's texture (`tex`, `texl` logical px; its part
// from `src0` is the screen), which only the end uses: the squares take the
// wallpaper's colour under their middles (`mosaic`), then grow small
// (`zoom` down to a physical pixel a square), the lines going as they do,
// until the squares are the wallpaper's pixels.
//
// All in the screen's logical px: the panels `pa` and `pb` (x, y, w, h);
// the table's point of a screen point on `pb` is `gap` back (the hinge
// taken out). A square `sq` wide at zoom 1; `zoom` about `anchor` (the
// table's point that stays).
uniform vec2 texl;
uniform vec2 src0;
uniform vec4 pa;
uniform vec4 pb;
uniform vec2 gap;
uniform float sq;
uniform float zoom;
uniform vec2 anchor;
uniform float mosaic;
// The lines: how strong over the paper, how wide (physical px).
uniform float ink;
uniform float thick;

const vec3 PAPER = vec3(0.141, 0.145, 0.149);
const float SCALE = 2.0;

bool inside(vec2 p, vec4 r) {
    return p.x >= r.x && p.x < r.x + r.z && p.y >= r.y && p.y < r.y + r.w;
}

// How far `v` is from the nearest line at `o + k * s`.
float to_line(float v, float o, float s) {
    float m = mod(v - o, s);
    return min(m, s - m);
}

void main() {
    vec2 p = v_coords * texl - src0;
    bool b = inside(p, pb);
    if (!b && !inside(p, pa)) {
        // The hinge: nothing to see there.
        gl_FragColor = vec4(0.0, 0.0, 0.0, alpha);
        return;
    }
    vec2 t = b ? p - gap : p;
    float s = sq * zoom;
    vec2 o = anchor * (1.0 - zoom);
    // The square this point is in, its middle on the screen.
    vec2 cell = floor((t - o) / s);
    vec2 mid = o + (cell + 0.5) * s;
    vec2 mid_screen = mid + (b ? gap : vec2(0.0));
    vec3 under = texture2D(tex, (mid_screen + src0) / texl).rgb;
    vec3 c = mix(PAPER, under, mosaic);

    // The lines, fading as the squares grow small (gone at 3 px a square).
    float d = min(to_line(t.x, o.x, s), to_line(t.y, o.y, s)) * SCALE;
    float half_w = thick / 2.0;
    float on = clamp(half_w - d + 0.5, 0.0, 1.0);
    float small = smoothstep(3.0, 14.0, s * SCALE);
    c = mix(c, vec3(1.0), on * ink * small);

    gl_FragColor = vec4(dither(c), 1.0) * alpha;
}
