#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
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

// Night light (output.rs): the frame on its way to the screen, its colours
// warmed to the colour temperature's white.
uniform vec3 warm;

void main() {
    vec4 c = texture2D(tex, v_coords);
    gl_FragColor = vec4(c.rgb * warm, c.a) * alpha;
}
