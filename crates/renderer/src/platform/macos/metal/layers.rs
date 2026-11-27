use super::*;

impl Renderer {
  pub fn promote(
    &mut self,
    key: u64,
    index: usize,
    region: [i32; 4],
    [step, repeat, since]: [f64; 3],
    opacities: &[f32],
    transforms: &[[f64; 16]],
  ) {
    CATransaction::setDisableActions(true);
    let scale = self.layer.contentsScale();
    let [x, y, w, h] = region.map(|value| value as f64 / scale);
    if let Some(at) = self
      .promoted
      .iter()
      .position(|promoted| promoted.key == key && promoted.index != index)
    {
      self.promoted.remove(at).sublayer.removeFromSuperlayer();
    }
    let at = match self
      .promoted
      .iter()
      .position(|promoted| promoted.key == key)
    {
      Some(at) => at,
      None => {
        let sublayer = metal_layer(&self.device, false, self.wide);
        sublayer.setPresentsWithTransaction(true);
        sublayer.setContentsScale(scale);
        sublayer.setDrawableSize(CGSize::new(region[2] as f64, region[3] as f64));
        sublayer.setAnchorPoint(objc2_core_foundation::CGPoint::new(0., 0.));
        sublayer.setBounds(objc2_core_foundation::CGRect::new(
          objc2_core_foundation::CGPoint::new(0., 0.),
          CGSize::new(w, h),
        ));
        sublayer.setPosition(objc2_core_foundation::CGPoint::new(x, y));
        // Keep promoted layers below the caret: promotion does not account for it.
        self.overlay.insertSublayer_atIndex(&sublayer, 0);
        self.promoted.push(Promoted {
          key,
          index,
          sublayer,
          drawn: false,
        });
        self.restacked = true;
        self.promoted.len() - 1
      }
    };
    let sublayer = &self.promoted[at].sublayer;
    let count = opacities.len().max(1);
    let times: Vec<_> = (0..count)
      .map(|k| NSNumber::new_f64(k as f64 / (count - 1).max(1) as f64))
      .collect();
    let times = NSArray::from_retained_slice(&times);
    let begin = CACurrentMediaTime() - since;
    let play = |path: &NSString, values: &NSArray| {
      let animation = CAKeyframeAnimation::animationWithKeyPath(Some(path));
      // SAFETY: the values are what the key path animates: numbers or transform values.
      unsafe { animation.setValues(Some(values)) };
      animation.setKeyTimes(Some(&times));
      animation.setDuration(step * (count - 1) as f64);
      animation.setRepeatCount(repeat as f32);
      animation.setBeginTime(begin);
      // SAFETY: a constant of Core Animation.
      animation.setFillMode(unsafe { kCAFillModeBoth });
      animation.setRemovedOnCompletion(false);
      sublayer.addAnimation_forKey(&animation, Some(path));
    };
    let numbers: Vec<_> = opacities
      .iter()
      .map(|&value| NSNumber::new_f32(value))
      .collect();
    let numbers = NSArray::from_retained_slice(&numbers);
    // SAFETY: an array of numbers is an array of objects.
    play(ns_string!("opacity"), unsafe {
      &*Retained::as_ptr(&numbers).cast::<NSArray>()
    });
    sublayer.setOpacity(opacities.last().copied().unwrap_or(1.));
    if !transforms.is_empty() {
      let mul = |a: &[f64; 16], b: &[f64; 16]| -> [f64; 16] {
        std::array::from_fn(|i| (0..4).map(|k| a[k * 4 + i % 4] * b[i / 4 * 4 + k]).sum())
      };
      let shift = |dx: f64, dy: f64| {
        let mut m = [0.; 16];
        [m[0], m[5], m[10], m[15], m[12], m[13]] = [1., 1., 1., 1., dx, dy];
        m
      };
      let (place, anchor) = (shift(x, y), shift(-x, -y));
      let values: Vec<_> = transforms
        .iter()
        .map(|matrix| {
          let m = mul(&anchor, &mul(matrix, &place));
          // SAFETY: a `CATransform3D` is 16 `CGFloat`s, which are `f64`, column by column.
          let t = unsafe { mem::transmute::<[f64; 16], CATransform3D>(m) };
          // SAFETY: a plain struct value.
          unsafe { NSValue::valueWithCATransform3D(t) }
        })
        .collect();
      if let Some(last) = values.last() {
        // SAFETY: the value holds a transform.
        sublayer.setTransform(unsafe { last.CATransform3DValue() });
      }
      let values = NSArray::from_retained_slice(&values);
      // SAFETY: an array of values is an array of objects.
      play(ns_string!("transform"), unsafe {
        &*Retained::as_ptr(&values).cast::<NSArray>()
      });
    }
  }

  pub fn demote(&mut self) {
    if self.promoted.is_empty() {
      return;
    }
    CATransaction::setDisableActions(true);
    for promoted in self.promoted.drain(..) {
      promoted.sublayer.removeFromSuperlayer();
    }
    self.restacked = true;
  }

  pub fn set_caret(&mut self, caret: Option<(Rect<f32>, Color, std::time::Instant)>) {
    if self
      .caret
      .as_ref()
      .map(|(_, bounds, color, since)| (*bounds, *color, *since))
      == caret
    {
      return;
    }
    CATransaction::setDisableActions(true);
    self.restacked = true;
    if let Some((layer, ..)) = self.caret.take() {
      layer.removeFromSuperlayer();
    }
    let Some((bounds, color, since)) = caret else {
      return;
    };
    let scale = self.layer.contentsScale();
    let (x, y, w, h) = (
      f64::from(bounds.origin.x) / scale,
      f64::from(bounds.origin.y) / scale,
      f64::from(bounds.size.width) / scale,
      f64::from(bounds.size.height) / scale,
    );
    let layer = CALayer::new();
    layer.setAnchorPoint(objc2_core_foundation::CGPoint::new(0., 0.));
    layer.setBounds(objc2_core_foundation::CGRect::new(
      objc2_core_foundation::CGPoint::new(0., 0.),
      CGSize::new(w, h),
    ));
    layer.setPosition(objc2_core_foundation::CGPoint::new(x, y));
    let srgb = objc2_core_graphics::CGColor::new_srgb(
      f64::from(color.r),
      f64::from(color.g),
      f64::from(color.b),
      f64::from(color.a),
    );
    layer.setBackgroundColor(Some(&srgb));
    let blink = crate::CARET_BLINK.as_secs_f64();
    let animation = CAKeyframeAnimation::animationWithKeyPath(Some(ns_string!("opacity")));
    let values = NSArray::from_retained_slice(&[NSNumber::new_f32(1.), NSNumber::new_f32(0.)]);
    // SAFETY: an array of numbers is an array of objects, which is what this key path animates.
    unsafe { animation.setValues(Some(&*Retained::as_ptr(&values).cast::<NSArray>())) };
    animation.setKeyTimes(Some(&NSArray::from_retained_slice(&[
      NSNumber::new_f64(0.),
      NSNumber::new_f64(0.5),
      NSNumber::new_f64(1.),
    ])));
    // SAFETY: constants of Core Animation.
    animation.setCalculationMode(unsafe { kCAAnimationDiscrete });
    animation.setDuration(blink * 2.);
    animation.setRepeatCount(f32::INFINITY);
    animation.setBeginTime(CACurrentMediaTime() - since.elapsed().as_secs_f64());
    // SAFETY: constants of Core Animation.
    animation.setFillMode(unsafe { kCAFillModeBoth });
    animation.setRemovedOnCompletion(false);
    layer.addAnimation_forKey(&animation, Some(ns_string!("opacity")));
    self.overlay.addSublayer(&layer);
    self.caret = Some((layer, bounds, color, since));
  }

  // Present glass and the frame in one main-thread Core Animation transaction, including AppKit's resize
  // transaction.
  pub fn take_glass_update(
    &mut self,
    apply: impl FnOnce(Vec<super::super::glass::GlassSurface>) + Send + 'static,
  ) -> Option<Present> {
    if !mem::take(&mut self.glass_update) {
      return None;
    }
    self.glass_stale = false;
    let surfaces = super::super::Sendable::new(
      self
        .glass
        .iter()
        .map(|glass| glass.surface.clone())
        .collect::<Vec<_>>(),
    );
    let frame = self.present.take().and_then(|mut present| present.0.take());
    let update = Present::new(move || {
      let run = move || {
        apply(surfaces.take());
        if let Some(frame) = frame {
          frame.take()();
        }
      };
      if MainThreadMarker::new().is_some() {
        run();
      } else {
        super::super::native_post(Box::new(run));
      }
    });
    if self.in_transaction {
      self.present = Some(update);
      return None;
    }
    Some(update)
  }
}
