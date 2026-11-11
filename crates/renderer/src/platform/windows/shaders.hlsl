// A hand port of src/shaders.wgsl. Keep the math, the layouts and the entry points in step with it.
//
// Registers, by the WGSL binding they stand for:
//   b0  globals      group 0 binding 0
//   s0  filtered     group 0 binding 1, linear and clamped
//   t0  t1  t2       group 1 bindings 0 to 2
//   b1  composite    group 2 binding 0
//   b2  blur         group 2 binding 1
// Instances arrive as 13 float4 attributes with the semantics I0 to I12, in the order of gpu.rs, one step per
// instance. Copies use CopySubresourceRegion, so the WGSL copy entry points have no port.

static const uint BOX = 0u;
static const uint SHADOW = 1u;
static const uint LINE = 2u;
static const uint GLYPH = 3u;
static const uint IMAGE = 4u;
static const uint FILL = 5u;

static const uint SOLID = 0u;
static const uint DASHED = 1u;
static const uint DOTTED = 2u;
static const uint DOUBLE = 3u;

static const uint COMPOSITE_MATRIX = 1u;
static const uint COMPOSITE_CLIP = 2u;
static const uint COMPOSITE_MASK = 4u;
static const uint COMPOSITE_SHADOW = 8u;
static const uint COMPOSITE_BACKDROP = 16u;
static const uint COMPOSITE_BLEND = 32u;

static const float PI = 3.14159265;
static const float3 REC709 = float3(0.2126, 0.7152, 0.0722);

struct Globals {
    float4 viewport;
    float text_contrast;
};

struct Composite {
    column_major float4x4 transform;
    column_major float4x4 color_matrix;
    float4 color_offset;
    float4 shadow_color;
    float4 bounds;
    float4 clip_box;
    float4 clip_rx;
    float4 clip_ry;
    float4 mask_rect;
    float4 mask_tile;
    float4 content_mask;
    float4 rounded_mask;
    float4 rounded_rx;
    float4 rounded_ry;
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

cbuffer GlobalsBlock : register(b0) { Globals globals; };
cbuffer CompositeBlock : register(b1) { Composite composite; };
cbuffer BlurBlock : register(b2) { Blur blur; };
SamplerState filtered : register(s0);
Texture2D t0 : register(t0);
Texture2D t1 : register(t1);
Texture2D t2 : register(t2);

struct Instance {
    float4 rect : I0;
    float4 clip_box : I1;
    float4 rounded : I2;
    float4 rounded_rx : I3;
    float4 rounded_ry : I4;
    float4 rx : I5;
    float4 ry : I6;
    float4 color : I7;
    float4 border_color : I8;
    float4 aux : I9;
    float4 aux_rx : I10;
    float4 aux_ry : I11;
    float4 info : I12;
};

struct Varyings {
    float4 position : SV_Position;
    float2 texel : TEXCOORD0;
    nointerpolation float4 rect : TEXCOORD1;
    nointerpolation float4 rx : TEXCOORD2;
    nointerpolation float4 ry : TEXCOORD3;
    nointerpolation float4 color : TEXCOORD4;
    nointerpolation float4 border_color : TEXCOORD5;
    nointerpolation float4 aux : TEXCOORD6;
    nointerpolation float4 aux_rx : TEXCOORD7;
    nointerpolation float4 aux_ry : TEXCOORD8;
    nointerpolation float4 info : TEXCOORD9;
    nointerpolation float4 rounded : TEXCOORD10;
    nointerpolation float4 rounded_rx : TEXCOORD11;
    nointerpolation float4 rounded_ry : TEXCOORD12;
};

float2 strip_corner(uint index) {
    return float2(index & 1u, index >> 1u);
}

float4 to_clip_space(float2 at) {
    float2 uv = (at - globals.viewport.xy) / globals.viewport.zw;
    return float4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
}

float4 premultiply(float4 color) {
    return float4(color.rgb * color.a, color.a);
}

float2 texture_size(Texture2D source) {
    uint width, height;
    source.GetDimensions(width, height);
    return float2(width, height);
}

uint pick(float2 from_center, float2 hs, float4 rx, float4 ry) {
    bool west = from_center.x < 0.0;
    uint corner = from_center.y < 0.0 ? (west ? 0u : 1u) : (west ? 3u : 2u);
    bool4 in_x = from_center.x * float4(-1.0, 1.0, 1.0, -1.0) > hs.x - rx;
    bool4 in_y = from_center.y * float4(-1.0, -1.0, 1.0, 1.0) > hs.y - ry;
    bool4 inside = in_x && in_y;
    if (inside.w) { corner = 3u; }
    if (inside.z) { corner = 2u; }
    if (inside.y) { corner = 1u; }
    if (inside.x) { corner = 0u; }
    return corner;
}

float2 mirror(uint corner) {
    return float2(corner == 0u || corner == 3u ? -1.0 : 1.0, corner < 2u ? -1.0 : 1.0);
}

float2 radius_of(uint corner, float4 rx, float4 ry) {
    float2 radius = float2(rx[corner], ry[corner]);
    return all(radius > 0.0) ? radius : float2(0.0, 0.0);
}

float box_distance(float2 p, float4 box, float4 rx, float4 ry) {
    float2 hs = box.zw * 0.5;
    float2 from_center = p - box.xy - hs;
    uint corner = pick(from_center, hs, rx, ry);
    float2 radius = radius_of(corner, rx, ry);
    float2 edge = from_center * mirror(corner) - hs;
    float2 curve = edge + radius;
    if (all(radius > 0.0) && all(curve > 0.0)) {
        float scaled = length(curve / radius);
        return scaled * (scaled - 1.0) / length(curve / (radius * radius));
    }
    return max(edge.x, edge.y);
}

float rounded_clip(float2 p, float4 box, float4 rx, float4 ry) {
    if (all(rx == 0.0)) {
        return 1.0;
    }
    return saturate(0.5 - box_distance(p, box, rx, ry));
}

float either_speed(float a, float b) {
    if (a == 0.0) {
        return b;
    }
    if (b == 0.0) {
        return a;
    }
    return min(a, b);
}

float dash_coverage(float2 p, float4 rect, float4 rx, float4 ry, float4 widths, bool dotted, float depth, float inner) {
    float period_widths = dotted ? 2.0 : 3.0;
    float dash = (dotted ? 1.0 : 2.0) / period_widths;
    float2 local = p - rect.xy;
    float2 hs = rect.zw * 0.5;
    float2 from_center = local - hs;
    uint corner = pick(from_center, hs, rx, ry);
    float2 radius = radius_of(corner, rx, ry);
    float2 curve = from_center * mirror(corner) - hs + radius;
    bool across = curve.x < curve.y;
    float t = 0.0;
    float speed = 0.0;
    float total = 0.0;
    if (all(rx == 0.0)) {
        float width = across ? max(widths.x, widths.z) : max(widths.y, widths.w);
        speed = 1.0 / (period_widths * width);
        t = (across ? local.x : local.y) * speed;
        total = (across ? rect.z : rect.w) * speed - dash;
    } else {
        float4 side_speed = widths > 0.0 ? 1.0 / (period_widths * widths) : (float4)0.0;
        float4 corner_speed = float4(
            either_speed(side_speed.x, side_speed.y),
            either_speed(side_speed.z, side_speed.y),
            either_speed(side_speed.z, side_speed.w),
            either_speed(side_speed.x, side_speed.w));
        float middle = dotted ? 0.5 : 0.0;
        float4 sweep = max((float4)0.0, (rx.yzwx + ry.yzwx) * 0.5 - middle * max(widths.xzzx, widths.yyww)) * (PI * 0.5) * corner_speed;
        float top_end = (rect.z - rx.x - rx.y) * side_speed.x;
        float right_start = top_end + sweep.x;
        float right_end = right_start + (rect.w - ry.y - ry.z) * side_speed.y;
        float bottom_start = right_end + sweep.y;
        float bottom_end = bottom_start + (rect.z - rx.z - rx.w) * side_speed.z;
        float left_start = bottom_end + sweep.z;
        float left_end = left_start + (rect.w - ry.w - ry.x) * side_speed.w;
        total = left_end + sweep.w;
        if (all(curve >= 0.0) && all(radius > 0.0)) {
            float2 flip = mirror(corner);
            float2 nearby = float2(flip.x < 0.0 ? widths.w : widths.y, flip.y < 0.0 ? widths.x : widths.z);
            float along = atan2(curve.y / radius.y, curve.x / radius.x) * max(0.0, (radius.x + radius.y) * 0.5 - middle * max(nearby.x, nearby.y));
            if (corner == 1u) {
                speed = corner_speed.x;
                t = right_start - along * speed;
            } else if (corner == 2u) {
                speed = corner_speed.y;
                t = right_end + along * speed;
            } else if (corner == 3u) {
                speed = corner_speed.z;
                t = left_start - along * speed;
            } else {
                speed = corner_speed.w;
                t = left_end + along * speed;
            }
        } else if (across) {
            if (corner < 2u) {
                speed = side_speed.x;
                t = (local.x - rx.x) * speed;
            } else {
                speed = side_speed.z;
                t = bottom_end - (local.x - rx.w) * speed;
            }
        } else if (corner == 0u || corner == 3u) {
            speed = side_speed.w;
            t = left_end - (local.y - ry.x) * speed;
        } else {
            speed = side_speed.y;
            t = right_start + (local.y - ry.y) * speed;
        }
    }
    if (total >= 1.0) {
        float period = total / floor(total);
        float from_dash = fmod(t + (period - dash) * 0.5, period) - period * 0.5;
        if (dotted) {
            float width = depth + inner;
            return saturate(0.5 - (length(float2(from_dash / speed, depth - width * 0.5)) - width * 0.5));
        }
        return saturate(0.5 - (abs(from_dash) - dash * 0.5) / speed);
    }
    if (all(rx == 0.0) && total > dash) {
        float from_dash = fmod(t + (total - dash) * 0.5, total) - total * 0.5;
        return saturate(0.5 - (abs(from_dash) - dash * 0.5) / speed);
    }
    return 1.0;
}

float side_coverage(float2 local, float2 size, float4 widths, uint sides) {
    float4 depths = float4(local.y, size.x - local.x, size.y - local.y, local.x);
    float coverage = 0.0;
    [unroll] for (uint drawn = 0u; drawn < 4u; drawn++) {
        if ((sides & (1u << drawn)) == 0u || widths[drawn] <= 0.0) {
            continue;
        }
        float nearest = 1e6;
        [unroll] for (uint other = 0u; other < 4u; other++) {
            if ((sides & (1u << other)) != 0u || widths[other] <= 0.0) {
                continue;
            }
            float2 pair = float2(widths[drawn], widths[other]);
            float norm = ((drawn + other) & 1u) == 1u ? length(pair) : pair.x + pair.y;
            nearest = min(nearest, (depths[other] * pair.x - depths[drawn] * pair.y) / norm);
        }
        coverage = max(coverage, saturate(0.5 + nearest));
    }
    return coverage;
}

float4 box_color(float2 p, float4 rect, float4 rx, float4 ry, float4 fill, float4 border, float4 widths, uint flags) {
    float outer = box_distance(p, rect, rx, ry);
    float4 base = premultiply(fill) * saturate(0.5 - outer);
    if (all(widths == 0.0)) {
        return base;
    }
    float4 w = widths == 0.0 ? (float4)-0.5 : widths;
    float4 padding = float4(rect.xy + w.wx, rect.zw - w.wx - w.yz);
    float inner = box_distance(p, padding, max(rx - w.wyyw, (float4)0.0), max(ry - w.xxzz, (float4)0.0));
    float ring = saturate(0.5 + inner);
    if (ring <= 0.0) {
        return base;
    }
    float alpha = border.a;
    uint sides = (flags >> 8u) & 15u;
    if (sides != 0u) {
        alpha *= side_coverage(p - rect.xy, rect.zw, widths, sides);
    }
    uint style = flags & 255u;
    if (style == DOUBLE) {
        float width = inner - outer;
        alpha *= saturate(0.5 - min(-outer - width / 3.0, width * 2.0 / 3.0 + outer));
    } else if (style == DASHED || style == DOTTED) {
        alpha *= dash_coverage(p, rect, rx, ry, widths, style == DOTTED, -outer, inner);
    }
    float4 fill_color = premultiply(fill);
    float4 over = float4(border.rgb * alpha, alpha) + fill_color * (1.0 - alpha);
    return lerp(fill_color, over, ring) * saturate(0.5 - outer);
}

// Abramowitz and Stegun 7.1.27.
float2 erf(float2 x) {
    float2 a = abs(x);
    float2 q = 1.0 + a * (0.278393 + a * (0.230389 + a * (0.000972 + a * 0.078108)));
    float2 q2 = q * q;
    return sign(x) * (1.0 - 1.0 / (q2 * q2));
}

float blurred_box(float2 p, float4 box, float4 rx, float4 ry, float sigma) {
    float2 hs = box.zw * 0.5;
    float2 from_center = p - box.xy - hs;
    float2 radius = radius_of(pick(from_center, hs, rx, ry), rx, ry);
    float low = clamp(-3.0 * sigma, from_center.y - hs.y, from_center.y + hs.y);
    float high = clamp(3.0 * sigma, from_center.y - hs.y, from_center.y + hs.y);
    float stride = (high - low) * 0.25;
    float sum = 0.0;
    [unroll] for (uint row = 0u; row < 4u; row++) {
        float y = low + (float(row) + 0.5) * stride;
        float into = min(hs.y - radius.y - abs(from_center.y - y), 0.0);
        float bulge = radius.y > 0.0 ? radius.x * sqrt(max(0.0, 1.0 - into * into / (radius.y * radius.y))) : 0.0;
        float extent = hs.x - radius.x + bulge;
        float2 span = erf((from_center.x + float2(-extent, extent)) * (0.70710678 / sigma));
        sum += (span.y - span.x) * exp(-0.5 * y * y / (sigma * sigma));
    }
    return 0.5 * sum * stride / (sqrt(2.0 * PI) * sigma);
}

float wave_coverage(float2 p, float4 rect, float thickness) {
    float x = p.x - rect.x;
    float frequency = 2.0 * PI * thickness / (rect.w * rect.w);
    float amplitude = 0.8 * thickness;
    float slope = amplitude * frequency * cos(x * frequency);
    float gap = (p.y - rect.y - rect.w * 0.5 - amplitude * sin(x * frequency)) / sqrt(1.0 + slope * slope);
    return saturate(0.5 - (abs(gap) - thickness * 0.5));
}

Varyings draw_vertex(uint index : SV_VertexID, Instance instance) {
    uint kind = (uint)instance.info.x;
    float4 area = instance.rect;
    if (kind == SHADOW) {
        if (((uint)instance.info.y & 1u) != 0u) {
            area = instance.aux;
        } else {
            float spread = instance.info.z * 3.0;
            area = float4(instance.rect.xy - spread, instance.rect.zw + spread * 2.0);
        }
    }
    float2 low = max(area.xy, instance.clip_box.xy);
    float2 high = max(low, min(area.xy + area.zw, instance.clip_box.xy + instance.clip_box.zw));
    float2 at = lerp(low, high, strip_corner(index));
    Varyings o;
    o.position = to_clip_space(at);
    o.texel = instance.aux.xy + (at - instance.rect.xy) / instance.rect.zw * instance.aux.zw;
    o.rect = instance.rect;
    o.rx = instance.rx;
    o.ry = instance.ry;
    o.color = instance.color;
    o.border_color = instance.border_color;
    o.aux = instance.aux;
    o.aux_rx = instance.aux_rx;
    o.aux_ry = instance.aux_ry;
    o.info = instance.info;
    o.rounded = instance.rounded;
    o.rounded_rx = instance.rounded_rx;
    o.rounded_ry = instance.rounded_ry;
    if (kind == BOX && all(instance.rx == 0.0) && all(instance.aux == 0.0) && all(instance.rounded_rx == 0.0)) {
        o.info.x = float(FILL);
    }
    return o;
}

float4 draw_fragment(Varyings v) : SV_Target {
    uint kind = (uint)v.info.x;
    if (kind == FILL) {
        return premultiply(v.color);
    }
    float2 p = v.position.xy + globals.viewport.xy;
    uint flags = (uint)v.info.y;
    float4 color;
    if (kind == BOX) {
        color = box_color(p, v.rect, v.rx, v.ry, v.color, v.border_color, v.aux, flags);
    } else if (kind == SHADOW) {
        float shape;
        if (v.info.z < 0.25) {
            shape = saturate(0.5 - box_distance(p, v.rect, v.rx, v.ry));
        } else {
            shape = blurred_box(p, v.rect, v.rx, v.ry, v.info.z);
        }
        float caster = saturate(0.5 - box_distance(p, v.aux, v.aux_rx, v.aux_ry));
        color = premultiply(v.color) * ((flags & 1u) != 0u ? (1.0 - shape) * caster : shape * (1.0 - caster));
    } else if (kind == LINE) {
        color = premultiply(v.color);
        if ((flags & 1u) != 0u) {
            color *= wave_coverage(p, v.rect, v.info.z);
        }
    } else if (kind == GLYPH) {
        float coverage = t0.SampleLevel(filtered, v.texel / texture_size(t0), 0.0).r;
        float ink = dot(v.color.rgb, REC709) * 2.0 - 1.0;
        coverage = saturate(coverage + coverage * (1.0 - coverage) * ink * globals.text_contrast);
        color = premultiply(v.color) * coverage;
    } else {
        float4 pixel = t1.SampleLevel(filtered, v.texel / texture_size(t1), 0.0);
        if ((flags & 2u) != 0u && pixel.a > 0.0) {
            pixel = float4(pixel.rgb / pixel.a, pixel.a);
        }
        if ((flags & 1u) != 0u) {
            pixel = float4((float3)dot(pixel.rgb, REC709), pixel.a);
        }
        float alpha = pixel.a * v.info.z;
        if (any(v.rx != 0.0)) {
            alpha *= saturate(0.5 - box_distance(p, v.rect, v.rx, v.ry));
        }
        color = float4(pixel.rgb * alpha, alpha);
    }
    return color * rounded_clip(p, v.rounded, v.rounded_rx, v.rounded_ry);
}

struct Placed {
    float4 position : SV_Position;
    float2 local : TEXCOORD0;
};

Placed composite_vertex(uint index : SV_VertexID) {
    float2 local = composite.bounds.xy + strip_corner(index) * composite.bounds.zw;
    float4 q = mul(composite.transform, float4(local, 0.0, 1.0));
    float2 at = q.xy - globals.viewport.xy * q.w;
    Placed o;
    o.position = float4(at.x / globals.viewport.z * 2.0 - q.w, q.w - at.y / globals.viewport.w * 2.0, 0.0, q.w);
    o.local = local;
    return o;
}

float4 layer_pixel(Texture2D source, float2 at, float4 rect) {
    float2 uv = (at - rect.xy) / rect.zw;
    if (any(uv < 0.0) || any(uv > 1.0)) {
        return (float4)0.0;
    }
    return source.SampleLevel(filtered, uv, 0.0);
}

float4 gaussian(Texture2D source, float2 at, float2 direction, float sigma, float texel, float4 rect) {
    if (sigma < 0.25) {
        return layer_pixel(source, at, rect);
    }
    float reach = ceil(3.0 * sigma / texel);
    float taps = min(reach, 32.0);
    float stride = reach / taps * texel;
    float4 sum = layer_pixel(source, at, rect);
    float total = 1.0;
    [loop] for (float tap = 1.0; tap <= taps; tap += 1.0) {
        float offset = tap * stride;
        float weight = exp(-0.5 * offset * offset / (sigma * sigma));
        sum += (layer_pixel(source, at + direction * offset, rect) + layer_pixel(source, at - direction * offset, rect)) * weight;
        total += 2.0 * weight;
    }
    return sum / total;
}

float luminosity(float3 c) {
    return dot(c, float3(0.3, 0.59, 0.11));
}

float3 with_luminosity(float3 c, float l) {
    float3 result = c + (l - luminosity(c));
    float center = luminosity(result);
    float low = min(min(result.r, result.g), result.b);
    float high = max(max(result.r, result.g), result.b);
    if (low < 0.0) {
        result = center + (result - center) * center / (center - low);
    }
    if (high > 1.0) {
        result = center + (result - center) * (1.0 - center) / (high - center);
    }
    return result;
}

float chroma(float3 c) {
    return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
}

float3 with_chroma(float3 c, float s) {
    float low = min(min(c.r, c.g), c.b);
    float span = max(max(c.r, c.g), c.b) - low;
    if (span <= 0.0) {
        return (float3)0.0;
    }
    return (c - low) * s / span;
}

float3 hard_light(float3 b, float3 s) {
    float3 doubled = 2.0 * s;
    return s <= 0.5 ? b * doubled : b + doubled - 1.0 - b * (doubled - 1.0);
}

float3 soft_light(float3 b, float3 s) {
    float3 d = b <= 0.25 ? ((16.0 * b - 12.0) * b + 4.0) * b : sqrt(b);
    return s <= 0.5 ? b - (1.0 - 2.0 * s) * b * (1.0 - b) : b + (2.0 * s - 1.0) * (d - b);
}

float3 blend_colors(float3 b, float3 s, uint mode) {
    float3 one = (float3)1.0;
    float3 zero = (float3)0.0;
    switch (mode) {
        case 1u: return b * s;
        case 2u: return b + s - b * s;
        case 3u: return hard_light(s, b);
        case 4u: return min(b, s);
        case 5u: return max(b, s);
        case 6u: return b <= zero ? zero : (s >= one ? one : min(one, b / max(one - s, (float3)1e-6)));
        case 7u: return b >= one ? one : (s <= zero ? zero : one - min(one, (one - b) / max(s, (float3)1e-6)));
        case 8u: return hard_light(b, s);
        case 9u: return soft_light(b, s);
        case 10u: return abs(b - s);
        case 11u: return b + s - 2.0 * b * s;
        case 12u: return with_luminosity(with_chroma(s, chroma(b)), luminosity(b));
        case 13u: return with_luminosity(with_chroma(b, chroma(s)), luminosity(b));
        case 14u: return with_luminosity(s, luminosity(b));
        case 15u: return with_luminosity(b, luminosity(s));
        default: return s;
    }
}

float4 composite_fragment(Placed v) : SV_Target {
    Composite c = composite;
    float2 p = v.position.xy + globals.viewport.xy;
    if (any(p < c.content_mask.xy) || any(p > c.content_mask.xy + c.content_mask.zw)) {
        return (float4)0.0;
    }
    float4 color;
    if ((c.flags & COMPOSITE_BACKDROP) != 0u) {
        color = gaussian(t0, p, float2(0.0, 1.0), c.blur, 1.0, c.source_rect);
    } else if ((c.flags & COMPOSITE_SHADOW) != 0u) {
        color = c.shadow_color * gaussian(t0, v.local - c.shadow.xy, float2(0.0, 1.0), c.blur, 1.0, c.source_rect).a;
    } else {
        color = gaussian(t0, v.local, float2(0.0, 1.0), c.blur, 1.0, c.source_rect);
    }
    if ((c.flags & COMPOSITE_MATRIX) != 0u && color.a > 0.0) {
        float4 mapped = saturate(mul(c.color_matrix, float4(color.rgb / color.a, color.a)) + c.color_offset);
        color = premultiply(mapped);
    }
    float coverage = c.opacity * rounded_clip(p, c.rounded_mask, c.rounded_rx, c.rounded_ry);
    if ((c.flags & COMPOSITE_CLIP) != 0u) {
        coverage *= saturate(0.5 - box_distance(v.local, c.clip_box, c.clip_rx, c.clip_ry));
    }
    if ((c.flags & COMPOSITE_MASK) != 0u) {
        float2 at = (v.local - c.mask_rect.xy) / c.mask_rect.zw;
        if (any(at < 0.0) || any(at > 1.0)) {
            coverage = 0.0;
        } else {
            float2 uv = (c.mask_tile.xy + at * c.mask_tile.zw) / texture_size(t1);
            coverage *= t1.SampleLevel(filtered, uv, 0.0).a;
        }
    }
    color *= coverage;
    if ((c.flags & COMPOSITE_BLEND) != 0u && color.a > 0.0) {
        float4 below = t2.SampleLevel(filtered, (p - c.backdrop_rect.xy) / c.backdrop_rect.zw, 0.0);
        if (c.blend == 16u) {
            return float4(min(color.rgb + below.rgb, (float3)1.0) - below.rgb * (1.0 - color.a), color.a);
        }
        float3 b = below.a > 0.0 ? below.rgb / below.a : (float3)0.0;
        float3 s = color.rgb / color.a;
        float3 mixed = (1.0 - below.a) * s + below.a * saturate(blend_colors(b, s, c.blend));
        color = float4(mixed * color.a, color.a);
    }
    return color;
}

float4 blur_vertex(uint index : SV_VertexID) : SV_Position {
    return to_clip_space(blur.region.xy + strip_corner(index) * blur.region.zw);
}

float4 blur_fragment(float4 position : SV_Position) : SV_Target {
    float2 at = globals.viewport.xy + position.xy * blur.scale;
    return gaussian(t0, at, blur.direction, blur.sigma, blur.scale, blur.source_rect);
}
