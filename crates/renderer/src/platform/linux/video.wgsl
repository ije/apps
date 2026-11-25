// Converts the Y and UV planes of an NV12 frame to RGB.
struct Frame {
    // The place in the target, in pixels.
    target_rect: vec4<f32>,
    // Rows of the matrix from (Y, U, V, 1) to RGB.
    red: vec4<f32>,
    green: vec4<f32>,
    blue: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var filtered: sampler;
@group(0) @binding(2) var luma: texture_2d<f32>;
@group(0) @binding(3) var chroma: texture_2d<f32>;

@vertex
fn video_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn video_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(luma));
    let at = (position.xy - frame.target_rect.xy) / frame.target_rect.zw * size;
    // Chroma samples are at the left luma sample of each pair.
    let chroma_at = at * 0.5 + vec2<f32>(0.25, 0.0);
    let y = textureSampleLevel(luma, filtered, at / size, 0.0).r;
    let uv = textureSampleLevel(chroma, filtered, chroma_at / vec2<f32>(textureDimensions(chroma)), 0.0).rg;
    let yuv = vec4<f32>(y, uv, 1.0);
    return vec4<f32>(dot(frame.red, yuv), dot(frame.green, yuv), dot(frame.blue, yuv), 1.0);
}
