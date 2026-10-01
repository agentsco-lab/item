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

//_WALLPAPER_

// The wallpaper over the whole screen, under everything (output.rs).
void main() {
    gl_FragColor = vec4(wallpaper(v_coords * size), 1.0) * alpha;
}
