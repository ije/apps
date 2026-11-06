// WARNING: these structs must match the `repr(C)` structs in `scene.rs` and `frame.rs`, like the
// Metal and HLSL shaders.

// Contrast and gamma correction adapted from https://github.com/microsoft/terminal/blob/1283c0f5b99a2961673249fa77c6b986efb5086c/src/renderer/atlas/dwrite.hlsl
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
fn color_brightness(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.30, 0.59, 0.11));
}

fn light_on_dark_contrast(enhancedContrast: f32, color: vec3<f32>) -> f32 {
    let brightness = color_brightness(color);
    let multiplier = saturate(4.0 * (0.75 - brightness));
    return enhancedContrast * multiplier;
}

fn enhance_contrast(alpha: f32, k: f32) -> f32 {
    return alpha * (k + 1.0) / (alpha * k + 1.0);
}

fn apply_alpha_correction(a: f32, b: f32, g: vec4<f32>) -> f32 {
    let brightness_adjustment = g.x * b + g.y;
    let correction = brightness_adjustment * a + (g.z * b + g.w);
    return a + a * (1.0 - a) * correction;
}

fn apply_contrast_and_gamma_correction(sample: f32, color: vec3<f32>, enhanced_contrast_factor: f32, gamma_ratios: vec4<f32>) -> f32 {
    let enhanced_contrast = light_on_dark_contrast(enhanced_contrast_factor, color);
    let brightness = color_brightness(color);

    let contrasted = enhance_contrast(sample, enhanced_contrast);
    return apply_alpha_correction(contrasted, brightness, gamma_ratios);
}

struct Globals {
    gamma_ratios: vec4<f32>,
    viewport_size: vec2<f32>,
    viewport_origin: vec2<f32>,
    window_size: vec2<f32>,
    grayscale_enhanced_contrast: f32,
    subpixel_enhanced_contrast: f32,
    premultiplied_alpha: u32,
    is_bgr: u32,
    pad: vec2<u32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(2) @binding(0) var t_sprite: texture_2d<f32>;
@group(2) @binding(1) var s_sprite: sampler;

const M_PI_F: f32 = 3.1415926;
const GRAYSCALE_FACTORS: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

struct Bounds {
    origin: vec2<f32>,
    size: vec2<f32>,
}

struct Corners {
    top_left: f32,
    top_right: f32,
    bottom_right: f32,
    bottom_left: f32,
}

struct Radii {
    x: Corners,
    y: Corners,
}

struct Edges {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

struct ContentMask {
    bounds: Bounds,
    rounded: Bounds,
    radii: Radii,
}

// WARNING: four floats, not `vec4<f32>`, which WGSL aligns to 16 bytes.
struct Rgba {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

struct AtlasTextureId {
    index: u32,
    kind: u32,
}

struct AtlasBounds {
    origin: vec2<i32>,
    size: vec2<i32>,
}

struct AtlasTile {
    texture_id: AtlasTextureId,
    tile_id: u32,
    padding: u32,
    bounds: AtlasBounds,
}

fn to_device_position_impl(position: vec2<f32>) -> vec4<f32> {
    let device_position = (position - globals.viewport_origin) / globals.viewport_size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    return vec4<f32>(device_position, 0.0, 1.0);
}

fn to_device_position(unit_vertex: vec2<f32>, bounds: Bounds) -> vec4<f32> {
    let position = unit_vertex * vec2<f32>(bounds.size) + bounds.origin;
    return to_device_position_impl(position);
}

fn to_tile_position(unit_vertex: vec2<f32>, tile: AtlasTile) -> vec2<f32> {
  let atlas_size = vec2<f32>(textureDimensions(t_sprite, 0));
  return (vec2<f32>(tile.bounds.origin) + unit_vertex * vec2<f32>(tile.bounds.size)) / atlas_size;
}

fn distance_from_clip_rect_impl(position: vec2<f32>, clip_bounds: Bounds) -> vec4<f32> {
    let tl = position - clip_bounds.origin;
    let br = clip_bounds.origin + clip_bounds.size - position;
    return vec4<f32>(tl.x, br.x, tl.y, br.y);
}

fn distance_from_clip_rect(unit_vertex: vec2<f32>, bounds: Bounds, clip_bounds: Bounds) -> vec4<f32> {
    let position = unit_vertex * vec2<f32>(bounds.size) + bounds.origin;
    return distance_from_clip_rect_impl(position, clip_bounds);
}

fn rgba(color: Rgba) -> vec4<f32> {
    return vec4<f32>(color.r, color.g, color.b, color.a);
}

fn over(below: vec4<f32>, above: vec4<f32>) -> vec4<f32> {
    let alpha = above.a + below.a * (1.0 - above.a);
    // WARNING: two transparent colors would divide by zero, and blending a NaN is undefined on some GPUs.
    if (alpha <= 0.0) {
        return vec4<f32>(0.0);
    }
    let color = (above.rgb * above.a + below.rgb * below.a * (1.0 - above.a)) / alpha;
    return vec4<f32>(color, alpha);
}

fn gaussian(x: f32, sigma: f32) -> f32{
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * M_PI_F) * sigma);
}

fn erf(v: vec2<f32>) -> vec2<f32> {
    let s = sign(v);
    let a = abs(v);
    let r1 = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    return s - s / (r2 * r2);
}

fn blur_along_x(x: f32, y: f32, sigma: f32, corner: vec2<f32>, half_size: vec2<f32>) -> f32 {
  let delta = min(half_size.y - corner.y - abs(y), 0.0);
  var curved = half_size.x - corner.x + sqrt(max(0.0, corner.x * corner.x - delta * delta));
  if (corner.x != corner.y) {
    curved = half_size.x - corner.x + corner.x * sqrt(max(0.0, 1.0 - delta * delta / (corner.y * corner.y)));
  }
  let integral = 0.5 + 0.5 * erf((x + vec2<f32>(-curved, curved)) * (sqrt(0.5) / sigma));
  return integral.y - integral.x;
}

fn pick_corner(center_to_point: vec2<f32>, half_size: vec2<f32>, radii: Radii) -> vec4<f32> {
    let c = center_to_point;
    let tl = vec2<f32>(radii.x.top_left, radii.y.top_left);
    let tr = vec2<f32>(radii.x.top_right, radii.y.top_right);
    let br = vec2<f32>(radii.x.bottom_right, radii.y.bottom_right);
    let bl = vec2<f32>(radii.x.bottom_left, radii.y.bottom_left);
    var side = select(vec2<f32>(1.0), vec2<f32>(-1.0), c < vec2<f32>(0.0));
    if (all(c < tl - half_size)) {
        side = vec2<f32>(-1.0, -1.0);
    } else if (c.x > half_size.x - tr.x && c.y < tr.y - half_size.y) {
        side = vec2<f32>(1.0, -1.0);
    } else if (all(c > half_size - br)) {
        side = vec2<f32>(1.0, 1.0);
    } else if (c.x < bl.x - half_size.x && c.y > half_size.y - bl.y) {
        side = vec2<f32>(-1.0, 1.0);
    }
    var radius = select(br, tr, side.y < 0.0);
    if (side.x < 0.0) {
        radius = select(bl, tl, side.y < 0.0);
    }
    return vec4<f32>(select(vec2<f32>(0.0), radius, radius.x > 0.0 && radius.y > 0.0), side);
}

fn quad_sdf(point: vec2<f32>, bounds: Bounds, corner_radii: Radii) -> f32 {
    let half_size = bounds.size / 2.0;
    let center = bounds.origin + half_size;
    let center_to_point = point - center;
    let corner = pick_corner(center_to_point, half_size, corner_radii);
    let corner_radius = corner.xy;
    let corner_to_point = corner.zw * center_to_point - half_size;
    let corner_center_to_point = corner_to_point + corner_radius;
    return quad_sdf_impl(corner_center_to_point, corner_radius);
}

fn quad_sdf_impl(corner_center_to_point: vec2<f32>, corner_radius: vec2<f32>) -> f32 {
    if (corner_radius.x == 0.0) {
        return max(corner_center_to_point.x, corner_center_to_point.y);
    } else if (corner_radius.x != corner_radius.y) {
        if (corner_center_to_point.x <= 0.0 || corner_center_to_point.y <= 0.0) {
            return max(corner_center_to_point.x - corner_radius.x, corner_center_to_point.y - corner_radius.y);
        }
        let k0 = length(corner_center_to_point / corner_radius);
        let k1 = length(corner_center_to_point / (corner_radius * corner_radius));
        return k0 * (k0 - 1.0) / k1;
    } else {
        let signed_distance_to_inset_quad =
            length(max(vec2<f32>(0.0), corner_center_to_point)) +
            min(0.0, max(corner_center_to_point.x, corner_center_to_point.y));

        return signed_distance_to_inset_quad - corner_radius.x;
    }
}

fn window_position(position: vec4<f32>) -> vec2<f32> {
    return position.xy + globals.viewport_origin;
}

fn mask_alpha(position: vec2<f32>, mask: ContentMask) -> f32 {
    if (mask.radii.x.top_left == 0.0 && mask.radii.x.top_right == 0.0 &&
            mask.radii.x.bottom_right == 0.0 && mask.radii.x.bottom_left == 0.0) {
        return 1.0;
    }
    return saturate(0.5 - quad_sdf(position, mask.rounded, mask.radii));
}

fn blend_color(color: vec4<f32>, alpha_factor: f32) -> vec4<f32> {
    let alpha = color.a * alpha_factor;
    let multiplier = select(1.0, alpha, globals.premultiplied_alpha != 0u);
    return vec4<f32>(color.rgb * multiplier, alpha);
}

struct Quad {
    order: u32,
    border_style: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    background: Rgba,
    border_color: Rgba,
    corner_radii: Radii,
    border_widths: Edges,
}
@group(1) @binding(0) var<storage, read> b_quads: array<Quad>;

struct QuadVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) border_color: vec4<f32>,
    @location(1) @interpolate(flat) quad_id: u32,
    // WARNING: Naga has no `clip_distance`, so fragments clip with these.
    @location(2) clip_distances: vec4<f32>,
    @location(3) @interpolate(flat) background_color: vec4<f32>,
}

@vertex
fn vs_quad(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> QuadVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let quad = b_quads[instance_id];

    var out = QuadVarying();
    out.position = to_device_position(unit_vertex, quad.bounds);

    out.background_color = rgba(quad.background);
    out.border_color = rgba(quad.border_color);
    out.quad_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, quad.bounds, quad.content_mask.bounds);
    return out;
}

@fragment
fn fs_quad(input: QuadVarying) -> @location(0) vec4<f32> {
    let color = quad_color(input);
    let alpha = mask_alpha(window_position(input.position), b_quads[input.quad_id].content_mask);
    let multiplier = select(1.0, alpha, globals.premultiplied_alpha != 0u);
    return vec4<f32>(color.rgb * multiplier, color.a * alpha);
}

fn quad_color(input: QuadVarying) -> vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let quad = b_quads[input.quad_id];

    let background_color = input.background_color;

    let unrounded = quad.corner_radii.x.top_left == 0.0 &&
        quad.corner_radii.x.bottom_left == 0.0 &&
        quad.corner_radii.x.top_right == 0.0 &&
        quad.corner_radii.x.bottom_right == 0.0;

    if (quad.border_widths.top == 0.0 &&
            quad.border_widths.left == 0.0 &&
            quad.border_widths.right == 0.0 &&
            quad.border_widths.bottom == 0.0 &&
            unrounded) {
        return blend_color(background_color, 1.0);
    }

    let size = quad.bounds.size;
    let half_size = size / 2.0;
    let point = window_position(input.position) - quad.bounds.origin;
    let center_to_point = point - half_size;

    let antialias_threshold = 0.5;

    let corner = pick_corner(center_to_point, half_size, quad.corner_radii);
    let corner_radius = corner.xy;
    let left = corner.z < 0.0;
    let top = corner.w < 0.0;

    let border = vec2<f32>(
        select(quad.border_widths.right, quad.border_widths.left, left),
        select(quad.border_widths.bottom, quad.border_widths.top, top));

    // WARNING: zero-width borders turn negative so that `inner_sdf >= antialias_threshold`, which draws no
    // anti-aliasing pixels for them.
    let reduced_border =
        vec2<f32>(select(border.x, -antialias_threshold, border.x == 0.0),
                  select(border.y, -antialias_threshold, border.y == 0.0));

    let far_border = vec2<f32>(
        select(quad.border_widths.left, quad.border_widths.right, left),
        select(quad.border_widths.top, quad.border_widths.bottom, top));
    let reduced_far_border = select(far_border, vec2<f32>(-antialias_threshold), far_border == vec2<f32>(0.0));

    let corner_to_point = corner.zw * center_to_point - half_size;

    let corner_center_to_point = corner_to_point + corner_radius;

    let is_near_rounded_corner =
            corner_center_to_point.x >= 0 &&
            corner_center_to_point.y >= 0;

    let straight_border_inner_corner_to_point =
        max(corner_to_point + reduced_border, reduced_far_border - size - corner_to_point);

    let is_beyond_inner_straight_border =
            straight_border_inner_corner_to_point.x > 0 ||
            straight_border_inner_corner_to_point.y > 0;

    let is_within_inner_straight_border =
        straight_border_inner_corner_to_point.x < -antialias_threshold &&
        straight_border_inner_corner_to_point.y < -antialias_threshold;

    if (is_within_inner_straight_border && !is_near_rounded_corner) {
        return blend_color(background_color, 1.0);
    }

    let outer_sdf = quad_sdf_impl(corner_center_to_point, corner_radius);

    var inner_sdf = 0.0;
    if (corner_center_to_point.x <= 0 || corner_center_to_point.y <= 0) {
        inner_sdf = -max(straight_border_inner_corner_to_point.x,
                         straight_border_inner_corner_to_point.y);
    } else if (is_beyond_inner_straight_border) {
        inner_sdf = -1.0;
    } else if (reduced_border.x == reduced_border.y && corner_radius.x == corner_radius.y) {
        inner_sdf = -(outer_sdf + reduced_border.x);
    } else {
        let ellipse_radii = max(vec2<f32>(0.0), corner_radius - reduced_border);
        inner_sdf = quarter_ellipse_sdf(corner_center_to_point, ellipse_radii);
    }

    let border_sdf = max(inner_sdf, outer_sdf);

    var color = background_color;
    if (border_sdf < antialias_threshold) {
        var border_color = input.border_color;

        let sides = quad.border_style >> 8u;
        if (sides != 0u) {
            let widths = vec4<f32>(quad.border_widths.top, quad.border_widths.right,
                quad.border_widths.bottom, quad.border_widths.left);
            let depths = vec4<f32>(point.y, size.x - point.x, size.y - point.y, point.x);
            var coverage = 0.0;
            for (var a = 0u; a < 4u; a++) {
                if ((sides & (1u << a)) == 0u || widths[a] <= 0.0) {
                    continue;
                }
                var nearest = 1e6;
                for (var b = 0u; b < 4u; b++) {
                    if ((sides & (1u << b)) != 0u || widths[b] <= 0.0) {
                        continue;
                    }
                    let scale = select(widths[a] + widths[b], length(vec2<f32>(widths[a], widths[b])), (a + b) % 2u == 1u);
                    nearest = min(nearest, (depths[b] * widths[a] - depths[a] * widths[b]) / scale);
                }
                coverage = max(coverage, saturate(0.5 + nearest));
            }
            border_color.a *= coverage;
        }

        let inner_bounds = Bounds(
            quad.bounds.origin + vec2<f32>(quad.border_widths.left, quad.border_widths.top),
            max(vec2<f32>(0.0), quad.bounds.size - vec2<f32>(
                quad.border_widths.left + quad.border_widths.right,
                quad.border_widths.top + quad.border_widths.bottom)));
        let inner_radii = Radii(
            Corners(
                max(0.0, quad.corner_radii.x.top_left - quad.border_widths.left),
                max(0.0, quad.corner_radii.x.top_right - quad.border_widths.right),
                max(0.0, quad.corner_radii.x.bottom_right - quad.border_widths.right),
                max(0.0, quad.corner_radii.x.bottom_left - quad.border_widths.left)),
            Corners(
                max(0.0, quad.corner_radii.y.top_left - quad.border_widths.top),
                max(0.0, quad.corner_radii.y.top_right - quad.border_widths.top),
                max(0.0, quad.corner_radii.y.bottom_right - quad.border_widths.bottom),
                max(0.0, quad.corner_radii.y.bottom_left - quad.border_widths.bottom)));
        let inner_depth = quad_sdf(point + quad.bounds.origin, inner_bounds, inner_radii);

        if ((quad.border_style & 255u) == 3u) {
            let depth = -outer_sdf;
            let width = depth + inner_depth;
            border_color.a *= saturate(antialias_threshold - min(depth - width / 3.0, width * 2.0 / 3.0 - depth));
        }

        if ((quad.border_style & 255u) == 1u || (quad.border_style & 255u) == 2u) {
            var t = 0.0;

            var max_t = 0.0;

            let dotted = (quad.border_style & 255u) == 2u;
            let dash_length_per_width = select(2.0, 1.0, dotted);
            let dash_gap_per_width = 1.0;
            let dash_period_per_width = dash_length_per_width + dash_gap_per_width;

            var dash_velocity = 0.0;

            let dv_numerator = 1.0 / dash_period_per_width;

            if (unrounded) {
                let is_horizontal =
                        corner_center_to_point.x <
                        corner_center_to_point.y;

                let dashed_border = vec2<f32>(
                        max(
                            quad.border_widths.bottom,
                            quad.border_widths.top,
                        ),
                        max(
                            quad.border_widths.right,
                            quad.border_widths.left,
                        )
                   );

                let border_width = select(dashed_border.y, dashed_border.x, is_horizontal);
                dash_velocity = dv_numerator / border_width;
                t = select(point.y, point.x, is_horizontal) * dash_velocity;
                max_t = select(size.y, size.x, is_horizontal) * dash_velocity;
            } else {
                let r_tr = vec2<f32>(quad.corner_radii.x.top_right, quad.corner_radii.y.top_right);
                let r_br = vec2<f32>(quad.corner_radii.x.bottom_right, quad.corner_radii.y.bottom_right);
                let r_bl = vec2<f32>(quad.corner_radii.x.bottom_left, quad.corner_radii.y.bottom_left);
                let r_tl = vec2<f32>(quad.corner_radii.x.top_left, quad.corner_radii.y.top_left);

                let w_t = quad.border_widths.top;
                let w_r = quad.border_widths.right;
                let w_b = quad.border_widths.bottom;
                let w_l = quad.border_widths.left;

                let dv_t = select(dv_numerator / w_t, 0.0, w_t <= 0.0);
                let dv_r = select(dv_numerator / w_r, 0.0, w_r <= 0.0);
                let dv_b = select(dv_numerator / w_b, 0.0, w_b <= 0.0);
                let dv_l = select(dv_numerator / w_l, 0.0, w_l <= 0.0);

                let s_t = (size.x - r_tl.x - r_tr.x) * dv_t;
                let s_r = (size.y - r_tr.y - r_br.y) * dv_r;
                let s_b = (size.x - r_br.x - r_bl.x) * dv_b;
                let s_l = (size.y - r_bl.y - r_tl.y) * dv_l;

                let corner_dash_velocity_tr = corner_dash_velocity(dv_t, dv_r);
                let corner_dash_velocity_br = corner_dash_velocity(dv_b, dv_r);
                let corner_dash_velocity_bl = corner_dash_velocity(dv_b, dv_l);
                let corner_dash_velocity_tl = corner_dash_velocity(dv_t, dv_l);

                let middle = select(0.0, 0.5, dotted);
                let c_tr = max(0.0, (r_tr.x + r_tr.y) / 2.0 - middle * max(w_t, w_r)) * (M_PI_F / 2.0) * corner_dash_velocity_tr;
                let c_br = max(0.0, (r_br.x + r_br.y) / 2.0 - middle * max(w_b, w_r)) * (M_PI_F / 2.0) * corner_dash_velocity_br;
                let c_bl = max(0.0, (r_bl.x + r_bl.y) / 2.0 - middle * max(w_b, w_l)) * (M_PI_F / 2.0) * corner_dash_velocity_bl;
                let c_tl = max(0.0, (r_tl.x + r_tl.y) / 2.0 - middle * max(w_t, w_l)) * (M_PI_F / 2.0) * corner_dash_velocity_tl;

                let upto_tr = s_t;
                let upto_r = upto_tr + c_tr;
                let upto_br = upto_r + s_r;
                let upto_b = upto_br + c_br;
                let upto_bl = upto_b + s_b;
                let upto_l = upto_bl + c_bl;
                let upto_tl = upto_l + s_l;
                max_t = upto_tl + c_tl;

                if (is_near_rounded_corner) {
                    let radians = atan2(corner_center_to_point.y / corner_radius.y,
                                        corner_center_to_point.x / corner_radius.x);
                    let corner_t = radians * max(0.0, (corner_radius.x + corner_radius.y) / 2.0 - middle * max(border.x, border.y));

                    if (!left) {
                        if (top) {
                            dash_velocity = corner_dash_velocity_tr;
                            t = upto_r - corner_t * dash_velocity;
                        } else {
                            dash_velocity = corner_dash_velocity_br;
                            t = upto_br + corner_t * dash_velocity;
                        }
                    } else {
                        if (!top) {
                            dash_velocity = corner_dash_velocity_bl;
                            t = upto_l - corner_t * dash_velocity;
                        } else {
                            dash_velocity = corner_dash_velocity_tl;
                            t = upto_tl + corner_t * dash_velocity;
                        }
                    }
                } else {
                    let is_horizontal =
                            corner_center_to_point.x <
                            corner_center_to_point.y;
                    if (is_horizontal) {
                        if (top) {
                            dash_velocity = dv_t;
                            t = (point.x - r_tl.x) * dash_velocity;
                        } else {
                            dash_velocity = dv_b;
                            t = upto_bl - (point.x - r_bl.x) * dash_velocity;
                        }
                    } else {
                        if (left) {
                            dash_velocity = dv_l;
                            t = upto_tl - (point.y - r_tl.y) * dash_velocity;
                        } else {
                            dash_velocity = dv_r;
                            t = upto_r + (point.y - r_tr.y) * dash_velocity;
                        }
                    }
                }
            }

            let dash_length = dash_length_per_width / dash_period_per_width;

            max_t -= select(0.0, dash_length, unrounded);
            if (max_t >= 1.0) {
                let dash_count = floor(max_t);
                let dash_period = max_t / dash_count;
                if (dotted) {
                    border_color.a *= dot_alpha(t, dash_period, dash_length, dash_velocity, -outer_sdf, inner_depth, antialias_threshold);
                } else {
                    border_color.a *= dash_alpha(
                        t,
                        dash_period,
                        dash_length,
                        dash_velocity,
                        antialias_threshold);
                }
            } else if (unrounded) {
                let dash_gap = max_t - dash_length;
                if (dash_gap > 0.0) {
                    let dash_period = dash_length + dash_gap;
                    border_color.a *= dash_alpha(
                        t,
                        dash_period,
                        dash_length,
                        dash_velocity,
                        antialias_threshold);
                }
            }
        }

        let blended_border = over(background_color, border_color);
        color = mix(background_color, blended_border,
                    saturate(antialias_threshold - inner_sdf));
    }

    return blend_color(color, saturate(antialias_threshold - outer_sdf));
}

fn corner_dash_velocity(dv1: f32, dv2: f32) -> f32 {
    if (dv1 == 0.0) {
        return dv2;
    } else if (dv2 == 0.0) {
        return dv1;
    } else {
        return min(dv1, dv2);
    }
}

fn dash_alpha(t: f32, period: f32, length: f32, dash_velocity: f32, antialias_threshold: f32) -> f32 {
    let half_period = period / 2;
    let half_length = length / 2;
    let centered = fmod(t + half_period - half_length, period) - half_period;
    let signed_distance = abs(centered) - half_length;
    return saturate(antialias_threshold - signed_distance / dash_velocity);
}

fn dot_alpha(t: f32, period: f32, dot_length: f32, dash_velocity: f32, depth: f32, inner_depth: f32, antialias_threshold: f32) -> f32 {
    let width = depth + inner_depth;
    let half_period = period / 2;
    let half_length = dot_length / 2;
    let along = (fmod(t + half_period - half_length, period) - half_period) / dash_velocity;
    let across = depth - width / 2.0;
    return saturate(antialias_threshold - (length(vec2<f32>(along, across)) - width / 2.0));
}

fn quarter_ellipse_sdf(point: vec2<f32>, radii: vec2<f32>) -> f32 {
    let circle_vec = point / radii;
    let unit_circle_sdf = length(circle_vec) - 1.0;
    return unit_circle_sdf * (radii.x + radii.y) * -0.5;
}

fn fmod(a: f32, b: f32) -> f32 {
    return a - b * trunc(a / b);
}

struct Shadow {
    order: u32,
    blur_radius: f32,
    bounds: Bounds,
    corner_radii: Radii,
    content_mask: ContentMask,
    color: Rgba,
    element_bounds: Bounds,
    element_corner_radii: Radii,
    inset: u32,
    pad: u32,
}
@group(1) @binding(0) var<storage, read> b_shadows: array<Shadow>;

struct ShadowVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) shadow_id: u32,
    // WARNING: Naga has no `clip_distance`, so fragments clip with these.
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_shadow(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> ShadowVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    var shadow = b_shadows[instance_id];

    var geometry: Bounds;
    if (shadow.inset != 0u) {
        geometry = shadow.element_bounds;
    } else {
        let margin = 3.0 * shadow.blur_radius;
        geometry = shadow.bounds;
        geometry.origin -= vec2<f32>(margin);
        geometry.size += 2.0 * vec2<f32>(margin);
    }

    var out = ShadowVarying();
    out.position = to_device_position(unit_vertex, geometry);
    out.color = rgba(shadow.color);
    out.shadow_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, geometry, shadow.content_mask.bounds);
    return out;
}

@fragment
fn fs_shadow(input: ShadowVarying) -> @location(0) vec4<f32> {
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let shadow = b_shadows[input.shadow_id];
    let half_size = shadow.bounds.size / 2.0;
    let center = shadow.bounds.origin + half_size;
    let position = window_position(input.position);
    let center_to_point = position - center;

    let corner_radius = pick_corner(center_to_point, half_size, shadow.corner_radii).xy;

    var alpha: f32;
    if (shadow.blur_radius == 0.0) {
        let distance = quad_sdf(position, shadow.bounds, shadow.corner_radii);
        alpha = saturate(0.5 - distance);
    } else {
        let low = center_to_point.y - half_size.y;
        let high = center_to_point.y + half_size.y;
        let start = clamp(-3.0 * shadow.blur_radius, low, high);
        let end = clamp(3.0 * shadow.blur_radius, low, high);

        let step = (end - start) / 4.0;
        var y = start + step * 0.5;
        alpha = 0.0;
        for (var i = 0; i < 4; i += 1) {
            let blur = blur_along_x(center_to_point.x, center_to_point.y - y,
                shadow.blur_radius, corner_radius, half_size);
            alpha +=  blur * gaussian(y, shadow.blur_radius) * step;
            y += step;
        }
    }

    let inside = saturate(0.5 - quad_sdf(position, shadow.element_bounds, shadow.element_corner_radii));
    if (shadow.inset != 0u) {
        alpha = (1.0 - alpha) * inside;
    } else {
        alpha *= 1.0 - inside;
    }

    return blend_color(input.color, alpha * mask_alpha(position, shadow.content_mask));
}

struct Underline {
    order: u32,
    wavy: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    color: Rgba,
    thickness: f32,
    pad: u32,
}
@group(1) @binding(0) var<storage, read> b_underlines: array<Underline>;

struct UnderlineVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) underline_id: u32,
    // WARNING: Naga has no `clip_distance`, so fragments clip with these.
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_underline(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> UnderlineVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let underline = b_underlines[instance_id];

    var out = UnderlineVarying();
    out.position = to_device_position(unit_vertex, underline.bounds);
    out.color = rgba(underline.color);
    out.underline_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, underline.bounds, underline.content_mask.bounds);
    return out;
}

@fragment
fn fs_underline(input: UnderlineVarying) -> @location(0) vec4<f32> {
    const WAVE_FREQUENCY: f32 = 2.0;
    const WAVE_HEIGHT_RATIO: f32 = 0.8;

    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let underline = b_underlines[input.underline_id];
    let masked = mask_alpha(window_position(input.position), underline.content_mask);
    if (underline.wavy == 0u)
    {
        return blend_color(input.color, masked);
    }

    let half_thickness = underline.thickness * 0.5;

    let st = (window_position(input.position) - underline.bounds.origin) / underline.bounds.size.y - vec2<f32>(0.0, 0.5);
    let frequency = M_PI_F * WAVE_FREQUENCY * underline.thickness / underline.bounds.size.y;
    let amplitude = (underline.thickness * WAVE_HEIGHT_RATIO) / underline.bounds.size.y;

    let sine = sin(st.x * frequency) * amplitude;
    let dSine = cos(st.x * frequency) * amplitude * frequency;
    let distance = (st.y - sine) / sqrt(1.0 + dSine * dSine);
    let distance_in_pixels = distance * underline.bounds.size.y;
    let distance_from_top_border = distance_in_pixels - half_thickness;
    let distance_from_bottom_border = distance_in_pixels + half_thickness;
    let alpha = saturate(0.5 - max(-distance_from_bottom_border, distance_from_top_border));
    return blend_color(input.color, alpha * masked);
}

struct MonochromeSprite {
    order: u32,
    pad: u32,
    tile: AtlasTile,
    bounds: Bounds,
    content_mask: ContentMask,
    color: Rgba,
}
@group(1) @binding(0) var<storage, read> b_mono_sprites: array<MonochromeSprite>;

struct MonoSpriteVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(3) clip_distances: vec4<f32>,
    @location(4) @interpolate(flat) sprite_id: u32,
}

@vertex
fn vs_mono_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_mono_sprites[instance_id];

    var out = MonoSpriteVarying();
    out.position = to_device_position(unit_vertex, sprite.bounds);

    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.color = rgba(sprite.color);
    out.clip_distances = distance_from_clip_rect(unit_vertex, sprite.bounds, sprite.content_mask.bounds);
    out.sprite_id = instance_id;
    return out;
}

@fragment
fn fs_mono_sprite(input: MonoSpriteVarying) -> @location(0) vec4<f32> {
    let sample = textureSample(t_sprite, s_sprite, input.tile_position).r;
    let alpha_corrected = apply_contrast_and_gamma_correction(sample, input.color.rgb, globals.grayscale_enhanced_contrast, globals.gamma_ratios);

    // WARNING: alpha clip after using the derivatives.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let masked = mask_alpha(window_position(input.position), b_mono_sprites[input.sprite_id].content_mask);
    return blend_color(input.color, alpha_corrected * masked);
}

struct PolychromeSprite {
    order: u32,
    grayscale: u32,
    tile: AtlasTile,
    opacity: f32,
    pad: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    corner_radii: Radii,
}
@group(1) @binding(0) var<storage, read> b_poly_sprites: array<PolychromeSprite>;

struct PolySpriteVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) sprite_id: u32,
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_poly_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> PolySpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_poly_sprites[instance_id];

    var out = PolySpriteVarying();
    out.position = to_device_position(unit_vertex, sprite.bounds);
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.sprite_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, sprite.bounds, sprite.content_mask.bounds);
    return out;
}

@fragment
fn fs_poly_sprite(input: PolySpriteVarying) -> @location(0) vec4<f32> {
    let sample = textureSample(t_sprite, s_sprite, input.tile_position);
    // WARNING: alpha clip after using the derivatives.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let sprite = b_poly_sprites[input.sprite_id];
    let position = window_position(input.position);
    let rx = sprite.corner_radii.x;
    let square = rx.top_left == 0.0 && rx.top_right == 0.0 && rx.bottom_right == 0.0 && rx.bottom_left == 0.0;
    var corners = 1.0;
    if (!square) {
        corners = saturate(0.5 - quad_sdf(position, sprite.bounds, sprite.corner_radii));
    }

    var color = sample;
    if (sprite.grayscale != 0u) {
        let grayscale = dot(color.rgb, GRAYSCALE_FACTORS);
        color = vec4<f32>(vec3<f32>(grayscale), sample.a);
    }
    return blend_color(color, sprite.opacity * corners * mask_alpha(position, sprite.content_mask));
}

struct Composite {
    transform: mat4x4<f32>,
    color_matrix: mat4x4<f32>,
    color_offset: vec4<f32>,
    shadow_color: vec4<f32>,
    bounds: vec4<f32>,
    clip: vec4<f32>,
    clip_radii: vec4<f32>,
    clip_radii_y: vec4<f32>,
    mask_rect: vec4<f32>,
    mask_tile: vec4<f32>,
    content_mask: vec4<f32>,
    rounded_mask: vec4<f32>,
    rounded_radii: vec4<f32>,
    rounded_radii_y: vec4<f32>,
    shadow: vec4<f32>,
    source_rect: vec4<f32>,
    backdrop_rect: vec4<f32>,
    opacity: f32,
    blur: f32,
    flags: u32,
    blend: u32,
}
@group(1) @binding(0) var<storage, read> b_composites: array<Composite>;
@group(2) @binding(2) var t_mask: texture_2d<f32>;
@group(2) @binding(3) var t_backdrop: texture_2d<f32>;

const COMPOSITE_MATRIX: u32 = 1u;
const COMPOSITE_CLIP: u32 = 2u;
const COMPOSITE_MASK: u32 = 4u;
const COMPOSITE_SHADOW: u32 = 8u;
const COMPOSITE_BACKDROP: u32 = 16u;
const COMPOSITE_BLEND: u32 = 32u;

struct CompositeVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) composite_id: u32,
}

@vertex
fn vs_composite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> CompositeVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let c = b_composites[instance_id];
    let local = c.bounds.xy + unit_vertex * c.bounds.zw;
    let p = c.transform * vec4<f32>(local, 0.0, 1.0);
    var out = CompositeVarying();
    let target_position = p.xy - globals.viewport_origin * p.w;
    out.position = vec4<f32>(
        target_position.x / globals.viewport_size.x * 2.0 - p.w,
        p.w - target_position.y / globals.viewport_size.y * 2.0,
        0.0,
        p.w,
    );
    out.local = local;
    out.composite_id = instance_id;
    return out;
}

fn sample_layer(position: vec2<f32>, rect: vec4<f32>) -> vec4<f32> {
    let uv = (position - rect.xy) / rect.zw;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(t_sprite, s_sprite, uv, 0.0);
}

fn gaussian_sample(position: vec2<f32>, direction: vec2<f32>, sigma: f32, texel: f32, rect: vec4<f32>) -> vec4<f32> {
    if (sigma < 0.25) {
        return sample_layer(position, rect);
    }
    let radius = ceil(3.0 * sigma / texel);
    let taps = min(radius, 32.0);
    let step = radius / taps * texel;
    let q = exp(-(step * step) / (2.0 * sigma * sigma));
    var sum = sample_layer(position, rect);
    var weight = 1.0;
    var w = 1.0;
    var g = q;
    for (var i = 1.0; i <= taps; i += 1.0) {
        w *= g;
        g *= q * q;
        let offset = direction * (i * step);
        sum += (sample_layer(position + offset, rect) + sample_layer(position - offset, rect)) * w;
        weight += 2.0 * w;
    }
    return sum / weight;
}

fn luminosity(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(0.3, 0.59, 0.11));
}

fn clip_color(color: vec3<f32>) -> vec3<f32> {
    let l = luminosity(color);
    let n = min(min(color.r, color.g), color.b);
    let x = max(max(color.r, color.g), color.b);
    var c = color;
    if (n < 0.0) {
        c = l + (c - l) * l / (l - n);
    }
    if (x > 1.0) {
        c = l + (c - l) * (1.0 - l) / (x - l);
    }
    return c;
}

fn set_luminosity(color: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(color + (l - luminosity(color)));
}

fn saturation(color: vec3<f32>) -> f32 {
    return max(max(color.r, color.g), color.b) - min(min(color.r, color.g), color.b);
}

fn set_saturation(color: vec3<f32>, s: f32) -> vec3<f32> {
    let n = min(min(color.r, color.g), color.b);
    let x = max(max(color.r, color.g), color.b);
    if (x <= n) {
        return vec3<f32>(0.0);
    }
    return (color - n) * s / (x - n);
}

fn hard_light(cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    return select(cb + 2.0 * cs - 1.0 - cb * (2.0 * cs - 1.0), cb * 2.0 * cs, cs <= vec3<f32>(0.5));
}

fn soft_light(cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    let d = select(sqrt(cb), ((16.0 * cb - 12.0) * cb + 4.0) * cb, cb <= vec3<f32>(0.25));
    return select(cb + (2.0 * cs - 1.0) * (d - cb), cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb), cs <= vec3<f32>(0.5));
}

fn blend_mode(cb: vec3<f32>, cs: vec3<f32>, mode: u32) -> vec3<f32> {
    switch (mode) {
        case 1u: { return cb * cs; }
        case 2u: { return cb + cs - cb * cs; }
        case 3u: { return hard_light(cs, cb); }
        case 4u: { return min(cb, cs); }
        case 5u: { return max(cb, cs); }
        case 6u: {
            return select(select(min(vec3<f32>(1.0), cb / max(1.0 - cs, vec3<f32>(1e-6))), vec3<f32>(1.0), cs >= vec3<f32>(1.0)), vec3<f32>(0.0), cb <= vec3<f32>(0.0));
        }
        case 7u: {
            return select(select(1.0 - min(vec3<f32>(1.0), (1.0 - cb) / max(cs, vec3<f32>(1e-6))), vec3<f32>(0.0), cs <= vec3<f32>(0.0)), vec3<f32>(1.0), cb >= vec3<f32>(1.0));
        }
        case 8u: { return hard_light(cb, cs); }
        case 9u: { return soft_light(cb, cs); }
        case 10u: { return abs(cb - cs); }
        case 11u: { return cb + cs - 2.0 * cb * cs; }
        case 12u: { return set_luminosity(set_saturation(cs, saturation(cb)), luminosity(cb)); }
        case 13u: { return set_luminosity(set_saturation(cb, saturation(cs)), luminosity(cb)); }
        case 14u: { return set_luminosity(cs, luminosity(cb)); }
        case 15u: { return set_luminosity(cb, luminosity(cs)); }
        default: { return cs; }
    }
}

@fragment
fn fs_composite(input: CompositeVarying) -> @location(0) vec4<f32> {
    let c = b_composites[input.composite_id];
    let target_position = window_position(input.position);
    if (any(target_position < c.content_mask.xy) || any(target_position > c.content_mask.xy + c.content_mask.zw)) {
        return vec4<f32>(0.0);
    }

    var color: vec4<f32>;
    if ((c.flags & COMPOSITE_BACKDROP) != 0u) {
        color = gaussian_sample(target_position, vec2<f32>(0.0, 1.0), c.blur, 1.0, c.source_rect);
    } else if ((c.flags & COMPOSITE_SHADOW) != 0u) {
        color = c.shadow_color * gaussian_sample(input.local - c.shadow.xy, vec2<f32>(0.0, 1.0), c.blur, 1.0, c.source_rect).a;
    } else {
        color = gaussian_sample(input.local, vec2<f32>(0.0, 1.0), c.blur, 1.0, c.source_rect);
    }

    if ((c.flags & COMPOSITE_MATRIX) != 0u && color.a > 0.0) {
        let straight = vec4<f32>(color.rgb / color.a, color.a);
        let mapped = saturate(c.color_matrix * straight + c.color_offset);
        color = vec4<f32>(mapped.rgb * mapped.a, mapped.a);
    }

    var coverage = c.opacity * mask_alpha(target_position, ContentMask(
        Bounds(c.content_mask.xy, c.content_mask.zw),
        Bounds(c.rounded_mask.xy, c.rounded_mask.zw),
        Radii(
            Corners(c.rounded_radii.x, c.rounded_radii.y, c.rounded_radii.z, c.rounded_radii.w),
            Corners(c.rounded_radii_y.x, c.rounded_radii_y.y, c.rounded_radii_y.z, c.rounded_radii_y.w),
        ),
    ));
    if ((c.flags & COMPOSITE_CLIP) != 0u) {
        let bounds = Bounds(c.clip.xy, c.clip.zw);
        let radii = Radii(
            Corners(c.clip_radii.x, c.clip_radii.y, c.clip_radii.z, c.clip_radii.w),
            Corners(c.clip_radii_y.x, c.clip_radii_y.y, c.clip_radii_y.z, c.clip_radii_y.w),
        );
        coverage *= saturate(0.5 - quad_sdf(input.local, bounds, radii));
    }
    if ((c.flags & COMPOSITE_MASK) != 0u) {
        let t = (input.local - c.mask_rect.xy) / c.mask_rect.zw;
        if (any(t < vec2<f32>(0.0)) || any(t > vec2<f32>(1.0))) {
            coverage = 0.0;
        } else {
            let mask_size = vec2<f32>(textureDimensions(t_mask, 0));
            coverage *= textureSampleLevel(t_mask, s_sprite, (c.mask_tile.xy + t * c.mask_tile.zw) / mask_size, 0.0).a;
        }
    }
    color *= coverage;

    if ((c.flags & COMPOSITE_BLEND) != 0u && color.a > 0.0) {
        let backdrop = textureSampleLevel(t_backdrop, s_sprite, (target_position - c.backdrop_rect.xy) / c.backdrop_rect.zw, 0.0);
        if (c.blend == 16u) {
            return vec4<f32>(min(color.rgb + backdrop.rgb, vec3<f32>(1.0)) - backdrop.rgb * (1.0 - color.a), color.a);
        }
        let cb = select(vec3<f32>(0.0), backdrop.rgb / backdrop.a, backdrop.a > 0.0);
        let cs = color.rgb / color.a;
        let mixed = (1.0 - backdrop.a) * cs + backdrop.a * saturate(blend_mode(cb, cs, c.blend));
        color = vec4<f32>(mixed * color.a, color.a);
    }
    return color;
}

struct Blur {
    region: vec4<f32>,
    source_rect: vec4<f32>,
    direction: vec2<f32>,
    sigma: f32,
    scale: f32,
}
@group(1) @binding(0) var<storage, read> b_blurs: array<Blur>;

struct BlurVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) blur_id: u32,
}

@vertex
fn vs_blur(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> BlurVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let blur = b_blurs[instance_id];
    var out = BlurVarying();
    out.position = to_device_position_impl(blur.region.xy + unit_vertex * blur.region.zw);
    out.blur_id = instance_id;
    return out;
}

@fragment
fn fs_blur(input: BlurVarying) -> @location(0) vec4<f32> {
    let blur = b_blurs[input.blur_id];
    let position = globals.viewport_origin + input.position.xy * blur.scale;
    return gaussian_sample(position, blur.direction, blur.sigma, blur.scale, blur.source_rect);
}
