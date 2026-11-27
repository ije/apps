use crate::dom::{Document, Node};
use crate::render::PageState;
use crate::render::media::Placeholder;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
  collections::HashMap,
  rc::Rc,
  sync::{Arc, LazyLock, Mutex, PoisonError},
};

static PLACEHOLDERS: LazyLock<Mutex<HashMap<u64, Placeholder>>> = LazyLock::new(Default::default);

fn new_canvas(node: &Node) -> Result<crate::render::raster::canvas::Canvas> {
  let size = |name, default| {
    node
      .attrs
      .get(name)
      .and_then(|value| crate::render::layout::size::dimension(value, true))
      .map_or(default, |value| value as u32)
  };
  let mut canvas =
    crate::render::raster::canvas::Canvas::new(size("width", 300), size("height", 150))?;
  canvas.present = node.tag == "canvas";
  Ok(canvas)
}

// Added to the node of a drawable element, this draws its snapshot and not the image of the node.
const SNAPSHOT: usize = 1 << 32;

pub(super) fn decoded(state: &mut PageState) -> Result<Value> {
  let decoded = std::mem::take(
    &mut *state
      .media
      .decoded
      .lock()
      .unwrap_or_else(PoisonError::into_inner),
  );
  let mut results = Vec::with_capacity(decoded.len());
  for (id, request, result) in decoded {
    if state.media.decoding.get(&id) != Some(&request) {
      results.push(json!([request, null]));
      continue;
    }
    state.media.decoding.remove(&id);
    state.dirty |= result.is_ok();
    if result.is_ok() {
      state.layout.relayout(id);
    }
    results.push(json!([
      request,
      match result {
        Ok(Ok(image)) => {
          let size = image.size();
          state
            .layout
            .set_natural(id, [size.width as u32 as f32, size.height as u32 as f32]);
          state.media.svg_images.remove(&id);
          state
            .media
            .retired
            .extend(state.media.images.insert(id, Arc::new(image)));
          json!([size.width as u32, size.height as u32])
        }
        Ok(Err((text, size))) => {
          state.layout.set_natural(id, size);
          state.media.retired.extend(state.media.images.remove(&id));
          state.media.svg_images.insert(id, (text.into(), size));
          json!(size)
        }
        Err(error) => json!(error.to_string()),
      }
    ]));
  }
  Ok(json!(results))
}

pub(super) fn frames(state: &mut PageState) -> Result<Value> {
  let frames = std::mem::take(
    &mut *state
      .media
      .frames
      .lock()
      .unwrap_or_else(PoisonError::into_inner),
  );
  let mut results = Vec::with_capacity(frames.len());
  for (node, image) in frames {
    let size = image.size();
    results.push(json!([node, size.width as u32, size.height as u32]));
    let old = state.media.images.insert(node, image);
    state.media.retire(node, old);
    state.dirty = true;
  }
  Ok(json!(results))
}

pub(super) fn canvas(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  if let Some(ops) = crate::jsc::payload::slice(&args[1]) {
    let ops: Vec<f64> = ops?
      .as_chunks::<8>()
      .0
      .iter()
      .map(|bytes| f64::from_ne_bytes(*bytes))
      .collect();
    let data = args
      .as_array()
      .into_iter()
      .flatten()
      .skip(3)
      .map(|value| crate::jsc::payload::slice(value).context("Expected image data")?)
      .collect::<Result<Vec<_>>>()?;
    let mut canvas = match state.media.canvases.remove(&id) {
      Some(canvas) => canvas,
      None => {
        let canvas = new_canvas(&doc.nodes[id])?;
        state.tree.note_created(canvas.pixels.data().len() >> 16);
        canvas
      }
    };
    let result = canvas.draw(
      &ops,
      args[2].as_str().unwrap_or(""),
      &data,
      &|(source, size)| match source.checked_sub(SNAPSHOT) {
        Some(element) => state
          .media
          .snapshots
          .get(&element)
          .map(|(pixels, _)| crate::render::raster::canvas::Source::Shared(pixels.clone())),
        None => state.media.source_pixels(source, Some(size)),
      },
      state
        .fonts
        .system
        .as_deref()
        .map(|system| (system, &state.fonts.aliases)),
    );
    state.media.canvases.insert(id, canvas);
    state.media.retire(id, result?);
  } else {
    state.media.retired.extend(
      state
        .media
        .canvases
        .remove(&id)
        .filter(|canvas| canvas.present)
        .map(|canvas| canvas.image),
    );
    state.media.retired.extend(state.media.images.remove(&id));
    state.media.sources.get_mut().remove(&id);
  }
  // Offscreen drawing changes nothing on the page, so it must not cost page frames.
  state.dirty |= doc.nodes[id].tag == "canvas";
  Ok(Value::Null)
}

pub(super) fn canvas_url(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  use base64::Engine as _;
  let base64 = base64::engine::general_purpose::STANDARD;
  let png = match state.media.source_pixels(id, None) {
    Some(pixels) => crate::render::raster::image::encode_png(&pixels)?,
    None => crate::render::raster::image::encode_png(&new_canvas(&doc.nodes[id])?.pixels)?,
  };
  let (bytes, mime) = match args[2].as_str() {
    #[cfg(target_os = "macos")]
    Some("image/jpeg") => {
      let quality = args[3].as_f64().filter(|q| (0. ..=1.).contains(q));
      let jpeg = crate::native::native_call(
        "image",
        &json!({"op": "jpeg", "data": base64.encode(&png), "quality": quality.unwrap_or(0.92) * 100.}),
      )?;
      (
        base64.decode(jpeg["png"].as_str().unwrap_or_default())?,
        "image/jpeg",
      )
    }
    _ => (png, "image/png"),
  };
  Ok(if args[1] == true {
    json!([crate::jsc::payload::write(bytes), mime])
  } else {
    json!(format!("data:{mime};base64,{}", base64.encode(bytes)))
  })
}

pub(super) fn canvas_pixels(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  let pixels = match state.media.source_pixels(id, None) {
    Some(pixels) => pixels,
    None => {
      crate::render::raster::canvas::Source::Shared(Rc::new(new_canvas(&doc.nodes[id])?.pixels))
    }
  };
  let x = args[1].as_i64().unwrap_or(0);
  let y = args[2].as_i64().unwrap_or(0);
  let width = args[3].as_u64().context("Expected width")? as usize;
  let height = args[4].as_u64().context("Expected height")? as usize;
  ensure!(
    width > 0 && height > 0 && width <= 16384 && height <= 16384 && width * height <= 16_777_216,
    "Invalid pixel region"
  );
  let mut bytes = vec![0u8; width * height * 4];
  let (w, h) = (i64::from(pixels.width()), i64::from(pixels.height()));
  let [left, right] = [x, x.saturating_add(width as i64)].map(|edge| edge.clamp(0, w));
  for sy in (y.max(0)..y.saturating_add(height as i64).min(h)).filter(|_| left < right) {
    let from = (sy * w + left) as usize..(sy * w + right) as usize;
    let at = (((sy - y) * width as i64 + left - x) * 4) as usize;
    let out = &mut bytes[at..at + from.len() * 4];
    if args[5] == true {
      out.copy_from_slice(&pixels.data()[from.start * 4..from.end * 4]);
      continue;
    }
    for (out, pixel) in out
      .as_chunks_mut::<4>()
      .0
      .iter_mut()
      .zip(&pixels.pixels()[from])
    {
      let color = pixel.demultiply();
      *out = [color.red(), color.green(), color.blue(), color.alpha()];
    }
  }
  Ok(crate::jsc::payload::write(bytes))
}

pub(super) fn canvas_take(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  let to = args[1].as_u64().context("Expected node id")? as usize;
  ensure!(to < doc.nodes.len(), "Invalid node id");
  let mut taken = match state.media.canvases.get_mut(&id) {
    Some(canvas) => {
      let mut taken =
        crate::render::raster::canvas::Canvas::new(canvas.pixels.width(), canvas.pixels.height())?;
      std::mem::swap(&mut canvas.pixels, &mut taken.pixels);
      taken
    }
    None => {
      let mut taken = new_canvas(&doc.nodes[id])?;
      if let Some(pixels) = state.media.source_pixels(id, None) {
        taken.pixels = (*pixels).clone();
      }
      taken
    }
  };
  taken.present = doc.nodes[to].tag == "canvas";
  if taken.present {
    taken.image = crate::render::raster::render_image(&taken.pixels, Vec::new());
  }
  state.tree.note_created(taken.pixels.data().len() >> 16);
  state.dirty |= taken.present;
  let old = state.media.canvases.insert(to, taken);
  state
    .media
    .retire(to, old.filter(|old| old.present).map(|old| old.image));
  Ok(Value::Null)
}

pub(super) fn canvas_adopt(
  id: usize,
  args: &Value,
  doc: &mut Document,
  state: &mut PageState,
) -> Result<Value> {
  let mut canvas = new_canvas(&doc.nodes[id])?;
  canvas.pixels = tiny_skia::Pixmap::from_vec(
    crate::web::bytes(&args[1])?,
    tiny_skia::IntSize::from_wh(canvas.pixels.width(), canvas.pixels.height())
      .context("Invalid canvas size")?,
  )
  .context("Invalid canvas pixels")?;
  state.tree.note_created(canvas.pixels.data().len() >> 16);
  state.media.canvases.insert(id, canvas);
  Ok(Value::Null)
}

// The size of the last drawing of a drawable element in CSS pixels. With a node, that canvas gets a copy.
pub(super) fn snapshot(id: usize, args: &Value, state: &mut PageState) -> Result<Value> {
  let Some((pixels, scale)) = state.media.snapshots.get(&id).cloned() else {
    return Ok(Value::Null);
  };
  if let Some(to) = args[1].as_u64() {
    let mut canvas = crate::render::raster::canvas::Canvas::new(pixels.width(), pixels.height())?;
    canvas.pixels = (*pixels).clone();
    state.tree.note_created(canvas.pixels.data().len() >> 16);
    state.media.canvases.insert(to as usize, canvas);
  }
  Ok(json!([
    pixels.width() as f32 / scale,
    pixels.height() as f32 / scale,
    pixels.width(),
    pixels.height()
  ]))
}

// Where a canvas drew a drawable element: `get`, `clear`, or `set` with a matrix and a clip, each of
// which stays as it is when null.
pub(super) fn drawn(id: usize, args: &Value, state: &mut PageState) -> Result<Value> {
  let floats = |value: &Value| {
    let list = value.as_array()?.iter().map(|v| Some(v.as_f64()? as f32));
    list.collect::<Option<Vec<f32>>>()
  };
  match args[1].as_str() {
    Some("get") => Ok(json!(
      state.geometry.drawn.get(&id).map(|drawn| drawn.matrix)
    )),
    Some("clear") => {
      state.dirty |= state.geometry.drawn.remove(&id).is_some();
      Ok(Value::Null)
    }
    _ => {
      let matrix = floats(&args[2]).and_then(|list| <[f32; 16]>::try_from(list).ok());
      let clip = floats(&args[3]).and_then(|list| <[f32; 4]>::try_from(list).ok());
      // The boxes take the pointer at their new place after the next frame.
      state.dirty |= state.geometry.draw(id, matrix, clip, args[4] == true);
      Ok(Value::Null)
    }
  }
}

pub(super) fn canvas_link(id: usize, state: &mut PageState) -> Result<Value> {
  static NEXT: AtomicU64 = AtomicU64::new(0);
  let token = NEXT.fetch_add(1, Ordering::Relaxed);
  let placeholder = state
    .media
    .placeholders
    .remove(&id)
    .unwrap_or_else(|| Placeholder {
      frames: state.media.frames.clone(),
      node: id,
      wake: crate::jsc::Waker::current(),
    });
  PLACEHOLDERS
    .lock()
    .unwrap_or_else(PoisonError::into_inner)
    .insert(token, placeholder);
  Ok(json!(token))
}

pub(super) fn canvas_bind(id: usize, args: &Value, state: &mut PageState) -> Result<Value> {
  let token = args[1].as_u64().context("Expected a token")?;
  let placeholder = PLACEHOLDERS
    .lock()
    .unwrap_or_else(PoisonError::into_inner)
    .remove(&token)
    .context("The canvas was already transferred")?;
  state.media.placeholders.insert(id, placeholder);
  Ok(Value::Null)
}

pub(super) fn image(id: usize, args: &Value, state: &mut PageState) -> Result<Value> {
  let bytes = crate::web::bytes(&args[1])?;
  Ok(state.media.load_image(id, move || Ok(bytes)))
}

pub(super) fn canvas_font(args: &Value) -> Result<Value> {
  Ok(json!(crate::render::raster::canvas::text::font(
    args[1].as_str().unwrap_or("")
  )))
}

// As a canvas serializes a color: `#rrggbb`, or `rgba()` with the shortest alpha that rounds back.
pub(super) fn canvas_color(args: &Value) -> Result<Value> {
  Ok(
    match crate::css::value::color::color(args[1].as_str().unwrap_or("")) {
      Some((rgb, 255)) => json!(format!("#{:06x}", rgb & 0xff_ffff)),
      Some((rgb, alpha)) => {
        let [_, r, g, b] = rgb.to_be_bytes();
        let value = (1..=3)
          .map(|digits| format!("{:.*}", digits, f64::from(alpha) / 255.))
          .find(|text| {
            text
              .parse::<f64>()
              .is_ok_and(|v| (v * 255.).round() as u8 == alpha)
          })
          .unwrap_or_else(|| format!("{:.3}", f64::from(alpha) / 255.));
        let value = value.trim_end_matches('0').trim_end_matches('.');
        json!(format!("rgba({r}, {g}, {b}, {value})"))
      }
      None => Value::Null,
    },
  )
}

pub(super) fn svg_box(id: usize, doc: &mut Document, state: &mut PageState) -> Result<Value> {
  let boxes = std::iter::successors(Some(id), |&node| doc.nodes[node].parent)
    .find_map(|node| state.rasters.svg_boxes(node).map(|boxes| (node, boxes)));
  Ok(json!(
    boxes
      .and_then(|(root, (_, _, _, boxes, content, ..))| if root == id {
        Some(*content)
      } else {
        boxes
          .iter()
          .find(|(node, ..)| *node == id)
          .map(|(.., object)| *object)
      })
      .unwrap_or_default()
  ))
}

pub(super) fn measure_text(args: &Value, state: &mut PageState) -> Result<Value> {
  let system = state
    .fonts
    .system
    .as_ref()
    .context("Text measurement requires a displayed window")?;
  let (line, [ascent, descent, em_ascent, em_descent]) = crate::render::raster::canvas::text::line(
    system,
    &state.fonts.aliases,
    args[2].as_str().unwrap_or("10px sans-serif"),
    args[1].as_str().context("Expected text")?,
    [
      args[3].as_f64().unwrap_or(0.) as f32,
      args[4].as_f64().unwrap_or(0.) as f32,
    ],
  );
  let [left, right, top, bottom] = crate::render::raster::canvas::text::ink(system, &line);
  Ok(json!([
    f32::from(line.width),
    left,
    right,
    top,
    bottom,
    ascent,
    descent,
    em_ascent,
    em_descent
  ]))
}

pub(super) fn canvas_query(id: usize, args: &Value, state: &mut PageState) -> Result<Value> {
  let Some(canvas) = state.media.canvases.get(&id) else {
    return Ok(json!(if args[1].is_number() {
      json!(false)
    } else {
      json!([1, 0, 0, 1, 0, 0])
    }));
  };
  Ok(match args[1].as_f64() {
    Some(x) => json!(canvas.hit(
      x as f32,
      args[2].as_f64().unwrap_or(0.) as f32,
      args[3].as_bool().unwrap_or(false),
      args[4].as_bool().unwrap_or(false),
      args[5].as_u64(),
    )),
    None => json!(canvas.transform()),
  })
}
