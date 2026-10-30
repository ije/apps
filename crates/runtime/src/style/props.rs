use rustc_hash::FxHashMap;
use std::{cell::RefCell, marker::PhantomData, rc::Rc};

/// A CSS property name. Known names have fixed ids in name order; other names are interned per thread.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Prop(u32, PhantomData<*const ()>);

const CUSTOM: u32 = 1 << 31;

macro_rules! props {
    ($($id:ident $name:literal $($inherited:ident)?,)*) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        enum Known { $($id,)* Count }
        impl Prop {
            $(pub(crate) const $id: Prop = Prop(Known::$id as u32, PhantomData);)*
        }
        const NAMES: [&str; Known::Count as usize] = [$($name,)*];
        const INHERITED: [bool; Known::Count as usize] = [$(props!(@inherited $($inherited)?),)*];
        fn known(name: &str) -> Option<Prop> {
            match name {
                $($name => Some(Prop::$id),)*
                _ => None,
            }
        }
    };
    (@inherited inherited) => { true };
    (@inherited) => { false };
}

props! {
    MOZ_OSX_FONT_SMOOTHING "-moz-osx-font-smoothing" inherited,
    WEBKIT_APP_REGION "-webkit-app-region",
    WEBKIT_APPEARANCE "-webkit-appearance",
    WEBKIT_BACKDROP_FILTER "-webkit-backdrop-filter",
    WEBKIT_BACKGROUND_CLIP "-webkit-background-clip",
    WEBKIT_BOX_ORIENT "-webkit-box-orient",
    WEBKIT_CLIP_PATH "-webkit-clip-path",
    WEBKIT_FILTER "-webkit-filter",
    WEBKIT_FONT_SMOOTHING "-webkit-font-smoothing" inherited,
    WEBKIT_LINE_CLAMP "-webkit-line-clamp",
    WEBKIT_MASK "-webkit-mask",
    WEBKIT_MASK_IMAGE "-webkit-mask-image",
    WEBKIT_MASK_POSITION "-webkit-mask-position",
    WEBKIT_MASK_REPEAT "-webkit-mask-repeat",
    WEBKIT_MASK_SIZE "-webkit-mask-size",
    WEBKIT_TAP_HIGHLIGHT_COLOR "-webkit-tap-highlight-color",
    WEBKIT_TEXT_FILL_COLOR "-webkit-text-fill-color" inherited,
    WEBKIT_TEXT_STROKE "-webkit-text-stroke",
    WEBKIT_TEXT_STROKE_COLOR "-webkit-text-stroke-color",
    WEBKIT_TEXT_STROKE_WIDTH "-webkit-text-stroke-width",
    WEBKIT_USER_SELECT "-webkit-user-select",
    ACCENT_COLOR "accent-color" inherited,
    ALIGN_CONTENT "align-content",
    ALIGN_ITEMS "align-items",
    ALIGN_SELF "align-self",
    ALL "all",
    ANIMATION "animation",
    ANIMATION_COMPOSITION "animation-composition",
    ANIMATION_DELAY "animation-delay",
    ANIMATION_DIRECTION "animation-direction",
    ANIMATION_DURATION "animation-duration",
    ANIMATION_FILL_MODE "animation-fill-mode",
    ANIMATION_ITERATION_COUNT "animation-iteration-count",
    ANIMATION_NAME "animation-name",
    ANIMATION_PLAY_STATE "animation-play-state",
    ANIMATION_TIMING_FUNCTION "animation-timing-function",
    APP_REGION "app-region" inherited,
    APPEARANCE "appearance",
    ASPECT_RATIO "aspect-ratio",
    BACKDROP_FILTER "backdrop-filter",
    BACKFACE_VISIBILITY "backface-visibility",
    BACKGROUND "background",
    BACKGROUND_ATTACHMENT "background-attachment",
    BACKGROUND_BLEND_MODE "background-blend-mode",
    BACKGROUND_CLIP "background-clip",
    BACKGROUND_COLOR "background-color",
    BACKGROUND_IMAGE "background-image",
    BACKGROUND_ORIGIN "background-origin",
    BACKGROUND_POSITION "background-position",
    BACKGROUND_POSITION_X "background-position-x",
    BACKGROUND_POSITION_Y "background-position-y",
    BACKGROUND_REPEAT "background-repeat",
    BACKGROUND_SIZE "background-size",
    BLOCK_SIZE "block-size",
    BORDER "border",
    BORDER_BLOCK "border-block",
    BORDER_BLOCK_COLOR "border-block-color",
    BORDER_BLOCK_END "border-block-end",
    BORDER_BLOCK_END_COLOR "border-block-end-color",
    BORDER_BLOCK_END_STYLE "border-block-end-style",
    BORDER_BLOCK_END_WIDTH "border-block-end-width",
    BORDER_BLOCK_START "border-block-start",
    BORDER_BLOCK_START_COLOR "border-block-start-color",
    BORDER_BLOCK_START_STYLE "border-block-start-style",
    BORDER_BLOCK_START_WIDTH "border-block-start-width",
    BORDER_BLOCK_STYLE "border-block-style",
    BORDER_BLOCK_WIDTH "border-block-width",
    BORDER_BOTTOM "border-bottom",
    BORDER_BOTTOM_COLOR "border-bottom-color",
    BORDER_BOTTOM_LEFT_RADIUS "border-bottom-left-radius",
    BORDER_BOTTOM_RIGHT_RADIUS "border-bottom-right-radius",
    BORDER_BOTTOM_STYLE "border-bottom-style",
    BORDER_BOTTOM_WIDTH "border-bottom-width",
    BORDER_COLLAPSE "border-collapse" inherited,
    BORDER_COLOR "border-color",
    BORDER_END_END_RADIUS "border-end-end-radius",
    BORDER_END_START_RADIUS "border-end-start-radius",
    BORDER_IMAGE "border-image",
    BORDER_INLINE "border-inline",
    BORDER_INLINE_COLOR "border-inline-color",
    BORDER_INLINE_END "border-inline-end",
    BORDER_INLINE_END_COLOR "border-inline-end-color",
    BORDER_INLINE_END_STYLE "border-inline-end-style",
    BORDER_INLINE_END_WIDTH "border-inline-end-width",
    BORDER_INLINE_START "border-inline-start",
    BORDER_INLINE_START_COLOR "border-inline-start-color",
    BORDER_INLINE_START_STYLE "border-inline-start-style",
    BORDER_INLINE_START_WIDTH "border-inline-start-width",
    BORDER_INLINE_STYLE "border-inline-style",
    BORDER_INLINE_WIDTH "border-inline-width",
    BORDER_LEFT "border-left",
    BORDER_LEFT_COLOR "border-left-color",
    BORDER_LEFT_STYLE "border-left-style",
    BORDER_LEFT_WIDTH "border-left-width",
    BORDER_RADIUS "border-radius",
    BORDER_RIGHT "border-right",
    BORDER_RIGHT_COLOR "border-right-color",
    BORDER_RIGHT_STYLE "border-right-style",
    BORDER_RIGHT_WIDTH "border-right-width",
    BORDER_SPACING "border-spacing" inherited,
    BORDER_START_END_RADIUS "border-start-end-radius",
    BORDER_START_START_RADIUS "border-start-start-radius",
    BORDER_STYLE "border-style",
    BORDER_TOP "border-top",
    BORDER_TOP_COLOR "border-top-color",
    BORDER_TOP_LEFT_RADIUS "border-top-left-radius",
    BORDER_TOP_RIGHT_RADIUS "border-top-right-radius",
    BORDER_TOP_STYLE "border-top-style",
    BORDER_TOP_WIDTH "border-top-width",
    BORDER_WIDTH "border-width",
    BOTTOM "bottom",
    BOX_DECORATION_BREAK "box-decoration-break",
    BOX_SHADOW "box-shadow",
    BOX_SIZING "box-sizing",
    BREAK_AFTER "break-after",
    BREAK_BEFORE "break-before",
    BREAK_INSIDE "break-inside",
    CAPTION_SIDE "caption-side",
    CARET_COLOR "caret-color" inherited,
    CLEAR "clear",
    CLIP "clip",
    CLIP_PATH "clip-path",
    CLIP_RULE "clip-rule",
    COLOR "color" inherited,
    COLOR_SCHEME "color-scheme" inherited,
    COLUMN_COUNT "column-count",
    COLUMN_FILL "column-fill",
    COLUMN_GAP "column-gap",
    COLUMN_RULE "column-rule",
    COLUMN_RULE_COLOR "column-rule-color",
    COLUMN_RULE_STYLE "column-rule-style",
    COLUMN_RULE_WIDTH "column-rule-width",
    COLUMN_SPAN "column-span",
    COLUMN_WIDTH "column-width",
    COLUMNS "columns",
    CONTAIN "contain",
    CONTAINER "container",
    CONTAINER_NAME "container-name",
    CONTAINER_TYPE "container-type",
    CONTENT "content",
    CONTENT_VISIBILITY "content-visibility",
    COUNTER_INCREMENT "counter-increment",
    COUNTER_RESET "counter-reset",
    COUNTER_SET "counter-set",
    CURSOR "cursor" inherited,
    DIRECTION "direction" inherited,
    DISPLAY "display",
    EMPTY_CELLS "empty-cells",
    FIELD_SIZING "field-sizing",
    FILL "fill" inherited,
    FILL_OPACITY "fill-opacity" inherited,
    FILL_RULE "fill-rule",
    FILTER "filter",
    FLEX "flex",
    FLEX_BASIS "flex-basis",
    FLEX_DIRECTION "flex-direction",
    FLEX_FLOW "flex-flow",
    FLEX_GROW "flex-grow",
    FLEX_SHRINK "flex-shrink",
    FLEX_WRAP "flex-wrap",
    FLOAT "float",
    FLOOD_COLOR "flood-color",
    FLOOD_OPACITY "flood-opacity",
    FONT "font",
    FONT_DISPLAY "font-display",
    FONT_FAMILY "font-family" inherited,
    FONT_FEATURE_SETTINGS "font-feature-settings" inherited,
    FONT_KERNING "font-kerning",
    FONT_OPTICAL_SIZING "font-optical-sizing",
    FONT_SIZE "font-size" inherited,
    FONT_SIZE_ADJUST "font-size-adjust",
    FONT_STRETCH "font-stretch" inherited,
    FONT_STYLE "font-style" inherited,
    FONT_SYNTHESIS "font-synthesis",
    FONT_VARIANT "font-variant" inherited,
    FONT_VARIANT_LIGATURES "font-variant-ligatures",
    FONT_VARIANT_NUMERIC "font-variant-numeric" inherited,
    FONT_VARIATION_SETTINGS "font-variation-settings",
    FONT_WEIGHT "font-weight" inherited,
    FORCED_COLOR_ADJUST "forced-color-adjust",
    GAP "gap",
    GRID "grid",
    GRID_AREA "grid-area",
    GRID_AUTO_COLUMNS "grid-auto-columns",
    GRID_AUTO_FLOW "grid-auto-flow",
    GRID_AUTO_ROWS "grid-auto-rows",
    GRID_COLUMN "grid-column",
    GRID_COLUMN_END "grid-column-end",
    GRID_COLUMN_GAP "grid-column-gap",
    GRID_COLUMN_START "grid-column-start",
    GRID_GAP "grid-gap",
    GRID_ROW "grid-row",
    GRID_ROW_END "grid-row-end",
    GRID_ROW_GAP "grid-row-gap",
    GRID_ROW_START "grid-row-start",
    GRID_TEMPLATE "grid-template",
    GRID_TEMPLATE_AREAS "grid-template-areas",
    GRID_TEMPLATE_COLUMNS "grid-template-columns",
    GRID_TEMPLATE_ROWS "grid-template-rows",
    HEIGHT "height",
    HYPHENS "hyphens" inherited,
    IMAGE_RENDERING "image-rendering",
    INLINE_SIZE "inline-size",
    INSET "inset",
    INSET_BLOCK "inset-block",
    INSET_BLOCK_END "inset-block-end",
    INSET_BLOCK_START "inset-block-start",
    INSET_INLINE "inset-inline",
    INSET_INLINE_END "inset-inline-end",
    INSET_INLINE_START "inset-inline-start",
    ISOLATION "isolation",
    JUSTIFY_CONTENT "justify-content",
    JUSTIFY_ITEMS "justify-items",
    JUSTIFY_SELF "justify-self",
    LEFT "left",
    LETTER_SPACING "letter-spacing" inherited,
    LIGHTING_COLOR "lighting-color",
    LINE_BREAK "line-break",
    LINE_CLAMP "line-clamp",
    LINE_HEIGHT "line-height" inherited,
    LIST_STYLE "list-style",
    LIST_STYLE_IMAGE "list-style-image" inherited,
    LIST_STYLE_POSITION "list-style-position" inherited,
    LIST_STYLE_TYPE "list-style-type" inherited,
    MARGIN "margin",
    MARGIN_BLOCK "margin-block",
    MARGIN_BLOCK_END "margin-block-end",
    MARGIN_BLOCK_START "margin-block-start",
    MARGIN_BOTTOM "margin-bottom",
    MARGIN_INLINE "margin-inline",
    MARGIN_INLINE_END "margin-inline-end",
    MARGIN_INLINE_START "margin-inline-start",
    MARGIN_LEFT "margin-left",
    MARGIN_RIGHT "margin-right",
    MARGIN_TOP "margin-top",
    MARKER "marker",
    MASK "mask",
    MASK_CLIP "mask-clip",
    MASK_COMPOSITE "mask-composite",
    MASK_IMAGE "mask-image",
    MASK_MODE "mask-mode",
    MASK_ORIGIN "mask-origin",
    MASK_POSITION "mask-position",
    MASK_REPEAT "mask-repeat",
    MASK_SIZE "mask-size",
    MASK_TYPE "mask-type",
    MAX_BLOCK_SIZE "max-block-size",
    MAX_HEIGHT "max-height",
    MAX_INLINE_SIZE "max-inline-size",
    MAX_WIDTH "max-width",
    MIN_BLOCK_SIZE "min-block-size",
    MIN_HEIGHT "min-height",
    MIN_INLINE_SIZE "min-inline-size",
    MIN_WIDTH "min-width",
    MIX_BLEND_MODE "mix-blend-mode",
    OBJECT_FIT "object-fit",
    OBJECT_POSITION "object-position",
    OFFSET "offset",
    OPACITY "opacity",
    ORDER "order",
    ORPHANS "orphans",
    OUTLINE "outline",
    OUTLINE_COLOR "outline-color",
    OUTLINE_OFFSET "outline-offset",
    OUTLINE_STYLE "outline-style",
    OUTLINE_WIDTH "outline-width",
    OVERFLOW "overflow",
    OVERFLOW_ANCHOR "overflow-anchor",
    OVERFLOW_CLIP_MARGIN "overflow-clip-margin",
    OVERFLOW_WRAP "overflow-wrap" inherited,
    OVERFLOW_X "overflow-x",
    OVERFLOW_Y "overflow-y",
    OVERSCROLL_BEHAVIOR "overscroll-behavior",
    OVERSCROLL_BEHAVIOR_X "overscroll-behavior-x",
    OVERSCROLL_BEHAVIOR_Y "overscroll-behavior-y",
    PADDING "padding",
    PADDING_BLOCK "padding-block",
    PADDING_BLOCK_END "padding-block-end",
    PADDING_BLOCK_START "padding-block-start",
    PADDING_BOTTOM "padding-bottom",
    PADDING_INLINE "padding-inline",
    PADDING_INLINE_END "padding-inline-end",
    PADDING_INLINE_START "padding-inline-start",
    PADDING_LEFT "padding-left",
    PADDING_RIGHT "padding-right",
    PADDING_TOP "padding-top",
    PAGE_BREAK_AFTER "page-break-after",
    PAGE_BREAK_BEFORE "page-break-before",
    PAGE_BREAK_INSIDE "page-break-inside",
    PAINT_ORDER "paint-order",
    PERSPECTIVE "perspective",
    PERSPECTIVE_ORIGIN "perspective-origin",
    PLACE_CONTENT "place-content",
    PLACE_ITEMS "place-items",
    PLACE_SELF "place-self",
    POINTER_EVENTS "pointer-events" inherited,
    POSITION "position",
    PRINT_COLOR_ADJUST "print-color-adjust",
    QUOTES "quotes" inherited,
    R "r",
    RESIZE "resize",
    RIGHT "right",
    ROTATE "rotate",
    ROW_GAP "row-gap",
    RX "rx",
    RY "ry",
    SCALE "scale",
    SCROLL_BEHAVIOR "scroll-behavior",
    SCROLL_MARGIN "scroll-margin",
    SCROLL_MARGIN_BOTTOM "scroll-margin-bottom",
    SCROLL_MARGIN_LEFT "scroll-margin-left",
    SCROLL_MARGIN_RIGHT "scroll-margin-right",
    SCROLL_MARGIN_TOP "scroll-margin-top",
    SCROLL_PADDING "scroll-padding",
    SCROLL_PADDING_BOTTOM "scroll-padding-bottom",
    SCROLL_PADDING_LEFT "scroll-padding-left",
    SCROLL_PADDING_RIGHT "scroll-padding-right",
    SCROLL_PADDING_TOP "scroll-padding-top",
    SCROLL_SNAP_ALIGN "scroll-snap-align",
    SCROLL_SNAP_STOP "scroll-snap-stop",
    SCROLL_SNAP_TYPE "scroll-snap-type",
    SCROLLBAR_COLOR "scrollbar-color",
    SCROLLBAR_GUTTER "scrollbar-gutter",
    SCROLLBAR_WIDTH "scrollbar-width",
    SHAPE_OUTSIDE "shape-outside",
    SHAPE_RENDERING "shape-rendering",
    SPEAK "speak",
    STOP_COLOR "stop-color",
    STOP_OPACITY "stop-opacity",
    STROKE "stroke" inherited,
    STROKE_DASHARRAY "stroke-dasharray",
    STROKE_DASHOFFSET "stroke-dashoffset",
    STROKE_LINECAP "stroke-linecap",
    STROKE_LINEJOIN "stroke-linejoin",
    STROKE_MITERLIMIT "stroke-miterlimit",
    STROKE_OPACITY "stroke-opacity" inherited,
    STROKE_WIDTH "stroke-width" inherited,
    TAB_SIZE "tab-size" inherited,
    TABLE_LAYOUT "table-layout",
    TEXT_ALIGN "text-align" inherited,
    TEXT_ALIGN_LAST "text-align-last",
    TEXT_ANCHOR "text-anchor",
    TEXT_DECORATION "text-decoration",
    TEXT_DECORATION_COLOR "text-decoration-color" inherited,
    TEXT_DECORATION_LINE "text-decoration-line" inherited,
    TEXT_DECORATION_SKIP_INK "text-decoration-skip-ink",
    TEXT_DECORATION_STYLE "text-decoration-style" inherited,
    TEXT_DECORATION_THICKNESS "text-decoration-thickness" inherited,
    TEXT_EMPHASIS "text-emphasis",
    TEXT_INDENT "text-indent" inherited,
    TEXT_ORIENTATION "text-orientation",
    TEXT_OVERFLOW "text-overflow",
    TEXT_RENDERING "text-rendering",
    TEXT_SHADOW "text-shadow" inherited,
    TEXT_SIZE_ADJUST "text-size-adjust",
    TEXT_TRANSFORM "text-transform" inherited,
    TEXT_UNDERLINE_OFFSET "text-underline-offset" inherited,
    TEXT_UNDERLINE_POSITION "text-underline-position",
    TEXT_WRAP "text-wrap" inherited,
    TEXT_WRAP_MODE "text-wrap-mode",
    TEXT_WRAP_STYLE "text-wrap-style",
    TOP "top",
    TOUCH_ACTION "touch-action",
    TRANSFORM "transform",
    TRANSFORM_BOX "transform-box",
    TRANSFORM_ORIGIN "transform-origin",
    TRANSFORM_STYLE "transform-style",
    TRANSITION "transition",
    TRANSITION_BEHAVIOR "transition-behavior",
    TRANSITION_DELAY "transition-delay",
    TRANSITION_DURATION "transition-duration",
    TRANSITION_PROPERTY "transition-property",
    TRANSITION_TIMING_FUNCTION "transition-timing-function",
    TRANSLATE "translate",
    UNICODE_BIDI "unicode-bidi",
    USER_SELECT "user-select",
    VECTOR_EFFECT "vector-effect",
    VERTICAL_ALIGN "vertical-align",
    VIEW_TRANSITION_NAME "view-transition-name",
    VISIBILITY "visibility" inherited,
    WHITE_SPACE "white-space" inherited,
    WHITE_SPACE_COLLAPSE "white-space-collapse",
    WIDOWS "widows",
    WIDTH "width",
    WILL_CHANGE "will-change",
    WORD_BREAK "word-break" inherited,
    WORD_SPACING "word-spacing" inherited,
    WORD_WRAP "word-wrap",
    WRITING_MODE "writing-mode",
    X "x",
    Y "y",
    Z_INDEX "z-index",
    ZOOM "zoom",
}

thread_local! {
    // WARNING: interned names are leaked. They are bounded by the property names that the app's CSS uses.
    static INTERNED: RefCell<(Vec<&'static str>, FxHashMap<&'static str, Prop>)> = Default::default();
}

impl Prop {
    /// The property, interning a name that is not known yet.
    pub(crate) fn new(name: &str) -> Self {
        known(name).unwrap_or_else(|| {
            INTERNED.with(|interned| {
                let (names, ids) = &mut *interned.borrow_mut();
                if let Some(&prop) = ids.get(name) {
                    return prop;
                }
                let name: &'static str = Box::leak(name.into());
                let index = names.len() as u32;
                let prop = Prop(
                    if name.starts_with("--") {
                        CUSTOM | index
                    } else {
                        NAMES.len() as u32 + index
                    },
                    PhantomData,
                );
                names.push(name);
                ids.insert(name, prop);
                prop
            })
        })
    }

    /// The property if any style can hold it.
    pub(crate) fn find(name: &str) -> Option<Self> {
        known(name).or_else(|| INTERNED.with(|interned| interned.borrow().1.get(name).copied()))
    }

    pub(crate) fn name(self) -> &'static str {
        match NAMES.get(self.0 as usize) {
            Some(name) => name,
            None => INTERNED.with(|interned| {
                interned.borrow().0[(if self.is_custom() {
                    self.0 & !CUSTOM
                } else {
                    self.0 - NAMES.len() as u32
                }) as usize]
            }),
        }
    }

    /// A `--*` custom property.
    pub(crate) fn is_custom(self) -> bool {
        self.0 & CUSTOM != 0
    }

    pub(crate) fn inherited(self) -> bool {
        INHERITED.get(self.0 as usize).copied().unwrap_or(false)
    }
}

impl std::fmt::Debug for Prop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Property values, sorted by property.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Style(Vec<(Prop, Rc<str>)>);

impl Style {
    pub(crate) fn new() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn get(&self, prop: Prop) -> Option<&str> {
        self.value(prop).map(|value| &**value)
    }

    pub(crate) fn value(&self, prop: Prop) -> Option<&Rc<str>> {
        self.0
            .binary_search_by_key(&prop, |entry| entry.0)
            .ok()
            .map(|index| &self.0[index].1)
    }

    pub(crate) fn contains(&self, prop: Prop) -> bool {
        self.0.binary_search_by_key(&prop, |entry| entry.0).is_ok()
    }

    pub(crate) fn insert(&mut self, prop: Prop, value: impl Into<Rc<str>>) {
        let value = value.into();
        match self.0.binary_search_by_key(&prop, |entry| entry.0) {
            Ok(index) => self.0[index].1 = value,
            Err(index) => self.0.insert(index, (prop, value)),
        }
    }

    pub(crate) fn remove(&mut self, prop: Prop) -> Option<Rc<str>> {
        let index = self.0.binary_search_by_key(&prop, |entry| entry.0).ok()?;
        Some(self.0.remove(index).1)
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(Prop, &mut Rc<str>) -> bool) {
        self.0.retain_mut(|(prop, value)| keep(*prop, value));
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (Prop, &Rc<str>)> {
        self.0.iter().map(|(prop, value)| (*prop, value))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromIterator<(Prop, Rc<str>)> for Style {
    fn from_iter<I: IntoIterator<Item = (Prop, Rc<str>)>>(entries: I) -> Self {
        let mut style = Self(Vec::new());
        style.extend(entries);
        style
    }
}

impl Extend<(Prop, Rc<str>)> for Style {
    fn extend<I: IntoIterator<Item = (Prop, Rc<str>)>>(&mut self, entries: I) {
        let start = self.0.len();
        self.0.extend(entries);
        if self.0[start..].is_sorted_by(|a, b| a.0 < b.0)
            && self.0[..start]
                .last()
                .zip(self.0.get(start))
                .is_none_or(|(a, b)| a.0 < b.0)
        {
            return;
        }
        // WARNING: the stable sort keeps the later of two values for the same property.
        self.0.sort_by_key(|entry| entry.0);
        self.0.reverse();
        self.0.dedup_by_key(|entry| entry.0);
        self.0.reverse();
    }
}

impl std::fmt::Debug for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl serde::Serialize for Style {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut entries: Vec<_> = self
            .iter()
            .map(|(prop, value)| (prop.name(), &**value))
            .collect();
        entries.sort_unstable_by_key(|entry| entry.0);
        serializer.collect_map(entries)
    }
}

#[cfg(test)]
impl std::ops::Index<&str> for Style {
    type Output = str;

    fn index(&self, name: &str) -> &str {
        Prop::find(name)
            .and_then(|prop| self.get(prop))
            .unwrap_or_else(|| panic!("no {name}"))
    }
}
