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

// The mercury dock (dock.rs), seen from above: up to four drops of water,
// each a capsule (its ends round as surface tension makes them), squashed
// as jelly is about its bottom middle (`s*`: width and height scales).
// Two drops apart do not reach for each other (`melt` a pixel, for a clean
// edge); pulled apart they keep a thread between them while it lasts
// (`melt` wider); meeting, they are one (`one`) as the waist between them
// fills (`meet.x`), the join swelling out and back (`meet.y` at `meet.z`).
//
// `metal` 0, water: the wallpaper seen through it, bent as a lens bends it
// (wallpaper.glsl, at the screen point `origin` + the area's); a thin light
// ring along the edge, brighter along the top; light gathered inside along
// the bottom; a small sharp highlight above left; a soft shadow under it;
// wet spots where a drop clung to an edge (`trail`, `wet`). Alive (`life`
// 0 to 1, `time` s): a ripple along the edge, a slow breath, the highlight
// drifting. `metal` 1: soft silver. Logical px of the area (smithay's
// `size`).
uniform vec4 b0;        // x, y, w, h
uniform vec4 b1;
uniform vec4 b2;
uniform vec4 b3;
uniform vec4 s01;       // b0's width and height scales, b1's
uniform vec4 s23;
uniform vec4 one;       // the drops as one, met
uniform vec4 meet;      // fill, bulge, join x, unused
uniform float radius;
uniform float melt;
uniform vec4 body;      // premultiplied
uniform float shine;
uniform float metal;
uniform vec2 origin;
uniform float time;
uniform float life;
uniform vec4 trail;     // x, y, w, h
uniform float wet;

//_WALLPAPER_

float box(vec2 p, vec4 r, vec2 s) {
    vec2 half_size = 0.5 * r.zw * s;
    // About the bottom middle: squashed, it stays on the ground.
    vec2 c = vec2(r.x + 0.5 * r.z, r.y + r.w - half_size.y);
    float rr = min(radius, min(half_size.x, half_size.y));
    vec2 q = abs(p - c) - half_size + vec2(rr);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - rr;
}

float smin(float a, float b, float k) {
    float h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

float field(vec2 p) {
    // Breathing: a little taller and narrower, and back.
    float br = life * sin(time * 1.7);
    vec2 breath = vec2(1.0 - 0.012 * br, 1.0 + 0.035 * br);
    float d = box(p, b0, s01.xy * breath);
    d = smin(d, box(p, b1, s01.zw * breath), melt);
    d = smin(d, box(p, b2, s23.xy * breath), melt);
    d = smin(d, box(p, b3, s23.zw * breath), melt);
    // Met: the waist fills, and the join swells out and back.
    if (meet.x > 0.0) {
        d = mix(d, box(p, one, breath), meet.x);
        float dx = (p.x - meet.z) / 28.0;
        d -= meet.y * exp(-dx * dx);
    }
    // The surface trembling along the edge.
    d += life * (1.5 * sin(p.x * 0.05 + time * 2.6) * sin(p.y * 0.11 - time * 2.0 + p.x * 0.013)
               + 0.6 * sin(p.x * 0.13 - time * 3.3 + p.y * 0.05));
    return d;
}

float trace(vec2 p) {
    vec2 hs = 0.5 * trail.zw;
    vec2 c = trail.xy + hs;
    float rr = min(hs.x, hs.y);
    vec2 q = abs(p - c) - hs + vec2(rr);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - rr;
}

void main() {
    vec2 p = v_coords * size;
    vec2 sp = p + origin;
    float d = field(p);
    float below = field(p - vec2(0.0, 5.0));
    float shadow = (1.0 - smoothstep(-4.0, 10.0, below)) * 0.4;
    float tw = trace(p);
    float wetness = wet * (1.0 - smoothstep(-9.0, 3.0, tw));
    if (d > 1.5 && shadow < 0.004 && wetness < 0.004) {
        discard;
    }
    float cover = 1.0 - smoothstep(-0.75, 0.75, d);
    float e = 0.75;
    vec2 g = vec2(field(p + vec2(e, 0.0)) - field(p - vec2(e, 0.0)), field(p + vec2(0.0, e)) - field(p - vec2(0.0, e))) / (2.0 * e);
    float t = clamp(-d / 12.0, 0.0, 1.0);
    float slope = 1.0 - t;
    vec3 n = normalize(vec3(g * slope * 1.6, 1.0));
    vec3 l = normalize(vec3(-0.45 + life * 0.16 * sin(time * 0.9), -0.7 + life * 0.08 * cos(time * 0.7), 0.55));
    vec3 h = normalize(l + vec3(0.0, 0.0, 1.0));
    float inside = max(-d, 0.0);

    // Water.
    float ring = exp(-inside / 1.6) * (0.28 + 0.55 * max(-g.y, 0.0));
    float gather = exp(-pow((inside - 6.0) / 4.0, 2.0)) * pow(max(g.y, 0.0), 1.5) * 0.45;
    float spark = pow(max(dot(n, h), 0.0), 140.0) * 1.1 + pow(max(dot(n, h), 0.0), 24.0) * 0.12;
    float light = (ring + gather + spark) * shine;
    vec2 bent = sp - g * slope * 26.0;
    vec3 seen = wallpaper(bent) * 1.12 + body.rgb * 0.35;
    vec4 water = vec4(seen, 1.0) + vec4(0.93, 0.97, 1.0, 1.0) * light;

    // Soft silver.
    float across = clamp((p.y - b0.y) / max(b0.w, 1.0), 0.0, 1.0);
    float tone = mix(0.84, 0.62, across);
    float top_line = slope * slope * max(-n.y, 0.0) * 1.6;
    float low_line = slope * slope * max(n.y, 0.0) * 0.7;
    float soft = pow(max(dot(n, h), 0.0), 18.0);
    vec3 silver = vec3(0.94, 0.95, 0.97) * (tone + 0.16 * top_line - 0.1 * low_line) + vec3(0.12) * soft * shine;
    vec4 mercury = vec4(min(silver, vec3(1.0)), 1.0);

    vec4 drop = clamp(mix(water, mercury, metal), 0.0, 1.0) * cover;
    // Wet spots, where the drop is not: the wallpaper a little darker and
    // bent through the film, a faint sheen along the edge.
    vec4 film = vec4(0.0);
    if (wetness > 0.004) {
        float te = 0.75;
        vec2 tg = vec2(trace(p + vec2(te, 0.0)) - trace(p - vec2(te, 0.0)), trace(p + vec2(0.0, te)) - trace(p - vec2(0.0, te))) / (2.0 * te);
        vec3 under_film = wallpaper(sp + tg * 2.0) * 0.86;
        float sheen = exp(-max(-tw, 0.0) / 4.0) * 0.05;
        film = vec4(under_film + vec3(sheen), 1.0) * wetness * 0.5;
    }
    vec4 under = (vec4(0.0, 0.0, 0.0, shadow) + film) * (1.0 - cover);
    gl_FragColor = (drop + under) * alpha;
}
