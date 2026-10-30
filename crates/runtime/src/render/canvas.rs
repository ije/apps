use anyhow::{Context, Result, ensure};
use resvg::{tiny_skia as skia, usvg};
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};

pub struct Canvas {
    pub pixels: skia::Pixmap,
    pub image: Arc<renderer::RenderImage>,
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn fonts() -> &'static Mutex<Arc<usvg::fontdb::Database>> {
    static FONTS: OnceLock<Mutex<Arc<usvg::fontdb::Database>>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        Mutex::new(Arc::new(fonts))
    })
}

fn render_image(pixels: &skia::Pixmap) -> Arc<renderer::RenderImage> {
    let mut bytes = vec![0; pixels.data().len()];
    for (out, pixel) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(pixels.pixels()) {
        if pixel.alpha() != 0 {
            let pixel = pixel.demultiply();
            *out = [pixel.blue(), pixel.green(), pixel.red(), pixel.alpha()];
        }
    }
    Arc::new(renderer::RenderImage::new(
        pixels.width(),
        pixels.height(),
        bytes,
    ))
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        ensure!(
            width > 0
                && height > 0
                && width <= 16384
                && height <= 16384
                && u64::from(width) * u64::from(height) <= 16_777_216,
            "Canvas dimensions exceed the pixel limit"
        );
        let pixels = skia::Pixmap::new(width, height).context("Cannot allocate canvas")?;
        let image = Arc::new(renderer::RenderImage::new(
            width,
            height,
            vec![0; (width * height * 4) as usize],
        ));
        Ok(Self { pixels, image })
    }
    fn render_svg(&mut self, body: &mut String) -> Result<()> {
        if body.is_empty() {
            return Ok(());
        }
        let fonts = fonts().lock().unwrap().clone();
        let options = usvg::Options {
            fontdb: fonts,
            ..Default::default()
        };
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\">{}</svg>",
            self.pixels.width(),
            self.pixels.height(),
            body
        );
        let tree = usvg::Tree::from_str(&svg, &options)?;
        resvg::render(
            &tree,
            skia::Transform::identity(),
            &mut self.pixels.as_mut(),
        );
        body.clear();
        Ok(())
    }
    pub fn draw(&mut self, commands: &[Value]) -> Result<()> {
        let mut body = String::new();
        for (index, command) in commands.iter().enumerate() {
            let op = command["op"].as_str().unwrap_or("");
            let num = |name: &str| command[name].as_f64().unwrap_or(0.) as f32;
            let matrix = &command["transform"];
            let transform = skia::Transform::from_row(
                matrix[0].as_f64().unwrap_or(1.) as f32,
                matrix[1].as_f64().unwrap_or(0.) as f32,
                matrix[2].as_f64().unwrap_or(0.) as f32,
                matrix[3].as_f64().unwrap_or(1.) as f32,
                matrix[4].as_f64().unwrap_or(0.) as f32,
                matrix[5].as_f64().unwrap_or(0.) as f32,
            );
            if op == "clearRect" {
                self.render_svg(&mut body)?;
                if let Some(rect) =
                    skia::Rect::from_xywh(num("x"), num("y"), num("width"), num("height"))
                {
                    let paint = skia::Paint {
                        blend_mode: skia::BlendMode::Clear,
                        ..Default::default()
                    };
                    let mut mask = None;
                    if let Some(clips) = command["clip"].as_array() {
                        for clip in clips {
                            let mut builder = skia::PathBuilder::new();
                            for part in clip["path"].as_array().context("Expected clip path")? {
                                let n = |index: usize| part[index].as_f64().unwrap_or(0.) as f32;
                                match part[0].as_str().unwrap_or("") {
                                    "move" => builder.move_to(n(1), n(2)),
                                    "line" => builder.line_to(n(1), n(2)),
                                    "quadratic" => builder.quad_to(n(1), n(2), n(3), n(4)),
                                    "bezier" => {
                                        builder.cubic_to(n(1), n(2), n(3), n(4), n(5), n(6))
                                    }
                                    "close" => builder.close(),
                                    _ => {}
                                }
                            }
                            if let Some(path) = builder.finish() {
                                let t = &clip["transform"];
                                let n = |i: usize| {
                                    t[i].as_f64()
                                        .unwrap_or(if i == 0 || i == 3 { 1. } else { 0. })
                                        as f32
                                };
                                let transform =
                                    skia::Transform::from_row(n(0), n(1), n(2), n(3), n(4), n(5));
                                let rule = if clip["rule"] == "evenodd" {
                                    skia::FillRule::EvenOdd
                                } else {
                                    skia::FillRule::Winding
                                };
                                if let Some(mask) = &mut mask {
                                    skia::Mask::intersect_path(mask, &path, rule, true, transform);
                                } else {
                                    let mut next =
                                        skia::Mask::new(self.pixels.width(), self.pixels.height())
                                            .context("Cannot allocate clip mask")?;
                                    next.fill_path(&path, rule, true, transform);
                                    mask = Some(next);
                                }
                            }
                        }
                    }
                    self.pixels
                        .fill_rect(rect, &paint, transform, mask.as_ref());
                }
                continue;
            }
            if op == "putImageData" {
                self.render_svg(&mut body)?;
                let width = command["width"].as_u64().context("Expected image width")? as u32;
                let height = command["height"]
                    .as_u64()
                    .context("Expected image height")? as u32;
                let bytes = crate::web::bytes(&command["data"])?;
                ensure!(
                    (width as usize)
                        .checked_mul(height as usize)
                        .and_then(|n| n.checked_mul(4))
                        == Some(bytes.len()),
                    "Invalid image data length"
                );
                let stride = self.pixels.width() as usize;
                for y in 0..height {
                    for x in 0..width {
                        let dx = num("x") as i64 + i64::from(x);
                        let dy = num("y") as i64 + i64::from(y);
                        if dx < 0
                            || dy < 0
                            || dx >= i64::from(self.pixels.width())
                            || dy >= i64::from(self.pixels.height())
                        {
                            continue;
                        }
                        let at = (y as usize * width as usize + x as usize) * 4;
                        self.pixels.pixels_mut()[dy as usize * stride + dx as usize] =
                            skia::ColorU8::from_rgba(
                                bytes[at],
                                bytes[at + 1],
                                bytes[at + 2],
                                bytes[at + 3],
                            )
                            .premultiply();
                    }
                }
                continue;
            }
            let stroke = op.starts_with("stroke") || op == "stroke";
            let color = escape(
                command[if stroke { "strokeStyle" } else { "fillStyle" }]
                    .as_str()
                    .unwrap_or("black"),
            );
            let alpha = command["globalAlpha"].as_f64().unwrap_or(1.).clamp(0., 1.);
            let paint = if stroke {
                format!(
                    "fill=\"none\" stroke=\"{color}\" stroke-width=\"{}\"",
                    command["lineWidth"].as_f64().unwrap_or(1.)
                )
            } else {
                format!("fill=\"{color}\"")
            };
            let element = match op {
                "fillRect" | "strokeRect" => format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" {paint}/>",
                    num("x"),
                    num("y"),
                    num("width"),
                    num("height")
                ),
                "fillText" => {
                    let font = command["font"].as_str().unwrap_or("10px sans-serif");
                    let (size, family) = font.split_once("px").unwrap_or(("10", "sans-serif"));
                    let size = size
                        .split_whitespace()
                        .last()
                        .and_then(|v| v.parse::<f32>().ok())
                        .unwrap_or(10.);
                    let baseline = match command["textBaseline"].as_str().unwrap_or("alphabetic") {
                        "top" | "hanging" => "hanging",
                        "middle" => "central",
                        "bottom" | "ideographic" => "text-after-edge",
                        _ => "alphabetic",
                    };
                    let anchor = match command["textAlign"].as_str().unwrap_or("start") {
                        "center" => "middle",
                        "end" | "right" => "end",
                        _ => "start",
                    };
                    format!(
                        "<text x=\"{}\" y=\"{}\" font-size=\"{size}\" font-family=\"{}\" font-weight=\"{}\" font-style=\"{}\" dominant-baseline=\"{baseline}\" text-anchor=\"{anchor}\" xml:space=\"preserve\" {paint}>{}</text>",
                        num("x"),
                        num("y"),
                        escape(family.trim().trim_matches(['\'', '"'])),
                        if font.contains("bold") { 700 } else { 400 },
                        if font.contains("italic") {
                            "italic"
                        } else {
                            "normal"
                        },
                        escape(command["text"].as_str().unwrap_or(""))
                    )
                }
                "fill" | "stroke" => {
                    let mut d = String::new();
                    for part in command["path"].as_array().context("Expected canvas path")? {
                        let kind = match part[0].as_str().unwrap_or("") {
                            "move" => "M",
                            "line" => "L",
                            "quadratic" => "Q",
                            "bezier" => "C",
                            "close" => "Z",
                            _ => continue,
                        };
                        d.push_str(kind);
                        for value in part.as_array().unwrap().iter().skip(1) {
                            d.push_str(&value.as_f64().unwrap_or(0.).to_string());
                            d.push(' ');
                        }
                    }
                    format!("<path d=\"{d}\" {paint}/>")
                }
                "image" => {
                    let data = command["url"].as_str().context("Expected image data URL")?;
                    format!(
                        "<svg x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\" preserveAspectRatio=\"none\"><image width=\"{}\" height=\"{}\" href=\"{}\"/></svg>",
                        num("dx"),
                        num("dy"),
                        num("dw"),
                        num("dh"),
                        num("sx"),
                        num("sy"),
                        num("sw"),
                        num("sh"),
                        num("iw"),
                        num("ih"),
                        escape(data)
                    )
                }
                _ => anyhow::bail!("Unsupported canvas operation: {op}"),
            };
            let mut element = format!(
                "<g opacity=\"{alpha}\" transform=\"matrix({} {} {} {} {} {})\">{element}</g>",
                transform.sx, transform.ky, transform.kx, transform.sy, transform.tx, transform.ty
            );
            if let Some(clips) = command["clip"].as_array() {
                for (clip_index, clip) in clips.iter().enumerate() {
                    let mut d = String::new();
                    for part in clip["path"].as_array().context("Expected clip path")? {
                        let kind = match part[0].as_str().unwrap_or("") {
                            "move" => "M",
                            "line" => "L",
                            "quadratic" => "Q",
                            "bezier" => "C",
                            "close" => "Z",
                            _ => continue,
                        };
                        d.push_str(kind);
                        for value in part.as_array().unwrap().iter().skip(1) {
                            d.push_str(&value.as_f64().unwrap_or(0.).to_string());
                            d.push(' ');
                        }
                    }
                    let t = &clip["transform"];
                    let rule = clip["rule"].as_str().unwrap_or("nonzero");
                    element = format!(
                        "<defs><clipPath id=\"clip{index}_{clip_index}\"><path d=\"{d}\" clip-rule=\"{rule}\" transform=\"matrix({} {} {} {} {} {})\"/></clipPath></defs><g clip-path=\"url(#clip{index}_{clip_index})\">{element}</g>",
                        t[0], t[1], t[2], t[3], t[4], t[5]
                    );
                }
            }
            body.push_str(&element);
        }
        self.render_svg(&mut body)?;
        self.image = render_image(&self.pixels);
        Ok(())
    }
}

pub fn svg(
    source: &str,
    width: u32,
    height: u32,
    transform: Option<([f32; 2], [f32; 6])>,
) -> Result<Arc<renderer::RenderImage>> {
    let tree = usvg::Tree::from_str(
        source,
        &usvg::Options {
            fontdb: fonts().lock().unwrap().clone(),
            ..Default::default()
        },
    )?;
    let mut pixels = skia::Pixmap::new(width, height).context("Cannot allocate SVG image")?;
    let size = tree.size();
    let ([fit_width, fit_height], [a, b, c, d, e, f]) =
        transform.unwrap_or(([width as f32, height as f32], [1., 0., 0., 1., 0., 0.]));
    resvg::render(
        &tree,
        skia::Transform::from_row(a, b, c, d, e, f)
            .pre_scale(fit_width / size.width(), fit_height / size.height()),
        &mut pixels.as_mut(),
    );
    Ok(render_image(&pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_keep_straight_alpha_at_display_resolution() {
        let mut canvas = Canvas::new(2, 2).unwrap();
        canvas.draw(&[serde_json::json!({"op":"fillRect","x":0,"y":0,"width":2,"height":2,"fillStyle":"#ff0000","globalAlpha":0.5})]).unwrap();
        assert_eq!(&canvas.image.bytes()[..4], &[0, 0, 255, 128]);
        assert_eq!(canvas.pixels.pixels()[0].red(), 128);
        let image = svg(r##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><path d="M0 0h2v2H0Z" fill="#ff0000" opacity="0.5"/></svg>"##, 4, 4, None).unwrap();
        assert_eq!(u32::from(image.size().width), 4);
        assert_eq!(image.bytes(), [0, 0, 255, 128].repeat(16));
        let rotated = svg(r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><path d="M0 0h2v2H0Z" fill="#ff0000"/><path d="M2 0h2v2H2Z" fill="#0000ff"/></svg>"##, 1, 2, Some(([2., 1.], [0., 1., -1., 0., 1., 0.]))).unwrap();
        assert_eq!(
            rotated.bytes(),
            [[0, 0, 255, 255], [255, 0, 0, 255]].concat()
        );
    }
}
