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

// The mercury dock (dock.rs): each half a drop - a capsule, its ends
// round as surface tension makes them - the two melting into one as they
// near (a smooth union, as drops do), each squashed as jelly is
// (`squash*`: its width and height scaled about its bottom middle).
//
// `metal` 0, water: the wallpaper seen through it, bent as a lens bends
// it (wallpaper.glsl, at the screen point `origin` + the area's); a thin
// light ring along the edge, where the surface reflects (brighter along the
// top); light gathered inside along the bottom, as a drop focuses it; a
// small sharp highlight above left; a soft shadow under it. Seen from
// above: spread thin (`thin*`, by the hinge) a drop grows clearer, bends
// less, its ring and highlight fainter; where one crossed the hinge it
// leaves a wet trace (`trail`, a box, `wet` how wet) drying away. Alive (`life`
// 0 to 1, `time` s): a ripple along the edge, a slow breath, the highlight
// drifting. `metal` 1: soft silver. Logical px of the area (smithay's
// `size`).
uniform vec4 half0;     // x, y, w, h
uniform vec4 half1;
uniform vec2 squash0;   // width, height scales
uniform vec2 squash1;
uniform float radius;
uniform float melt;     // the smooth union's reach, px
uniform vec4 body;      // premultiplied
uniform float shine;
uniform float metal;
uniform vec2 origin;
uniform float time;
uniform float life;
uniform float thin0;
uniform float thin1;
uniform vec4 trail;     // x, y, w, h
uniform float wet;

//_WALLPAPER_

float box(vec2 p, vec4 r, vec2 s) {
    vec2 half_size = 0.5 * r.zw * s;
    // About the bottom middle: squashed down, it stays on the ground.
    vec2 c = vec2(r.x + 0.5 * r.z, r.y + r.w - half_size.y);
    float rr = min(radius, min(half_size.x, half_size.y));
    vec2 q = abs(p - c) - half_size + vec2(rr);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - rr;
}

float field(vec2 p) {
    // Breathing: a little taller and back, each drop in its own time.
    vec2 b0 = squash0 * vec2(1.0 - life * 0.012 * sin(time * 1.7), 1.0 + life * 0.035 * sin(time * 1.7));
    vec2 b1 = squash1 * vec2(1.0 - life * 0.012 * sin(time * 1.7 + 2.1), 1.0 + life * 0.035 * sin(time * 1.7 + 2.1));
    float a = box(p, half0, b0);
    float b = box(p, half1, b1);
    float h = clamp(0.5 + 0.5 * (b - a) / melt, 0.0, 1.0);
    float d = mix(b, a, h) - melt * h * (1.0 - h);
    // The surface trembling along the edge, slowly.
    d += life * (1.5 * sin(p.x * 0.05 + time * 2.6) * sin(p.y * 0.11 - time * 2.0 + p.x * 0.013)
               + 0.6 * sin(p.x * 0.13 - time * 3.3 + p.y * 0.05));
    return d;
}

// The wet trace: a rounded box, its ends round.
float trace(vec2 p) {
    vec2 hs = 0.5 * trail.zw;
    vec2 c = trail.xy + hs;
    float rr = min(hs.x, hs.y);
    vec2 q = abs(p - c) - hs + vec2(rr);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - rr;
}

void main() {
    vec2 p = v_coords * size;
    float d = field(p);
    // Its shadow, a little below and soft.
    float below = field(p - vec2(0.0, 5.0));
    float shadow = (1.0 - smoothstep(-4.0, 10.0, below)) * 0.4;
    float tw = trace(p);
    float wetness = wet * (1.0 - smoothstep(-6.0, 2.0, tw));
    if (d > 1.5 && shadow < 0.004 && wetness < 0.004) {
        discard;
    }
    // How thin the water is here: the nearer drop's.
    float thin = mix(thin0, thin1, step(abs(box(p, half1, squash1)), abs(box(p, half0, squash0))));
    float cover = 1.0 - smoothstep(-0.75, 0.75, d);
    // The surface's slope, from the field.
    float e = 0.75;
    vec2 g = vec2(field(p + vec2(e, 0.0)) - field(p - vec2(e, 0.0)), field(p + vec2(0.0, e)) - field(p - vec2(0.0, e))) / (2.0 * e);
    float depth = 12.0 * (1.0 + 2.5 * thin);
    float t = clamp(-d / depth, 0.0, 1.0);
    float slope = 1.0 - t;
    vec3 n = normalize(vec3(g * slope * 1.6, 1.0));
    vec3 l = normalize(vec3(-0.45 + life * 0.16 * sin(time * 0.9), -0.7 + life * 0.08 * cos(time * 0.7), 0.55));
    vec3 h = normalize(l + vec3(0.0, 0.0, 1.0));
    float inside = max(-d, 0.0);

    // Water.
    float ring = exp(-inside / 1.6) * (0.28 + 0.55 * max(-g.y, 0.0)) * (1.0 - 0.6 * thin);
    float gather = exp(-pow((inside - 6.0) / 4.0, 2.0)) * pow(max(g.y, 0.0), 1.5) * 0.45 * (1.0 - thin);
    float spark = pow(max(dot(n, h), 0.0), mix(140.0, 30.0, thin)) * mix(1.1, 0.3, thin) + pow(max(dot(n, h), 0.0), 24.0) * 0.12;
    float light = (ring + gather + spark) * shine;
    // What is behind, bent: towards the edge it is pulled in from further
    // out, as through a lens.
    vec2 sp = p + origin;
    vec2 bent = sp - g * slope * 26.0 * (1.0 - 0.8 * thin);
    vec3 seen = mix(wallpaper(bent) * 1.12 + body.rgb * 0.35, wallpaper(sp) * 1.04 + body.rgb * 0.12, thin);
    vec4 water = vec4(seen, 1.0) + vec4(0.93, 0.97, 1.0, 1.0) * light;

    // Soft silver.
    float across = clamp((p.y - min(half0.y, half1.y)) / max(max(half0.w, half1.w), 1.0), 0.0, 1.0);
    float tone = mix(0.84, 0.62, across);
    float edge = 1.0 - t;
    float top_line = edge * edge * max(-n.y, 0.0) * 1.6;
    float low_line = edge * edge * max(n.y, 0.0) * 0.7;
    float soft = pow(max(dot(n, h), 0.0), 18.0);
    vec3 silver = vec3(0.94, 0.95, 0.97) * (tone + 0.16 * top_line - 0.1 * low_line) + vec3(0.12) * soft * shine;
    vec4 mercury = vec4(min(silver, vec3(1.0)), 1.0);

    vec4 drop = clamp(mix(water, mercury, metal), 0.0, 1.0) * cover;
    // The wet trace, where the drop is not: the wallpaper a little darker
    // and bent through the film, a faint sheen along its edge.
    vec4 film = vec4(0.0);
    if (wetness > 0.004) {
        float te = 0.75;
        vec2 tg = vec2(trace(p + vec2(te, 0.0)) - trace(p - vec2(te, 0.0)), trace(p + vec2(0.0, te)) - trace(p - vec2(0.0, te))) / (2.0 * te);
        vec3 under_film = wallpaper(sp + tg * 2.0) * 0.9;
        float sheen = exp(-max(-tw, 0.0) / 2.0) * 0.1;
        film = vec4(under_film + vec3(sheen), 1.0) * wetness * 0.55;
    }
    // The shadow and the trace only where the drop is not.
    vec4 under = (vec4(0.0, 0.0, 0.0, shadow) + film) * (1.0 - cover);
    gl_FragColor = (drop + under) * alpha;
}
