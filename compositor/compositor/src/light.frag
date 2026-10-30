//_DEFINES_

#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif

//_DITHER_
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

// The lamp's light on the first setup's table (table.frag draws the table,
// once while it is still; this, each frame, only round the lamp): over the
// paper, softly, in the accent; the slits between the squares near it lit,
// wider and stronger the nearer. Drawn over the area `origin` (screen,
// logical px) on; the table's geometry as table.frag has it.
uniform vec2 origin;
uniform vec4 pa;
uniform vec4 pb;
uniform vec2 gap;
uniform float sq;
uniform float zoom;
uniform vec2 anchor;
uniform vec2 lamp;
uniform float reach;
uniform float glow;
uniform vec3 accent;
uniform float thick;

const float SCALE = 2.0;

bool inside(vec2 p, vec4 r) {
    return p.x >= r.x && p.x < r.x + r.z && p.y >= r.y && p.y < r.y + r.w;
}

float to_line(float v, float o, float s) {
    float m = mod(v - o, s);
    return min(m, s - m);
}

void main() {
    vec2 p = origin + v_coords * size;
    bool b = inside(p, pb);
    if (!b && !inside(p, pa)) {
        gl_FragColor = vec4(0.0);
        return;
    }
    vec2 t = b ? p - gap : p;
    vec2 e = (t - lamp) * vec2(1.0, 1.25);
    float q = dot(e, e) / max(reach * reach, 1.0);
    if (q >= 1.0 || glow <= 0.0) {
        gl_FragColor = vec4(0.0);
        return;
    }
    float s = sq * zoom;
    vec2 o = anchor * (1.0 - zoom);
    float d = min(to_line(t.x, o.x, s), to_line(t.y, o.y, s)) * SCALE;
    float small = smoothstep(3.0, 14.0, s * SCALE);
    float r = sqrt(q);
    float k = pow(1.0 - r, 1.6) * glow;
    float kp = pow(1.0 - r, 1.3) * glow;
    // The paper's light, then the line's over it (premultiplied).
    float a1 = 0.10 * kp;
    float lit = clamp(thick / 2.0 + 1.1 * k * SCALE - d + 0.5, 0.0, 1.0);
    float a2 = 0.55 * k * lit * small;
    vec3 c2 = mix(vec3(1.0), accent, k);
    vec3 rgb = c2 * a2 + accent * a1 * (1.0 - a2);
    float a = a2 + a1 * (1.0 - a2);
    gl_FragColor = vec4(rgb, a) * alpha;
}
