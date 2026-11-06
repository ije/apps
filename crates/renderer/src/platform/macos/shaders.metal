#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

// WARNING: these structs must match the `repr(C)` structs in `scene.rs` and `frame.rs`, like the
// HLSL and WGSL shaders.

struct Globals {
  float4 gamma_ratios;
  float2 viewport_size;
  float2 viewport_origin;
  float2 window_size;
  float grayscale_enhanced_contrast;
  float subpixel_enhanced_contrast;
  uint premultiplied_alpha;
  uint is_bgr;
  uint pad[2];
};

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

// WARNING: four floats, not `float4`, which Metal aligns to 16 bytes.
struct Rgba {
  float r;
  float g;
  float b;
  float a;
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

struct Quad {
  uint order;
  uint border_style;
  Bounds bounds;
  ContentMask content_mask;
  Rgba background;
  Rgba border_color;
  Radii corner_radii;
  Edges border_widths;
};

struct Shadow {
  uint order;
  float blur_radius;
  Bounds bounds;
  Radii corner_radii;
  ContentMask content_mask;
  Rgba color;
  Bounds element_bounds;
  Radii element_corner_radii;
  uint inset;
  uint pad;
};

struct Underline {
  uint order;
  uint wavy;
  Bounds bounds;
  ContentMask content_mask;
  Rgba color;
  float thickness;
  uint pad;
};

struct MonochromeSprite {
  uint order;
  uint pad;
  AtlasTile tile;
  Bounds bounds;
  ContentMask content_mask;
  Rgba color;
};

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

struct Blur {
  float4 region;
  float4 source_rect;
  float2 direction;
  float sigma;
  float scale;
};

constant uint COMPOSITE_MATRIX = 1;
constant uint COMPOSITE_CLIP = 2;
constant uint COMPOSITE_MASK = 4;
constant uint COMPOSITE_SHADOW = 8;
constant uint COMPOSITE_BACKDROP = 16;
constant uint COMPOSITE_BLEND = 32;

constexpr sampler linear_sampler(mag_filter::linear, min_filter::linear);

float4 rgba(Rgba color) {
  return float4(color.r, color.g, color.b, color.a);
}

float2 unit_vertex(uint vertex_id) {
  return float2(vertex_id & 1u, (vertex_id >> 1) & 1u);
}

float4 to_device_position_impl(float2 position, constant Globals &globals) {
  float2 device_position = (position - globals.viewport_origin) / globals.viewport_size * float2(2.0, -2.0) + float2(-1.0, 1.0);
  return float4(device_position, 0.0, 1.0);
}

float4 to_device_position(float2 unit_vertex, Bounds bounds, constant Globals &globals) {
  return to_device_position_impl(unit_vertex * bounds.size + bounds.origin, globals);
}

float2 to_tile_position(float2 unit_vertex, AtlasTile tile, float2 atlas_size) {
  return (float2(tile.bounds.origin) + unit_vertex * float2(tile.bounds.size)) / atlas_size;
}

float4 distance_from_clip_rect_impl(float2 position, Bounds clip_bounds) {
  float2 tl = position - clip_bounds.origin;
  float2 br = clip_bounds.origin + clip_bounds.size - position;
  return float4(tl.x, br.x, tl.y, br.y);
}

float4 distance_from_clip_rect(float2 unit_vertex, Bounds bounds, Bounds clip_bounds) {
  return distance_from_clip_rect_impl(unit_vertex * bounds.size + bounds.origin, clip_bounds);
}

float4 over(float4 below, float4 above) {
  float alpha = above.a + below.a * (1.0 - above.a);
  // WARNING: two transparent colors would divide by zero, and blending a NaN is undefined on some GPUs.
  if (alpha <= 0.0) {
    return float4(0.0);
  }
  float3 color = (above.rgb * above.a + below.rgb * below.a * (1.0 - above.a)) / alpha;
  return float4(color, alpha);
}

float gaussian(float x, float sigma) {
  return exp(-(x * x) / (2. * sigma * sigma)) / (sqrt(2. * M_PI_F) * sigma);
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

float4 pick_corner(float2 center_to_point, float2 half_size, Radii radii) {
  float2 c = center_to_point;
  float2 tl = float2(radii.x.top_left, radii.y.top_left);
  float2 tr = float2(radii.x.top_right, radii.y.top_right);
  float2 br = float2(radii.x.bottom_right, radii.y.bottom_right);
  float2 bl = float2(radii.x.bottom_left, radii.y.bottom_left);
  float2 side = select(float2(1.), float2(-1.), c < 0.);
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
  return float4(radius.x > 0. && radius.y > 0. ? radius : float2(0.), side);
}

float quad_sdf_impl(float2 corner_center_to_point, float2 corner_radius) {
  if (corner_radius.x == 0.0) {
    return max(corner_center_to_point.x, corner_center_to_point.y);
  } else if (corner_radius.x == corner_radius.y) {
    float signed_distance_to_inset_quad =
        length(max(float2(0.0), corner_center_to_point)) +
        min(0.0, max(corner_center_to_point.x, corner_center_to_point.y));
    return signed_distance_to_inset_quad - corner_radius.x;
  } else if (corner_center_to_point.x <= 0.0 || corner_center_to_point.y <= 0.0) {
    return max(corner_center_to_point.x - corner_radius.x, corner_center_to_point.y - corner_radius.y);
  } else {
    float k0 = length(corner_center_to_point / corner_radius);
    float k1 = length(corner_center_to_point / (corner_radius * corner_radius));
    return k0 * (k0 - 1.0) / k1;
  }
}

float quad_sdf(float2 point, Bounds bounds, Radii corner_radii) {
  float2 half_size = bounds.size / 2.0;
  float2 center = bounds.origin + half_size;
  float2 center_to_point = point - center;
  float4 corner = pick_corner(center_to_point, half_size, corner_radii);
  float2 corner_radius = corner.xy;
  float2 corner_to_point = corner.zw * center_to_point - half_size;
  float2 corner_center_to_point = corner_to_point + corner_radius;
  return quad_sdf_impl(corner_center_to_point, corner_radius);
}

float2 window_position(float4 position, constant Globals &globals) {
  return position.xy + globals.viewport_origin;
}

float mask_alpha(float2 position, ContentMask mask) {
  if (mask.radii.x.top_left == 0.0 && mask.radii.x.top_right == 0.0 &&
      mask.radii.x.bottom_right == 0.0 && mask.radii.x.bottom_left == 0.0) {
    return 1.0;
  }
  return saturate(0.5 - quad_sdf(position, mask.rounded, mask.radii));
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

float dash_alpha(float t, float period, float length, float dash_velocity, float antialias_threshold) {
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

float quarter_ellipse_sdf(float2 point, float2 radii) {
  float2 circle_vec = point / radii;
  float unit_circle_sdf = length(circle_vec) - 1.0;
  return unit_circle_sdf * (radii.x + radii.y) * -0.5;
}

struct QuadVertexOutput {
  uint quad_id [[flat]];
  float4 position [[position]];
  float4 border_color [[flat]];
  float4 background_color [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct QuadFragmentInput {
  uint quad_id [[flat]];
  float4 position [[position]];
  float4 border_color [[flat]];
  float4 background_color [[flat]];
};

vertex QuadVertexOutput quad_vertex(uint vertex_id [[vertex_id]],
                                    uint quad_id [[instance_id]],
                                    constant Quad *quads [[buffer(0)]],
                                    constant Globals &globals [[buffer(1)]]) {
  float2 unit = unit_vertex(vertex_id);
  Quad quad = quads[quad_id];
  float4 device_position = to_device_position(unit, quad.bounds, globals);
  float4 clip_distance = distance_from_clip_rect(unit, quad.bounds, quad.content_mask.bounds);
  return QuadVertexOutput{
      quad_id,
      device_position,
      rgba(quad.border_color),
      rgba(quad.background),
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

float4 quad_color(QuadFragmentInput input, Quad quad, float2 position) {
  float4 background_color = input.background_color;

  bool unrounded = quad.corner_radii.x.top_left == 0.0 &&
    quad.corner_radii.x.bottom_left == 0.0 &&
    quad.corner_radii.x.top_right == 0.0 &&
    quad.corner_radii.x.bottom_right == 0.0;

  if (quad.border_widths.top == 0.0 &&
      quad.border_widths.left == 0.0 &&
      quad.border_widths.right == 0.0 &&
      quad.border_widths.bottom == 0.0 &&
      unrounded) {
    return background_color;
  }

  float2 size = quad.bounds.size;
  float2 half_size = size / 2.0;
  float2 point = position - quad.bounds.origin;
  float2 center_to_point = point - half_size;

  const float antialias_threshold = 0.5;

  float4 corner = pick_corner(center_to_point, half_size, quad.corner_radii);
  float2 corner_radius = corner.xy;
  bool left = corner.z < 0.0;
  bool top = corner.w < 0.0;

  float2 border = float2(
    left ? quad.border_widths.left : quad.border_widths.right,
    top ? quad.border_widths.top : quad.border_widths.bottom);

  // WARNING: zero-width borders turn negative so that `inner_sdf >= antialias_threshold`, which draws no
  // anti-aliasing pixels for them.
  float2 reduced_border = float2(
    border.x == 0.0 ? -antialias_threshold : border.x,
    border.y == 0.0 ? -antialias_threshold : border.y);

  float2 far_border = float2(
    left ? quad.border_widths.right : quad.border_widths.left,
    top ? quad.border_widths.bottom : quad.border_widths.top);
  float2 reduced_far_border = select(far_border, float2(-antialias_threshold), far_border == 0.0);

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
    float2 ellipse_radii = max(float2(0.0), corner_radius - reduced_border);
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
      float4 depths = float4(point.y, size.x - point.x, size.y - point.y, point.x);
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

    Bounds inner_bounds = {
      quad.bounds.origin + float2(quad.border_widths.left, quad.border_widths.top),
      max(float2(0.0), quad.bounds.size - float2(
        quad.border_widths.left + quad.border_widths.right,
        quad.border_widths.top + quad.border_widths.bottom))};
    Radii inner_radii = {
      {max(0.0, quad.corner_radii.x.top_left - quad.border_widths.left),
       max(0.0, quad.corner_radii.x.top_right - quad.border_widths.right),
       max(0.0, quad.corner_radii.x.bottom_right - quad.border_widths.right),
       max(0.0, quad.corner_radii.x.bottom_left - quad.border_widths.left)},
      {max(0.0, quad.corner_radii.y.top_left - quad.border_widths.top),
       max(0.0, quad.corner_radii.y.top_right - quad.border_widths.top),
       max(0.0, quad.corner_radii.y.bottom_right - quad.border_widths.bottom),
       max(0.0, quad.corner_radii.y.bottom_left - quad.border_widths.bottom)}};
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
      const float dash_length_per_width = dotted ? 1.0 : 2.0;
      const float dash_gap_per_width = 1.0;
      const float dash_period_per_width = dash_length_per_width + dash_gap_per_width;

      float dash_velocity = 0.0;

      const float dv_numerator = 1.0 / dash_period_per_width;

      if (unrounded) {
        bool is_horizontal = corner_center_to_point.x < corner_center_to_point.y;

        float2 dashed_border = float2(
          fmax(quad.border_widths.bottom, quad.border_widths.top),
          fmax(quad.border_widths.right, quad.border_widths.left));

        float border_width = is_horizontal ? dashed_border.x : dashed_border.y;
        dash_velocity = dv_numerator / border_width;
        t = is_horizontal ? point.x : point.y;
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
    color = mix(background_color, blended_border, saturate(antialias_threshold - inner_sdf));
  }

  return color * float4(1.0, 1.0, 1.0, saturate(antialias_threshold - outer_sdf));
}

fragment float4 quad_fragment(QuadFragmentInput input [[stage_in]],
                              constant Quad *quads [[buffer(0)]],
                              constant Globals &globals [[buffer(1)]]) {
  Quad quad = quads[input.quad_id];
  float2 position = window_position(input.position, globals);
  float4 color = quad_color(input, quad, position);
  color.a *= mask_alpha(position, quad.content_mask);
  return color;
}

struct ShadowVertexOutput {
  float4 position [[position]];
  float4 color [[flat]];
  uint shadow_id [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct ShadowFragmentInput {
  float4 position [[position]];
  float4 color [[flat]];
  uint shadow_id [[flat]];
};

vertex ShadowVertexOutput shadow_vertex(uint vertex_id [[vertex_id]],
                                        uint shadow_id [[instance_id]],
                                        constant Shadow *shadows [[buffer(0)]],
                                        constant Globals &globals [[buffer(1)]]) {
  float2 unit = unit_vertex(vertex_id);
  Shadow shadow = shadows[shadow_id];

  Bounds bounds = shadow.element_bounds;
  if (shadow.inset == 0) {
    float margin = 3. * shadow.blur_radius;
    bounds = shadow.bounds;
    bounds.origin -= float2(margin);
    bounds.size += float2(2. * margin);
  }

  float4 device_position = to_device_position(unit, bounds, globals);
  float4 clip_distance = distance_from_clip_rect(unit, bounds, shadow.content_mask.bounds);
  float4 color = rgba(shadow.color);

  return ShadowVertexOutput{
      device_position,
      color,
      shadow_id,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

float shadow_coverage(float2 position, Shadow shadow) {
  if (shadow.blur_radius < 0.25) {
    return saturate(0.5 - quad_sdf(position, shadow.bounds, shadow.corner_radii));
  }
  float2 half_size = shadow.bounds.size / 2.;
  float2 center = shadow.bounds.origin + half_size;
  float2 point = position - center;
  float2 corner_radius = pick_corner(point, half_size, shadow.corner_radii).xy;

  float low = point.y - half_size.y;
  float high = point.y + half_size.y;
  float start = clamp(-3. * shadow.blur_radius, low, high);
  float end = clamp(3. * shadow.blur_radius, low, high);

  float step = (end - start) / 4.;
  float y = start + step * 0.5;
  float alpha = 0.;
  for (int i = 0; i < 4; i++) {
    alpha += blur_along_x(point.x, point.y - y, shadow.blur_radius, corner_radius, half_size) *
             gaussian(y, shadow.blur_radius) * step;
    y += step;
  }
  return alpha;
}

fragment float4 shadow_fragment(ShadowFragmentInput input [[stage_in]],
                                constant Shadow *shadows [[buffer(0)]],
                                constant Globals &globals [[buffer(1)]]) {
  Shadow shadow = shadows[input.shadow_id];
  float2 position = window_position(input.position, globals);
  float coverage = shadow_coverage(position, shadow);
  float masked = mask_alpha(position, shadow.content_mask);
  float inside = saturate(0.5 - quad_sdf(position, shadow.element_bounds, shadow.element_corner_radii));
  coverage = shadow.inset != 0 ? (1.0 - coverage) * inside : coverage * (1.0 - inside);
  return input.color * float4(1., 1., 1., coverage * masked);
}

struct UnderlineVertexOutput {
  float4 position [[position]];
  float4 color [[flat]];
  uint underline_id [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct UnderlineFragmentInput {
  float4 position [[position]];
  float4 color [[flat]];
  uint underline_id [[flat]];
};

vertex UnderlineVertexOutput underline_vertex(uint vertex_id [[vertex_id]],
                                              uint underline_id [[instance_id]],
                                              constant Underline *underlines [[buffer(0)]],
                                              constant Globals &globals [[buffer(1)]]) {
  float2 unit = unit_vertex(vertex_id);
  Underline underline = underlines[underline_id];
  float4 device_position = to_device_position(unit, underline.bounds, globals);
  float4 clip_distance = distance_from_clip_rect(unit, underline.bounds, underline.content_mask.bounds);
  float4 color = rgba(underline.color);
  return UnderlineVertexOutput{
      device_position,
      color,
      underline_id,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 underline_fragment(UnderlineFragmentInput input [[stage_in]],
                                   constant Underline *underlines [[buffer(0)]],
                                   constant Globals &globals [[buffer(1)]]) {
  const float WAVE_FREQUENCY = 2.0;
  const float WAVE_HEIGHT_RATIO = 0.8;

  Underline underline = underlines[input.underline_id];
  float2 position = window_position(input.position, globals);
  float alpha = mask_alpha(position, underline.content_mask);
  if ((underline.wavy & 0xFF) != 0) {
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

struct MonochromeSpriteVertexOutput {
  float4 position [[position]];
  float2 tile_position;
  float4 color [[flat]];
  uint sprite_id [[flat]];
  float4 clip_distance;
};

vertex MonochromeSpriteVertexOutput monochrome_sprite_vertex(
    uint vertex_id [[vertex_id]], uint sprite_id [[instance_id]],
    constant MonochromeSprite *sprites [[buffer(0)]],
    constant Globals &globals [[buffer(1)]],
    texture2d<float> atlas_texture [[texture(0)]]) {
  float2 unit = unit_vertex(vertex_id);
  MonochromeSprite sprite = sprites[sprite_id];
  float2 position = unit * sprite.bounds.size + sprite.bounds.origin;
  float2 atlas_size = float2(atlas_texture.get_width(), atlas_texture.get_height());
  return MonochromeSpriteVertexOutput{
      to_device_position_impl(position, globals),
      to_tile_position(unit, sprite.tile, atlas_size),
      rgba(sprite.color),
      sprite_id,
      distance_from_clip_rect_impl(position, sprite.content_mask.bounds)};
}

fragment float4 monochrome_sprite_fragment(
    MonochromeSpriteVertexOutput input [[stage_in]],
    constant MonochromeSprite *sprites [[buffer(0)]],
    constant Globals &globals [[buffer(1)]],
    texture2d<float> atlas_texture [[texture(0)]]) {
  float4 sample = atlas_texture.sample(linear_sampler, input.tile_position);
  // WARNING: alpha clip after using the derivatives.
  if (any(input.clip_distance < float4(0.0))) {
    return float4(0.0);
  }
  float4 color = input.color;
  color.a *= sample.a * mask_alpha(window_position(input.position, globals), sprites[input.sprite_id].content_mask);
  return color;
}

struct PolychromeSpriteVertexOutput {
  float4 position [[position]];
  float2 tile_position;
  uint sprite_id [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct PolychromeSpriteFragmentInput {
  float4 position [[position]];
  float2 tile_position;
  uint sprite_id [[flat]];
};

vertex PolychromeSpriteVertexOutput polychrome_sprite_vertex(
    uint vertex_id [[vertex_id]], uint sprite_id [[instance_id]],
    constant PolychromeSprite *sprites [[buffer(0)]],
    constant Globals &globals [[buffer(1)]],
    texture2d<float> atlas_texture [[texture(0)]]) {
  float2 unit = unit_vertex(vertex_id);
  PolychromeSprite sprite = sprites[sprite_id];
  float4 device_position = to_device_position(unit, sprite.bounds, globals);
  float4 clip_distance = distance_from_clip_rect(unit, sprite.bounds, sprite.content_mask.bounds);
  float2 atlas_size = float2(atlas_texture.get_width(), atlas_texture.get_height());
  return PolychromeSpriteVertexOutput{
      device_position,
      to_tile_position(unit, sprite.tile, atlas_size),
      sprite_id,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 polychrome_sprite_fragment(
    PolychromeSpriteFragmentInput input [[stage_in]],
    constant PolychromeSprite *sprites [[buffer(0)]],
    constant Globals &globals [[buffer(1)]],
    texture2d<float> atlas_texture [[texture(0)]]) {
  PolychromeSprite sprite = sprites[input.sprite_id];
  float4 sample = atlas_texture.sample(linear_sampler, input.tile_position);
  float2 position = window_position(input.position, globals);
  Corners rx = sprite.corner_radii.x;
  bool square = rx.top_left == 0.0 && rx.top_right == 0.0 && rx.bottom_right == 0.0 && rx.bottom_left == 0.0;
  float corners = square ? 1.0 : saturate(0.5 - quad_sdf(position, sprite.bounds, sprite.corner_radii));

  float4 color = sample;
  if ((sprite.grayscale & 0xFF) != 0) {
    float grayscale = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
    color.rgb = float3(grayscale);
  }
  color.a *= sprite.opacity * corners * mask_alpha(position, sprite.content_mask);
  return color;
}

struct CompositeVertexOutput {
  float4 position [[position]];
  float2 local;
  uint composite_id [[flat]];
};

vertex CompositeVertexOutput composite_vertex(uint vertex_id [[vertex_id]],
                                              uint composite_id [[instance_id]],
                                              constant Composite *composites [[buffer(0)]],
                                              constant Globals &globals [[buffer(1)]]) {
  float2 unit = unit_vertex(vertex_id);
  Composite c = composites[composite_id];
  float2 local = c.bounds.xy + unit * c.bounds.zw;
  float4 p = c.transform * float4(local, 0.0, 1.0);
  float2 target_position = p.xy - globals.viewport_origin * p.w;
  return CompositeVertexOutput{
      float4(target_position.x / globals.viewport_size.x * 2.0 - p.w,
             p.w - target_position.y / globals.viewport_size.y * 2.0,
             0.0,
             p.w),
      local,
      composite_id};
}

float4 sample_layer(texture2d<float> source, float2 position, float4 rect) {
  float2 uv = (position - rect.xy) / rect.zw;
  if (any(uv < float2(0.0)) || any(uv > float2(1.0))) {
    return float4(0.0);
  }
  return source.sample(linear_sampler, uv, level(0));
}

float4 gaussian_sample(texture2d<float> source, float2 position, float2 direction, float sigma, float texel, float4 rect) {
  if (sigma < 0.25) {
    return sample_layer(source, position, rect);
  }
  float radius = ceil(3.0 * sigma / texel);
  float taps = min(radius, 32.0);
  float step = radius / taps * texel;
  float q = exp(-(step * step) / (2.0 * sigma * sigma));
  float4 sum = sample_layer(source, position, rect);
  float weight = 1.0;
  float w = 1.0;
  float g = q;
  for (float i = 1.0; i <= taps; i += 1.0) {
    w *= g;
    g *= q * q;
    float2 offset = direction * (i * step);
    sum += (sample_layer(source, position + offset, rect) + sample_layer(source, position - offset, rect)) * w;
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
    return float3(0.0);
  }
  return (color - n) * s / (x - n);
}

float3 hard_light(float3 cb, float3 cs) {
  return select(cb + 2.0 * cs - 1.0 - cb * (2.0 * cs - 1.0), cb * 2.0 * cs, cs <= float3(0.5));
}

float3 soft_light(float3 cb, float3 cs) {
  float3 d = select(sqrt(cb), ((16.0 * cb - 12.0) * cb + 4.0) * cb, cb <= float3(0.25));
  return select(cb + (2.0 * cs - 1.0) * (d - cb), cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb), cs <= float3(0.5));
}

float3 blend_mode(float3 cb, float3 cs, uint mode) {
  switch (mode) {
    case 1: return cb * cs;
    case 2: return cb + cs - cb * cs;
    case 3: return hard_light(cs, cb);
    case 4: return min(cb, cs);
    case 5: return max(cb, cs);
    case 6: return select(select(min(float3(1.0), cb / max(1.0 - cs, float3(1e-6))), float3(1.0), cs >= float3(1.0)), float3(0.0), cb <= float3(0.0));
    case 7: return select(select(1.0 - min(float3(1.0), (1.0 - cb) / max(cs, float3(1e-6))), float3(0.0), cs <= float3(0.0)), float3(1.0), cb >= float3(1.0));
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

fragment float4 composite_fragment(CompositeVertexOutput input [[stage_in]],
                                   constant Composite *composites [[buffer(0)]],
                                   constant Globals &globals [[buffer(1)]],
                                   texture2d<float> source [[texture(0)]],
                                   texture2d<float> mask [[texture(1)]],
                                   texture2d<float> backdrop_texture [[texture(2)]]) {
  Composite c = composites[input.composite_id];
  float2 target_position = window_position(input.position, globals);
  if (any(target_position < c.content_mask.xy) || any(target_position > c.content_mask.xy + c.content_mask.zw)) {
    return float4(0.0);
  }

  float4 color;
  if ((c.flags & COMPOSITE_BACKDROP) != 0) {
    color = gaussian_sample(source, target_position, float2(0.0, 1.0), c.blur, 1.0, c.source_rect);
  } else if ((c.flags & COMPOSITE_SHADOW) != 0) {
    color = c.shadow_color * gaussian_sample(source, input.local - c.shadow.xy, float2(0.0, 1.0), c.blur, 1.0, c.source_rect).a;
  } else {
    color = gaussian_sample(source, input.local, float2(0.0, 1.0), c.blur, 1.0, c.source_rect);
  }

  if ((c.flags & COMPOSITE_MATRIX) != 0 && color.a > 0.0) {
    float4 straight = float4(color.rgb / color.a, color.a);
    float4 mapped = saturate(c.color_matrix * straight + c.color_offset);
    color = float4(mapped.rgb * mapped.a, mapped.a);
  }

  ContentMask content_mask = {
    {c.content_mask.xy, c.content_mask.zw},
    {c.rounded_mask.xy, c.rounded_mask.zw},
    {{c.rounded_radii.x, c.rounded_radii.y, c.rounded_radii.z, c.rounded_radii.w},
     {c.rounded_radii_y.x, c.rounded_radii_y.y, c.rounded_radii_y.z, c.rounded_radii_y.w}}};
  float coverage = c.opacity * mask_alpha(target_position, content_mask);
  if ((c.flags & COMPOSITE_CLIP) != 0) {
    Bounds bounds = {c.clip.xy, c.clip.zw};
    Radii radii = {{c.clip_radii.x, c.clip_radii.y, c.clip_radii.z, c.clip_radii.w},
                   {c.clip_radii_y.x, c.clip_radii_y.y, c.clip_radii_y.z, c.clip_radii_y.w}};
    coverage *= saturate(0.5 - quad_sdf(input.local, bounds, radii));
  }
  if ((c.flags & COMPOSITE_MASK) != 0) {
    float2 t = (input.local - c.mask_rect.xy) / c.mask_rect.zw;
    if (any(t < float2(0.0)) || any(t > float2(1.0))) {
      coverage = 0.0;
    } else {
      float2 mask_size = float2(mask.get_width(), mask.get_height());
      coverage *= mask.sample(linear_sampler, (c.mask_tile.xy + t * c.mask_tile.zw) / mask_size, level(0)).a;
    }
  }
  color *= coverage;

  if ((c.flags & COMPOSITE_BLEND) != 0 && color.a > 0.0) {
    float4 backdrop = backdrop_texture.sample(linear_sampler, (target_position - c.backdrop_rect.xy) / c.backdrop_rect.zw, level(0));
    if (c.blend == 16) {
      return float4(min(color.rgb + backdrop.rgb, float3(1.0)) - backdrop.rgb * (1.0 - color.a), color.a);
    }
    float3 cb = backdrop.a > 0.0 ? backdrop.rgb / backdrop.a : float3(0.0);
    float3 cs = color.rgb / color.a;
    float3 mixed = (1.0 - backdrop.a) * cs + backdrop.a * saturate(blend_mode(cb, cs, c.blend));
    color = float4(mixed * color.a, color.a);
  }
  return color;
}

struct BlurVertexOutput {
  float4 position [[position]];
  uint blur_id [[flat]];
};

vertex BlurVertexOutput blur_vertex(uint vertex_id [[vertex_id]],
                                    uint blur_id [[instance_id]],
                                    constant Blur *blurs [[buffer(0)]],
                                    constant Globals &globals [[buffer(1)]]) {
  float2 unit = unit_vertex(vertex_id);
  Blur blur = blurs[blur_id];
  return BlurVertexOutput{
      to_device_position_impl(blur.region.xy + unit * blur.region.zw, globals),
      blur_id};
}

fragment float4 blur_fragment(BlurVertexOutput input [[stage_in]],
                              constant Blur *blurs [[buffer(0)]],
                              constant Globals &globals [[buffer(1)]],
                              texture2d<float> source [[texture(0)]]) {
  Blur blur = blurs[input.blur_id];
  float2 position = globals.viewport_origin + input.position.xy * blur.scale;
  return gaussian_sample(source, position, blur.direction, blur.sigma, blur.scale, blur.source_rect);
}
