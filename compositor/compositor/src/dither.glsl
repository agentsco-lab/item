// Dither (glass.rs's `dithered` puts this in): a gradient in 8 bits a
// channel falls into steps, widest in the dark, where the OLED panels show
// them most. Noise of about one level, triangular (two interleaved-gradient
// samples, fixed to the pixel: still while nothing moves), added at the
// output breaks the steps; after a blur, never before it (a blur takes the
// noise away and the steps come back). One level of an 8-bit channel.
float ign(vec2 p) {
    return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715))));
}

float dither_noise() {
    vec2 p = gl_FragCoord.xy;
    return (ign(p) + ign(p + vec2(23.0, 61.0)) - 1.0) / 255.0;
}

// A colour with the noise; black stays black (nothing lit on an OLED
// where nothing is drawn).
vec3 dither(vec3 c) {
    return max(c + vec3(dither_noise()), vec3(0.0)) * step(0.0001, c.r + c.g + c.b);
}
