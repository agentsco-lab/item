// The wallpaper (wall.frag; the dock's drops see it through themselves,
// dock.frag): a dark ground with soft glows of colour, low and wide, a
// point of the screen in logical px. `shift` moves it a little with the
// ribbon (parallax).
uniform float shift;

float glow(vec2 p, vec2 c, float r) {
    vec2 d = (p - c) / r;
    return exp(-dot(d, d) * 2.2);
}

vec3 wallpaper(vec2 p) {
    p.x += shift;
    vec3 c = vec3(0.050, 0.058, 0.082);
    c += vec3(0.07, 0.24, 0.26) * glow(p, vec2(210.0, 780.0), 560.0);
    c += vec3(0.20, 0.10, 0.32) * glow(p, vec2(1210.0, 160.0), 600.0);
    c += vec3(0.24, 0.13, 0.05) * glow(p, vec2(930.0, 860.0), 360.0);
    c += vec3(0.05, 0.08, 0.20) * glow(p, vec2(470.0, 90.0), 470.0);
    // A slow fall of light from the top.
    c *= 1.0 - 0.18 * (p.y / 900.0);
    return c;
}
