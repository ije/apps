#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

// WARNING: these structs must match the `repr(C)` structs in `scene.rs` and `platform/frame.rs`.

// The render target covers `viewport_size` device pixels from `viewport_origin` of the window.
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

struct Edges {
  float top;
  float right;
  float bottom;
  float left;
};

struct ContentMask {
  Bounds bounds;
  Bounds rounded;
  Corners radii;
};

struct Hsla {
  float h;
  float s;
  float l;
  float a;
};

struct LinearColorStop {
  Hsla color;
  float percentage;
};

struct Background {
  // 0 is Solid, 1 is LinearGradient, 2 is PatternSlash
  uint tag;
  // 0 is sRGB, 1 is Oklab
  uint color_space;
  Hsla solid;
  float gradient_angle_or_pattern_height;
  LinearColorStop colors[2];
  uint pad;
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

struct TransformationMatrix {
  // Stored row-major.
  float2x2 rotation_scale;
  float2 translation;
};

struct Quad {
  uint order;
  uint border_style;
  Bounds bounds;
  ContentMask content_mask;
  Background background;
  Hsla border_color;
  Corners corner_radii;
  Edges border_widths;
};

struct Shadow {
  uint order;
  float blur_radius;
  Bounds bounds;
  Corners corner_radii;
  ContentMask content_mask;
  Hsla color;
  Bounds element_bounds;
  Corners element_corner_radii;
  uint inset;
  uint pad;
};

struct PathRasterizationVertex {
  float2 xy_position;
  float2 st_position;
  Background color;
  Bounds bounds;
  Bounds rounded;
  Corners radii;
};

struct PathSprite {
  Bounds bounds;
};

struct Underline {
  uint order;
  uint pad;
  Bounds bounds;
  ContentMask content_mask;
  Hsla color;
  float thickness;
  uint wavy;
};

struct MonochromeSprite {
  uint order;
  uint pad;
  AtlasTile tile;
  Bounds bounds;
  ContentMask content_mask;
  Hsla color;
  TransformationMatrix transformation;
};

struct PolychromeSprite {
  uint order;
  uint pad;
  AtlasTile tile;
  uint grayscale;
  float opacity;
  Bounds bounds;
  ContentMask content_mask;
  Corners corner_radii;
};

struct Composite {
  float4x4 transform;
  float4x4 color_matrix;
  float4 color_offset;
  float4 shadow_color;
  float4 bounds;
  float4 clip;
  float4 clip_radii;
  float4 mask_rect;
  float4 mask_tile;
  float4 content_mask;
  float4 rounded_mask;
  float4 rounded_radii;
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
  float sigma;
  float pad[3];
};

constant uint COMPOSITE_MATRIX = 1;
constant uint COMPOSITE_CLIP = 2;
constant uint COMPOSITE_MASK = 4;
constant uint COMPOSITE_SHADOW = 8;
constant uint COMPOSITE_BACKDROP = 16;
constant uint COMPOSITE_BLEND = 32;

constexpr sampler linear_sampler(mag_filter::linear, min_filter::linear);

struct GradientColor {
  float4 solid;
  float4 color0;
  float4 color1;
};

float4 hsla_to_rgba(Hsla hsla) {
  float h = hsla.h * 6.0; // Now, it's an angle but scaled in [0, 6) range
  float s = hsla.s;
  float l = hsla.l;
  float a = hsla.a;

  float c = (1.0 - fabs(2.0 * l - 1.0)) * s;
  float x = c * (1.0 - fabs(fmod(h, 2.0) - 1.0));
  float m = l - c / 2.0;

  float3 color = float3(m);
  if (h >= 0.0 && h < 1.0) {
    color.r += c;
    color.g += x;
  } else if (h >= 1.0 && h < 2.0) {
    color.r += x;
    color.g += c;
  } else if (h >= 2.0 && h < 3.0) {
    color.g += c;
    color.b += x;
  } else if (h >= 3.0 && h < 4.0) {
    color.g += x;
    color.b += c;
  } else if (h >= 4.0 && h < 5.0) {
    color.r += x;
    color.b += c;
  } else {
    color.r += c;
    color.b += x;
  }
  return float4(color, a);
}

float3 srgb_to_linear(float3 color) {
  return pow(color, float3(2.2));
}

float3 linear_to_srgb(float3 color) {
  return pow(color, float3(1.0 / 2.2));
}

float4 srgb_to_oklab(float4 color) {
  color = float4(srgb_to_linear(color.rgb), color.a);

  float l = 0.4122214708 * color.r + 0.5363325363 * color.g + 0.0514459929 * color.b;
  float m = 0.2119034982 * color.r + 0.6806995451 * color.g + 0.1073969566 * color.b;
  float s = 0.0883024619 * color.r + 0.2817188376 * color.g + 0.6299787005 * color.b;

  float l_ = pow(l, 1.0 / 3.0);
  float m_ = pow(m, 1.0 / 3.0);
  float s_ = pow(s, 1.0 / 3.0);

  return float4(
    0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
    1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
    0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    color.a);
}

float4 oklab_to_srgb(float4 color) {
  float l_ = color.r + 0.3963377774 * color.g + 0.2158037573 * color.b;
  float m_ = color.r - 0.1055613458 * color.g - 0.0638541728 * color.b;
  float s_ = color.r - 0.0894841775 * color.g - 1.2914855480 * color.b;

  float l = l_ * l_ * l_;
  float m = m_ * m_ * m_;
  float s = s_ * s_ * s_;

  float3 linear_rgb = float3(
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s);

  return float4(linear_to_srgb(linear_rgb), color.a);
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

float2 transform_position(float2 position, TransformationMatrix transformation) {
  return transpose(transformation.rotation_scale) * position + transformation.translation;
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
  float3 color = (above.rgb * above.a + below.rgb * below.a * (1.0 - above.a)) / alpha;
  return float4(color, alpha);
}

// A standard gaussian function, used for weighting samples
float gaussian(float x, float sigma) {
  return exp(-(x * x) / (2. * sigma * sigma)) / (sqrt(2. * M_PI_F) * sigma);
}

// This approximates the error function, needed for the gaussian integral
float2 erf(float2 x) {
  float2 s = sign(x);
  float2 a = abs(x);
  float2 r1 = 1. + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
  float2 r2 = r1 * r1;
  return s - s / (r2 * r2);
}

float blur_along_x(float x, float y, float sigma, float corner, float2 half_size) {
  float delta = min(half_size.y - corner - abs(y), 0.);
  float curved = half_size.x - corner + sqrt(max(0., corner * corner - delta * delta));
  float2 integral = 0.5 + 0.5 * erf((x + float2(-curved, curved)) * (sqrt(0.5) / sigma));
  return integral.y - integral.x;
}

// Selects corner radius based on quadrant.
float pick_corner_radius(float2 center_to_point, Corners radii) {
  if (center_to_point.x < 0.) {
    return center_to_point.y < 0. ? radii.top_left : radii.bottom_left;
  } else {
    return center_to_point.y < 0. ? radii.top_right : radii.bottom_right;
  }
}

float quad_sdf_impl(float2 corner_center_to_point, float corner_radius) {
  if (corner_radius == 0.0) {
    // Fast path for unrounded corners
    return max(corner_center_to_point.x, corner_center_to_point.y);
  } else {
    // Signed distance of the point from a quad that is inset by corner_radius
    // It is negative inside this quad, and positive outside
    float signed_distance_to_inset_quad =
        // 0 inside the inset quad, and positive outside
        length(max(float2(0.0), corner_center_to_point)) +
        // 0 outside the inset quad, and negative inside
        min(0.0, max(corner_center_to_point.x, corner_center_to_point.y));
    return signed_distance_to_inset_quad - corner_radius;
  }
}

// Signed distance of the point to the quad's border - positive outside the
// border, and negative inside.
float quad_sdf(float2 point, Bounds bounds, Corners corner_radii) {
  float2 half_size = bounds.size / 2.0;
  float2 center = bounds.origin + half_size;
  float2 center_to_point = point - center;
  float corner_radius = pick_corner_radius(center_to_point, corner_radii);
  float2 corner_to_point = fabs(center_to_point) - half_size;
  float2 corner_center_to_point = corner_to_point + corner_radius;
  return quad_sdf_impl(corner_center_to_point, corner_radius);
}

// The window position of a fragment.
float2 window_position(float4 position, constant Globals &globals) {
  return position.xy + globals.viewport_origin;
}

// Coverage of the rounded part of a content mask.
float mask_alpha(float2 position, ContentMask mask) {
  if (mask.radii.top_left == 0.0 && mask.radii.top_right == 0.0 &&
      mask.radii.bottom_right == 0.0 && mask.radii.bottom_left == 0.0) {
    return 1.0;
  }
  return saturate(0.5 - quad_sdf(position, mask.rounded, mask.radii));
}

GradientColor prepare_fill_color(uint tag, uint color_space, Hsla solid, Hsla color0, Hsla color1) {
  GradientColor out;
  if (tag == 0 || tag == 2 || tag == 3) {
    out.solid = hsla_to_rgba(solid);
  } else if (tag == 1) {
    out.color0 = hsla_to_rgba(color0);
    out.color1 = hsla_to_rgba(color1);

    // Prepare color space in vertex for avoid conversion
    // in fragment shader for performance reasons
    if (color_space == 1) {
      // Oklab
      out.color0 = srgb_to_oklab(out.color0);
      out.color1 = srgb_to_oklab(out.color1);
    }
  }
  return out;
}

float2x2 rotate2d(float angle) {
  float s = sin(angle);
  float c = cos(angle);
  return float2x2(c, -s, s, c);
}

float4 fill_color(Background background, float2 position, Bounds bounds,
                  float4 solid_color, float4 color0, float4 color1) {
  float4 color;

  switch (background.tag) {
    case 0:
      color = solid_color;
      break;
    case 1: {
      // -90 degrees to match the CSS gradient angle.
      float gradient_angle = background.gradient_angle_or_pattern_height;
      float radians = (fmod(gradient_angle, 360.0) - 90.0) * (M_PI_F / 180.0);
      float2 direction = float2(cos(radians), sin(radians));

      // Expand the short side to be the same as the long side
      if (bounds.size.x > bounds.size.y) {
        direction.y *= bounds.size.y / bounds.size.x;
      } else {
        direction.x *= bounds.size.x / bounds.size.y;
      }

      // Get the t value for the linear gradient with the color stop percentages.
      float2 half_size = bounds.size / 2.;
      float2 center = bounds.origin + half_size;
      float2 center_to_point = position - center;
      float t = dot(center_to_point, direction) / length(direction);
      // Check the direction to determine whether to use x or y
      if (abs(direction.x) > abs(direction.y)) {
        t = (t + half_size.x) / bounds.size.x;
      } else {
        t = (t + half_size.y) / bounds.size.y;
      }

      // Adjust t based on the stop percentages
      t = (t - background.colors[0].percentage) /
          (background.colors[1].percentage - background.colors[0].percentage);
      t = clamp(t, 0.0, 1.0);

      if (background.color_space == 1) {
        color = oklab_to_srgb(mix(color0, color1, t));
      } else {
        color = mix(color0, color1, t);
      }

      // Dither to reduce banding in gradients (especially dark/alpha).
      // Triangular-distributed noise breaks up 8-bit quantization steps.
      {
        float2 seed = position * 0.6180339887; // golden ratio spread
        float r1 = fract(sin(dot(seed, float2(12.9898, 78.233))) * 43758.5453);
        float r2 = fract(sin(dot(seed, float2(39.3460, 11.135))) * 24634.6345);
        float tri = r1 + r2 - 1.0; // triangular PDF, range [-1, +1]
        color.rgb += tri * 2.0 / 255.0;
        color.a += tri * 3.0 / 255.0;
      }
      break;
    }
    case 2: {
      float gradient_angle_or_pattern_height = background.gradient_angle_or_pattern_height;
      float pattern_width = (gradient_angle_or_pattern_height / 65535.0f) / 255.0f;
      float pattern_interval = fmod(gradient_angle_or_pattern_height, 65535.0f) / 255.0f;
      float pattern_height = pattern_width + pattern_interval;
      float stripe_angle = M_PI_F / 4.0;
      float pattern_period = pattern_height * sin(stripe_angle);
      float2x2 rotation = rotate2d(stripe_angle);
      float2 relative_position = position - bounds.origin;
      float2 rotated_point = rotation * relative_position;
      float pattern = fmod(rotated_point.x, pattern_period);
      float distance = min(pattern, pattern_period - pattern) - pattern_period * (pattern_width / pattern_height) / 2.0f;
      color = solid_color;
      color.a *= saturate(0.5 - distance);
      break;
    }
    case 3: {
      // checkerboard
      float size = background.gradient_angle_or_pattern_height;
      float2 relative_position = position - bounds.origin;
      float x_index = floor(relative_position.x / size);
      float y_index = floor(relative_position.y / size);
      float should_be_colored = fmod(x_index + y_index, 2.0);
      color = solid_color;
      color.a *= saturate(should_be_colored);
      break;
    }
  }

  return color;
}

// Returns the dash velocity of a corner given the dash velocity of the two
// sides, by returning the slower velocity (larger dashes).
//
// Since 0 is used for dash velocity when the border width is 0 (instead of
// +inf), this returns the other dash velocity in that case.
float corner_dash_velocity(float dv1, float dv2) {
  if (dv1 == 0.0) {
    return dv2;
  } else if (dv2 == 0.0) {
    return dv1;
  } else {
    return min(dv1, dv2);
  }
}

// Returns alpha used to render antialiased dashes.
// `t` is within the dash when `fmod(t, period) < length`.
float dash_alpha(float t, float period, float length, float dash_velocity, float antialias_threshold) {
  float half_period = period / 2.0;
  float half_length = length / 2.0;
  // Value in [-half_period, half_period]
  // The dash is in [-half_length, half_length]
  float centered = fmod(t + half_period - half_length, period) - half_period;
  // Signed distance for the dash, negative values are inside the dash
  float signed_distance = abs(centered) - half_length;
  // Antialiased alpha based on the signed distance
  return saturate(antialias_threshold - signed_distance / dash_velocity);
}

// Returns alpha used to render antialiased round dots of a dotted border.
float dot_alpha(float t, float period, float dot_length, float dash_velocity, float depth, float inner_depth, float antialias_threshold) {
  float width = depth + inner_depth;
  float half_period = period / 2.0;
  float half_length = dot_length / 2.0;
  float along = (fmod(t + half_period - half_length, period) - half_period) / dash_velocity;
  float across = depth - width / 2.0;
  return saturate(antialias_threshold - (length(float2(along, across)) - width / 2.0));
}

// This approximates distance to the nearest point to a quarter ellipse in a way
// that is sufficient for anti-aliasing when the ellipse is not very eccentric.
// The components of `point` are expected to be positive.
//
// Negative on the outside and positive on the inside.
float quarter_ellipse_sdf(float2 point, float2 radii) {
  // Scale the space to treat the ellipse like a unit circle
  float2 circle_vec = point / radii;
  float unit_circle_sdf = length(circle_vec) - 1.0;
  // Approximate up-scaling of the length by using the average of the radii.
  //
  // TODO: A better solution would be to use the gradient of the implicit
  // function for an ellipse to approximate a scaling factor.
  return unit_circle_sdf * (radii.x + radii.y) * -0.5;
}

// --- quads --- //

struct QuadVertexOutput {
  uint quad_id [[flat]];
  float4 position [[position]];
  float4 border_color [[flat]];
  float4 background_solid [[flat]];
  float4 background_color0 [[flat]];
  float4 background_color1 [[flat]];
  float clip_distance [[clip_distance]][4];
};

struct QuadFragmentInput {
  uint quad_id [[flat]];
  float4 position [[position]];
  float4 border_color [[flat]];
  float4 background_solid [[flat]];
  float4 background_color0 [[flat]];
  float4 background_color1 [[flat]];
};

vertex QuadVertexOutput quad_vertex(uint vertex_id [[vertex_id]],
                                    uint quad_id [[instance_id]],
                                    constant Quad *quads [[buffer(0)]],
                                    constant Globals &globals [[buffer(1)]]) {
  float2 unit = unit_vertex(vertex_id);
  Quad quad = quads[quad_id];
  float4 device_position = to_device_position(unit, quad.bounds, globals);
  float4 clip_distance = distance_from_clip_rect(unit, quad.bounds, quad.content_mask.bounds);
  float4 border_color = hsla_to_rgba(quad.border_color);

  GradientColor gradient = prepare_fill_color(
    quad.background.tag,
    quad.background.color_space,
    quad.background.solid,
    quad.background.colors[0].color,
    quad.background.colors[1].color);

  return QuadVertexOutput{
      quad_id,
      device_position,
      border_color,
      gradient.solid,
      gradient.color0,
      gradient.color1,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

float4 quad_color(QuadFragmentInput input, Quad quad, float2 position) {
  float4 background_color = fill_color(quad.background, position, quad.bounds,
    input.background_solid, input.background_color0, input.background_color1);

  bool unrounded = quad.corner_radii.top_left == 0.0 &&
    quad.corner_radii.bottom_left == 0.0 &&
    quad.corner_radii.top_right == 0.0 &&
    quad.corner_radii.bottom_right == 0.0;

  // Fast path when the quad is not rounded and doesn't have any border
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

  // Signed distance field threshold for inclusion of pixels. 0.5 is the
  // minimum distance between the center of the pixel and the edge.
  const float antialias_threshold = 0.5;

  // Radius of the nearest corner
  float corner_radius = pick_corner_radius(center_to_point, quad.corner_radii);

  // Width of the nearest borders
  float2 border = float2(
    center_to_point.x < 0.0 ? quad.border_widths.left : quad.border_widths.right,
    center_to_point.y < 0.0 ? quad.border_widths.top : quad.border_widths.bottom);

  // 0-width borders are reduced so that `inner_sdf >= antialias_threshold`.
  // The purpose of this is to not draw antialiasing pixels in this case.
  float2 reduced_border = float2(
    border.x == 0.0 ? -antialias_threshold : border.x,
    border.y == 0.0 ? -antialias_threshold : border.y);

  // Vector from the corner of the quad bounds to the point, after mirroring
  // the point into the bottom right quadrant. Both components are <= 0.
  float2 corner_to_point = fabs(center_to_point) - half_size;

  // Vector from the point to the center of the rounded corner's circle, also
  // mirrored into bottom right quadrant.
  float2 corner_center_to_point = corner_to_point + corner_radius;

  // Whether the nearest point on the border is rounded
  bool is_near_rounded_corner =
    corner_center_to_point.x >= 0.0 &&
    corner_center_to_point.y >= 0.0;

  // Vector from straight border inner corner to point.
  float2 straight_border_inner_corner_to_point = corner_to_point + reduced_border;

  // Whether the point is beyond the inner edge of the straight border
  bool is_beyond_inner_straight_border =
    straight_border_inner_corner_to_point.x > 0.0 ||
    straight_border_inner_corner_to_point.y > 0.0;

  // Whether the point is far enough inside the quad, such that the pixels are
  // not affected by the straight border.
  bool is_within_inner_straight_border =
    straight_border_inner_corner_to_point.x < -antialias_threshold &&
    straight_border_inner_corner_to_point.y < -antialias_threshold;

  // Fast path for points that must be part of the background
  if (is_within_inner_straight_border && !is_near_rounded_corner) {
    return background_color;
  }

  // Signed distance of the point to the outside edge of the quad's border
  float outer_sdf = quad_sdf_impl(corner_center_to_point, corner_radius);

  // Approximate signed distance of the point to the inside edge of the quad's
  // border. It is negative outside this edge (within the border), and
  // positive inside.
  //
  // This is not always an accurate signed distance:
  // * The rounded portions with varying border width use an approximation of
  //   nearest-point-on-ellipse.
  // * When it is quickly known to be outside the edge, -1.0 is used.
  float inner_sdf = 0.0;
  if (corner_center_to_point.x <= 0.0 || corner_center_to_point.y <= 0.0) {
    // Fast paths for straight borders
    inner_sdf = -max(straight_border_inner_corner_to_point.x,
                     straight_border_inner_corner_to_point.y);
  } else if (is_beyond_inner_straight_border) {
    // Fast path for points that must be outside the inner edge
    inner_sdf = -1.0;
  } else if (reduced_border.x == reduced_border.y) {
    // Fast path for circular inner edge.
    inner_sdf = -(outer_sdf + reduced_border.x);
  } else {
    float2 ellipse_radii = max(float2(0.0), float2(corner_radius) - reduced_border);
    inner_sdf = quarter_ellipse_sdf(corner_center_to_point, ellipse_radii);
  }

  // Negative when inside the border
  float border_sdf = max(inner_sdf, outer_sdf);

  float4 color = background_color;
  if (border_sdf < antialias_threshold) {
    float4 border_color = input.border_color;

    // Exact distance to the inner edge, for the double and dotted styles.
    Bounds inner_bounds = {
      quad.bounds.origin + float2(quad.border_widths.left, quad.border_widths.top),
      max(float2(0.0), quad.bounds.size - float2(
        quad.border_widths.left + quad.border_widths.right,
        quad.border_widths.top + quad.border_widths.bottom))};
    Corners inner_radii = {
      max(0.0, quad.corner_radii.top_left - max(quad.border_widths.top, quad.border_widths.left)),
      max(0.0, quad.corner_radii.top_right - max(quad.border_widths.top, quad.border_widths.right)),
      max(0.0, quad.corner_radii.bottom_right - max(quad.border_widths.bottom, quad.border_widths.right)),
      max(0.0, quad.corner_radii.bottom_left - max(quad.border_widths.bottom, quad.border_widths.left))};
    float inner_depth = quad_sdf(position, inner_bounds, inner_radii);

    // Double borders keep the outer and inner thirds.
    if (quad.border_style == 3) {
      float depth = -outer_sdf;
      float width = depth + inner_depth;
      border_color.a *= saturate(antialias_threshold - min(depth - width / 3.0, width * 2.0 / 3.0 - depth));
    }

    // Dashed (1) and dotted (2) border logic
    if (quad.border_style == 1 || quad.border_style == 2) {
      // Position along the perimeter in "dash space", where each dash
      // period has length 1
      float t = 0.0;

      // Total number of dash periods, so that the dash spacing can be
      // adjusted to evenly divide it
      float max_t = 0.0;

      // Border width is proportional to dash size. This is the behavior
      // used by browsers, but also avoids dashes from different segments
      // overlapping when dash size is smaller than the border width.
      //
      // Dash pattern: (2 * border width) dash, (1 * border width) gap
      bool dotted = quad.border_style == 2;
      const float dash_length_per_width = dotted ? 1.0 : 2.0;
      const float dash_gap_per_width = 1.0;
      const float dash_period_per_width = dash_length_per_width + dash_gap_per_width;

      // Since the dash size is determined by border width, the density of
      // dashes varies. Multiplying a pixel distance by this returns a
      // position in dash space - it has units (dash period / pixels). So
      // a dash velocity of (1 / 10) is 1 dash every 10 pixels.
      float dash_velocity = 0.0;

      // Dividing this by the border width gives the dash velocity
      const float dv_numerator = 1.0 / dash_period_per_width;

      if (unrounded) {
        // When corners aren't rounded, the dashes are separately laid
        // out on each straight line, rather than around the whole
        // perimeter. This way each line starts and ends with a dash.
        bool is_horizontal = corner_center_to_point.x < corner_center_to_point.y;

        // Choosing the right border width for dashed borders.
        // TODO: A better solution exists taking a look at the whole file.
        // this does not fix single dashed borders at the corners
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
        // When corners are rounded, the dashes are laid out clockwise
        // around the whole perimeter.

        float r_tr = quad.corner_radii.top_right;
        float r_br = quad.corner_radii.bottom_right;
        float r_bl = quad.corner_radii.bottom_left;
        float r_tl = quad.corner_radii.top_left;

        float w_t = quad.border_widths.top;
        float w_r = quad.border_widths.right;
        float w_b = quad.border_widths.bottom;
        float w_l = quad.border_widths.left;

        // Straight side dash velocities
        float dv_t = w_t <= 0.0 ? 0.0 : dv_numerator / w_t;
        float dv_r = w_r <= 0.0 ? 0.0 : dv_numerator / w_r;
        float dv_b = w_b <= 0.0 ? 0.0 : dv_numerator / w_b;
        float dv_l = w_l <= 0.0 ? 0.0 : dv_numerator / w_l;

        // Straight side lengths in dash space
        float s_t = (size.x - r_tl - r_tr) * dv_t;
        float s_r = (size.y - r_tr - r_br) * dv_r;
        float s_b = (size.x - r_br - r_bl) * dv_b;
        float s_l = (size.y - r_bl - r_tl) * dv_l;

        float corner_dash_velocity_tr = corner_dash_velocity(dv_t, dv_r);
        float corner_dash_velocity_br = corner_dash_velocity(dv_b, dv_r);
        float corner_dash_velocity_bl = corner_dash_velocity(dv_b, dv_l);
        float corner_dash_velocity_tl = corner_dash_velocity(dv_t, dv_l);

        // Corner lengths in dash space. Dots follow the middle of the border.
        float middle = dotted ? 0.5 : 0.0;
        float c_tr = max(0.0, r_tr - middle * max(w_t, w_r)) * (M_PI_F / 2.0) * corner_dash_velocity_tr;
        float c_br = max(0.0, r_br - middle * max(w_b, w_r)) * (M_PI_F / 2.0) * corner_dash_velocity_br;
        float c_bl = max(0.0, r_bl - middle * max(w_b, w_l)) * (M_PI_F / 2.0) * corner_dash_velocity_bl;
        float c_tl = max(0.0, r_tl - middle * max(w_t, w_l)) * (M_PI_F / 2.0) * corner_dash_velocity_tl;

        // Cumulative dash space upto each segment
        float upto_tr = s_t;
        float upto_r = upto_tr + c_tr;
        float upto_br = upto_r + s_r;
        float upto_b = upto_br + c_br;
        float upto_bl = upto_b + s_b;
        float upto_l = upto_bl + c_bl;
        float upto_tl = upto_l + s_l;
        max_t = upto_tl + c_tl;

        if (is_near_rounded_corner) {
          float radians = atan2(corner_center_to_point.y, corner_center_to_point.x);
          float corner_t = radians * max(0.0, corner_radius - middle * max(border.x, border.y));

          if (center_to_point.x >= 0.0) {
            if (center_to_point.y < 0.0) {
              dash_velocity = corner_dash_velocity_tr;
              // Subtracted because radians is pi/2 to 0 when
              // going clockwise around the top right corner,
              // since the y axis has been flipped
              t = upto_r - corner_t * dash_velocity;
            } else {
              dash_velocity = corner_dash_velocity_br;
              // Added because radians is 0 to pi/2 when going
              // clockwise around the bottom-right corner
              t = upto_br + corner_t * dash_velocity;
            }
          } else {
            if (center_to_point.y >= 0.0) {
              dash_velocity = corner_dash_velocity_bl;
              // Subtracted because radians is pi/2 to 0 when
              // going clockwise around the bottom-left corner,
              // since the x axis has been flipped
              t = upto_l - corner_t * dash_velocity;
            } else {
              dash_velocity = corner_dash_velocity_tl;
              // Added because radians is 0 to pi/2 when going
              // clockwise around the top-left corner, since both
              // axis were flipped
              t = upto_tl + corner_t * dash_velocity;
            }
          }
        } else {
          // Straight borders
          bool is_horizontal = corner_center_to_point.x < corner_center_to_point.y;
          if (is_horizontal) {
            if (center_to_point.y < 0.0) {
              dash_velocity = dv_t;
              t = (point.x - r_tl) * dash_velocity;
            } else {
              dash_velocity = dv_b;
              t = upto_bl - (point.x - r_bl) * dash_velocity;
            }
          } else {
            if (center_to_point.x < 0.0) {
              dash_velocity = dv_l;
              t = upto_tl - (point.y - r_tl) * dash_velocity;
            } else {
              dash_velocity = dv_r;
              t = upto_r + (point.y - r_tr) * dash_velocity;
            }
          }
        }
      }

      float dash_length = dash_length_per_width / dash_period_per_width;

      // Straight borders should start and end with a dash, so max_t is
      // reduced to cause this.
      max_t -= unrounded ? dash_length : 0.0;
      if (max_t >= 1.0) {
        // Adjust dash gap to evenly divide max_t
        float dash_count = floor(max_t);
        float dash_period = max_t / dash_count;
        if (dotted) {
          border_color.a *= dot_alpha(t, dash_period, dash_length, dash_velocity, -outer_sdf, inner_depth, antialias_threshold);
        } else {
          border_color.a *= dash_alpha(t, dash_period, dash_length, dash_velocity, antialias_threshold);
        }
      } else if (unrounded) {
        // When there isn't enough space for the full gap between the
        // two start / end dashes of a straight border, reduce gap to
        // make them fit.
        float dash_gap = max_t - dash_length;
        if (dash_gap > 0.0) {
          float dash_period = dash_length + dash_gap;
          border_color.a *= dash_alpha(t, dash_period, dash_length, dash_velocity, antialias_threshold);
        }
      }
    }

    // Blend the border on top of the background and then linearly interpolate
    // between the two as we slide inside the background.
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

// --- shadows --- //

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
    // Set the bounds of the shadow and adjust its size based on the shadow's
    // spread radius to achieve the spreading effect
    bounds = shadow.bounds;
    bounds.origin -= float2(margin);
    bounds.size += float2(2. * margin);
  }

  float4 device_position = to_device_position(unit, bounds, globals);
  float4 clip_distance = distance_from_clip_rect(unit, bounds, shadow.content_mask.bounds);
  float4 color = hsla_to_rgba(shadow.color);

  return ShadowVertexOutput{
      device_position,
      color,
      shadow_id,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

// Coverage of the shadow-casting box, blurred by a gaussian.
float shadow_coverage(float2 position, Shadow shadow) {
  if (shadow.blur_radius < 0.25) {
    return saturate(0.5 - quad_sdf(position, shadow.bounds, shadow.corner_radii));
  }
  float2 half_size = shadow.bounds.size / 2.;
  float2 center = shadow.bounds.origin + half_size;
  float2 point = position - center;
  float corner_radius = pick_corner_radius(point, shadow.corner_radii);

  // The signal is only non-zero in a limited range, so don't waste samples
  float low = point.y - half_size.y;
  float high = point.y + half_size.y;
  float start = clamp(-3. * shadow.blur_radius, low, high);
  float end = clamp(3. * shadow.blur_radius, low, high);

  // Accumulate samples (we can get away with surprisingly few samples)
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
  if (shadow.inset != 0) {
    float inside = saturate(0.5 - quad_sdf(position, shadow.element_bounds, shadow.element_corner_radii));
    coverage = (1.0 - coverage) * inside;
  }
  return input.color * float4(1., 1., 1., coverage * masked);
}

// --- underlines --- //

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
  float4 color = hsla_to_rgba(underline.color);
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

// --- monochrome sprites --- //

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
  float2 position = transform_position(unit * sprite.bounds.size + sprite.bounds.origin, sprite.transformation);
  float2 atlas_size = float2(atlas_texture.get_width(), atlas_texture.get_height());
  return MonochromeSpriteVertexOutput{
      to_device_position_impl(position, globals),
      to_tile_position(unit, sprite.tile, atlas_size),
      hsla_to_rgba(sprite.color),
      sprite_id,
      distance_from_clip_rect_impl(position, sprite.content_mask.bounds)};
}

fragment float4 monochrome_sprite_fragment(
    MonochromeSpriteVertexOutput input [[stage_in]],
    constant MonochromeSprite *sprites [[buffer(0)]],
    constant Globals &globals [[buffer(1)]],
    texture2d<float> atlas_texture [[texture(0)]]) {
  float4 sample = atlas_texture.sample(linear_sampler, input.tile_position);
  // Alpha clip after using the derivatives.
  if (any(input.clip_distance < float4(0.0))) {
    return float4(0.0);
  }
  float4 color = input.color;
  color.a *= sample.a * mask_alpha(window_position(input.position, globals), sprites[input.sprite_id].content_mask);
  return color;
}

// --- polychrome sprites --- //

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
  float distance = quad_sdf(position, sprite.bounds, sprite.corner_radii);

  float4 color = sample;
  if ((sprite.grayscale & 0xFF) != 0) {
    float grayscale = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
    color.rgb = float3(grayscale);
  }
  color.a *= sprite.opacity * saturate(0.5 - distance) * mask_alpha(position, sprite.content_mask);
  return color;
}

// --- paths --- //

struct PathRasterizationVertexOutput {
  float4 position [[position]];
  float2 st_position;
  uint vertex_id [[flat]];
  float clip_rect_distance [[clip_distance]][4];
};

struct PathRasterizationFragmentInput {
  float4 position [[position]];
  float2 st_position;
  uint vertex_id [[flat]];
};

vertex PathRasterizationVertexOutput path_rasterization_vertex(
    uint vertex_id [[vertex_id]],
    constant PathRasterizationVertex *vertices [[buffer(0)]],
    constant Globals &globals [[buffer(1)]]) {
  PathRasterizationVertex v = vertices[vertex_id];
  float4 clip_distance = distance_from_clip_rect_impl(v.xy_position, v.bounds);
  return PathRasterizationVertexOutput{
      to_device_position_impl(v.xy_position, globals),
      v.st_position,
      vertex_id,
      {clip_distance.x, clip_distance.y, clip_distance.z, clip_distance.w}};
}

fragment float4 path_rasterization_fragment(
    PathRasterizationFragmentInput input [[stage_in]],
    constant PathRasterizationVertex *vertices [[buffer(0)]],
    constant Globals &globals [[buffer(1)]]) {
  float2 dx = dfdx(input.st_position);
  float2 dy = dfdy(input.st_position);

  PathRasterizationVertex v = vertices[input.vertex_id];
  Background background = v.color;
  float alpha;
  if (length(float2(dx.x, dy.x)) < 0.001) {
    alpha = 1.0;
  } else {
    float2 gradient = float2(
      (2. * input.st_position.x) * dx.x - dx.y,
      (2. * input.st_position.x) * dy.x - dy.y);
    float f = (input.st_position.x * input.st_position.x) - input.st_position.y;
    float distance = f / length(gradient);
    alpha = saturate(0.5 - distance);
  }

  GradientColor gradient_color = prepare_fill_color(
    background.tag,
    background.color_space,
    background.solid,
    background.colors[0].color,
    background.colors[1].color);

  float2 position = window_position(input.position, globals);
  float4 color = fill_color(background, position, v.bounds,
    gradient_color.solid, gradient_color.color0, gradient_color.color1);
  ContentMask mask = {v.bounds, v.rounded, v.radii};
  alpha *= color.a * mask_alpha(position, mask);
  return float4(color.rgb * alpha, alpha);
}

struct PathSpriteVertexOutput {
  float4 position [[position]];
  float2 texture_coords;
};

vertex PathSpriteVertexOutput path_sprite_vertex(
    uint vertex_id [[vertex_id]], uint sprite_id [[instance_id]],
    constant PathSprite *sprites [[buffer(0)]],
    constant Globals &globals [[buffer(1)]]) {
  float2 unit = unit_vertex(vertex_id);
  PathSprite sprite = sprites[sprite_id];
  // The path texture covers the window.
  float2 window_position = sprite.bounds.origin + unit * sprite.bounds.size;
  return PathSpriteVertexOutput{
      to_device_position(unit, sprite.bounds, globals),
      window_position / globals.window_size};
}

fragment float4 path_sprite_fragment(
    PathSpriteVertexOutput input [[stage_in]],
    texture2d<float> intermediate_texture [[texture(0)]]) {
  return intermediate_texture.sample(linear_sampler, input.texture_coords);
}

// --- effects --- //

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

// Samples a premultiplied texel of a texture covering `rect` of the window, transparent outside it.
float4 sample_layer(texture2d<float> source, float2 position, float4 rect) {
  float2 uv = (position - rect.xy) / rect.zw;
  if (any(uv < float2(0.0)) || any(uv > float2(1.0))) {
    return float4(0.0);
  }
  return source.sample(linear_sampler, uv, level(0));
}

// Sums gaussian taps along `direction`, spaced to keep large blurs cheap.
float4 gaussian_sample(texture2d<float> source, float2 position, float2 direction, float sigma, float4 rect) {
  if (sigma < 0.25) {
    return sample_layer(source, position, rect);
  }
  float radius = ceil(3.0 * sigma);
  float taps = min(radius, 32.0);
  float step = radius / taps;
  float4 sum = float4(0.0);
  float weight = 0.0;
  for (float i = -taps; i <= taps; i += 1.0) {
    float offset = i * step;
    float w = exp(-(offset * offset) / (2.0 * sigma * sigma));
    sum += sample_layer(source, position + direction * offset, rect) * w;
    weight += w;
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

// The CSS blend modes, on unpremultiplied colors.
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
    color = gaussian_sample(source, target_position, float2(0.0, 1.0), c.blur, c.source_rect);
  } else if ((c.flags & COMPOSITE_SHADOW) != 0) {
    color = c.shadow_color * gaussian_sample(source, input.local - c.shadow.xy, float2(0.0, 1.0), c.blur, c.source_rect).a;
  } else {
    color = gaussian_sample(source, input.local, float2(0.0, 1.0), c.blur, c.source_rect);
  }

  if ((c.flags & COMPOSITE_MATRIX) != 0 && color.a > 0.0) {
    float4 straight = float4(color.rgb / color.a, color.a);
    float4 mapped = saturate(c.color_matrix * straight + c.color_offset);
    color = float4(mapped.rgb * mapped.a, mapped.a);
  }

  ContentMask content_mask = {
    {c.content_mask.xy, c.content_mask.zw},
    {c.rounded_mask.xy, c.rounded_mask.zw},
    {c.rounded_radii.x, c.rounded_radii.y, c.rounded_radii.z, c.rounded_radii.w}};
  float coverage = c.opacity * mask_alpha(target_position, content_mask);
  if ((c.flags & COMPOSITE_CLIP) != 0) {
    Bounds bounds = {c.clip.xy, c.clip.zw};
    Corners radii = {c.clip_radii.x, c.clip_radii.y, c.clip_radii.z, c.clip_radii.w};
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
      // plus-lighter adds both layers.
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

// The horizontal half of a separable blur; the composite does the vertical half.
fragment float4 blur_fragment(BlurVertexOutput input [[stage_in]],
                              constant Blur *blurs [[buffer(0)]],
                              constant Globals &globals [[buffer(1)]],
                              texture2d<float> source [[texture(0)]]) {
  Blur blur = blurs[input.blur_id];
  return gaussian_sample(source, window_position(input.position, globals), float2(1.0, 0.0), blur.sigma, blur.source_rect);
}
