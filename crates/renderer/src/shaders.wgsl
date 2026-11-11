// The one shader source of all backends. Keep the layouts in sync with gpu.rs.
//
// Bindings:
//   group 0  binding 0  `globals`    uniform Globals, set per pass
//            binding 1  `filtered`   linear sampler, clamped to the edge
//   group 1  binding 0  `t0`         draw: the mask page (one channel, in red). composite, blur, copy: the source
//            binding 1  `t1`         draw: the color page. composite: the mask image
//            binding 2  `t2`         composite: the backdrop copy
//   group 2  binding 0  `composite`  uniform Composite, set per composite draw
//            binding 1  `blur`       uniform Blur, set per blur draw
//            binding 2  `copy`       uniform Copy, set per copy draw
// Instances arrive as 13 vec4 vertex attributes, one step per instance. Every pipeline draws a 4-vertex
// triangle strip.
// Metal (see build.rs): instances in buffer 0, globals in buffer 1, the group 2 block in buffer 2,
// textures 0-2, and an inline sampler.
//
// Colors come in with straight alpha. Every fragment returns premultiplied alpha, which blends with
// one and one minus source alpha.

const BOX: u32 = 0u;
const SHADOW: u32 = 1u;
const LINE: u32 = 2u;
const GLYPH: u32 = 3u;
const IMAGE: u32 = 4u;
// A box without radii, borders or a rounded clip, set by the vertex stage.
const FILL: u32 = 5u;

const SOLID: u32 = 0u;
const DASHED: u32 = 1u;
const DOTTED: u32 = 2u;
const DOUBLE: u32 = 3u;

const COMPOSITE_MATRIX: u32 = 1u;
const COMPOSITE_CLIP: u32 = 2u;
const COMPOSITE_MASK: u32 = 4u;
const COMPOSITE_SHADOW: u32 = 8u;
const COMPOSITE_BACKDROP: u32 = 16u;
const COMPOSITE_BLEND: u32 = 32u;

const PI: f32 = 3.14159265;
const REC709: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

struct Globals {
    viewport: vec4<f32>,
    text_contrast: f32,
}

struct Composite {
    transform: mat4x4<f32>,
    color_matrix: mat4x4<f32>,
    color_offset: vec4<f32>,
    shadow_color: vec4<f32>,
    bounds: vec4<f32>,
    clip: vec4<f32>,
    clip_rx: vec4<f32>,
    clip_ry: vec4<f32>,
    mask_rect: vec4<f32>,
    mask_tile: vec4<f32>,
    content_mask: vec4<f32>,
    rounded_mask: vec4<f32>,
    rounded_rx: vec4<f32>,
    rounded_ry: vec4<f32>,
    shadow: vec4<f32>,
    source_rect: vec4<f32>,
    backdrop_rect: vec4<f32>,
    opacity: f32,
    blur: f32,
    flags: u32,
    blend: u32,
}

struct Blur {
    region: vec4<f32>,
    source_rect: vec4<f32>,
    direction: vec2<f32>,
    sigma: f32,
    scale: f32,
}

struct Copy {
    offset: vec4<i32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var filtered: sampler;
@group(1) @binding(0) var t0: texture_2d<f32>;
@group(1) @binding(1) var t1: texture_2d<f32>;
@group(1) @binding(2) var t2: texture_2d<f32>;
@group(2) @binding(0) var<uniform> composite: Composite;
@group(2) @binding(1) var<uniform> blur: Blur;
@group(2) @binding(2) var<uniform> copy: Copy;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) clip: vec4<f32>,
    @location(2) round: vec4<f32>,
    @location(3) round_rx: vec4<f32>,
    @location(4) round_ry: vec4<f32>,
    @location(5) rx: vec4<f32>,
    @location(6) ry: vec4<f32>,
    @location(7) color: vec4<f32>,
    @location(8) border_color: vec4<f32>,
    @location(9) aux: vec4<f32>,
    @location(10) aux_rx: vec4<f32>,
    @location(11) aux_ry: vec4<f32>,
    @location(12) info: vec4<f32>,
}

// 13 vectors, within the 15 of GL ES 3.
struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) texel: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) rx: vec4<f32>,
    @location(3) @interpolate(flat) ry: vec4<f32>,
    @location(4) @interpolate(flat) color: vec4<f32>,
    @location(5) @interpolate(flat) border_color: vec4<f32>,
    @location(6) @interpolate(flat) aux: vec4<f32>,
    @location(7) @interpolate(flat) aux_rx: vec4<f32>,
    @location(8) @interpolate(flat) aux_ry: vec4<f32>,
    @location(9) @interpolate(flat) info: vec4<f32>,
    @location(10) @interpolate(flat) round: vec4<f32>,
    @location(11) @interpolate(flat) round_rx: vec4<f32>,
    @location(12) @interpolate(flat) round_ry: vec4<f32>,
}

fn strip_corner(index: u32) -> vec2<f32> {
    return vec2<f32>(f32(index & 1u), f32(index >> 1u));
}

fn to_clip_space(at: vec2<f32>) -> vec4<f32> {
    let unit = (at - globals.viewport.xy) / globals.viewport.zw;
    return vec4<f32>(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
}

fn premultiply(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(color.rgb * color.a, color.a);
}

// Corners are numbered 0 top left, 1 top right, 2 bottom right, 3 bottom left. `from_center` picks the
// corner whose curve box holds the point, or else the quarter of the box the point is in.
fn pick(from_center: vec2<f32>, half: vec2<f32>, rx: vec4<f32>, ry: vec4<f32>) -> u32 {
    let west = from_center.x < 0.0;
    var corner = select(select(2u, 3u, west), select(1u, 0u, west), from_center.y < 0.0);
    let in_x = from_center.x * vec4<f32>(-1.0, 1.0, 1.0, -1.0) > vec4<f32>(half.x) - rx;
    let in_y = from_center.y * vec4<f32>(-1.0, -1.0, 1.0, 1.0) > vec4<f32>(half.y) - ry;
    let inside = in_x & in_y;
    if inside.w { corner = 3u; }
    if inside.z { corner = 2u; }
    if inside.y { corner = 1u; }
    if inside.x { corner = 0u; }
    return corner;
}

fn mirror(corner: u32) -> vec2<f32> {
    return vec2<f32>(select(1.0, -1.0, corner == 0u || corner == 3u), select(1.0, -1.0, corner < 2u));
}

// A corner radius is only round when both of its axes are.
fn radius_of(corner: u32, rx: vec4<f32>, ry: vec4<f32>) -> vec2<f32> {
    let radius = vec2<f32>(rx[corner], ry[corner]);
    return select(vec2<f32>(0.0), radius, all(radius > vec2<f32>(0.0)));
}

// Signed distance, negative inside.
fn box_distance(p: vec2<f32>, box: vec4<f32>, rx: vec4<f32>, ry: vec4<f32>) -> f32 {
    let half = box.zw * 0.5;
    let from_center = p - box.xy - half;
    let corner = pick(from_center, half, rx, ry);
    let radius = radius_of(corner, rx, ry);
    let edge = from_center * mirror(corner) - half;
    let curve = edge + radius;
    if all(radius > vec2<f32>(0.0)) && all(curve > vec2<f32>(0.0)) {
        let scaled = length(curve / radius);
        return scaled * (scaled - 1.0) / length(curve / (radius * radius));
    }
    return max(edge.x, edge.y);
}

fn rounded_clip(p: vec2<f32>, box: vec4<f32>, rx: vec4<f32>, ry: vec4<f32>) -> f32 {
    if all(rx == vec4<f32>(0.0)) {
        return 1.0;
    }
    return saturate(0.5 - box_distance(p, box, rx, ry));
}

fn either_speed(a: f32, b: f32) -> f32 {
    if a == 0.0 {
        return b;
    }
    if b == 0.0 {
        return a;
    }
    return min(a, b);
}

fn dash_coverage(p: vec2<f32>, rect: vec4<f32>, rx: vec4<f32>, ry: vec4<f32>, widths: vec4<f32>, dotted: bool, depth: f32, inner: f32) -> f32 {
    let period_widths = select(3.0, 2.0, dotted);
    let dash = select(2.0, 1.0, dotted) / period_widths;
    let local = p - rect.xy;
    let half = rect.zw * 0.5;
    let from_center = local - half;
    let corner = pick(from_center, half, rx, ry);
    let radius = radius_of(corner, rx, ry);
    let curve = from_center * mirror(corner) - half + radius;
    let across = curve.x < curve.y;
    var t = 0.0;
    var speed = 0.0;
    var total = 0.0;
    if all(rx == vec4<f32>(0.0)) {
        let width = select(max(widths.y, widths.w), max(widths.x, widths.z), across);
        speed = 1.0 / (period_widths * width);
        t = select(local.y, local.x, across) * speed;
        total = select(rect.w, rect.z, across) * speed - dash;
    } else {
        let side_speed = select(vec4<f32>(0.0), 1.0 / (period_widths * widths), widths > vec4<f32>(0.0));
        // Speeds and arc lengths of the corners in the order top right, bottom right, bottom left, top left.
        let corner_speed = vec4<f32>(
            either_speed(side_speed.x, side_speed.y),
            either_speed(side_speed.z, side_speed.y),
            either_speed(side_speed.z, side_speed.w),
            either_speed(side_speed.x, side_speed.w),
        );
        let middle = select(0.0, 0.5, dotted);
        let sweep = max(vec4<f32>(0.0), (rx.yzwx + ry.yzwx) * 0.5 - middle * max(widths.xzzx, widths.yyww)) * (PI * 0.5) * corner_speed;
        let top_end = (rect.z - rx.x - rx.y) * side_speed.x;
        let right_start = top_end + sweep.x;
        let right_end = right_start + (rect.w - ry.y - ry.z) * side_speed.y;
        let bottom_start = right_end + sweep.y;
        let bottom_end = bottom_start + (rect.z - rx.z - rx.w) * side_speed.z;
        let left_start = bottom_end + sweep.z;
        let left_end = left_start + (rect.w - ry.w - ry.x) * side_speed.w;
        total = left_end + sweep.w;
        if all(curve >= vec2<f32>(0.0)) && all(radius > vec2<f32>(0.0)) {
            let flip = mirror(corner);
            let near = vec2<f32>(select(widths.y, widths.w, flip.x < 0.0), select(widths.z, widths.x, flip.y < 0.0));
            let along = atan2(curve.y / radius.y, curve.x / radius.x) * max(0.0, (radius.x + radius.y) * 0.5 - middle * max(near.x, near.y));
            switch corner {
                case 1u: {
                    speed = corner_speed.x;
                    t = right_start - along * speed;
                }
                case 2u: {
                    speed = corner_speed.y;
                    t = right_end + along * speed;
                }
                case 3u: {
                    speed = corner_speed.z;
                    t = left_start - along * speed;
                }
                default: {
                    speed = corner_speed.w;
                    t = left_end + along * speed;
                }
            }
        } else if across {
            if corner < 2u {
                speed = side_speed.x;
                t = (local.x - rx.x) * speed;
            } else {
                speed = side_speed.z;
                t = bottom_end - (local.x - rx.w) * speed;
            }
        } else if corner == 0u || corner == 3u {
            speed = side_speed.w;
            t = left_end - (local.y - ry.x) * speed;
        } else {
            speed = side_speed.y;
            t = right_start + (local.y - ry.y) * speed;
        }
    }
    if total >= 1.0 {
        let period = total / floor(total);
        let from_dash = (t + (period - dash) * 0.5) % period - period * 0.5;
        if dotted {
            let width = depth + inner;
            return saturate(0.5 - (length(vec2<f32>(from_dash / speed, depth - width * 0.5)) - width * 0.5));
        }
        return saturate(0.5 - (abs(from_dash) - dash * 0.5) / speed);
    }
    if all(rx == vec4<f32>(0.0)) && total > dash {
        let from_dash = (t + (total - dash) * 0.5) % total - total * 0.5;
        return saturate(0.5 - (abs(from_dash) - dash * 0.5) / speed);
    }
    return 1.0;
}

fn side_coverage(local: vec2<f32>, size: vec2<f32>, widths: vec4<f32>, sides: u32) -> f32 {
    let depths = vec4<f32>(local.y, size.x - local.x, size.y - local.y, local.x);
    var coverage = 0.0;
    for (var drawn = 0u; drawn < 4u; drawn++) {
        if (sides & (1u << drawn)) == 0u || widths[drawn] <= 0.0 {
            continue;
        }
        var nearest = 1e6;
        for (var other = 0u; other < 4u; other++) {
            if (sides & (1u << other)) != 0u || widths[other] <= 0.0 {
                continue;
            }
            let pair = vec2<f32>(widths[drawn], widths[other]);
            let norm = select(pair.x + pair.y, length(pair), ((drawn + other) & 1u) == 1u);
            nearest = min(nearest, (depths[other] * pair.x - depths[drawn] * pair.y) / norm);
        }
        coverage = max(coverage, saturate(0.5 + nearest));
    }
    return coverage;
}

fn box_color(p: vec2<f32>, rect: vec4<f32>, rx: vec4<f32>, ry: vec4<f32>, fill: vec4<f32>, border: vec4<f32>, widths: vec4<f32>, flags: u32) -> vec4<f32> {
    let outer = box_distance(p, rect, rx, ry);
    let base = premultiply(fill) * saturate(0.5 - outer);
    if all(widths == vec4<f32>(0.0)) {
        return base;
    }
    // A side without a border moves half a pixel out, so its edge gets no border color.
    let w = select(widths, vec4<f32>(-0.5), widths == vec4<f32>(0.0));
    let padding = vec4<f32>(rect.xy + w.wx, rect.zw - w.wx - w.yz);
    let inner = box_distance(p, padding, max(rx - w.wyyw, vec4<f32>(0.0)), max(ry - w.xxzz, vec4<f32>(0.0)));
    let ring = saturate(0.5 + inner);
    if ring <= 0.0 {
        return base;
    }
    var alpha = border.a;
    let sides = (flags >> 8u) & 15u;
    if sides != 0u {
        alpha *= side_coverage(p - rect.xy, rect.zw, widths, sides);
    }
    let style = flags & 255u;
    if style == DOUBLE {
        let width = inner - outer;
        alpha *= saturate(0.5 - min(-outer - width / 3.0, width * 2.0 / 3.0 + outer));
    } else if style == DASHED || style == DOTTED {
        alpha *= dash_coverage(p, rect, rx, ry, widths, style == DOTTED, -outer, inner);
    }
    let fill_color = premultiply(fill);
    let over = vec4<f32>(border.rgb * alpha, alpha) + fill_color * (1.0 - alpha);
    return mix(fill_color, over, ring) * saturate(0.5 - outer);
}

// Abramowitz and Stegun 7.1.27.
fn erf(x: vec2<f32>) -> vec2<f32> {
    let a = abs(x);
    let q = 1.0 + a * (0.278393 + a * (0.230389 + a * (0.000972 + a * 0.078108)));
    let q2 = q * q;
    return sign(x) * (1.0 - 1.0 / (q2 * q2));
}

// Exact across each row, sampled at 4 rows within 3 sigma.
fn blurred_box(p: vec2<f32>, box: vec4<f32>, rx: vec4<f32>, ry: vec4<f32>, sigma: f32) -> f32 {
    let half = box.zw * 0.5;
    let from_center = p - box.xy - half;
    let radius = radius_of(pick(from_center, half, rx, ry), rx, ry);
    let low = clamp(-3.0 * sigma, from_center.y - half.y, from_center.y + half.y);
    let high = clamp(3.0 * sigma, from_center.y - half.y, from_center.y + half.y);
    let step = (high - low) * 0.25;
    var sum = 0.0;
    for (var row = 0u; row < 4u; row++) {
        let y = low + (f32(row) + 0.5) * step;
        let into = min(half.y - radius.y - abs(from_center.y - y), 0.0);
        let bulge = select(0.0, radius.x * sqrt(max(0.0, 1.0 - into * into / (radius.y * radius.y))), radius.y > 0.0);
        let extent = half.x - radius.x + bulge;
        let span = erf((from_center.x + vec2<f32>(-extent, extent)) * (0.70710678 / sigma));
        sum += (span.y - span.x) * exp(-0.5 * y * y / (sigma * sigma));
    }
    return 0.5 * sum * step / (sqrt(2.0 * PI) * sigma);
}

fn wave_coverage(p: vec2<f32>, rect: vec4<f32>, thickness: f32) -> f32 {
    let x = p.x - rect.x;
    let frequency = 2.0 * PI * thickness / (rect.w * rect.w);
    let amplitude = 0.8 * thickness;
    let slope = amplitude * frequency * cos(x * frequency);
    let distance = (p.y - rect.y - rect.w * 0.5 - amplitude * sin(x * frequency)) / sqrt(1.0 + slope * slope);
    return saturate(0.5 - (abs(distance) - thickness * 0.5));
}

@vertex
fn draw_vertex(@builtin(vertex_index) index: u32, instance: Instance) -> Varyings {
    let kind = u32(instance.info.x);
    var area = instance.rect;
    if kind == SHADOW {
        if (u32(instance.info.y) & 1u) != 0u {
            area = instance.aux;
        } else {
            let spread = instance.info.z * 3.0;
            area = vec4<f32>(instance.rect.xy - spread, instance.rect.zw + spread * 2.0);
        }
    }
    let low = max(area.xy, instance.clip.xy);
    let high = max(low, min(area.xy + area.zw, instance.clip.xy + instance.clip.zw));
    let at = mix(low, high, strip_corner(index));
    var out: Varyings;
    out.position = to_clip_space(at);
    out.texel = instance.aux.xy + (at - instance.rect.xy) / instance.rect.zw * instance.aux.zw;
    out.rect = instance.rect;
    out.rx = instance.rx;
    out.ry = instance.ry;
    out.color = instance.color;
    out.border_color = instance.border_color;
    out.aux = instance.aux;
    out.aux_rx = instance.aux_rx;
    out.aux_ry = instance.aux_ry;
    out.info = instance.info;
    out.round = instance.round;
    out.round_rx = instance.round_rx;
    out.round_ry = instance.round_ry;
    let zero = vec4<f32>(0.0);
    if kind == BOX && all(instance.rx == zero) && all(instance.aux == zero) && all(instance.round_rx == zero) {
        out.info.x = f32(FILL);
    }
    return out;
}

@fragment
fn draw_fragment(v: Varyings) -> @location(0) vec4<f32> {
    let kind = u32(v.info.x);
    if kind == FILL {
        return premultiply(v.color);
    }
    let p = v.position.xy + globals.viewport.xy;
    let flags = u32(v.info.y);
    var color: vec4<f32>;
    switch kind {
        case BOX: {
            color = box_color(p, v.rect, v.rx, v.ry, v.color, v.border_color, v.aux, flags);
        }
        case SHADOW: {
            var shape = 0.0;
            if v.info.z < 0.25 {
                shape = saturate(0.5 - box_distance(p, v.rect, v.rx, v.ry));
            } else {
                shape = blurred_box(p, v.rect, v.rx, v.ry, v.info.z);
            }
            let caster = saturate(0.5 - box_distance(p, v.aux, v.aux_rx, v.aux_ry));
            color = premultiply(v.color) * select(shape * (1.0 - caster), (1.0 - shape) * caster, (flags & 1u) != 0u);
        }
        case LINE: {
            color = premultiply(v.color);
            if (flags & 1u) != 0u {
                color *= wave_coverage(p, v.rect, v.info.z);
            }
        }
        case GLYPH: {
            var coverage = textureSampleLevel(t0, filtered, v.texel / vec2<f32>(textureDimensions(t0)), 0.0).r;
            let ink = dot(v.color.rgb, REC709) * 2.0 - 1.0;
            coverage = saturate(coverage + coverage * (1.0 - coverage) * ink * globals.text_contrast);
            color = premultiply(v.color) * coverage;
        }
        default: {
            var pixel = textureSampleLevel(t1, filtered, v.texel / vec2<f32>(textureDimensions(t1)), 0.0);
            if (flags & 2u) != 0u && pixel.a > 0.0 {
                pixel = vec4<f32>(pixel.rgb / pixel.a, pixel.a);
            }
            if (flags & 1u) != 0u {
                pixel = vec4<f32>(vec3<f32>(dot(pixel.rgb, REC709)), pixel.a);
            }
            var alpha = pixel.a * v.info.z;
            if any(v.rx != vec4<f32>(0.0)) {
                alpha *= saturate(0.5 - box_distance(p, v.rect, v.rx, v.ry));
            }
            color = vec4<f32>(pixel.rgb * alpha, alpha);
        }
    }
    return color * rounded_clip(p, v.round, v.round_rx, v.round_ry);
}

struct Placed {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
}

@vertex
fn composite_vertex(@builtin(vertex_index) index: u32) -> Placed {
    let local = composite.bounds.xy + strip_corner(index) * composite.bounds.zw;
    let q = composite.transform * vec4<f32>(local, 0.0, 1.0);
    let at = q.xy - globals.viewport.xy * q.w;
    var out: Placed;
    out.position = vec4<f32>(at.x / globals.viewport.z * 2.0 - q.w, q.w - at.y / globals.viewport.w * 2.0, 0.0, q.w);
    out.local = local;
    return out;
}

// A pixel of a layer whose texture covers `rect` in window pixels, transparent outside it.
fn layer_pixel(source: texture_2d<f32>, at: vec2<f32>, rect: vec4<f32>) -> vec4<f32> {
    let uv = (at - rect.xy) / rect.zw;
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(source, filtered, uv, 0.0);
}

// A Gaussian along `direction` with at most 32 taps a side. `texel` is a source pixel in window pixels.
fn gaussian(source: texture_2d<f32>, at: vec2<f32>, direction: vec2<f32>, sigma: f32, texel: f32, rect: vec4<f32>) -> vec4<f32> {
    if sigma < 0.25 {
        return layer_pixel(source, at, rect);
    }
    let reach = ceil(3.0 * sigma / texel);
    let taps = min(reach, 32.0);
    let step = reach / taps * texel;
    var sum = layer_pixel(source, at, rect);
    var total = 1.0;
    for (var tap = 1.0; tap <= taps; tap += 1.0) {
        let offset = tap * step;
        let weight = exp(-0.5 * offset * offset / (sigma * sigma));
        sum += (layer_pixel(source, at + direction * offset, rect) + layer_pixel(source, at - direction * offset, rect)) * weight;
        total += 2.0 * weight;
    }
    return sum / total;
}

fn luminosity(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.3, 0.59, 0.11));
}

fn with_luminosity(c: vec3<f32>, l: f32) -> vec3<f32> {
    var out = c + (l - luminosity(c));
    let center = luminosity(out);
    let low = min(min(out.r, out.g), out.b);
    let high = max(max(out.r, out.g), out.b);
    if low < 0.0 {
        out = center + (out - center) * center / (center - low);
    }
    if high > 1.0 {
        out = center + (out - center) * (1.0 - center) / (high - center);
    }
    return out;
}

fn chroma(c: vec3<f32>) -> f32 {
    return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
}

fn with_chroma(c: vec3<f32>, s: f32) -> vec3<f32> {
    let low = min(min(c.r, c.g), c.b);
    let span = max(max(c.r, c.g), c.b) - low;
    if span <= 0.0 {
        return vec3<f32>(0.0);
    }
    return (c - low) * s / span;
}

fn hard_light(b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let doubled = 2.0 * s;
    return select(b + doubled - 1.0 - b * (doubled - 1.0), b * doubled, s <= vec3<f32>(0.5));
}

fn soft_light(b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let d = select(sqrt(b), ((16.0 * b - 12.0) * b + 4.0) * b, b <= vec3<f32>(0.25));
    return select(b + (2.0 * s - 1.0) * (d - b), b - (1.0 - 2.0 * s) * b * (1.0 - b), s <= vec3<f32>(0.5));
}

fn blend_colors(b: vec3<f32>, s: vec3<f32>, mode: u32) -> vec3<f32> {
    let one = vec3<f32>(1.0);
    let zero = vec3<f32>(0.0);
    switch mode {
        case 1u: { return b * s; }
        case 2u: { return b + s - b * s; }
        case 3u: { return hard_light(s, b); }
        case 4u: { return min(b, s); }
        case 5u: { return max(b, s); }
        case 6u: { return select(select(min(one, b / max(one - s, vec3<f32>(1e-6))), one, s >= one), zero, b <= zero); }
        case 7u: { return select(select(one - min(one, (one - b) / max(s, vec3<f32>(1e-6))), zero, s <= zero), one, b >= one); }
        case 8u: { return hard_light(b, s); }
        case 9u: { return soft_light(b, s); }
        case 10u: { return abs(b - s); }
        case 11u: { return b + s - 2.0 * b * s; }
        case 12u: { return with_luminosity(with_chroma(s, chroma(b)), luminosity(b)); }
        case 13u: { return with_luminosity(with_chroma(b, chroma(s)), luminosity(b)); }
        case 14u: { return with_luminosity(s, luminosity(b)); }
        case 15u: { return with_luminosity(b, luminosity(s)); }
        default: { return s; }
    }
}

@fragment
fn composite_fragment(v: Placed) -> @location(0) vec4<f32> {
    let c = composite;
    let p = v.position.xy + globals.viewport.xy;
    if any(p < c.content_mask.xy) || any(p > c.content_mask.xy + c.content_mask.zw) {
        return vec4<f32>(0.0);
    }
    var color: vec4<f32>;
    if (c.flags & COMPOSITE_BACKDROP) != 0u {
        color = gaussian(t0, p, vec2<f32>(0.0, 1.0), c.blur, 1.0, c.source_rect);
    } else if (c.flags & COMPOSITE_SHADOW) != 0u {
        color = c.shadow_color * gaussian(t0, v.local - c.shadow.xy, vec2<f32>(0.0, 1.0), c.blur, 1.0, c.source_rect).a;
    } else {
        color = gaussian(t0, v.local, vec2<f32>(0.0, 1.0), c.blur, 1.0, c.source_rect);
    }
    if (c.flags & COMPOSITE_MATRIX) != 0u && color.a > 0.0 {
        let mapped = saturate(c.color_matrix * vec4<f32>(color.rgb / color.a, color.a) + c.color_offset);
        color = premultiply(mapped);
    }
    var coverage = c.opacity * rounded_clip(p, c.rounded_mask, c.rounded_rx, c.rounded_ry);
    if (c.flags & COMPOSITE_CLIP) != 0u {
        coverage *= saturate(0.5 - box_distance(v.local, c.clip, c.clip_rx, c.clip_ry));
    }
    if (c.flags & COMPOSITE_MASK) != 0u {
        let at = (v.local - c.mask_rect.xy) / c.mask_rect.zw;
        if any(at < vec2<f32>(0.0)) || any(at > vec2<f32>(1.0)) {
            coverage = 0.0;
        } else {
            let uv = (c.mask_tile.xy + at * c.mask_tile.zw) / vec2<f32>(textureDimensions(t1));
            coverage *= textureSampleLevel(t1, filtered, uv, 0.0).a;
        }
    }
    color *= coverage;
    if (c.flags & COMPOSITE_BLEND) != 0u && color.a > 0.0 {
        let below = textureSampleLevel(t2, filtered, (p - c.backdrop_rect.xy) / c.backdrop_rect.zw, 0.0);
        if c.blend == 16u {
            return vec4<f32>(min(color.rgb + below.rgb, vec3<f32>(1.0)) - below.rgb * (1.0 - color.a), color.a);
        }
        let b = select(vec3<f32>(0.0), below.rgb / below.a, below.a > 0.0);
        let s = color.rgb / color.a;
        let mixed = (1.0 - below.a) * s + below.a * saturate(blend_colors(b, s, c.blend));
        color = vec4<f32>(mixed * color.a, color.a);
    }
    return color;
}

@vertex
fn blur_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return to_clip_space(blur.region.xy + strip_corner(index) * blur.region.zw);
}

@fragment
fn blur_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let at = globals.viewport.xy + position.xy * blur.scale;
    return gaussian(t0, at, blur.direction, blur.sigma, blur.scale, blur.source_rect);
}

@vertex
fn copy_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(strip_corner(index) * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn copy_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(t0, vec2<i32>(position.xy) + copy.offset.xy, 0);
}
