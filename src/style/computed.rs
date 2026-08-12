//! `ComputedStyle`: the curated set of computed properties layout and paint
//! consume (brief §4). All CSS syntax is parsed upstream; only these properties
//! survive the cascade with meaning. Everything else is counted and ignored
//! (the `--stats` ignored-declaration counter, M5).

use crate::surface::Color;

/// A length that may be a percentage of the containing block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LengthPercentage {
    Px(f32),
    Percent(f32),
}

/// A length/percentage that may also be `auto` (margins).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LengthPercentageAuto {
    Px(f32),
    Percent(f32),
    Auto,
}

/// A sizing value for `width`/`height`/`flex-basis`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dimension {
    Px(f32),
    Percent(f32),
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    None,
    Inline,
    Block,
    Flex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontFamily {
    Serif,
    SansSerif,
    Monospace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyle {
    Normal,
    Italic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontWeight {
    Normal,
    Bold,
}

/// `normal` resolves against the font; an explicit value is in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    Normal,
    Px(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextDecoration {
    pub underline: bool,
    pub line_through: bool,
    pub overline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    Pre,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAlign {
    Baseline,
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Float {
    None,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStyleType {
    Disc,
    Circle,
    Square,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlexWrap {
    NoWrap,
    Wrap,
    WrapReverse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JustifyContent {
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignItems {
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignSelf {
    Auto,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    Baseline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    None,
    Solid,
}

/// One side of a border. Only `solid` is honored in v0 (brief §4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BorderSide {
    pub width: f32,
    pub style: BorderStyle,
    pub color: Color,
}

impl Default for BorderSide {
    fn default() -> Self {
        BorderSide {
            width: 0.0,
            style: BorderStyle::None,
            color: Color::BLACK,
        }
    }
}

/// The four sides of a box (margin/padding/border).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> Edges<T> {
    pub fn all(v: T) -> Self {
        Edges {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }
}

/// The computed style of one node: the frozen contract between cascade (P2)
/// and layout/paint (P6/P7/P9). Initial values follow CSS; the user-agent sheet
/// (e.g. `div { display: block }`) overrides them during cascade.
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    // Text & font
    pub color: Color,
    pub background_color: Color,
    pub font_family: FontFamily,
    pub font_size: f32,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub line_height: LineHeight,
    pub text_align: TextAlign,
    pub text_decoration: TextDecoration,
    pub white_space: WhiteSpace,
    pub vertical_align: VerticalAlign,
    pub list_style_type: ListStyleType,

    // Box
    pub display: Display,
    pub width: Dimension,
    pub height: Dimension,
    pub margin: Edges<LengthPercentageAuto>,
    pub padding: Edges<LengthPercentage>,
    pub border: Edges<BorderSide>,
    pub float: Float,
    pub clear: Clear,

    // Flex
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub align_self: AlignSelf,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Dimension,
    pub gap: f32,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            color: Color::BLACK,
            background_color: Color::TRANSPARENT,
            font_family: FontFamily::Serif,
            font_size: 16.0,
            font_weight: FontWeight::Normal,
            font_style: FontStyle::Normal,
            line_height: LineHeight::Normal,
            text_align: TextAlign::Left,
            text_decoration: TextDecoration::default(),
            white_space: WhiteSpace::Normal,
            vertical_align: VerticalAlign::Baseline,
            list_style_type: ListStyleType::Disc,

            display: Display::Inline, // CSS initial; UA sheet makes blocks block
            width: Dimension::Auto,
            height: Dimension::Auto,
            margin: Edges::all(LengthPercentageAuto::Px(0.0)),
            padding: Edges::all(LengthPercentage::Px(0.0)),
            border: Edges::all(BorderSide::default()),
            float: Float::None,
            clear: Clear::None,

            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            justify_content: JustifyContent::FlexStart,
            align_items: AlignItems::Stretch,
            align_self: AlignSelf::Auto,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dimension::Auto,
            gap: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_values_match_css() {
        let s = ComputedStyle::default();
        assert_eq!(s.display, Display::Inline);
        assert_eq!(s.color, Color::BLACK);
        assert_eq!(s.background_color, Color::TRANSPARENT);
        assert_eq!(s.flex_shrink, 1.0);
        assert_eq!(s.flex_grow, 0.0);
        assert_eq!(s.margin.top, LengthPercentageAuto::Px(0.0));
    }
}
