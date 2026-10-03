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

// The locked Duo folded back past flat (output.rs, fold_glass): the near
// panel (the one facing the user, #116) turns to glass and the far one is
// seen through it from behind. `tex` is the lock screen's picture, the whole
// screen; this draws the near panel. `k` is how far folded (0 flat, 1 back
// to back). `side` is 1 when the near panel is the left one (the hinge at
// its right edge), -1 when it is the right one.
//
// A wave of water goes out from the hinge as it folds: past its front dark
// glass, where the far panel (mirrored: seen from behind, its edges dark)
// develops once the fold is complete and fades as it unfolds (`develop`);
// ahead of it the near panel's own picture, receding and darker and
// darker. The front bows out in the middle, as a spreading drop, trembles a
// little; on its crest the picture bends and catches the light, in its
// trough it is shaded. A little glitch midway, none at either end: the
// colours a little apart, now and then a thin band shifted.
uniform vec2 texl;      // the picture, logical px
uniform vec2 near;      // the near panel: x, width
uniform vec2 far;       // the far panel: x, width
uniform float side;
uniform float k;
uniform float t;        // seconds, for the glitch's bands
uniform float develop;  // the far panel developed, once folded all the way

const float RECEDE = 0.08;      // how much smaller the near panel gets
const float FROST = 10.0;       // the blur just behind the front, px
const float BEND = 26.0;        // the crest's bending of the picture, px
const float CREST = 0.045;      // the crest's width, of the panel's
const float BOW = 0.35;         // how far the front's middle leads
const float DARKEN = 0.88;      // the near panel at the end
const float EDGES = 0.65;       // the turning picture's edges at the start
const float SPLIT = 3.0;        // the glitch's colours apart, px at most
const float SHIFT = 10.0;       // its bands moved, px at most

float hash(float n) {
    return fract(sin(n) * 43758.5453);
}

vec3 at(vec2 px) {
    return texture2D(tex, px / texl).rgb;
}

// The colours a little apart sideways: red one way, blue the other.
vec3 split(vec2 px, float d) {
    if (d < 0.05) {
        return at(px);
    }
    return vec3(at(px + vec2(d, 0.0)).r, at(px).g, at(px - vec2(d, 0.0)).b);
}

vec3 frosted(vec2 px, float r) {
    if (r < 0.5) {
        return at(px);
    }
    vec3 c = vec3(0.0);
    for (int i = -1; i <= 1; i++) {
        for (int j = -1; j <= 1; j++) {
            c += at(px + vec2(float(i), float(j)) * r);
        }
    }
    return c / 9.0;
}

// The near panel's picture at p across (0 its outer edge, 1 the hinge).
float near_x(float p) {
    return side > 0.0 ? near.x + p * near.y : near.x + (1.0 - p) * near.y;
}

// The far panel's picture at x from the hinge (0 the hinge, 1 its outer
// edge): mirrored, seen from behind, its edge by the hinge by the hinge.
float far_x(float x) {
    return side > 0.0 ? far.x + x * far.y : far.x + far.y - x * far.y;
}

void main() {
    // Where in the near panel: p across (0 its outer edge, 1 the hinge),
    // y down.
    float u = (v_coords.x * texl.x - near.x) / near.y;
    float p = side > 0.0 ? u : 1.0 - u;
    float y = v_coords.y;

    // The near panel's own picture, receding.
    float scale = 1.0 - RECEDE * k;
    vec2 q = vec2(0.5) + (vec2(p, y) - vec2(0.5)) / scale;
    vec3 mine = vec3(0.0);
    if (q.x >= 0.0 && q.x <= 1.0 && q.y >= 0.0 && q.y <= 1.0) {
        mine = at(vec2(near_x(q.x), q.y * texl.y)) * (1.0 - DARKEN * k * k * (3.0 - 2.0 * k));
    }

    // The glitch: strongest midway, none at either end; a band now and
    // then (a few in a hundred, changing ~12 times a second) moved.
    float g = sin(k * 3.14159);
    g = g * g;
    float row = floor(y * 48.0);
    float n = hash(row * 7.13 + floor(t * 12.0) * 1.37);
    float shift = n > 0.96 ? (hash(row + floor(t * 12.0)) - 0.5) * 2.0 * SHIFT * g : 0.0;

    // The front, out from the hinge: x is how far from it (0 the hinge, 1
    // the outer edge). Bowed - its middle leads - and trembling.
    float x = 1.0 - p;
    float dy = y - 0.5;
    // Held part-way it lives: the front breathes back and forth a little,
    // ripples run along it.
    float life = 4.0 * k * (1.0 - k);
    float front = -0.12 + 1.45 * k - BOW * dy * dy * 4.0 * (1.0 - k)
        + 0.014 * life * sin(t * 1.6) + 0.006 * life * sin(t * 2.7 + y * 3.0)
        + 0.008 * sin(y * 23.0 + t * 4.0) + 0.005 * sin(y * 41.0 - t * 6.0);
    float d = (x - front) / CREST;
    float bump = exp(-d * d);
    // The slope of the crest bends what is seen through it.
    float bend = -2.0 * d * bump * BEND;

    // Behind the front: the far panel, mirrored (its edge by the hinge
    // stays by the hinge), frosted just behind the front, its edges dark.
    float behind = smoothstep(0.6, -0.6, d);
    vec2 px = vec2(far_x(x) + (shift + bend) * side, y * texl.y);
    float r = max(FROST * smoothstep(-6.0, 0.0, d) * (1.0 - k), FROST * (1.0 - develop));
    vec3 seen = r < 0.5 ? split(px, SPLIT * g) : frosted(px, r);
    float edge = smoothstep(0.0, 0.22, min(x, 1.0 - x)) * smoothstep(0.0, 0.14, min(y, 1.0 - y));
    seen *= mix(1.0 - EDGES * mix(1.0, 0.35, k), 1.0, edge);
    // Dark glass until it develops: out of the dark, eased.
    float dv = develop * develop * (3.0 - 2.0 * develop);
    seen = mix(vec3(0.015, 0.018, 0.025), seen, dv);

    // Ahead of it the near panel's picture, bent too where the crest is.
    if (bump > 0.01) {
        float qb = q.x + bend / near.y;
        if (qb >= 0.0 && qb <= 1.0 && q.y >= 0.0 && q.y <= 1.0) {
            mine = at(vec2(near_x(qb), q.y * texl.y)) * (1.0 - DARKEN * k * k * (3.0 - 2.0 * k));
        }
    }
    vec3 col = mix(mine, seen, behind);

    // The crest catches the light; just behind it, a shade.
    col += vec3(0.16) * bump * smoothstep(0.0, 0.08, k) * (1.0 - smoothstep(0.9, 1.0, k));
    float e2 = (d + 1.6) / 1.2;
    col *= 1.0 - 0.18 * exp(-e2 * e2);

    gl_FragColor = vec4(col, 1.0) * alpha;
}
