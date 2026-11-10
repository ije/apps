use rustc_hash::FxHashMap;
use std::{cell::RefCell, marker::PhantomData, rc::Rc};

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
    MACOS_GLASS "-macos-glass",
    MACOS_GLASS_INTERACTIVE "-macos-glass-interactive",
    MACOS_GLASS_TINT_COLOR "-macos-glass-tint-color",
    MOZ_OSX_FONT_SMOOTHING "-moz-osx-font-smoothing" inherited,
    WEBKIT_APP_REGION "-webkit-app-region" inherited,
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
    WEBKIT_TAP_HIGHLIGHT_COLOR "-webkit-tap-highlight-color" inherited,
    WEBKIT_TEXT_FILL_COLOR "-webkit-text-fill-color" inherited,
    WEBKIT_TEXT_STROKE "-webkit-text-stroke" inherited,
    WEBKIT_TEXT_STROKE_COLOR "-webkit-text-stroke-color" inherited,
    WEBKIT_TEXT_STROKE_WIDTH "-webkit-text-stroke-width" inherited,
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
    CAPTION_SIDE "caption-side" inherited,
    CARET_COLOR "caret-color" inherited,
    CLEAR "clear",
    CLIP "clip",
    CLIP_PATH "clip-path",
    CLIP_RULE "clip-rule" inherited,
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
    CX "cx",
    CY "cy",
    DIRECTION "direction" inherited,
    DISPLAY "display",
    EMPTY_CELLS "empty-cells" inherited,
    FIELD_SIZING "field-sizing",
    FILL "fill" inherited,
    FILL_OPACITY "fill-opacity" inherited,
    FILL_RULE "fill-rule" inherited,
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
    FONT_KERNING "font-kerning" inherited,
    FONT_OPTICAL_SIZING "font-optical-sizing" inherited,
    FONT_SIZE "font-size" inherited,
    FONT_SIZE_ADJUST "font-size-adjust" inherited,
    FONT_STRETCH "font-stretch" inherited,
    FONT_STYLE "font-style" inherited,
    FONT_SYNTHESIS "font-synthesis" inherited,
    FONT_VARIANT "font-variant" inherited,
    FONT_VARIANT_LIGATURES "font-variant-ligatures" inherited,
    FONT_VARIANT_NUMERIC "font-variant-numeric" inherited,
    FONT_VARIATION_SETTINGS "font-variation-settings" inherited,
    FONT_WEIGHT "font-weight" inherited,
    FORCED_COLOR_ADJUST "forced-color-adjust" inherited,
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
    IMAGE_RENDERING "image-rendering" inherited,
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
    LINE_BREAK "line-break" inherited,
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
    MARKER "marker" inherited,
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
    ORPHANS "orphans" inherited,
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
    OVERLAY "overlay",
    OVERSCROLL_BEHAVIOR "overscroll-behavior",
    OVERSCROLL_BEHAVIOR_BLOCK "overscroll-behavior-block",
    OVERSCROLL_BEHAVIOR_INLINE "overscroll-behavior-inline",
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
    PAINT_ORDER "paint-order" inherited,
    PERSPECTIVE "perspective",
    PERSPECTIVE_ORIGIN "perspective-origin",
    PLACE_CONTENT "place-content",
    PLACE_ITEMS "place-items",
    PLACE_SELF "place-self",
    POINTER_EVENTS "pointer-events" inherited,
    POSITION "position",
    PRINT_COLOR_ADJUST "print-color-adjust" inherited,
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
    SCROLLBAR_COLOR "scrollbar-color" inherited,
    SCROLLBAR_GUTTER "scrollbar-gutter",
    SCROLLBAR_WIDTH "scrollbar-width",
    SHAPE_OUTSIDE "shape-outside",
    SHAPE_RENDERING "shape-rendering" inherited,
    SPEAK "speak" inherited,
    STOP_COLOR "stop-color",
    STOP_OPACITY "stop-opacity",
    STROKE "stroke" inherited,
    STROKE_DASHARRAY "stroke-dasharray" inherited,
    STROKE_DASHOFFSET "stroke-dashoffset" inherited,
    STROKE_LINECAP "stroke-linecap" inherited,
    STROKE_LINEJOIN "stroke-linejoin" inherited,
    STROKE_MITERLIMIT "stroke-miterlimit" inherited,
    STROKE_OPACITY "stroke-opacity" inherited,
    STROKE_WIDTH "stroke-width" inherited,
    TAB_SIZE "tab-size" inherited,
    TABLE_LAYOUT "table-layout",
    TEXT_ALIGN "text-align" inherited,
    TEXT_ALIGN_LAST "text-align-last" inherited,
    TEXT_BOX "text-box",
    TEXT_BOX_EDGE "text-box-edge" inherited,
    TEXT_BOX_TRIM "text-box-trim",
    TEXT_ANCHOR "text-anchor" inherited,
    TEXT_DECORATION "text-decoration",
    TEXT_DECORATION_COLOR "text-decoration-color" inherited,
    TEXT_DECORATION_LINE "text-decoration-line" inherited,
    TEXT_DECORATION_SKIP_INK "text-decoration-skip-ink" inherited,
    TEXT_DECORATION_STYLE "text-decoration-style" inherited,
    TEXT_DECORATION_THICKNESS "text-decoration-thickness" inherited,
    TEXT_EMPHASIS "text-emphasis" inherited,
    TEXT_INDENT "text-indent" inherited,
    TEXT_ORIENTATION "text-orientation" inherited,
    TEXT_OVERFLOW "text-overflow",
    TEXT_RENDERING "text-rendering" inherited,
    TEXT_SHADOW "text-shadow" inherited,
    TEXT_SIZE_ADJUST "text-size-adjust" inherited,
    TEXT_TRANSFORM "text-transform" inherited,
    TEXT_UNDERLINE_OFFSET "text-underline-offset" inherited,
    TEXT_UNDERLINE_POSITION "text-underline-position" inherited,
    TEXT_WRAP "text-wrap" inherited,
    TEXT_WRAP_MODE "text-wrap-mode" inherited,
    TEXT_WRAP_STYLE "text-wrap-style" inherited,
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
    WHITE_SPACE_COLLAPSE "white-space-collapse" inherited,
    WIDOWS "widows" inherited,
    WIDTH "width",
    WILL_CHANGE "will-change",
    WORD_BREAK "word-break" inherited,
    WORD_SPACING "word-spacing" inherited,
    WORD_WRAP "word-wrap" inherited,
    WRITING_MODE "writing-mode" inherited,
    X "x",
    Y "y",
    Z_INDEX "z-index",
    ZOOM "zoom",
}

thread_local! {
    // WARNING: interned names are leaked. They are bounded by the property names that the app's CSS uses, and by
    // the custom property names that scripts set inline; inline styles drop other unknown names.
    static INTERNED: RefCell<(Vec<&'static str>, FxHashMap<&'static str, Prop>)> = Default::default();
}

impl Prop {
    pub(crate) fn new(name: &str) -> Self {
        known(name).unwrap_or_else(|| {
            INTERNED.with(|interned| {
                let (names, ids) = &mut *interned.borrow_mut();
                if let Some(&prop) = ids.get(name) {
                    return prop;
                }
                // WARNING: ids are per thread, so the leaked names are shared across threads to leak each name only
                // once.
                static LEAKED: std::sync::Mutex<rustc_hash::FxHashSet<&'static str>> =
                    std::sync::Mutex::new(rustc_hash::FxHashSet::with_hasher(
                        rustc_hash::FxBuildHasher,
                    ));
                let name: &'static str = {
                    let mut leaked = LEAKED
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match leaked.get(name) {
                        Some(&name) => name,
                        None => {
                            let name: &'static str = Box::leak(name.into());
                            leaked.insert(name);
                            name
                        }
                    }
                };
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

    pub(crate) fn is_custom(self) -> bool {
        self.0 & CUSTOM != 0
    }

    pub(crate) fn is_known(self) -> bool {
        (self.0 as usize) < NAMES.len()
    }

    pub(crate) fn index(self) -> Option<usize> {
        self.is_known().then_some(self.0 as usize)
    }

    pub(crate) fn names() -> &'static [&'static str] {
        &NAMES
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

pub(crate) type Vars = Rc<Vec<(Prop, Rc<str>)>>;

const WORDS: usize = (Known::Count as usize).div_ceil(64);

#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Style {
    entries: Vec<(Prop, Rc<str>)>,
    vars: Vars,
    known: [u64; WORDS],
    before: [u16; WORDS],
}

impl Style {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn position(&self, prop: Prop) -> Result<usize, usize> {
        let id = prop.0 as usize;
        if prop.is_custom() {
            return self.vars.binary_search_by_key(&prop, |entry| entry.0);
        }
        if id >= NAMES.len() {
            return self.entries.binary_search_by_key(&prop, |entry| entry.0);
        }
        let (word, bit) = (id / 64, id % 64);
        let index = self.before[word] as usize
            + (self.known[word] & ((1 << bit) - 1)).count_ones() as usize;
        if self.known[word] >> bit & 1 == 1 {
            Ok(index)
        } else {
            Err(index)
        }
    }

    fn count(&mut self, prop: Prop, delta: i16) {
        let id = prop.0 as usize;
        if id < NAMES.len() {
            self.known[id / 64] ^= 1 << (id % 64);
            for before in &mut self.before[id / 64 + 1..] {
                *before = before.wrapping_add_signed(delta);
            }
        }
    }

    fn index(&mut self) {
        self.known = [0; WORDS];
        for (prop, _) in &self.entries {
            if let Some(word) = self.known.get_mut(prop.0 as usize / 64)
                && (prop.0 as usize) < NAMES.len()
            {
                *word |= 1 << (prop.0 % 64);
            }
        }
        let mut count = 0;
        for (before, word) in self.before.iter_mut().zip(self.known) {
            *before = count;
            count += word.count_ones() as u16;
        }
    }

    pub(crate) fn get(&self, prop: Prop) -> Option<&str> {
        self.value(prop).map(|value| &**value)
    }

    pub(crate) fn value(&self, prop: Prop) -> Option<&Rc<str>> {
        let index = self.position(prop).ok()?;
        Some(if prop.is_custom() {
            &self.vars[index].1
        } else {
            &self.entries[index].1
        })
    }

    pub(crate) fn contains(&self, prop: Prop) -> bool {
        self.position(prop).is_ok()
    }

    pub(crate) fn insert(&mut self, prop: Prop, value: impl Into<Rc<str>>) {
        let value = value.into();
        match (self.position(prop), prop.is_custom()) {
            (Ok(index), true) => Rc::make_mut(&mut self.vars)[index].1 = value,
            (Err(index), true) => Rc::make_mut(&mut self.vars).insert(index, (prop, value)),
            (Ok(index), false) => self.entries[index].1 = value,
            (Err(index), false) => {
                self.entries.insert(index, (prop, value));
                self.count(prop, 1);
            }
        }
    }

    pub(crate) fn remove(&mut self, prop: Prop) -> Option<Rc<str>> {
        let index = self.position(prop).ok()?;
        if prop.is_custom() {
            return Some(Rc::make_mut(&mut self.vars).remove(index).1);
        }
        self.count(prop, -1);
        Some(self.entries.remove(index).1)
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(Prop, &mut Rc<str>) -> bool) {
        let len = self.entries.len();
        self.entries.retain_mut(|(prop, value)| keep(*prop, value));
        if self.entries.len() != len {
            self.index();
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (Prop, &Rc<str>)> {
        self.entries
            .iter()
            .chain(self.vars.iter())
            .map(|(prop, value)| (*prop, value))
    }

    pub(crate) fn own(&self) -> impl Iterator<Item = (Prop, &Rc<str>)> {
        self.entries.iter().map(|(prop, value)| (*prop, value))
    }

    pub(crate) fn vars(&self) -> &Vars {
        &self.vars
    }

    pub(crate) fn set_vars(&mut self, vars: Vars) {
        self.vars = vars;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.vars.is_empty()
    }
}

impl FromIterator<(Prop, Rc<str>)> for Style {
    fn from_iter<I: IntoIterator<Item = (Prop, Rc<str>)>>(entries: I) -> Self {
        let mut style = Self::default();
        style.extend(entries);
        style
    }
}

impl Extend<(Prop, Rc<str>)> for Style {
    fn extend<I: IntoIterator<Item = (Prop, Rc<str>)>>(&mut self, entries: I) {
        let start = self.entries.len();
        let mut custom = Vec::new();
        for entry in entries {
            if entry.0.is_custom() {
                custom.push(entry);
            } else {
                self.entries.push(entry);
            }
        }
        if self.entries.len() != start {
            merge(&mut self.entries, start);
            self.index();
        }
        if !custom.is_empty() {
            let list = Rc::make_mut(&mut self.vars);
            let start = list.len();
            list.extend(custom);
            merge(list, start);
        }
    }
}

fn merge(list: &mut Vec<(Prop, Rc<str>)>, start: usize) {
    if list[start..].is_sorted_by(|a, b| a.0 < b.0)
        && list[..start]
            .last()
            .zip(list.get(start))
            .is_none_or(|(a, b)| a.0 < b.0)
    {
        return;
    }
    // WARNING: the sort must stay stable, so reverse, dedup and reverse keeps the later of two values for one property.
    list.sort_by_key(|entry| entry.0);
    list.reverse();
    list.dedup_by_key(|entry| entry.0);
    list.reverse();
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
