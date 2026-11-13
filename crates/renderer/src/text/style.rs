use super::{Decoration, Features, Font, FontWeight};
use crate::{Color, Pixels, px};
use std::rc::Rc;

/// The first keyword is the initial value.
macro_rules! keywords {
    ($($name:ident { $first:ident $(, $rest:ident)* })*) => {$(
        #[derive(Clone, Copy, Debug, Default, PartialEq, Hash)]
        pub enum $name {
            #[default]
            $first,
            $($rest,)*
        }
    )*};
}

keywords! {
    FontStyle { Normal, Italic, Oblique }
    DecorationStyle { Solid, Double, Dotted, Dashed, Wavy }
    TextAlign { Left, Center, Right }
    WhiteSpace { Normal, Nowrap }
    WordBreak { Normal, BreakWord, Anywhere, BreakAll, KeepAll }
    TextWrap { Wrap, Balance, Pretty }
}

#[derive(Clone, Default, PartialEq)]
pub struct TextRun {
    pub len: usize,
    pub font: Font,
    pub color: Color,
    pub background_color: Option<Color>,
    pub underline: Option<Decoration>,
    pub strikethrough: Option<Decoration>,
    pub overline: Option<Decoration>,
    pub letter_spacing: Pixels,
    pub word_spacing: Pixels,
    pub font_size: Option<Pixels>,
    pub inset: [Pixels; 2],
    pub nowrap: bool,
    /// Spaces per tab stop, or pixels when negative.
    pub tab_size: f32,
}

/// Declares `TextStyle` and its overrides from one list, so the two cannot drift.
macro_rules! text_style {
    ($($field:ident $set:ident: $type:ty = $default:expr,)*) => {
        #[derive(Clone, Debug, PartialEq)]
        pub struct TextStyle {
            $(pub $field: $type,)*
        }

        impl Default for TextStyle {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }

        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct TextOverrides {
            $(pub(crate) $field: Option<$type>,)*
        }

        impl TextOverrides {
            pub fn inherits(&self) -> bool {
                $(self.$field.is_none())&&*
            }

            $(
                pub fn $set(&mut self, value: $type) -> &mut Self {
                    self.$field = Some(value);
                    self
                }
            )*
        }

        impl TextStyle {
            pub(crate) fn refine(&mut self, overrides: &TextOverrides) {
                $(if let Some(value) = &overrides.$field {
                    self.$field = value.clone();
                })*
            }
        }
    };
}

text_style! {
    color set_color: Color = Color::BLACK,
    font_family set_font_family: Rc<str> = super::SYSTEM_UI_FONT.with(Rc::clone),
    font_features set_font_features: Features = Features::default(),
    font_size set_font_size: Pixels = px(16.),
    line_height set_line_height: Pixels = px(20.),
    font_weight set_font_weight: FontWeight = 400.,
    font_style set_font_style: FontStyle = FontStyle::Normal,
    background_color set_background_color: Option<Color> = None,
    underline set_underline: Option<Decoration> = None,
    strikethrough set_strikethrough: Option<Decoration> = None,
    white_space set_white_space: WhiteSpace = WhiteSpace::Normal,
    text_overflow set_text_overflow: Option<Rc<str>> = None,
    text_align set_text_align: TextAlign = TextAlign::Left,
    line_clamp set_line_clamp: Option<usize> = None,
    letter_spacing set_letter_spacing: Pixels = Pixels::ZERO,
    word_spacing set_word_spacing: Pixels = Pixels::ZERO,
    text_indent set_text_indent: Pixels = Pixels::ZERO,
    float set_float: Pixels = Pixels::ZERO,
    text_wrap set_text_wrap: TextWrap = TextWrap::Wrap,
    word_break set_word_break: WordBreak = WordBreak::Normal,
    font_smoothing set_font_smoothing: bool = true,
}

impl TextStyle {
    pub fn font(&self) -> Font {
        let (family, features) = (self.font_family.clone(), self.font_features.clone());
        let (weight, style) = (self.font_weight, self.font_style);
        Font {
            family,
            features,
            weight,
            style,
        }
    }

    pub fn run(&self, len: usize) -> TextRun {
        let &Self {
            color,
            background_color,
            underline,
            strikethrough,
            letter_spacing,
            word_spacing,
            ..
        } = self;
        TextRun {
            len,
            font: self.font(),
            color,
            background_color,
            underline,
            strikethrough,
            overline: None,
            letter_spacing,
            word_spacing,
            font_size: None,
            inset: [Pixels::ZERO; 2],
            nowrap: false,
            tab_size: 8.,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_replace_their_field_and_make_fonts_and_runs() {
        let mut overrides = TextOverrides::default();
        assert!(overrides.inherits());
        overrides
            .set_font_size(px(12.))
            .set_color(Color::WHITE)
            .set_font_size(px(14.));
        overrides
            .set_font_family("Menlo".into())
            .set_font_weight(700.)
            .set_font_style(FontStyle::Italic);
        overrides
            .set_letter_spacing(px(2.))
            .set_text_overflow(Some("…".into()));
        assert!(overrides.font_size == Some(px(14.)) && overrides.line_clamp.is_none());
        let mut text = TextStyle::default();
        let initial = text.clone();
        text.refine(&overrides);
        assert!(
            text.font_size == px(14.)
                && text.color == Color::WHITE
                && text.line_height == initial.line_height
        );
        let font = text.font();
        assert!(&*font.family == "Menlo" && font.weight == 700. && font.style == FontStyle::Italic);
        let run = text.run(5);
        assert!(
            run.len == 5
                && run.font == font
                && run.letter_spacing == px(2.)
                && run.font_size.is_none()
        );
        assert!(
            text.text_overflow.as_deref() == Some("…")
                && &*initial.font().family == ".SystemUIFont"
        );
    }
}
