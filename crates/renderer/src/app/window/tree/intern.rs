use super::*;

pub(super) struct Interned<T> {
    values: FxHashMap<u64, Vec<Rc<T>>>,
    // The number of stored values in all buckets, which bounds memory and the cost of a bucket scan.
    count: usize,
    limit: usize,
}

impl<T> Default for Interned<T> {
    fn default() -> Self {
        Self {
            values: FxHashMap::default(),
            count: 0,
            limit: 1024,
        }
    }
}

pub(super) trait Fingerprint {
    fn fingerprint(&self, hasher: &mut FxHasher);
}

impl Fingerprint for Style {
    fn fingerprint(&self, hasher: &mut FxHasher) {
        let length = |hasher: &mut FxHasher, length: crate::Length| match length {
            crate::Length::Auto => 0u32.hash(hasher),
            crate::Length::Px(value) => value.0.to_bits().hash(hasher),
            crate::Length::Fraction(value) => (1, value.to_bits()).hash(hasher),
            crate::Length::Calc(value, offset) => {
                (value.to_bits(), offset.0.to_bits()).hash(hasher)
            }
            crate::Length::Bound(index) => (2, index).hash(hasher),
        };
        (
            self.display as u8,
            self.position as u8,
            self.flex_direction as u8,
        )
            .hash(hasher);
        for value in [
            self.size.width,
            self.size.height,
            self.padding.top,
            self.padding.left,
        ]
        .into_iter()
        .chain([self.margin.top, self.margin.left, self.margin.bottom])
        .chain([self.inset.top, self.inset.left])
        {
            length(hasher, value);
        }
        for color in [self.background, self.border_color, self.text.color] {
            color
                .map(|color| [color.r, color.g, color.b, color.a].map(f32::to_bits))
                .hash(hasher);
        }
        self.border_widths.top.0.to_bits().hash(hasher);
        length(hasher, self.corner_radii.x.top_left);
        (
            self.opacity.map(f32::to_bits),
            self.z_index,
            self.border_style as u8,
            self.box_shadow.len(),
        )
            .hash(hasher);
        if let Some(shadow) = self.box_shadow.first() {
            [
                shadow.offset.x.0,
                shadow.offset.y.0,
                shadow.blur_radius.0,
                shadow.spread_radius.0,
                shadow.color.a,
            ]
            .map(f32::to_bits)
            .hash(hasher);
        }
        self.text
            .font_size
            .map(|size| size.0.to_bits())
            .hash(hasher);
        (
            self.breaks,
            self.text.lines_kept,
            self.columns.as_deref().map(|columns| {
                let height = columns.height.map(|height| height.0.to_bits());
                (columns.count, columns.gap.0.to_bits(), height, columns.fill)
            }),
        )
            .hash(hasher);
        self.bounds.iter().for_each(|bound| bound.hash(hasher));
    }
}

impl Fingerprint for TextStyle {
    fn fingerprint(&self, hasher: &mut FxHasher) {
        [
            self.font_size.0,
            self.line_height.0,
            self.color.r,
            self.color.g,
            self.color.b,
        ]
        .map(f32::to_bits)
        .hash(hasher);
        (
            self.font_family.len(),
            self.white_space as u8,
            self.text_align as u8,
            self.lines_kept,
        )
            .hash(hasher);
    }
}

impl<T: Fingerprint + PartialEq> Interned<T> {
    pub(super) fn get(&mut self, value: T) -> Rc<T> {
        let mut hasher = FxHasher::default();
        value.fingerprint(&mut hasher);
        self.find(
            hasher.finish(),
            value,
            |kept, value| kept == value,
            |value| value,
        )
    }
}

impl Interned<Styled> {
    pub(super) fn styled(&mut self, style: Style, scale: f32) -> Rc<Styled> {
        let mut hasher = FxHasher::default();
        style.fingerprint(&mut hasher);
        self.find(
            hasher.finish(),
            style,
            |kept, style| kept.style == *style,
            |style| {
                let (taffy, bounds) = style.to_taffy(scale);
                let fixed = |raw: taffy::CompactLength| {
                    matches!(
                        raw.tag(),
                        taffy::CompactLength::LENGTH_TAG | taffy::CompactLength::AUTO_TAG
                    )
                };
                let sides = |rect: taffy::Rect<taffy::CompactLength>| {
                    [rect.top, rect.right, rect.bottom, rect.left]
                };
                let relative = ![taffy.size.width.into_raw(), taffy.size.height.into_raw()]
                    .into_iter()
                    .chain([
                        taffy.min_size.width.into_raw(),
                        taffy.min_size.height.into_raw(),
                    ])
                    .chain([
                        taffy.max_size.width.into_raw(),
                        taffy.max_size.height.into_raw(),
                    ])
                    .chain(sides(taffy.padding.map(|value| value.into_raw())))
                    .chain(sides(taffy.border.map(|value| value.into_raw())))
                    .chain(sides(taffy.margin.map(|value| value.into_raw())))
                    .all(fixed);
                Styled {
                    taffy,
                    _bounds: bounds,
                    style,
                    relative,
                }
            },
        )
    }
}

impl<T> Interned<T> {
    fn find<V>(
        &mut self,
        fingerprint: u64,
        value: V,
        same: impl Fn(&T, &V) -> bool,
        make: impl FnOnce(V) -> T,
    ) -> Rc<T> {
        let list = self.values.entry(fingerprint).or_default();
        if let Some(kept) = list.iter().find(|kept| same(kept, &value)) {
            return kept.clone();
        }
        let value = Rc::new(make(value));
        list.push(value.clone());
        self.count += 1;
        if self.count > self.limit {
            let mut count = 0;
            self.values.retain(|_, list| {
                list.retain(|value| Rc::strong_count(value) > 1);
                count += list.len();
                !list.is_empty()
            });
            self.count = count;
            self.limit = (count * 2).max(1024);
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::px;

    #[test]
    fn interned_styles_stay_bounded() {
        let mut styles = Interned::<super::Styled>::default();
        let kept = styles.styled(crate::Style::default(), 2.);
        let mut style = crate::Style {
            box_shadow: vec![Default::default(); 2],
            ..Default::default()
        };
        // An animated value that the fingerprint skips lands in one bucket.
        for frame in 0..10_000 {
            style.box_shadow[1].blur_radius = px(frame as f32);
            styles.styled(style.clone(), 2.);
        }
        let stored: usize = styles.values.values().map(Vec::len).sum();
        assert!(stored == styles.count && stored <= 1024, "{stored}");
        assert!(std::rc::Rc::ptr_eq(
            &kept,
            &styles.styled(crate::Style::default(), 2.)
        ));
    }

    #[test]
    fn column_and_break_styles_intern_apart() {
        let mut styles = Interned::<super::Styled>::default();
        let mut style = crate::Style::default();
        let plain = styles.styled(style.clone(), 2.);
        style.breaks = crate::Break::AVOID_INSIDE;
        let avoid = styles.styled(style.clone(), 2.);
        style.columns = Some(Box::new(crate::Columns {
            count: 2,
            gap: px(10.),
            height: None,
            fill: false,
        }));
        let set = styles.styled(style.clone(), 2.);
        style.text.set_lines_kept([3, 2]);
        let kept = styles.styled(style, 2.);
        assert!(!std::rc::Rc::ptr_eq(&plain, &avoid));
        assert!(!std::rc::Rc::ptr_eq(&avoid, &set));
        assert!(!std::rc::Rc::ptr_eq(&set, &kept));
    }
}
