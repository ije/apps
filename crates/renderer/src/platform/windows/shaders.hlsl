// Match scene.rs, frame.rs, Metal and WGSL layouts. HLSL structured buffers pack float4 without padding.

cbuffer Globals: register(b0) {
    float4 gamma_ratios;
    float2 viewport_size;
    float2 viewport_origin;
    float2 window_size;
    float grayscale_enhanced_contrast;
    float subpixel_enhanced_contrast;
    uint premultiplied_alpha;
    uint is_bgr;
    uint2 globals_pad;
};

// Instance IDs start at zero, so draws pass the index of their first instance.
cbuffer Batch: register(b1) {
    uint batch_start;
    uint3 batch_pad;
};

Texture2D<float4> t_sprite: register(t0);
Texture2D<float4> t_mask: register(t2);
Texture2D<float4> t_backdrop: register(t3);
SamplerState s_sprite: register(s0);

struct Bounds {
    float2 origin;
    float2 size;
};

struct Corners {
    float top_left;
    float top_right;
    float bottom_right;
    float bottom_left;
};

struct Radii {
    Corners x;
    Corners y;
};

struct Edges {
    float top;
    float right;
    float bottom;
    float left;
};

struct ContentMask {
    Bounds bounds;
    Bounds rounded;
    Radii radii;
};

struct AtlasTextureId {
    uint index;
    uint kind;
};

struct AtlasBounds {
    int2 origin;
    int2 size;
};

struct AtlasTile {
    AtlasTextureId texture_id;
    uint tile_id;
    uint padding;
    AtlasBounds bounds;
};

static const float M_PI_F = 3.141592653f;
static const float3 GRAYSCALE_FACTORS = float3(0.2126f, 0.7152f, 0.0722f);

float2 unit_vertex_of(uint vertex_id) {
    return float2(float(vertex_id & 1u), 0.5 * float(vertex_id & 2u));
}

float4 to_device_position_impl(float2 position) {
    float2 device_position = (position - viewport_origin) / viewport_size * float2(2.0, -2.0) + float2(-1.0, 1.0);
    return float4(device_position, 0., 1.);
}

float4 to_device_position(float2 unit_vertex, Bounds bounds) {
    return to_device_position_impl(unit_vertex * bounds.size + bounds.origin);
}

float4 distance_from_clip_rect_impl(float2 position, Bounds clip_bounds) {
    float2 tl = position - clip_bounds.origin;
    float2 br = clip_bounds.origin + clip_bounds.size - position;
    return float4(tl.x, br.x, tl.y, br.y);
}

float4 distance_from_clip_rect(float2 unit_vertex, Bounds bounds, Bounds clip_bounds) {
    return distance_from_clip_rect_impl(unit_vertex * bounds.size + bounds.origin, clip_bounds);
}

float2 window_position(float4 position) {
    return position.xy + viewport_origin;
}

float2 erf(float2 x) {
    float2 s = sign(x);
    float2 a = abs(x);
    float2 r1 = 1. + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    float2 r2 = r1 * r1;
    return s - s / (r2 * r2);
}

float blur_along_x(float x, float y, float sigma, float2 corner, float2 half_size) {
    float delta = min(half_size.y - corner.y - abs(y), 0.);
    float curved = half_size.x - corner.x + (corner.x == corner.y
        ? sqrt(max(0., corner.x * corner.x - delta * delta))
        : corner.x * sqrt(max(0., 1. - delta * delta / (corner.y * corner.y))));
    float2 integral = 0.5 + 0.5 * erf((x + float2(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

float gaussian(float x, float sigma) {
    return exp(-(x * x) / (2. * sigma * sigma)) / (sqrt(2. * M_PI_F) * sigma);
}

float4 over(float4 below, float4 above) {
    float4 result;
    float alpha = above.a + below.a * (1.0 - above.a);
    // Two transparent colors would divide by zero, and blending a NaN is undefined on some GPUs.
    if (alpha <= 0.0) {
        return float4(0.0, 0.0, 0.0, 0.0);
    }
    result.rgb = (above.rgb * above.a + below.rgb * below.a * (1.0 - above.a)) / alpha;
    result.a = alpha;
    return result;
}

float2 to_tile_position(float2 unit_vertex, AtlasTile tile) {
    float2 atlas_size;
    t_sprite.GetDimensions(atlas_size.x, atlas_size.y);
    return (float2(tile.bounds.origin) + unit_vertex * float2(tile.bounds.size)) / atlas_size;
}

float4 pick_corner(float2 center_to_point, float2 half_size, Radii radii) {
    float2 c = center_to_point;
    float2 tl = float2(radii.x.top_left, radii.y.top_left);
    float2 tr = float2(radii.x.top_right, radii.y.top_right);
    float2 br = float2(radii.x.bottom_right, radii.y.bottom_right);
    float2 bl = float2(radii.x.bottom_left, radii.y.bottom_left);
    float2 side = float2(c.x < 0. ? -1. : 1., c.y < 0. ? -1. : 1.);
    if (all(c < tl - half_size)) {
        side = float2(-1., -1.);
    } else if (c.x > half_size.x - tr.x && c.y < tr.y - half_size.y) {
        side = float2(1., -1.);
    } else if (all(c > half_size - br)) {
        side = float2(1., 1.);
    } else if (c.x < bl.x - half_size.x && c.y > half_size.y - bl.y) {
        side = float2(-1., 1.);
    }
    float2 radius = side.x < 0. ? (side.y < 0. ? tl : bl) : (side.y < 0. ? tr : br);
    return float4(radius.x > 0. && radius.y > 0. ? radius : float2(0., 0.), side);
}

float quad_sdf_impl(float2 corner_center_to_point, float2 corner_radius) {
    if (corner_radius.x == 0.0) {
        return max(corner_center_to_point.x, corner_center_to_point.y);
    } else if (corner_radius.x != corner_radius.y) {
        if (corner_center_to_point.x <= 0.0 || corner_center_to_point.y <= 0.0) {
            return max(corner_center_to_point.x - corner_radius.x, corner_center_to_point.y - corner_radius.y);
        }
        float k0 = length(corner_center_to_point / corner_radius);
        float k1 = length(corner_center_to_point / (corner_radius * corner_radius));
        return k0 * (k0 - 1.0) / k1;
    } else {
        float signed_distance_to_inset_quad =
            length(max(float2(0.0, 0.0), corner_center_to_point)) +
            min(0.0, max(corner_center_to_point.x, corner_center_to_point.y));

        return signed_distance_to_inset_quad - corner_radius.x;
    }
}

float quad_sdf(float2 pt, Bounds bounds, Radii corner_radii) {
    float2 half_size = bounds.size / 2.;
    float2 center = bounds.origin + half_size;
    float2 center_to_point = pt - center;
    float4 corner = pick_corner(center_to_point, half_size, corner_radii);
    float2 corner_radius = corner.xy;
    float2 corner_to_point = corner.zw * center_to_point - half_size;
    float2 corner_center_to_point = corner_to_point + corner_radius;
    return quad_sdf_impl(corner_center_to_point, corner_radius);
}

float mask_alpha(float2 position, ContentMask mask) {
    if (mask.radii.x.top_left == 0.0 && mask.radii.x.top_right == 0.0 &&
            mask.radii.x.bottom_right == 0.0 && mask.radii.x.bottom_left == 0.0) {
        return 1.0;
    }
    return saturate(0.5 - quad_sdf(position, mask.rounded, mask.radii));
}

// Adapted from https://github.com/microsoft/terminal/blob/1283c0f5b99a2961673249fa77c6b986efb5086c/src/renderer/atlas/dwrite.hlsl
// Copyright (c) Microsoft Corporation. Licensed under the MIT license.
float apply_contrast_and_gamma_correction(float sample, float3 color) {
    float brightness = dot(color, float3(0.30f, 0.59f, 0.11f));
    float k = grayscale_enhanced_contrast * saturate(4.0f * (0.75f - brightness));
    float contrasted = sample * (k + 1.0f) / (sample * k + 1.0f);
    float4 g = gamma_ratios;
    float correction = (g.x * brightness + g.y) * contrasted + (g.z * brightness + g.w);
    return contrasted + contrasted * (1.0f - contrasted) * correction;
}

float corner_dash_velocity(float dv1, float dv2) {
    if (dv1 == 0.0) {
        return dv2;
    } else if (dv2 == 0.0) {
        return dv1;
    } else {
        return min(dv1, dv2);
    }
}

float dash_alpha(
    float t, float period, float length, float dash_velocity,
    float antialias_threshold
) {
    float half_period = period / 2.0;
    float half_length = length / 2.0;
    float centered = fmod(t + half_period - half_length, period) - half_period;
    float signed_distance = abs(centered) - half_length;
    return saturate(antialias_threshold - signed_distance / dash_velocity);
}

float dot_alpha(float t, float period, float dot_length, float dash_velocity, float depth, float inner_depth, float antialias_threshold) {
    float width = depth + inner_depth;
    float half_period = period / 2.0;
    float half_length = dot_length / 2.0;
    float along = (fmod(t + half_period - half_length, period) - half_period) / dash_velocity;
    float across = depth - width / 2.0;
    return saturate(antialias_threshold - (length(float2(along, across)) - width / 2.0));
}

float quarter_ellipse_sdf(float2 pt, float2 radii) {
    float2 circle_vec = pt / radii;
    float unit_circle_sdf = length(circle_vec) - 1.0;
    return unit_circle_sdf * (radii.x + radii.y) * -0.5;
}

struct Quad {
    uint order;
    uint border_style;
    Bounds bounds;
    ContentMask content_mask;
    float4 background;
    float4 border_color;
    Radii corner_radii;
    Edges border_widths;
};

struct QuadVertexOutput {
    nointerpolation uint quad_id: TEXCOORD0;
    float4 position: SV_Position;
    nointerpolation float4 border_color: COLOR0;
    nointerpolation float4 background_color: COLOR1;
    nointerpolation uint plain: TEXCOORD1;
    float4 clip_distance: SV_ClipDistance;
};

struct QuadFragmentInput {
    nointerpolation uint quad_id: TEXCOORD0;
    float4 position: SV_Position;
    nointerpolation float4 border_color: COLOR0;
    nointerpolation float4 background_color: COLOR1;
    nointerpolation uint plain: TEXCOORD1;
};

StructuredBuffer<Quad> quads: register(t1);

QuadVertexOutput quad_vertex(uint vertex_id: SV_VertexID, uint instance_id: SV_InstanceID) {
    uint quad_id = batch_start + instance_id;
    float2 unit_vertex = unit_vertex_of(vertex_id);
    Quad quad = quads[quad_id];
    QuadVertexOutput output;
    output.position = to_device_position(unit_vertex, quad.bounds);
    output.border_color = quad.border_color;
    output.background_color = quad.background;
    output.quad_id = quad_id;
    // Plain boxes fill with their color, so their pixels don't read the quad again.
    output.plain = all(float4(quad.corner_radii.x.top_left, quad.corner_radii.x.top_right,
                              quad.corner_radii.x.bottom_right, quad.corner_radii.x.bottom_left) == 0.0) &&
                   all(float4(quad.border_widths.top, quad.border_widths.right,
                              quad.border_widths.bottom, quad.border_widths.left) == 0.0) &&
                   all(float4(quad.content_mask.radii.x.top_left, quad.content_mask.radii.x.top_right,
                              quad.content_mask.radii.x.bottom_right,
                              quad.content_mask.radii.x.bottom_left) == 0.0);
    output.clip_distance = distance_from_clip_rect(unit_vertex, quad.bounds, quad.content_mask.bounds);
    return output;
}

float4 quad_color(QuadFragmentInput input, Quad quad, float2 position) {
    float4 background_color = input.background_color;

    bool unrounded = quad.corner_radii.x.top_left == 0.0 &&
        quad.corner_radii.x.top_right == 0.0 &&
        quad.corner_radii.x.bottom_left == 0.0 &&
        quad.corner_radii.x.bottom_right == 0.0;

    if (quad.border_widths.top == 0.0 &&
        quad.border_widths.left == 0.0 &&
        quad.border_widths.right == 0.0 &&
        quad.border_widths.bottom == 0.0 &&
        unrounded) {
        return background_color;
    }

    float2 size = quad.bounds.size;
    float2 half_size = size / 2.;
    float2 the_point = position - quad.bounds.origin;
    float2 center_to_point = the_point - half_size;

    const float antialias_threshold = 0.5;

    float4 corner = pick_corner(center_to_point, half_size, quad.corner_radii);
    float2 corner_radius = corner.xy;
    bool left = corner.z < 0.0;
    bool top = corner.w < 0.0;

    float2 border = float2(
        left ? quad.border_widths.left : quad.border_widths.right,
        top ? quad.border_widths.top : quad.border_widths.bottom
    );

    // Negative inner widths prevent antialiasing pixels on zero-width borders.
    float2 reduced_border = float2(
        border.x == 0.0 ? -antialias_threshold : border.x,
        border.y == 0.0 ? -antialias_threshold : border.y
    );

    float2 far_border = float2(
        left ? quad.border_widths.right : quad.border_widths.left,
        top ? quad.border_widths.bottom : quad.border_widths.top
    );
    float2 reduced_far_border = far_border == 0.0 ? -antialias_threshold : far_border;

    float2 corner_to_point = corner.zw * center_to_point - half_size;

    float2 corner_center_to_point = corner_to_point + corner_radius;

    bool is_near_rounded_corner =
            corner_center_to_point.x >= 0.0 &&
            corner_center_to_point.y >= 0.0;

    float2 straight_border_inner_corner_to_point =
        max(corner_to_point + reduced_border, reduced_far_border - size - corner_to_point);

    bool is_beyond_inner_straight_border =
            straight_border_inner_corner_to_point.x > 0.0 ||
            straight_border_inner_corner_to_point.y > 0.0;

    bool is_within_inner_straight_border =
        straight_border_inner_corner_to_point.x < -antialias_threshold &&
        straight_border_inner_corner_to_point.y < -antialias_threshold;

    if (is_within_inner_straight_border && !is_near_rounded_corner) {
        return background_color;
    }

    float outer_sdf = quad_sdf_impl(corner_center_to_point, corner_radius);

    float inner_sdf = 0.0;
    if (corner_center_to_point.x <= 0.0 || corner_center_to_point.y <= 0.0) {
        inner_sdf = -max(straight_border_inner_corner_to_point.x,
                        straight_border_inner_corner_to_point.y);
    } else if (is_beyond_inner_straight_border) {
        inner_sdf = -1.0;
    } else if (reduced_border.x == reduced_border.y && corner_radius.x == corner_radius.y) {
        inner_sdf = -(outer_sdf + reduced_border.x);
    } else {
        float2 ellipse_radii = max(float2(0.0, 0.0), corner_radius - reduced_border);
        inner_sdf = quarter_ellipse_sdf(corner_center_to_point, ellipse_radii);
    }

    float border_sdf = max(inner_sdf, outer_sdf);

    float4 color = background_color;
    if (border_sdf < antialias_threshold) {
        float4 border_color = input.border_color;

        uint sides = quad.border_style >> 8;
        if (sides != 0) {
            float4 widths = float4(quad.border_widths.top, quad.border_widths.right,
                quad.border_widths.bottom, quad.border_widths.left);
            float4 depths = float4(the_point.y, size.x - the_point.x, size.y - the_point.y, the_point.x);
            float coverage = 0.0;
            for (uint a = 0; a < 4; a++) {
                if ((sides & (1u << a)) == 0 || widths[a] <= 0.0) continue;
                float nearest = 1e6;
                for (uint b = 0; b < 4; b++) {
                    if ((sides & (1u << b)) != 0 || widths[b] <= 0.0) continue;
                    float scale = (a + b) % 2 == 1 ? length(float2(widths[a], widths[b])) : widths[a] + widths[b];
                    nearest = min(nearest, (depths[b] * widths[a] - depths[a] * widths[b]) / scale);
                }
                coverage = max(coverage, saturate(0.5 + nearest));
            }
            border_color.a *= coverage;
        }

        Bounds inner_bounds;
        inner_bounds.origin = quad.bounds.origin + float2(quad.border_widths.left, quad.border_widths.top);
        inner_bounds.size = max(float2(0.0, 0.0), quad.bounds.size - float2(
            quad.border_widths.left + quad.border_widths.right,
            quad.border_widths.top + quad.border_widths.bottom));
        Radii inner_radii;
        inner_radii.x.top_left = max(0.0, quad.corner_radii.x.top_left - quad.border_widths.left);
        inner_radii.x.top_right = max(0.0, quad.corner_radii.x.top_right - quad.border_widths.right);
        inner_radii.x.bottom_right = max(0.0, quad.corner_radii.x.bottom_right - quad.border_widths.right);
        inner_radii.x.bottom_left = max(0.0, quad.corner_radii.x.bottom_left - quad.border_widths.left);
        inner_radii.y.top_left = max(0.0, quad.corner_radii.y.top_left - quad.border_widths.top);
        inner_radii.y.top_right = max(0.0, quad.corner_radii.y.top_right - quad.border_widths.top);
        inner_radii.y.bottom_right = max(0.0, quad.corner_radii.y.bottom_right - quad.border_widths.bottom);
        inner_radii.y.bottom_left = max(0.0, quad.corner_radii.y.bottom_left - quad.border_widths.bottom);
        float inner_depth = quad_sdf(position, inner_bounds, inner_radii);

        if ((quad.border_style & 255) == 3) {
            float depth = -outer_sdf;
            float width = depth + inner_depth;
            border_color.a *= saturate(antialias_threshold - min(depth - width / 3.0, width * 2.0 / 3.0 - depth));
        }

        if ((quad.border_style & 255) == 1 || (quad.border_style & 255) == 2) {
            float t = 0.0;

            float max_t = 0.0;

            bool dotted = (quad.border_style & 255) == 2;
            float dash_length_per_width = dotted ? 1.0 : 2.0;
            const float dash_gap_per_width = 1.0;
            float dash_period_per_width = dash_length_per_width + dash_gap_per_width;

            float dash_velocity = 0.0;

            float dv_numerator = 1.0 / dash_period_per_width;

            if (unrounded) {
                bool is_horizontal = corner_center_to_point.x < corner_center_to_point.y;
                float2 dashed_border = float2(
                    max(quad.border_widths.bottom, quad.border_widths.top),
                    max(quad.border_widths.right, quad.border_widths.left)
                );
                float border_width = is_horizontal ? dashed_border.x : dashed_border.y;
                dash_velocity = dv_numerator / border_width;
                t = is_horizontal ? the_point.x : the_point.y;
                t *= dash_velocity;
                max_t = is_horizontal ? size.x : size.y;
                max_t *= dash_velocity;
            } else {
                float2 r_tr = float2(quad.corner_radii.x.top_right, quad.corner_radii.y.top_right);
                float2 r_br = float2(quad.corner_radii.x.bottom_right, quad.corner_radii.y.bottom_right);
                float2 r_bl = float2(quad.corner_radii.x.bottom_left, quad.corner_radii.y.bottom_left);
                float2 r_tl = float2(quad.corner_radii.x.top_left, quad.corner_radii.y.top_left);

                float w_t = quad.border_widths.top;
                float w_r = quad.border_widths.right;
                float w_b = quad.border_widths.bottom;
                float w_l = quad.border_widths.left;

                float dv_t = w_t <= 0.0 ? 0.0 : dv_numerator / w_t;
                float dv_r = w_r <= 0.0 ? 0.0 : dv_numerator / w_r;
                float dv_b = w_b <= 0.0 ? 0.0 : dv_numerator / w_b;
                float dv_l = w_l <= 0.0 ? 0.0 : dv_numerator / w_l;

                float s_t = (size.x - r_tl.x - r_tr.x) * dv_t;
                float s_r = (size.y - r_tr.y - r_br.y) * dv_r;
                float s_b = (size.x - r_br.x - r_bl.x) * dv_b;
                float s_l = (size.y - r_bl.y - r_tl.y) * dv_l;

                float corner_dash_velocity_tr = corner_dash_velocity(dv_t, dv_r);
                float corner_dash_velocity_br = corner_dash_velocity(dv_b, dv_r);
                float corner_dash_velocity_bl = corner_dash_velocity(dv_b, dv_l);
                float corner_dash_velocity_tl = corner_dash_velocity(dv_t, dv_l);

                float middle = dotted ? 0.5 : 0.0;
                float c_tr = max(0.0, (r_tr.x + r_tr.y) / 2.0 - middle * max(w_t, w_r)) * (M_PI_F / 2.0) * corner_dash_velocity_tr;
                float c_br = max(0.0, (r_br.x + r_br.y) / 2.0 - middle * max(w_b, w_r)) * (M_PI_F / 2.0) * corner_dash_velocity_br;
                float c_bl = max(0.0, (r_bl.x + r_bl.y) / 2.0 - middle * max(w_b, w_l)) * (M_PI_F / 2.0) * corner_dash_velocity_bl;
                float c_tl = max(0.0, (r_tl.x + r_tl.y) / 2.0 - middle * max(w_t, w_l)) * (M_PI_F / 2.0) * corner_dash_velocity_tl;

                float upto_tr = s_t;
                float upto_r = upto_tr + c_tr;
                float upto_br = upto_r + s_r;
                float upto_b = upto_br + c_br;
                float upto_bl = upto_b + s_b;
                float upto_l = upto_bl + c_bl;
                float upto_tl = upto_l + s_l;
                max_t = upto_tl + c_tl;

                if (is_near_rounded_corner) {
                    float radians = atan2(corner_center_to_point.y / corner_radius.y, corner_center_to_point.x / corner_radius.x);
                    float corner_t = radians * max(0.0, (corner_radius.x + corner_radius.y) / 2.0 - middle * max(border.x, border.y));

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
                    bool is_horizontal = corner_center_to_point.x < corner_center_to_point.y;
                    if (is_horizontal) {
                        if (top) {
                            dash_velocity = dv_t;
                            t = (the_point.x - r_tl.x) * dash_velocity;
                        } else {
                            dash_velocity = dv_b;
                            t = upto_bl - (the_point.x - r_bl.x) * dash_velocity;
                        }
                    } else {
                        if (left) {
                            dash_velocity = dv_l;
                            t = upto_tl - (the_point.y - r_tl.y) * dash_velocity;
                        } else {
                            dash_velocity = dv_r;
                            t = upto_r + (the_point.y - r_tr.y) * dash_velocity;
                        }
                    }
                }
            }
            float dash_length = dash_length_per_width / dash_period_per_width;

            max_t -= unrounded ? dash_length : 0.0;
            if (max_t >= 1.0) {
                float dash_count = floor(max_t);
                float dash_period = max_t / dash_count;
                if (dotted) {
                    border_color.a *= dot_alpha(t, dash_period, dash_length, dash_velocity, -outer_sdf, inner_depth, antialias_threshold);
                } else {
                    border_color.a *= dash_alpha(t, dash_period, dash_length, dash_velocity, antialias_threshold);
                }
            } else if (unrounded) {
                float dash_gap = max_t - dash_length;
                if (dash_gap > 0.0) {
                    float dash_period = dash_length + dash_gap;
                    border_color.a *= dash_alpha(t, dash_period, dash_length, dash_velocity, antialias_threshold);
                }
            }
        }

        float4 blended_border = over(background_color, border_color);
        color = lerp(background_color, blended_border,
                    saturate(antialias_threshold - inner_sdf));
    }

    return color * float4(1.0, 1.0, 1.0, saturate(antialias_threshold - outer_sdf));
}

float4 quad_fragment(QuadFragmentInput input): SV_Target {
    if (input.plain) {
        return input.background_color;
    }
    Quad quad = quads[input.quad_id];
    float2 position = window_position(input.position);
    float4 color = quad_color(input, quad, position);
    color.a *= mask_alpha(position, quad.content_mask);
    return color;
}

struct Shadow {
    uint order;
    float blur_radius;
    Bounds bounds;
    Radii corner_radii;
    ContentMask content_mask;
    float4 color;
    Bounds element_bounds;
    Radii element_corner_radii;
    uint inset;
    uint pad;
};

struct ShadowVertexOutput {
    nointerpolation uint shadow_id: TEXCOORD0;
    float4 position: SV_Position;
    nointerpolation float4 color: COLOR;
    float4 clip_distance: SV_ClipDistance;
};

struct ShadowFragmentInput {
  nointerpolation uint shadow_id: TEXCOORD0;
  float4 position: SV_Position;
  nointerpolation float4 color: COLOR;
};

StructuredBuffer<Shadow> shadows: register(t1);

ShadowVertexOutput shadow_vertex(uint vertex_id: SV_VertexID, uint instance_id: SV_InstanceID) {
    uint shadow_id = batch_start + instance_id;
    float2 unit_vertex = unit_vertex_of(vertex_id);
    Shadow shadow = shadows[shadow_id];

    Bounds bounds = shadow.element_bounds;
    if (shadow.inset == 0) {
        float margin = 3.0 * shadow.blur_radius;
        bounds = shadow.bounds;
        bounds.origin -= margin;
        bounds.size += 2.0 * margin;
    }

    ShadowVertexOutput output;
    output.position = to_device_position(unit_vertex, bounds);
    output.color = shadow.color;
    output.shadow_id = shadow_id;
    output.clip_distance = distance_from_clip_rect(unit_vertex, bounds, shadow.content_mask.bounds);
    return output;
}

float shadow_coverage(float2 position, Shadow shadow) {
    if (shadow.blur_radius < 0.25) {
        return saturate(0.5 - quad_sdf(position, shadow.bounds, shadow.corner_radii));
    }
    float2 half_size = shadow.bounds.size / 2.;
    float2 center = shadow.bounds.origin + half_size;
    float2 point0 = position - center;
    float2 corner_radius = pick_corner(point0, half_size, shadow.corner_radii).xy;

    float low = point0.y - half_size.y;
    float high = point0.y + half_size.y;
    float start = clamp(-3. * shadow.blur_radius, low, high);
    float end = clamp(3. * shadow.blur_radius, low, high);

    float step = (end - start) / 4.;
    float y = start + step * 0.5;
    float alpha = 0.;
    for (int i = 0; i < 4; i++) {
        alpha += blur_along_x(point0.x, point0.y - y, shadow.blur_radius,
                            corner_radius, half_size) *
                gaussian(y, shadow.blur_radius) * step;
        y += step;
    }
    return alpha;
}

float4 shadow_fragment(ShadowFragmentInput input): SV_TARGET {
    Shadow shadow = shadows[input.shadow_id];
    float2 position = window_position(input.position);
    float coverage = shadow_coverage(position, shadow);
    float masked = mask_alpha(position, shadow.content_mask);
    float inside = saturate(0.5 - quad_sdf(position, shadow.element_bounds, shadow.element_corner_radii));
    coverage = shadow.inset != 0 ? (1.0 - coverage) * inside : coverage * (1.0 - inside);
    return input.color * float4(1., 1., 1., coverage * masked);
}

struct Underline {
    uint order;
    uint wavy;
    Bounds bounds;
    ContentMask content_mask;
    float4 color;
    float thickness;
    uint pad;
};

struct UnderlineVertexOutput {
  nointerpolation uint underline_id: TEXCOORD0;
  float4 position: SV_Position;
  nointerpolation float4 color: COLOR;
  float4 clip_distance: SV_ClipDistance;
};

struct UnderlineFragmentInput {
  nointerpolation uint underline_id: TEXCOORD0;
  float4 position: SV_Position;
  nointerpolation float4 color: COLOR;
};

StructuredBuffer<Underline> underlines: register(t1);

UnderlineVertexOutput underline_vertex(uint vertex_id: SV_VertexID, uint instance_id: SV_InstanceID) {
    uint underline_id = batch_start + instance_id;
    float2 unit_vertex = unit_vertex_of(vertex_id);
    Underline underline = underlines[underline_id];

    UnderlineVertexOutput output;
    output.position = to_device_position(unit_vertex, underline.bounds);
    output.color = underline.color;
    output.underline_id = underline_id;
    output.clip_distance = distance_from_clip_rect(unit_vertex, underline.bounds, underline.content_mask.bounds);
    return output;
}

float4 underline_fragment(UnderlineFragmentInput input): SV_Target {
    const float WAVE_FREQUENCY = 2.0;
    const float WAVE_HEIGHT_RATIO = 0.8;

    Underline underline = underlines[input.underline_id];
    float2 position = window_position(input.position);
    float alpha = mask_alpha(position, underline.content_mask);
    if ((underline.wavy & 0xFFu) != 0u) {
        float half_thickness = underline.thickness * 0.5;
        float2 st = ((position - underline.bounds.origin) / underline.bounds.size.y) - float2(0., 0.5);
        float frequency = (M_PI_F * WAVE_FREQUENCY * underline.thickness) / underline.bounds.size.y;
        float amplitude = (underline.thickness * WAVE_HEIGHT_RATIO) / underline.bounds.size.y;

        float sine = sin(st.x * frequency) * amplitude;
        float dSine = cos(st.x * frequency) * amplitude * frequency;
        float distance = (st.y - sine) / sqrt(1. + dSine * dSine);
        float distance_in_pixels = distance * underline.bounds.size.y;
        float distance_from_top_border = distance_in_pixels - half_thickness;
        float distance_from_bottom_border = distance_in_pixels + half_thickness;
        alpha *= saturate(0.5 - max(-distance_from_bottom_border, distance_from_top_border));
    }
    return input.color * float4(1., 1., 1., alpha);
}

struct MonochromeSprite {
    uint order;
    uint pad;
    AtlasTile tile;
    Bounds bounds;
    ContentMask content_mask;
    float4 color;
};

struct MonochromeSpriteVertexOutput {
    float4 position: SV_Position;
    float2 tile_position: POSITION;
    nointerpolation float4 color: COLOR;
    nointerpolation uint sprite_id: TEXCOORD0;
    float4 clip_distance: TEXCOORD1;
};

StructuredBuffer<MonochromeSprite> mono_sprites: register(t1);

MonochromeSpriteVertexOutput monochrome_sprite_vertex(uint vertex_id: SV_VertexID, uint instance_id: SV_InstanceID) {
    uint sprite_id = batch_start + instance_id;
    float2 unit_vertex = unit_vertex_of(vertex_id);
    MonochromeSprite sprite = mono_sprites[sprite_id];
    float2 position = unit_vertex * sprite.bounds.size + sprite.bounds.origin;

    MonochromeSpriteVertexOutput output;
    output.position = to_device_position_impl(position);
    output.tile_position = to_tile_position(unit_vertex, sprite.tile);
    output.color = sprite.color;
    output.sprite_id = sprite_id;
    output.clip_distance = distance_from_clip_rect_impl(position, sprite.content_mask.bounds);
    return output;
}

float4 monochrome_sprite_fragment(MonochromeSpriteVertexOutput input): SV_Target {
    float sample = t_sprite.Sample(s_sprite, input.tile_position).r;
    // Sample before the early return: derivatives require uniform control flow.
    if (any(input.clip_distance < 0.0)) {
        return float4(0.0, 0.0, 0.0, 0.0);
    }
    float alpha_corrected = apply_contrast_and_gamma_correction(sample, input.color.rgb);
    float masked = mask_alpha(window_position(input.position), mono_sprites[input.sprite_id].content_mask);
    return float4(input.color.rgb, input.color.a * alpha_corrected * masked);
}

struct PolychromeSprite {
    uint order;
    uint grayscale;
    AtlasTile tile;
    float opacity;
    uint pad;
    Bounds bounds;
    ContentMask content_mask;
    Radii corner_radii;
};

struct PolychromeSpriteVertexOutput {
    nointerpolation uint sprite_id: TEXCOORD0;
    float4 position: SV_Position;
    float2 tile_position: POSITION;
    float4 clip_distance: SV_ClipDistance;
};

struct PolychromeSpriteFragmentInput {
    nointerpolation uint sprite_id: TEXCOORD0;
    float4 position: SV_Position;
    float2 tile_position: POSITION;
};

StructuredBuffer<PolychromeSprite> poly_sprites: register(t1);

PolychromeSpriteVertexOutput polychrome_sprite_vertex(uint vertex_id: SV_VertexID, uint instance_id: SV_InstanceID) {
    uint sprite_id = batch_start + instance_id;
    float2 unit_vertex = unit_vertex_of(vertex_id);
    PolychromeSprite sprite = poly_sprites[sprite_id];

    PolychromeSpriteVertexOutput output;
    output.position = to_device_position(unit_vertex, sprite.bounds);
    output.tile_position = to_tile_position(unit_vertex, sprite.tile);
    output.sprite_id = sprite_id;
    output.clip_distance = distance_from_clip_rect(unit_vertex, sprite.bounds, sprite.content_mask.bounds);
    return output;
}

float4 polychrome_sprite_fragment(PolychromeSpriteFragmentInput input): SV_Target {
    PolychromeSprite sprite = poly_sprites[input.sprite_id];
    float4 sample = t_sprite.Sample(s_sprite, input.tile_position);
    float2 position = window_position(input.position);
    Corners rx = sprite.corner_radii.x;
    bool square = rx.top_left == 0.0 && rx.top_right == 0.0 && rx.bottom_right == 0.0 && rx.bottom_left == 0.0;
    float corners = square ? 1.0 : saturate(0.5 - quad_sdf(position, sprite.bounds, sprite.corner_radii));

    float4 color = sample;
    if ((sprite.grayscale & 0x100u) != 0u && color.a > 0.0) {
        color.rgb /= color.a;
    }
    if ((sprite.grayscale & 0xFFu) != 0u) {
        float3 grayscale = dot(color.rgb, GRAYSCALE_FACTORS);
        color = float4(grayscale, sample.a);
    }
    color.a *= sprite.opacity * corners * mask_alpha(position, sprite.content_mask);
    return color;
}

struct Composite {
    float4x4 transform;
    float4x4 color_matrix;
    float4 color_offset;
    float4 shadow_color;
    float4 bounds;
    float4 clip;
    float4 clip_radii;
    float4 clip_radii_y;
    float4 mask_rect;
    float4 mask_tile;
    float4 content_mask;
    float4 rounded_mask;
    float4 rounded_radii;
    float4 rounded_radii_y;
    float4 shadow;
    float4 source_rect;
    float4 backdrop_rect;
    float opacity;
    float blur;
    uint flags;
    uint blend;
};

static const uint COMPOSITE_MATRIX = 1;
static const uint COMPOSITE_CLIP = 2;
static const uint COMPOSITE_MASK = 4;
static const uint COMPOSITE_SHADOW = 8;
static const uint COMPOSITE_BACKDROP = 16;
static const uint COMPOSITE_BLEND = 32;

StructuredBuffer<Composite> composites: register(t1);

struct CompositeVertexOutput {
    float4 position: SV_Position;
    float2 local: TEXCOORD0;
    nointerpolation uint composite_id: TEXCOORD1;
};

CompositeVertexOutput composite_vertex(uint vertex_id: SV_VertexID, uint instance_id: SV_InstanceID) {
    uint composite_id = batch_start + instance_id;
    float2 unit_vertex = unit_vertex_of(vertex_id);
    Composite c = composites[composite_id];
    float2 local = c.bounds.xy + unit_vertex * c.bounds.zw;
    float4 p = mul(c.transform, float4(local, 0.0, 1.0));
    float2 target_position = p.xy - viewport_origin * p.w;

    CompositeVertexOutput output;
    output.position = float4(
        target_position.x / viewport_size.x * 2.0 - p.w,
        p.w - target_position.y / viewport_size.y * 2.0,
        0.0,
        p.w);
    output.local = local;
    output.composite_id = composite_id;
    return output;
}

float4 sample_layer(float2 position, float4 rect) {
    float2 uv = (position - rect.xy) / rect.zw;
    if (any(uv < 0.0) || any(uv > 1.0)) {
        return float4(0.0, 0.0, 0.0, 0.0);
    }
    return t_sprite.SampleLevel(s_sprite, uv, 0.0);
}

float4 gaussian_sample(float2 position, float2 direction, float sigma, float texel, float4 rect) {
    if (sigma < 0.25) {
        return sample_layer(position, rect);
    }
    float radius = ceil(3.0 * sigma / texel);
    float taps = min(radius, 32.0);
    float step = radius / taps * texel;
    float q = exp(-(step * step) / (2.0 * sigma * sigma));
    float4 sum = sample_layer(position, rect);
    float weight = 1.0;
    float w = 1.0;
    float g = q;
    [loop]
    for (float i = 1.0; i <= taps; i += 1.0) {
        w *= g;
        g *= q * q;
        float2 offset = direction * (i * step);
        sum += (sample_layer(position + offset, rect) + sample_layer(position - offset, rect)) * w;
        weight += 2.0 * w;
    }
    return sum / weight;
}

float luminosity(float3 color) {
    return dot(color, float3(0.3, 0.59, 0.11));
}

float3 clip_color(float3 color) {
    float l = luminosity(color);
    float n = min(min(color.r, color.g), color.b);
    float x = max(max(color.r, color.g), color.b);
    float3 c = color;
    if (n < 0.0) {
        c = l + (c - l) * l / (l - n);
    }
    if (x > 1.0) {
        c = l + (c - l) * (1.0 - l) / (x - l);
    }
    return c;
}

float3 set_luminosity(float3 color, float l) {
    return clip_color(color + (l - luminosity(color)));
}

float saturation(float3 color) {
    return max(max(color.r, color.g), color.b) - min(min(color.r, color.g), color.b);
}

float3 set_saturation(float3 color, float s) {
    float n = min(min(color.r, color.g), color.b);
    float x = max(max(color.r, color.g), color.b);
    if (x <= n) {
        return float3(0.0, 0.0, 0.0);
    }
    return (color - n) * s / (x - n);
}

float3 hard_light(float3 cb, float3 cs) {
    return cs <= 0.5 ? cb * 2.0 * cs : cb + 2.0 * cs - 1.0 - cb * (2.0 * cs - 1.0);
}

float3 soft_light(float3 cb, float3 cs) {
    float3 d = cb <= 0.25 ? ((16.0 * cb - 12.0) * cb + 4.0) * cb : sqrt(cb);
    return cs <= 0.5 ? cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb) : cb + (2.0 * cs - 1.0) * (d - cb);
}

float3 blend_mode(float3 cb, float3 cs, uint mode) {
    switch (mode) {
        case 1: return cb * cs;
        case 2: return cb + cs - cb * cs;
        case 3: return hard_light(cs, cb);
        case 4: return min(cb, cs);
        case 5: return max(cb, cs);
        case 6: return cb <= 0.0 ? (float3)0.0 : (cs >= 1.0 ? (float3)1.0 : min((float3)1.0, cb / max(1.0 - cs, 1e-6)));
        case 7: return cb >= 1.0 ? (float3)1.0 : (cs <= 0.0 ? (float3)0.0 : 1.0 - min((float3)1.0, (1.0 - cb) / max(cs, 1e-6)));
        case 8: return hard_light(cb, cs);
        case 9: return soft_light(cb, cs);
        case 10: return abs(cb - cs);
        case 11: return cb + cs - 2.0 * cb * cs;
        case 12: return set_luminosity(set_saturation(cs, saturation(cb)), luminosity(cb));
        case 13: return set_luminosity(set_saturation(cb, saturation(cs)), luminosity(cb));
        case 14: return set_luminosity(cs, luminosity(cb));
        case 15: return set_luminosity(cb, luminosity(cs));
        default: return cs;
    }
}

float4 composite_fragment(CompositeVertexOutput input): SV_Target {
    Composite c = composites[input.composite_id];
    float2 target_position = window_position(input.position);
    if (any(target_position < c.content_mask.xy) || any(target_position > c.content_mask.xy + c.content_mask.zw)) {
        return float4(0.0, 0.0, 0.0, 0.0);
    }

    float4 color;
    if ((c.flags & COMPOSITE_BACKDROP) != 0) {
        color = gaussian_sample(target_position, float2(0.0, 1.0), c.blur, 1.0, c.source_rect);
    } else if ((c.flags & COMPOSITE_SHADOW) != 0) {
        color = c.shadow_color * gaussian_sample(input.local - c.shadow.xy, float2(0.0, 1.0), c.blur, 1.0, c.source_rect).a;
    } else {
        color = gaussian_sample(input.local, float2(0.0, 1.0), c.blur, 1.0, c.source_rect);
    }

    if ((c.flags & COMPOSITE_MATRIX) != 0 && color.a > 0.0) {
        float4 straight = float4(color.rgb / color.a, color.a);
        float4 mapped = saturate(mul(c.color_matrix, straight) + c.color_offset);
        color = float4(mapped.rgb * mapped.a, mapped.a);
    }

    ContentMask content_mask;
    content_mask.bounds.origin = c.content_mask.xy;
    content_mask.bounds.size = c.content_mask.zw;
    content_mask.rounded.origin = c.rounded_mask.xy;
    content_mask.rounded.size = c.rounded_mask.zw;
    content_mask.radii.x.top_left = c.rounded_radii.x;
    content_mask.radii.x.top_right = c.rounded_radii.y;
    content_mask.radii.x.bottom_right = c.rounded_radii.z;
    content_mask.radii.x.bottom_left = c.rounded_radii.w;
    content_mask.radii.y.top_left = c.rounded_radii_y.x;
    content_mask.radii.y.top_right = c.rounded_radii_y.y;
    content_mask.radii.y.bottom_right = c.rounded_radii_y.z;
    content_mask.radii.y.bottom_left = c.rounded_radii_y.w;
    float coverage = c.opacity * mask_alpha(target_position, content_mask);
    if ((c.flags & COMPOSITE_CLIP) != 0) {
        Bounds bounds;
        bounds.origin = c.clip.xy;
        bounds.size = c.clip.zw;
        Radii radii;
        radii.x.top_left = c.clip_radii.x;
        radii.x.top_right = c.clip_radii.y;
        radii.x.bottom_right = c.clip_radii.z;
        radii.x.bottom_left = c.clip_radii.w;
        radii.y.top_left = c.clip_radii_y.x;
        radii.y.top_right = c.clip_radii_y.y;
        radii.y.bottom_right = c.clip_radii_y.z;
        radii.y.bottom_left = c.clip_radii_y.w;
        coverage *= saturate(0.5 - quad_sdf(input.local, bounds, radii));
    }
    if ((c.flags & COMPOSITE_MASK) != 0) {
        float2 t = (input.local - c.mask_rect.xy) / c.mask_rect.zw;
        if (any(t < 0.0) || any(t > 1.0)) {
            coverage = 0.0;
        } else {
            float2 mask_size;
            t_mask.GetDimensions(mask_size.x, mask_size.y);
            coverage *= t_mask.SampleLevel(s_sprite, (c.mask_tile.xy + t * c.mask_tile.zw) / mask_size, 0.0).a;
        }
    }
    color *= coverage;

    if ((c.flags & COMPOSITE_BLEND) != 0 && color.a > 0.0) {
        float4 backdrop = t_backdrop.SampleLevel(s_sprite, (target_position - c.backdrop_rect.xy) / c.backdrop_rect.zw, 0.0);
        if (c.blend == 16) {
            return float4(min(color.rgb + backdrop.rgb, (float3)1.0) - backdrop.rgb * (1.0 - color.a), color.a);
        }
        float3 cb = backdrop.a > 0.0 ? backdrop.rgb / backdrop.a : float3(0.0, 0.0, 0.0);
        float3 cs = color.rgb / color.a;
        float3 mixed = (1.0 - backdrop.a) * cs + backdrop.a * saturate(blend_mode(cb, cs, c.blend));
        color = float4(mixed * color.a, color.a);
    }
    return color;
}

struct Blur {
    float4 region;
    float4 source_rect;
    float2 direction;
    float sigma;
    float scale;
};

StructuredBuffer<Blur> blurs: register(t1);

struct BlurVertexOutput {
    float4 position: SV_Position;
    nointerpolation uint blur_id: TEXCOORD0;
};

BlurVertexOutput blur_vertex(uint vertex_id: SV_VertexID, uint instance_id: SV_InstanceID) {
    uint blur_id = batch_start + instance_id;
    float2 unit_vertex = unit_vertex_of(vertex_id);
    Blur blur = blurs[blur_id];

    BlurVertexOutput output;
    output.position = to_device_position_impl(blur.region.xy + unit_vertex * blur.region.zw);
    output.blur_id = blur_id;
    return output;
}

float4 blur_fragment(BlurVertexOutput input): SV_Target {
    Blur blur = blurs[input.blur_id];
    float2 position = viewport_origin + input.position.xy * blur.scale;
    return gaussian_sample(position, blur.direction, blur.sigma, blur.scale, blur.source_rect);
}
