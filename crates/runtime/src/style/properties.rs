use rustc_hash::FxHashMap;
use std::{cell::RefCell, marker::PhantomData, rc::Rc};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Prop(u32, PhantomData<*const ()>);

const CUSTOM: u32 = 1 << 31;

macro_rules! props {
    ($($id:ident $name:literal $($inherited:ident)? $($grammar:literal)?,)*) => {
        #[allow(dead_code, non_camel_case_types, clippy::upper_case_acronyms)]
        enum Known { $($id,)* Count }
        #[allow(dead_code)]
        impl Prop {
            $(pub(crate) const $id: Prop = Prop(Known::$id as u32, PhantomData);)*
        }
        const NAMES: [&str; Known::Count as usize] = [$($name,)*];
        const INHERITED: [bool; Known::Count as usize] = [$(props!(@inherited $($inherited)?),)*];
        const GRAMMARS: [Option<&str>; Known::Count as usize] = [$(props!(@grammar $($grammar)?),)*];
    };
    (@inherited inherited) => { true };
    (@inherited) => { false };
    (@grammar $grammar:literal) => { Some($grammar) };
    (@grammar) => { None };
}

fn known(name: &str) -> Option<Prop> {
    NAMES
        .binary_search(&name)
        .ok()
        .map(|index| Prop(index as u32, PhantomData))
}

// Keep sorted: lookups binary-search the names. Only properties that apply have a grammar, which `@supports` and the
// parser check values against.
props! {
    MACOS_GLASS "-macos-glass" "none|regular|clear",
    MACOS_GLASS_INTERACTIVE "-macos-glass-interactive" "false|true",
    MACOS_GLASS_TINT_COLOR "-macos-glass-tint-color" "<color>",
    MOZ_OSX_FONT_SMOOTHING "-moz-osx-font-smoothing" inherited "auto|grayscale",
    WEBKIT_APP_REGION "-webkit-app-region" inherited "none|drag|no-drag",
    WEBKIT_APPEARANCE "-webkit-appearance",
    WEBKIT_BACKDROP_FILTER "-webkit-backdrop-filter" "none|[blur(|brightness(|contrast(|drop-shadow(|grayscale(|hue-rotate(|invert(|opacity(|saturate(|sepia(|url(]+",
    WEBKIT_BACKGROUND_CLIP "-webkit-background-clip" "[border-box|padding-box|content-box|text]#",
    WEBKIT_BOX_ORIENT "-webkit-box-orient" "horizontal|vertical|inline-axis|block-axis",
    WEBKIT_CLIP_PATH "-webkit-clip-path" "none|[inset(|circle(|ellipse(|polygon(|path(|url(|border-box|padding-box|content-box|margin-box|fill-box|stroke-box|view-box]+",
    WEBKIT_FILTER "-webkit-filter" "none|[blur(|brightness(|contrast(|drop-shadow(|grayscale(|hue-rotate(|invert(|opacity(|saturate(|sepia(|url(]+",
    WEBKIT_FONT_SMOOTHING "-webkit-font-smoothing" inherited "auto|none|antialiased|subpixel-antialiased",
    WEBKIT_LINE_CLAMP "-webkit-line-clamp" "none|<integer>",
    WEBKIT_MASK "-webkit-mask" "*",
    WEBKIT_MASK_IMAGE "-webkit-mask-image" "[none|<image>]#",
    WEBKIT_MASK_POSITION "-webkit-mask-position" "*",
    WEBKIT_MASK_REPEAT "-webkit-mask-repeat" "[repeat|repeat-x|repeat-y|no-repeat|space|round]#",
    WEBKIT_MASK_SIZE "-webkit-mask-size" "*",
    WEBKIT_TAP_HIGHLIGHT_COLOR "-webkit-tap-highlight-color" inherited "<color>",
    WEBKIT_TEXT_FILL_COLOR "-webkit-text-fill-color" inherited "<color>",
    WEBKIT_TEXT_STROKE "-webkit-text-stroke" inherited,
    WEBKIT_TEXT_STROKE_COLOR "-webkit-text-stroke-color" inherited,
    WEBKIT_TEXT_STROKE_WIDTH "-webkit-text-stroke-width" inherited,
    WEBKIT_USER_SELECT "-webkit-user-select" "auto|none|text|all|contain",
    ACCENT_COLOR "accent-color" inherited "auto|<color>",
    ALIGN_CONTENT "align-content" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ALIGN_ITEMS "align-items" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ALIGN_SELF "align-self" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    ALL "all" "*",
    ANIMATION "animation" "*",
    ANIMATION_COMPOSITION "animation-composition",
    ANIMATION_DELAY "animation-delay" "[<time>]#",
    ANIMATION_DIRECTION "animation-direction" "[normal|reverse|alternate|alternate-reverse]#",
    ANIMATION_DURATION "animation-duration" "[auto|<time>]#",
    ANIMATION_FILL_MODE "animation-fill-mode" "[none|forwards|backwards|both]#",
    ANIMATION_ITERATION_COUNT "animation-iteration-count" "[infinite|<number>]#",
    ANIMATION_NAME "animation-name" "[none|<custom-ident>|<string>]#",
    ANIMATION_PLAY_STATE "animation-play-state" "[running|paused]#",
    ANIMATION_TIMING_FUNCTION "animation-timing-function" "[<easing-function>]#",
    APP_REGION "app-region" inherited "none|drag|no-drag",
    APPEARANCE "appearance" "none|auto|menulist-button|textfield|button|checkbox|radio|searchfield|textarea|menulist|listbox|meter|progress-bar|push-button|square-button|slider-horizontal|base-select",
    ASPECT_RATIO "aspect-ratio" "*",
    BACKDROP_FILTER "backdrop-filter" "none|[blur(|brightness(|contrast(|drop-shadow(|grayscale(|hue-rotate(|invert(|opacity(|saturate(|sepia(|url(]+",
    BACKFACE_VISIBILITY "backface-visibility" "visible|hidden",
    BACKGROUND "background" "*",
    BACKGROUND_ATTACHMENT "background-attachment",
    BACKGROUND_BLEND_MODE "background-blend-mode" "[normal|multiply|screen|overlay|darken|lighten|color-dodge|color-burn|hard-light|soft-light|difference|exclusion|hue|saturation|color|luminosity|plus-lighter]#",
    BACKGROUND_CLIP "background-clip" "[border-box|padding-box|content-box|text]#",
    BACKGROUND_COLOR "background-color" "<color>",
    BACKGROUND_IMAGE "background-image" "[none|<image>]#",
    BACKGROUND_ORIGIN "background-origin",
    BACKGROUND_POSITION "background-position" "*",
    BACKGROUND_POSITION_X "background-position-x",
    BACKGROUND_POSITION_Y "background-position-y",
    BACKGROUND_REPEAT "background-repeat" "*",
    BACKGROUND_SIZE "background-size" "*",
    BLOCK_SIZE "block-size" "auto|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    BORDER "border" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_BLOCK "border-block" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_BLOCK_COLOR "border-block-color" "[<color>]{1,2}",
    BORDER_BLOCK_END "border-block-end" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_BLOCK_END_COLOR "border-block-end-color" "<color>",
    BORDER_BLOCK_END_STYLE "border-block-end-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    BORDER_BLOCK_END_WIDTH "border-block-end-width" "thin|medium|thick|<non-negative-length>",
    BORDER_BLOCK_START "border-block-start" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_BLOCK_START_COLOR "border-block-start-color" "<color>",
    BORDER_BLOCK_START_STYLE "border-block-start-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    BORDER_BLOCK_START_WIDTH "border-block-start-width" "thin|medium|thick|<non-negative-length>",
    BORDER_BLOCK_STYLE "border-block-style" "[none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset]{1,2}",
    BORDER_BLOCK_WIDTH "border-block-width" "[thin|medium|thick|<non-negative-length>]{1,2}",
    BORDER_BOTTOM "border-bottom" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_BOTTOM_COLOR "border-bottom-color" "<color>",
    BORDER_BOTTOM_LEFT_RADIUS "border-bottom-left-radius" "[<non-negative-length-percentage>]{1,2}",
    BORDER_BOTTOM_RIGHT_RADIUS "border-bottom-right-radius" "[<non-negative-length-percentage>]{1,2}",
    BORDER_BOTTOM_STYLE "border-bottom-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    BORDER_BOTTOM_WIDTH "border-bottom-width" "thin|medium|thick|<non-negative-length>",
    BORDER_COLLAPSE "border-collapse" inherited "separate|collapse",
    BORDER_COLOR "border-color" "[<color>]{1,4}",
    BORDER_END_END_RADIUS "border-end-end-radius" "[<non-negative-length-percentage>]{1,2}",
    BORDER_END_START_RADIUS "border-end-start-radius" "[<non-negative-length-percentage>]{1,2}",
    BORDER_IMAGE "border-image",
    BORDER_INLINE "border-inline" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_INLINE_COLOR "border-inline-color" "[<color>]{1,2}",
    BORDER_INLINE_END "border-inline-end" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_INLINE_END_COLOR "border-inline-end-color" "<color>",
    BORDER_INLINE_END_STYLE "border-inline-end-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    BORDER_INLINE_END_WIDTH "border-inline-end-width" "thin|medium|thick|<non-negative-length>",
    BORDER_INLINE_START "border-inline-start" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_INLINE_START_COLOR "border-inline-start-color" "<color>",
    BORDER_INLINE_START_STYLE "border-inline-start-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    BORDER_INLINE_START_WIDTH "border-inline-start-width" "thin|medium|thick|<non-negative-length>",
    BORDER_INLINE_STYLE "border-inline-style" "[none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset]{1,2}",
    BORDER_INLINE_WIDTH "border-inline-width" "[thin|medium|thick|<non-negative-length>]{1,2}",
    BORDER_LEFT "border-left" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_LEFT_COLOR "border-left-color" "<color>",
    BORDER_LEFT_STYLE "border-left-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    BORDER_LEFT_WIDTH "border-left-width" "thin|medium|thick|<non-negative-length>",
    BORDER_RADIUS "border-radius" "*",
    BORDER_RIGHT "border-right" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_RIGHT_COLOR "border-right-color" "<color>",
    BORDER_RIGHT_STYLE "border-right-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    BORDER_RIGHT_WIDTH "border-right-width" "thin|medium|thick|<non-negative-length>",
    BORDER_SPACING "border-spacing" inherited "[<non-negative-length>]{1,2}",
    BORDER_START_END_RADIUS "border-start-end-radius" "[<non-negative-length-percentage>]{1,2}",
    BORDER_START_START_RADIUS "border-start-start-radius" "[<non-negative-length-percentage>]{1,2}",
    BORDER_STYLE "border-style" "[none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset]{1,4}",
    BORDER_TOP "border-top" "[thin|medium|thick|<non-negative-length>|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    BORDER_TOP_COLOR "border-top-color" "<color>",
    BORDER_TOP_LEFT_RADIUS "border-top-left-radius" "[<non-negative-length-percentage>]{1,2}",
    BORDER_TOP_RIGHT_RADIUS "border-top-right-radius" "[<non-negative-length-percentage>]{1,2}",
    BORDER_TOP_STYLE "border-top-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    BORDER_TOP_WIDTH "border-top-width" "thin|medium|thick|<non-negative-length>",
    BORDER_WIDTH "border-width" "[thin|medium|thick|<non-negative-length>]{1,4}",
    BOTTOM "bottom" "auto|<length-percentage>",
    BOX_DECORATION_BREAK "box-decoration-break" "slice|clone",
    BOX_SHADOW "box-shadow" "*",
    BOX_SIZING "box-sizing" "content-box|border-box",
    BREAK_AFTER "break-after",
    BREAK_BEFORE "break-before",
    BREAK_INSIDE "break-inside",
    CAPTION_SIDE "caption-side" inherited,
    CARET_COLOR "caret-color" inherited "auto|<color>",
    CLEAR "clear" "none|left|right|both|inline-start|inline-end",
    CLIP "clip",
    CLIP_PATH "clip-path" "none|[inset(|circle(|ellipse(|polygon(|path(|url(|border-box|padding-box|content-box|margin-box|fill-box|stroke-box|view-box]+",
    CLIP_RULE "clip-rule" inherited,
    COLOR "color" inherited "<color>",
    COLOR_SCHEME "color-scheme" inherited "normal|[light|dark|only]+",
    COLUMN_COUNT "column-count" "auto|<integer>",
    COLUMN_FILL "column-fill",
    COLUMN_GAP "column-gap" "normal|<non-negative-length-percentage>",
    COLUMN_RULE "column-rule" "*",
    COLUMN_RULE_COLOR "column-rule-color" "<color>",
    COLUMN_RULE_STYLE "column-rule-style" "none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    COLUMN_RULE_WIDTH "column-rule-width" "thin|medium|thick|<non-negative-length>",
    COLUMN_SPAN "column-span",
    COLUMN_WIDTH "column-width" "auto|<non-negative-length>",
    COLUMNS "columns" "*",
    CONTAIN "contain",
    CONTAINER "container" "*",
    CONTAINER_NAME "container-name" "none|[<custom-ident>]+",
    CONTAINER_TYPE "container-type" "normal|size|inline-size",
    CONTENT "content" "*",
    CONTENT_VISIBILITY "content-visibility",
    COUNTER_INCREMENT "counter-increment" "*",
    COUNTER_RESET "counter-reset" "*",
    COUNTER_SET "counter-set" "*",
    CURSOR "cursor" inherited "*",
    CX "cx",
    CY "cy",
    DIRECTION "direction" inherited "ltr|rtl",
    DISPLAY "display" "none|block|inline|inline-block|flex|inline-flex|grid|inline-grid|contents|list-item|flow-root|table|inline-table|table-row-group|table-header-group|table-footer-group|table-row|table-cell|table-column-group|table-column|table-caption|ruby|ruby-text|-webkit-box|-webkit-inline-box",
    EMPTY_CELLS "empty-cells" inherited,
    FIELD_SIZING "field-sizing" "fixed|content",
    FILL "fill" inherited "*",
    FILL_OPACITY "fill-opacity" inherited "<number>|<percentage>",
    FILL_RULE "fill-rule" inherited "nonzero|evenodd",
    FILTER "filter" "none|[blur(|brightness(|contrast(|drop-shadow(|grayscale(|hue-rotate(|invert(|opacity(|saturate(|sepia(|url(]+",
    FLEX "flex" "none|[<non-negative-number>|content|auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available]{1,3}",
    FLEX_BASIS "flex-basis" "content|auto|<length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    FLEX_DIRECTION "flex-direction" "row|row-reverse|column|column-reverse",
    FLEX_FLOW "flex-flow" "[row|row-reverse|column|column-reverse|nowrap|wrap|wrap-reverse]+",
    FLEX_GROW "flex-grow" "<non-negative-number>",
    FLEX_SHRINK "flex-shrink" "<non-negative-number>",
    FLEX_WRAP "flex-wrap" "nowrap|wrap|wrap-reverse",
    FLOAT "float" "none|left|right|inline-start|inline-end",
    FLOOD_COLOR "flood-color",
    FLOOD_OPACITY "flood-opacity",
    FONT "font" "*",
    FONT_DISPLAY "font-display",
    FONT_FAMILY "font-family" inherited "*",
    FONT_FEATURE_SETTINGS "font-feature-settings" inherited "*",
    FONT_KERNING "font-kerning" inherited,
    FONT_OPTICAL_SIZING "font-optical-sizing" inherited,
    FONT_SIZE "font-size" inherited "xx-small|x-small|small|medium|large|x-large|xx-large|xxx-large|larger|smaller|<non-negative-length-percentage>",
    FONT_SIZE_ADJUST "font-size-adjust" inherited,
    FONT_STRETCH "font-stretch" inherited "normal|ultra-condensed|extra-condensed|condensed|semi-condensed|semi-expanded|expanded|extra-expanded|ultra-expanded|<percentage>",
    FONT_STYLE "font-style" inherited "normal|italic|oblique|*",
    FONT_SYNTHESIS "font-synthesis" inherited,
    FONT_VARIANT "font-variant" inherited "normal",
    FONT_VARIANT_LIGATURES "font-variant-ligatures" inherited,
    FONT_VARIANT_NUMERIC "font-variant-numeric" inherited,
    FONT_VARIATION_SETTINGS "font-variation-settings" inherited "*",
    FONT_WEIGHT "font-weight" inherited "normal|bold|bolder|lighter|<number>",
    FORCED_COLOR_ADJUST "forced-color-adjust" inherited,
    GAP "gap" "[normal|<non-negative-length-percentage>]{1,2}",
    GRID "grid" "*",
    GRID_AREA "grid-area" "*",
    GRID_AUTO_COLUMNS "grid-auto-columns" "*",
    GRID_AUTO_FLOW "grid-auto-flow" "[row|column|dense]+",
    GRID_AUTO_ROWS "grid-auto-rows" "*",
    GRID_COLUMN "grid-column" "*",
    GRID_COLUMN_END "grid-column-end" "*",
    GRID_COLUMN_GAP "grid-column-gap" "normal|<non-negative-length-percentage>",
    GRID_COLUMN_START "grid-column-start" "*",
    GRID_GAP "grid-gap" "[normal|<non-negative-length-percentage>]{1,2}",
    GRID_ROW "grid-row" "*",
    GRID_ROW_END "grid-row-end" "*",
    GRID_ROW_GAP "grid-row-gap" "normal|<non-negative-length-percentage>",
    GRID_ROW_START "grid-row-start" "*",
    GRID_TEMPLATE "grid-template" "*",
    GRID_TEMPLATE_AREAS "grid-template-areas" "*",
    GRID_TEMPLATE_COLUMNS "grid-template-columns" "*",
    GRID_TEMPLATE_ROWS "grid-template-rows" "*",
    HEIGHT "height" "auto|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    HYPHENS "hyphens" inherited,
    IMAGE_RENDERING "image-rendering" inherited,
    INLINE_SIZE "inline-size" "auto|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    INSET "inset" "[auto|<length-percentage>]{1,4}",
    INSET_BLOCK "inset-block" "[auto|<length-percentage>]{1,2}",
    INSET_BLOCK_END "inset-block-end" "auto|<length-percentage>",
    INSET_BLOCK_START "inset-block-start" "auto|<length-percentage>",
    INSET_INLINE "inset-inline" "[auto|<length-percentage>]{1,2}",
    INSET_INLINE_END "inset-inline-end" "auto|<length-percentage>",
    INSET_INLINE_START "inset-inline-start" "auto|<length-percentage>",
    ISOLATION "isolation" "auto|isolate",
    JUSTIFY_CONTENT "justify-content" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    JUSTIFY_ITEMS "justify-items" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    JUSTIFY_SELF "justify-self" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    LEFT "left" "auto|<length-percentage>",
    LETTER_SPACING "letter-spacing" inherited "normal|<length-percentage>",
    LIGHTING_COLOR "lighting-color",
    LINE_BREAK "line-break" inherited,
    LINE_CLAMP "line-clamp" "none|<integer>|*",
    LINE_HEIGHT "line-height" inherited "normal|<non-negative-number>|<non-negative-length-percentage>",
    LIST_STYLE "list-style" "*",
    LIST_STYLE_IMAGE "list-style-image" inherited "none",
    LIST_STYLE_POSITION "list-style-position" inherited "inside|outside",
    LIST_STYLE_TYPE "list-style-type" inherited "*",
    MARGIN "margin" "[auto|<length-percentage>]{1,4}",
    MARGIN_BLOCK "margin-block" "[auto|<length-percentage>]{1,2}",
    MARGIN_BLOCK_END "margin-block-end" "auto|<length-percentage>",
    MARGIN_BLOCK_START "margin-block-start" "auto|<length-percentage>",
    MARGIN_BOTTOM "margin-bottom" "auto|<length-percentage>",
    MARGIN_INLINE "margin-inline" "[auto|<length-percentage>]{1,2}",
    MARGIN_INLINE_END "margin-inline-end" "auto|<length-percentage>",
    MARGIN_INLINE_START "margin-inline-start" "auto|<length-percentage>",
    MARGIN_LEFT "margin-left" "auto|<length-percentage>",
    MARGIN_RIGHT "margin-right" "auto|<length-percentage>",
    MARGIN_TOP "margin-top" "auto|<length-percentage>",
    MARKER "marker" inherited,
    MASK "mask" "*",
    MASK_CLIP "mask-clip",
    MASK_COMPOSITE "mask-composite",
    MASK_IMAGE "mask-image" "[none|<image>]#",
    MASK_MODE "mask-mode",
    MASK_ORIGIN "mask-origin",
    MASK_POSITION "mask-position" "*",
    MASK_REPEAT "mask-repeat" "[repeat|repeat-x|repeat-y|no-repeat|space|round]#",
    MASK_SIZE "mask-size" "*",
    MASK_TYPE "mask-type",
    MAX_BLOCK_SIZE "max-block-size" "none|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available",
    MAX_HEIGHT "max-height" "none|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available",
    MAX_INLINE_SIZE "max-inline-size" "none|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available",
    MAX_WIDTH "max-width" "none|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available",
    MIN_BLOCK_SIZE "min-block-size" "auto|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    MIN_HEIGHT "min-height" "auto|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    MIN_INLINE_SIZE "min-inline-size" "auto|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    MIN_WIDTH "min-width" "auto|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    MIX_BLEND_MODE "mix-blend-mode" "[normal|multiply|screen|overlay|darken|lighten|color-dodge|color-burn|hard-light|soft-light|difference|exclusion|hue|saturation|color|luminosity|plus-lighter]#",
    OBJECT_FIT "object-fit" "fill|contain|cover|none|scale-down",
    OBJECT_POSITION "object-position",
    OFFSET "offset",
    OPACITY "opacity" "<number>|<percentage>",
    ORDER "order" "<integer>",
    ORPHANS "orphans" inherited,
    OUTLINE "outline" "[thin|medium|thick|<non-negative-length>|auto|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset|<color>]{1,3}",
    OUTLINE_COLOR "outline-color" "auto|<color>",
    OUTLINE_OFFSET "outline-offset" "<length>",
    OUTLINE_STYLE "outline-style" "auto|none|hidden|dotted|dashed|solid|double|groove|ridge|inset|outset",
    OUTLINE_WIDTH "outline-width" "thin|medium|thick|<non-negative-length>",
    OVERFLOW "overflow" "[visible|hidden|clip|scroll|auto|overlay]{1,2}",
    OVERFLOW_ANCHOR "overflow-anchor",
    OVERFLOW_CLIP_MARGIN "overflow-clip-margin",
    OVERFLOW_WRAP "overflow-wrap" inherited "normal|break-word|anywhere",
    OVERFLOW_X "overflow-x" "visible|hidden|clip|scroll|auto|overlay",
    OVERFLOW_Y "overflow-y" "visible|hidden|clip|scroll|auto|overlay",
    OVERLAY "overlay" "none|auto",
    OVERSCROLL_BEHAVIOR "overscroll-behavior" "[auto|contain|none]{1,2}",
    OVERSCROLL_BEHAVIOR_BLOCK "overscroll-behavior-block" "auto|contain|none",
    OVERSCROLL_BEHAVIOR_INLINE "overscroll-behavior-inline" "auto|contain|none",
    OVERSCROLL_BEHAVIOR_X "overscroll-behavior-x" "auto|contain|none",
    OVERSCROLL_BEHAVIOR_Y "overscroll-behavior-y" "auto|contain|none",
    PADDING "padding" "[<non-negative-length-percentage>]{1,4}",
    PADDING_BLOCK "padding-block" "[<non-negative-length-percentage>]{1,2}",
    PADDING_BLOCK_END "padding-block-end" "<non-negative-length-percentage>",
    PADDING_BLOCK_START "padding-block-start" "<non-negative-length-percentage>",
    PADDING_BOTTOM "padding-bottom" "<non-negative-length-percentage>",
    PADDING_INLINE "padding-inline" "[<non-negative-length-percentage>]{1,2}",
    PADDING_INLINE_END "padding-inline-end" "<non-negative-length-percentage>",
    PADDING_INLINE_START "padding-inline-start" "<non-negative-length-percentage>",
    PADDING_LEFT "padding-left" "<non-negative-length-percentage>",
    PADDING_RIGHT "padding-right" "<non-negative-length-percentage>",
    PADDING_TOP "padding-top" "<non-negative-length-percentage>",
    PAGE_BREAK_AFTER "page-break-after",
    PAGE_BREAK_BEFORE "page-break-before",
    PAGE_BREAK_INSIDE "page-break-inside",
    PAINT_ORDER "paint-order" inherited,
    PERSPECTIVE "perspective" "none|<non-negative-length>",
    PERSPECTIVE_ORIGIN "perspective-origin" "*",
    PLACE_CONTENT "place-content" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    PLACE_ITEMS "place-items" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    PLACE_SELF "place-self" "[normal|stretch|baseline|first|last|center|start|end|self-start|self-end|flex-start|flex-end|left|right|safe|unsafe|space-between|space-around|space-evenly|legacy|auto|anchor-center]+",
    POINTER_EVENTS "pointer-events" inherited "auto|none|visiblepainted|visiblefill|visiblestroke|visible|painted|fill|stroke|all",
    POSITION "position" "static|relative|absolute|fixed|sticky",
    PRINT_COLOR_ADJUST "print-color-adjust" inherited,
    QUOTES "quotes" inherited,
    R "r",
    RESIZE "resize" "none|both|horizontal|vertical|block|inline",
    RIGHT "right" "auto|<length-percentage>",
    ROTATE "rotate" "*",
    ROW_GAP "row-gap" "normal|<non-negative-length-percentage>",
    RX "rx",
    RY "ry",
    SCALE "scale" "*",
    SCROLL_BEHAVIOR "scroll-behavior" "auto|smooth",
    SCROLL_MARGIN "scroll-margin" "[<length>]{1,4}",
    SCROLL_MARGIN_BOTTOM "scroll-margin-bottom",
    SCROLL_MARGIN_LEFT "scroll-margin-left",
    SCROLL_MARGIN_RIGHT "scroll-margin-right",
    SCROLL_MARGIN_TOP "scroll-margin-top",
    SCROLL_PADDING "scroll-padding" "[auto|<non-negative-length-percentage>]{1,4}",
    SCROLL_PADDING_BOTTOM "scroll-padding-bottom",
    SCROLL_PADDING_LEFT "scroll-padding-left",
    SCROLL_PADDING_RIGHT "scroll-padding-right",
    SCROLL_PADDING_TOP "scroll-padding-top",
    SCROLL_SNAP_ALIGN "scroll-snap-align",
    SCROLL_SNAP_STOP "scroll-snap-stop",
    SCROLL_SNAP_TYPE "scroll-snap-type",
    SCROLLBAR_COLOR "scrollbar-color" inherited "auto|[<color>]+",
    SCROLLBAR_GUTTER "scrollbar-gutter" "auto|stable|[stable|both-edges]+",
    SCROLLBAR_WIDTH "scrollbar-width" "auto|thin|none",
    SHAPE_OUTSIDE "shape-outside",
    SHAPE_RENDERING "shape-rendering" inherited,
    SPEAK "speak" inherited,
    STOP_COLOR "stop-color",
    STOP_OPACITY "stop-opacity",
    STROKE "stroke" inherited "*",
    STROKE_DASHARRAY "stroke-dasharray" inherited,
    STROKE_DASHOFFSET "stroke-dashoffset" inherited,
    STROKE_LINECAP "stroke-linecap" inherited "butt|round|square",
    STROKE_LINEJOIN "stroke-linejoin" inherited "miter|round|bevel|miter-clip|arcs",
    STROKE_MITERLIMIT "stroke-miterlimit" inherited,
    STROKE_OPACITY "stroke-opacity" inherited "<number>|<percentage>",
    STROKE_WIDTH "stroke-width" inherited "<length-percentage>|<number>",
    TAB_SIZE "tab-size" inherited,
    TABLE_LAYOUT "table-layout",
    TEXT_ALIGN "text-align" inherited "start|end|left|right|center|justify|match-parent|-webkit-left|-webkit-right|-webkit-center|-internal-center",
    TEXT_ALIGN_LAST "text-align-last" inherited "auto|start|end|left|right|center|justify|match-parent",
    TEXT_ANCHOR "text-anchor" inherited,
    TEXT_BOX "text-box" "*",
    TEXT_BOX_EDGE "text-box-edge" inherited "*",
    TEXT_BOX_TRIM "text-box-trim" "none|trim-start|trim-end|trim-both",
    TEXT_DECORATION "text-decoration" "*",
    TEXT_DECORATION_COLOR "text-decoration-color" inherited "<color>",
    TEXT_DECORATION_LINE "text-decoration-line" inherited "none|[underline|overline|line-through|blink]+",
    TEXT_DECORATION_SKIP_INK "text-decoration-skip-ink" inherited,
    TEXT_DECORATION_STYLE "text-decoration-style" inherited "solid|double|dotted|dashed|wavy",
    TEXT_DECORATION_THICKNESS "text-decoration-thickness" inherited "auto|from-font|<length-percentage>",
    TEXT_EMPHASIS "text-emphasis" inherited,
    TEXT_INDENT "text-indent" inherited "*",
    TEXT_ORIENTATION "text-orientation" inherited,
    TEXT_OVERFLOW "text-overflow" "clip|ellipsis",
    TEXT_RENDERING "text-rendering" inherited,
    TEXT_SHADOW "text-shadow" inherited "*",
    TEXT_SIZE_ADJUST "text-size-adjust" inherited,
    TEXT_TRANSFORM "text-transform" inherited "none|capitalize|uppercase|lowercase|full-width|full-size-kana",
    TEXT_UNDERLINE_OFFSET "text-underline-offset" inherited "auto|<length-percentage>",
    TEXT_UNDERLINE_POSITION "text-underline-position" inherited,
    TEXT_WRAP "text-wrap" inherited "[wrap|nowrap|balance|pretty|stable|auto]+",
    TEXT_WRAP_MODE "text-wrap-mode" inherited,
    TEXT_WRAP_STYLE "text-wrap-style" inherited,
    TOP "top" "auto|<length-percentage>",
    TOUCH_ACTION "touch-action",
    TRANSFORM "transform" "none|[matrix(|matrix3d(|translate(|translate3d(|translatex(|translatey(|translatez(|scale(|scale3d(|scalex(|scaley(|scalez(|rotate(|rotate3d(|rotatex(|rotatey(|rotatez(|skew(|skewx(|skewy(|perspective(]+",
    TRANSFORM_BOX "transform-box",
    TRANSFORM_ORIGIN "transform-origin" "*",
    TRANSFORM_STYLE "transform-style" "flat|preserve-3d",
    TRANSITION "transition" "*",
    TRANSITION_BEHAVIOR "transition-behavior" "[normal|allow-discrete]#",
    TRANSITION_DELAY "transition-delay" "[<time>]#",
    TRANSITION_DURATION "transition-duration" "[<time>]#",
    TRANSITION_PROPERTY "transition-property" "*",
    TRANSITION_TIMING_FUNCTION "transition-timing-function" "[<easing-function>]#",
    TRANSLATE "translate" "*",
    UNICODE_BIDI "unicode-bidi" "normal|embed|isolate|bidi-override|isolate-override|plaintext",
    USER_SELECT "user-select" "auto|none|text|all|contain",
    VECTOR_EFFECT "vector-effect",
    VERTICAL_ALIGN "vertical-align" "baseline|sub|super|text-top|text-bottom|middle|top|bottom|<length-percentage>",
    VIEW_TRANSITION_NAME "view-transition-name",
    VISIBILITY "visibility" inherited "visible|hidden|collapse",
    WHITE_SPACE "white-space" inherited "normal|pre|nowrap|pre-wrap|pre-line|break-spaces",
    WHITE_SPACE_COLLAPSE "white-space-collapse" inherited,
    WIDOWS "widows" inherited,
    WIDTH "width" "auto|<non-negative-length-percentage>|min-content|max-content|fit-content|fit-content(|stretch|-webkit-fill-available|-webkit-min-content|-webkit-max-content|-webkit-fit-content",
    WILL_CHANGE "will-change" "*",
    WORD_BREAK "word-break" inherited "normal|break-all|break-word",
    WORD_SPACING "word-spacing" inherited "normal|<length>",
    WORD_WRAP "word-wrap" inherited "normal|break-word|anywhere",
    WRITING_MODE "writing-mode" inherited "horizontal-tb|vertical-rl|vertical-lr|sideways-rl|sideways-lr",
    X "x",
    Y "y",
    Z_INDEX "z-index" "auto|<integer>",
    ZOOM "zoom",
}

thread_local! {
    // Interned names never free. Accept only app CSS names and inline custom properties. Reject other unknown
    // inline names.
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
                // Share leaked names across threads even though property IDs are thread-local.
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

    pub(crate) fn grammar(self) -> Option<&'static str> {
        GRAMMARS.get(self.0 as usize).copied().flatten()
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
    // Preserve stable order and retain the last value for each property.
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

#[test]
fn names_are_sorted() {
    assert!(NAMES.is_sorted());
}
