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

// The dock as two drops of mercury (dock.rs): each half a rounded box,
// the two melting into one as they near (a smooth union, as drops do),
// each squashed as jelly is (`squash*`: its width and height scaled about
// its bottom middle). Mercury (`metal` 1): an opaque mirror, its dome
// reflecting a bright sky above a dark horizon and a dim floor, a hard
// highlight from above left; water (`metal` 0): a clear body, a lit rim, a
// soft highlight. Logical px of the area (smithay's `size`).
uniform vec4 half0;     // x, y, w, h
uniform vec4 half1;
uniform vec2 squash0;   // width, height scales
uniform vec2 squash1;
uniform float radius;
uniform float melt;     // the smooth union's reach, px
uniform vec4 body;      // premultiplied
uniform float shine;
uniform float metal;

float box(vec2 p, vec4 r, vec2 s) {
    vec2 half_size = 0.5 * r.zw * s;
    // About the bottom middle: squashed down, it stays on the ground.
    vec2 c = vec2(r.x + 0.5 * r.z, r.y + r.w - half_size.y);
    vec2 q = abs(p - c) - half_size + vec2(radius);
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

float field(vec2 p) {
    float a = box(p, half0, squash0);
    float b = box(p, half1, squash1);
    float h = clamp(0.5 + 0.5 * (b - a) / melt, 0.0, 1.0);
    return mix(b, a, h) - melt * h * (1.0 - h);
}

void main() {
    vec2 p = v_coords * size;
    float d = field(p);
    if (d > 1.5) {
        discard;
    }
    float cover = 1.0 - smoothstep(-0.75, 0.75, d);
    // The surface: a dome rising from the edge over `depth` px.
    float e = 0.75;
    vec2 g = vec2(field(p + vec2(e, 0.0)) - field(p - vec2(e, 0.0)), field(p + vec2(0.0, e)) - field(p - vec2(0.0, e))) / (2.0 * e);
    // Water a thin film over its edge; mercury a drop, round all over.
    float depth = mix(14.0, 30.0, metal);
    float t = clamp(-d / depth, 0.0, 1.0);
    float slope = 1.0 - t;
    vec3 n = normalize(vec3(g * slope * 1.6, 1.0));
    // Light from above left and in front.
    vec3 l = normalize(vec3(-0.45, -0.7, 0.55));
    vec3 h = normalize(l + vec3(0.0, 0.0, 1.0));
    float spec = pow(max(dot(n, h), 0.0), 36.0);
    float rim = pow(1.0 - n.z, 1.6);
    // Darker inside the drop at its bottom: light through water.
    float low = smoothstep(0.35, 1.0, (p.y - half0.y) / max(half0.w, 1.0));
    vec4 water = body * (1.0 - 0.25 * low);
    water.rgb += vec3(0.75, 0.9, 0.95) * rim * 0.55 * shine;
    water += vec4(1.0) * spec * 0.55 * shine;
    // Mercury: what the mirror sees along the reflected ray (screen y
    // down: a ray going up sees the sky).
    vec3 r = reflect(vec3(0.0, 0.0, -1.0), n);
    float up = -r.y;
    float sky = smoothstep(-0.05, 0.6, up);
    float horizon = 1.0 - smoothstep(0.0, 0.18, abs(up - 0.02));
    float env = mix(0.28, 0.97, sky) - 0.22 * horizon;
    // The floor's glow under the bottom edge, and the flat top a soft grey.
    env += 0.12 * smoothstep(0.2, 0.9, -up);
    env = mix(env, 0.7, t * t * 0.3);
    vec3 silver = vec3(0.93, 0.95, 1.0) * env;
    float hard = pow(max(dot(n, h), 0.0), 90.0);
    silver += vec3(1.0) * hard * 0.9 * shine;
    // A darker line where the mirror turns away, at the very edge.
    silver *= 1.0 - 0.35 * pow(rim, 3.0);
    vec4 mercury = vec4(silver, 1.0);
    gl_FragColor = mix(water, mercury, metal) * cover * alpha;
}
