//! Curated property/value parsing (brief §4): colors, lengths, and the
//! declaration accumulator the cascade folds rules into. Everything here is
//! total — an unparseable value simply fails to apply (the caller counts it
//! against `Stylesheet::ignored_declarations`), it never panics.

use crate::style::computed::{
    AlignItems, AlignSelf, BorderStyle, Clear, Display, FlexDirection, FlexWrap, Float, FontFamily,
    FontStyle, FontWeight, JustifyContent, ListStyleType, TextAlign, TextDecoration, VerticalAlign,
    WhiteSpace,
};
use crate::style::tokenizer::Token;
use crate::surface::Color;

/// A length before it is resolved to pixels: only `em`/`%` need cascade
/// context (the element's own — or for `font-size`, the parent's — computed
/// font size); `px`/`pt` are context-free and could be resolved here, but
/// keeping all four uniform until resolution keeps the cascade's math in one
/// place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RawLength {
    Px(f32),
    Pt(f32),
    Em(f32),
    Percent(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RawLengthAuto {
    Length(RawLength),
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RawLineHeight {
    Normal,
    /// Unitless multiplier of the element's own resolved font size.
    Number(f32),
    Length(RawLength),
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BorderRaw {
    pub width: Option<RawLength>,
    pub style: Option<BorderStyle>,
    pub color: Option<Color>,
}

/// Margin/padding's 1–4 value shorthand, pre-expansion-per-edge but still
/// raw (unresolved units). Manually `Default`-implemented because `derive`
/// would otherwise demand `T: Default`, which the raw length types don't
/// (and needn't) provide.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EdgesRaw<T> {
    pub top: Option<T>,
    pub right: Option<T>,
    pub bottom: Option<T>,
    pub left: Option<T>,
}

impl<T> Default for EdgesRaw<T> {
    fn default() -> Self {
        EdgesRaw {
            top: None,
            right: None,
            bottom: None,
            left: None,
        }
    }
}

impl<T: Copy> EdgesRaw<T> {
    fn overlay(&mut self, other: &EdgesRaw<T>) {
        if other.top.is_some() {
            self.top = other.top;
        }
        if other.right.is_some() {
            self.right = other.right;
        }
        if other.bottom.is_some() {
            self.bottom = other.bottom;
        }
        if other.left.is_some() {
            self.left = other.left;
        }
    }
}

/// One declaration block's worth of curated properties, `None` where the
/// block didn't set the property. The cascade folds these together in
/// specificity/source order (later `Some` wins) and only then resolves units.
#[derive(Debug, Clone, Default)]
pub(crate) struct Declarations {
    pub color: Option<Color>,
    pub background_color: Option<Color>,
    pub font_family: Option<FontFamily>,
    pub font_size: Option<RawLength>,
    pub font_weight: Option<FontWeight>,
    pub font_style: Option<FontStyle>,
    pub text_align: Option<TextAlign>,
    pub text_decoration: Option<TextDecoration>,
    pub white_space: Option<WhiteSpace>,
    pub line_height: Option<RawLineHeight>,
    pub vertical_align: Option<VerticalAlign>,
    pub list_style_type: Option<ListStyleType>,

    pub display: Option<Display>,
    pub width: Option<RawLengthAuto>,
    pub height: Option<RawLengthAuto>,
    pub margin: EdgesRaw<RawLengthAuto>,
    pub padding: EdgesRaw<RawLength>,
    pub border: Option<BorderRaw>,
    pub float: Option<Float>,
    pub clear: Option<Clear>,

    pub flex_direction: Option<FlexDirection>,
    pub flex_wrap: Option<FlexWrap>,
    pub justify_content: Option<JustifyContent>,
    pub align_items: Option<AlignItems>,
    pub align_self: Option<AlignSelf>,
    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub flex_basis: Option<RawLengthAuto>,
    pub gap: Option<RawLength>,
}

impl Declarations {
    /// Apply `other` on top of `self`: every field `other` set overwrites the
    /// corresponding field here. Called in increasing precedence order, so
    /// the last overlay wins per property — exactly the cascade's per-
    /// property resolution, without needing to compare declaration blocks.
    pub(crate) fn overlay(&mut self, other: &Declarations) {
        macro_rules! ov {
            ($f:ident) => {
                if other.$f.is_some() {
                    self.$f = other.$f;
                }
            };
        }
        ov!(color);
        ov!(background_color);
        ov!(font_family);
        ov!(font_size);
        ov!(font_weight);
        ov!(font_style);
        ov!(text_align);
        ov!(text_decoration);
        ov!(white_space);
        ov!(line_height);
        ov!(vertical_align);
        ov!(list_style_type);
        ov!(display);
        ov!(width);
        ov!(height);
        ov!(border);
        ov!(float);
        ov!(clear);
        ov!(flex_direction);
        ov!(flex_wrap);
        ov!(justify_content);
        ov!(align_items);
        ov!(align_self);
        ov!(flex_grow);
        ov!(flex_shrink);
        ov!(flex_basis);
        ov!(gap);
        self.margin.overlay(&other.margin);
        self.padding.overlay(&other.padding);
    }
}

fn named_color(name: &str) -> Option<Color> {
    Some(match name.to_ascii_lowercase().as_str() {
        "black" => Color::rgb(0, 0, 0),
        "white" => Color::rgb(255, 255, 255),
        "silver" => Color::rgb(192, 192, 192),
        "gray" | "grey" => Color::rgb(128, 128, 128),
        "maroon" => Color::rgb(128, 0, 0),
        "red" => Color::rgb(255, 0, 0),
        "purple" => Color::rgb(128, 0, 128),
        "fuchsia" | "magenta" => Color::rgb(255, 0, 255),
        "green" => Color::rgb(0, 128, 0),
        "lime" => Color::rgb(0, 255, 0),
        "olive" => Color::rgb(128, 128, 0),
        "yellow" => Color::rgb(255, 255, 0),
        "navy" => Color::rgb(0, 0, 128),
        "blue" => Color::rgb(0, 0, 255),
        "teal" => Color::rgb(0, 128, 128),
        "aqua" | "cyan" => Color::rgb(0, 255, 255),
        "orange" => Color::rgb(255, 165, 0),
        "pink" => Color::rgb(255, 192, 203),
        "brown" => Color::rgb(165, 42, 42),
        "transparent" => Color::TRANSPARENT,
        _ => return None,
    })
}

fn hex_color(s: &str) -> Option<Color> {
    let digit = |c: char| c.to_digit(16);
    let chars: Vec<char> = s.chars().collect();
    match chars.len() {
        3 => {
            let r = digit(chars[0])?;
            let g = digit(chars[1])?;
            let b = digit(chars[2])?;
            Some(Color::rgb((r * 17) as u8, (g * 17) as u8, (b * 17) as u8))
        }
        6 => {
            let mut bytes = [0u8; 3];
            for i in 0..3 {
                let hi = digit(chars[i * 2])?;
                let lo = digit(chars[i * 2 + 1])?;
                bytes[i] = (hi * 16 + lo) as u8;
            }
            Some(Color::rgb(bytes[0], bytes[1], bytes[2]))
        }
        _ => None,
    }
}

pub(crate) fn parse_color(tokens: &[Token]) -> Option<Color> {
    match tokens.first()? {
        Token::Ident(name) => named_color(name),
        Token::Hash(h) => hex_color(h),
        Token::Function(name) if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") => {
            let mut comps: Vec<f32> = Vec::new();
            for t in &tokens[1..] {
                match t {
                    Token::Number(n) => comps.push(*n),
                    Token::Percentage(p) => comps.push(p * 255.0 / 100.0),
                    Token::RParen => break,
                    _ => {}
                }
            }
            if comps.len() >= 3 {
                let clamp = |v: f32| v.round().clamp(0.0, 255.0) as u8;
                Some(Color::rgb(clamp(comps[0]), clamp(comps[1]), clamp(comps[2])))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn classify_font_family(tokens: &[Token]) -> Option<FontFamily> {
    let names: Vec<String> = tokens
        .iter()
        .filter_map(|t| match t {
            Token::Ident(s) | Token::Str(s) => Some(s.to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    let first = names.first()?;
    if names.iter().any(|n| n == "monospace") {
        return Some(FontFamily::Monospace);
    }
    if names.iter().any(|n| n == "sans-serif") {
        return Some(FontFamily::SansSerif);
    }
    if names.iter().any(|n| n == "serif") {
        return Some(FontFamily::Serif);
    }
    const MONO_HINTS: &[&str] = &["courier", "monaco", "consolas", "mono", "menlo"];
    const SANS_HINTS: &[&str] = &["arial", "helvetica", "verdana", "tahoma", "sans", "geneva", "calibri", "segoe"];
    if MONO_HINTS.iter().any(|h| first.contains(h)) {
        return Some(FontFamily::Monospace);
    }
    if SANS_HINTS.iter().any(|h| first.contains(h)) {
        return Some(FontFamily::SansSerif);
    }
    Some(FontFamily::Serif)
}

fn token_to_raw_length(t: &Token) -> Option<RawLength> {
    match t {
        Token::Dimension(v, unit) => match unit.as_str() {
            "px" => Some(RawLength::Px(*v)),
            "pt" => Some(RawLength::Pt(*v)),
            "em" => Some(RawLength::Em(*v)),
            _ => None,
        },
        Token::Percentage(v) => Some(RawLength::Percent(*v)),
        Token::Number(v) if *v == 0.0 => Some(RawLength::Px(0.0)), // unitless 0 is a valid length
        _ => None,
    }
}

/// `border-width` (unlike margin/padding/width/height) has no percentage
/// form in CSS — a `%` token must not parse as a border width.
fn token_to_border_width(t: &Token) -> Option<RawLength> {
    match token_to_raw_length(t)? {
        RawLength::Percent(_) => None,
        other => Some(other),
    }
}

fn token_to_raw_length_auto(t: &Token) -> Option<RawLengthAuto> {
    if let Token::Ident(s) = t {
        if s.eq_ignore_ascii_case("auto") {
            return Some(RawLengthAuto::Auto);
        }
    }
    token_to_raw_length(t).map(RawLengthAuto::Length)
}

/// Real CSS invalidates a whole shorthand declaration if any single
/// component fails to parse (it does not silently drop the bad token and
/// reinterpret the rest as a shorthand with fewer values) — so this returns
/// `false` (and leaves `edges` untouched) the moment any token doesn't
/// convert, rather than filtering unrecognized tokens out.
fn apply_edges_shorthand<T: Copy>(tokens: &[Token], edges: &mut EdgesRaw<T>, conv: impl Fn(&Token) -> Option<T>) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let mut vals: Vec<T> = Vec::with_capacity(tokens.len());
    for t in tokens {
        match conv(t) {
            Some(v) => vals.push(v),
            None => return false,
        }
    }
    match vals.len() {
        1 => {
            edges.top = Some(vals[0]);
            edges.right = Some(vals[0]);
            edges.bottom = Some(vals[0]);
            edges.left = Some(vals[0]);
            true
        }
        2 => {
            edges.top = Some(vals[0]);
            edges.bottom = Some(vals[0]);
            edges.right = Some(vals[1]);
            edges.left = Some(vals[1]);
            true
        }
        3 => {
            edges.top = Some(vals[0]);
            edges.right = Some(vals[1]);
            edges.left = Some(vals[1]);
            edges.bottom = Some(vals[2]);
            true
        }
        4 => {
            edges.top = Some(vals[0]);
            edges.right = Some(vals[1]);
            edges.bottom = Some(vals[2]);
            edges.left = Some(vals[3]);
            true
        }
        _ => false,
    }
}

fn keyword(tokens: &[Token]) -> Option<String> {
    match tokens.first() {
        Some(Token::Ident(s)) => Some(s.to_ascii_lowercase()),
        _ => None,
    }
}

/// Apply one already-lowercased property `name` with its (whitespace-
/// filtered) value tokens onto `d`. Returns whether it was recognized *and*
/// parsed successfully — the caller counts `false` against
/// `Stylesheet::ignored_declarations` (charter C2's ignore-unknown treaty).
pub(crate) fn apply_property(name: &str, tokens: &[Token], d: &mut Declarations) -> bool {
    match name {
        "color" => parse_color(tokens).map(|c| d.color = Some(c)).is_some(),
        "background-color" => parse_color(tokens).map(|c| d.background_color = Some(c)).is_some(),
        "font-family" => classify_font_family(tokens).map(|f| d.font_family = Some(f)).is_some(),
        "font-size" => tokens.first().and_then(token_to_raw_length).map(|l| d.font_size = Some(l)).is_some(),
        "font-weight" => match tokens.first() {
            Some(Token::Ident(s)) => match s.to_ascii_lowercase().as_str() {
                "bold" | "bolder" => {
                    d.font_weight = Some(FontWeight::Bold);
                    true
                }
                "normal" | "lighter" => {
                    d.font_weight = Some(FontWeight::Normal);
                    true
                }
                _ => false,
            },
            Some(Token::Number(n)) => {
                d.font_weight = Some(if *n >= 600.0 { FontWeight::Bold } else { FontWeight::Normal });
                true
            }
            _ => false,
        },
        "font-style" => match keyword(tokens).as_deref() {
            Some("italic") | Some("oblique") => {
                d.font_style = Some(FontStyle::Italic);
                true
            }
            Some("normal") => {
                d.font_style = Some(FontStyle::Normal);
                true
            }
            _ => false,
        },
        "text-align" => match keyword(tokens).as_deref() {
            Some("left") => {
                d.text_align = Some(TextAlign::Left);
                true
            }
            Some("right") => {
                d.text_align = Some(TextAlign::Right);
                true
            }
            Some("center") => {
                d.text_align = Some(TextAlign::Center);
                true
            }
            Some("justify") => {
                d.text_align = Some(TextAlign::Justify);
                true
            }
            _ => false,
        },
        "text-decoration" => {
            let mut td = TextDecoration::default();
            let mut any = false;
            for t in tokens {
                if let Token::Ident(s) = t {
                    match s.to_ascii_lowercase().as_str() {
                        "underline" => {
                            td.underline = true;
                            any = true;
                        }
                        "line-through" => {
                            td.line_through = true;
                            any = true;
                        }
                        "overline" => {
                            td.overline = true;
                            any = true;
                        }
                        "none" => any = true,
                        _ => {}
                    }
                }
            }
            if any {
                d.text_decoration = Some(td);
            }
            any
        }
        "white-space" => match keyword(tokens).as_deref() {
            Some("normal") => {
                d.white_space = Some(WhiteSpace::Normal);
                true
            }
            Some("pre") => {
                d.white_space = Some(WhiteSpace::Pre);
                true
            }
            _ => false,
        },
        "line-height" => match tokens.first() {
            Some(Token::Ident(s)) if s.eq_ignore_ascii_case("normal") => {
                d.line_height = Some(RawLineHeight::Normal);
                true
            }
            Some(Token::Number(n)) => {
                d.line_height = Some(RawLineHeight::Number(*n));
                true
            }
            Some(t) => token_to_raw_length(t).map(|l| d.line_height = Some(RawLineHeight::Length(l))).is_some(),
            None => false,
        },
        "vertical-align" => match keyword(tokens).as_deref() {
            Some("baseline") => {
                d.vertical_align = Some(VerticalAlign::Baseline);
                true
            }
            Some("top") => {
                d.vertical_align = Some(VerticalAlign::Top);
                true
            }
            Some("middle") => {
                d.vertical_align = Some(VerticalAlign::Middle);
                true
            }
            Some("bottom") => {
                d.vertical_align = Some(VerticalAlign::Bottom);
                true
            }
            _ => false,
        },
        "list-style-type" => match keyword(tokens).as_deref() {
            Some("disc") => {
                d.list_style_type = Some(ListStyleType::Disc);
                true
            }
            Some("circle") => {
                d.list_style_type = Some(ListStyleType::Circle);
                true
            }
            Some("square") => {
                d.list_style_type = Some(ListStyleType::Square);
                true
            }
            Some("decimal") => {
                d.list_style_type = Some(ListStyleType::Decimal);
                true
            }
            Some("lower-alpha") => {
                d.list_style_type = Some(ListStyleType::LowerAlpha);
                true
            }
            Some("upper-alpha") => {
                d.list_style_type = Some(ListStyleType::UpperAlpha);
                true
            }
            Some("none") => {
                d.list_style_type = Some(ListStyleType::None);
                true
            }
            _ => false,
        },
        "display" => match keyword(tokens).as_deref() {
            Some("block") => {
                d.display = Some(Display::Block);
                true
            }
            Some("inline") => {
                d.display = Some(Display::Inline);
                true
            }
            Some("none") => {
                d.display = Some(Display::None);
                true
            }
            Some("flex") => {
                d.display = Some(Display::Flex);
                true
            }
            _ => false,
        },
        "width" => tokens.first().and_then(token_to_raw_length_auto).map(|l| d.width = Some(l)).is_some(),
        "height" => tokens.first().and_then(token_to_raw_length_auto).map(|l| d.height = Some(l)).is_some(),
        "margin" => apply_edges_shorthand(tokens, &mut d.margin, token_to_raw_length_auto),
        "margin-top" => tokens.first().and_then(token_to_raw_length_auto).map(|l| d.margin.top = Some(l)).is_some(),
        "margin-right" => tokens.first().and_then(token_to_raw_length_auto).map(|l| d.margin.right = Some(l)).is_some(),
        "margin-bottom" => tokens.first().and_then(token_to_raw_length_auto).map(|l| d.margin.bottom = Some(l)).is_some(),
        "margin-left" => tokens.first().and_then(token_to_raw_length_auto).map(|l| d.margin.left = Some(l)).is_some(),
        "padding" => apply_edges_shorthand(tokens, &mut d.padding, token_to_raw_length),
        "padding-top" => tokens.first().and_then(token_to_raw_length).map(|l| d.padding.top = Some(l)).is_some(),
        "padding-right" => tokens.first().and_then(token_to_raw_length).map(|l| d.padding.right = Some(l)).is_some(),
        "padding-bottom" => tokens.first().and_then(token_to_raw_length).map(|l| d.padding.bottom = Some(l)).is_some(),
        "padding-left" => tokens.first().and_then(token_to_raw_length).map(|l| d.padding.left = Some(l)).is_some(),
        "border" => {
            // Real CSS invalidates the whole shorthand if any single
            // component is unrecognized — it does not silently apply the
            // components it understood and drop the rest. `border-width`
            // also has no percentage form in CSS (unlike margin/padding),
            // so a `%` token must fail to parse as a width here rather than
            // resolve to `0px` later.
            let mut b = BorderRaw::default();
            for t in tokens {
                if let Some(l) = token_to_border_width(t) {
                    b.width = Some(l);
                    continue;
                }
                if let Token::Ident(s) = t {
                    match s.to_ascii_lowercase().as_str() {
                        "solid" => {
                            b.style = Some(BorderStyle::Solid);
                            continue;
                        }
                        "none" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {
                            // Curated set is solid-only (brief §4); every other
                            // named style resolves to "no visible border".
                            b.style = Some(BorderStyle::None);
                            continue;
                        }
                        _ => {}
                    }
                }
                if let Some(c) = parse_color(std::slice::from_ref(t)) {
                    b.color = Some(c);
                    continue;
                }
                return false; // unrecognized token: the whole shorthand is invalid
            }
            if b.width.is_none() && b.style.is_none() && b.color.is_none() {
                return false;
            }
            d.border = Some(b);
            true
        }
        "float" => match keyword(tokens).as_deref() {
            Some("left") => {
                d.float = Some(Float::Left);
                true
            }
            Some("right") => {
                d.float = Some(Float::Right);
                true
            }
            Some("none") => {
                d.float = Some(Float::None);
                true
            }
            _ => false,
        },
        "clear" => match keyword(tokens).as_deref() {
            Some("left") => {
                d.clear = Some(Clear::Left);
                true
            }
            Some("right") => {
                d.clear = Some(Clear::Right);
                true
            }
            Some("both") => {
                d.clear = Some(Clear::Both);
                true
            }
            Some("none") => {
                d.clear = Some(Clear::None);
                true
            }
            _ => false,
        },
        "flex-direction" => match keyword(tokens).as_deref() {
            Some("row") => {
                d.flex_direction = Some(FlexDirection::Row);
                true
            }
            Some("row-reverse") => {
                d.flex_direction = Some(FlexDirection::RowReverse);
                true
            }
            Some("column") => {
                d.flex_direction = Some(FlexDirection::Column);
                true
            }
            Some("column-reverse") => {
                d.flex_direction = Some(FlexDirection::ColumnReverse);
                true
            }
            _ => false,
        },
        "flex-wrap" => match keyword(tokens).as_deref() {
            Some("nowrap") => {
                d.flex_wrap = Some(FlexWrap::NoWrap);
                true
            }
            Some("wrap") => {
                d.flex_wrap = Some(FlexWrap::Wrap);
                true
            }
            Some("wrap-reverse") => {
                d.flex_wrap = Some(FlexWrap::WrapReverse);
                true
            }
            _ => false,
        },
        "justify-content" => match keyword(tokens).as_deref() {
            Some("flex-start") | Some("start") => {
                d.justify_content = Some(JustifyContent::FlexStart);
                true
            }
            Some("flex-end") | Some("end") => {
                d.justify_content = Some(JustifyContent::FlexEnd);
                true
            }
            Some("center") => {
                d.justify_content = Some(JustifyContent::Center);
                true
            }
            Some("space-between") => {
                d.justify_content = Some(JustifyContent::SpaceBetween);
                true
            }
            Some("space-around") => {
                d.justify_content = Some(JustifyContent::SpaceAround);
                true
            }
            Some("space-evenly") => {
                d.justify_content = Some(JustifyContent::SpaceEvenly);
                true
            }
            _ => false,
        },
        "align-items" => match keyword(tokens).as_deref() {
            Some("flex-start") | Some("start") => {
                d.align_items = Some(AlignItems::FlexStart);
                true
            }
            Some("flex-end") | Some("end") => {
                d.align_items = Some(AlignItems::FlexEnd);
                true
            }
            Some("center") => {
                d.align_items = Some(AlignItems::Center);
                true
            }
            Some("stretch") => {
                d.align_items = Some(AlignItems::Stretch);
                true
            }
            Some("baseline") => {
                d.align_items = Some(AlignItems::Baseline);
                true
            }
            _ => false,
        },
        "align-self" => match keyword(tokens).as_deref() {
            Some("auto") => {
                d.align_self = Some(AlignSelf::Auto);
                true
            }
            Some("flex-start") | Some("start") => {
                d.align_self = Some(AlignSelf::FlexStart);
                true
            }
            Some("flex-end") | Some("end") => {
                d.align_self = Some(AlignSelf::FlexEnd);
                true
            }
            Some("center") => {
                d.align_self = Some(AlignSelf::Center);
                true
            }
            Some("stretch") => {
                d.align_self = Some(AlignSelf::Stretch);
                true
            }
            Some("baseline") => {
                d.align_self = Some(AlignSelf::Baseline);
                true
            }
            _ => false,
        },
        "flex-grow" => match tokens.first() {
            Some(Token::Number(n)) => {
                d.flex_grow = Some(*n);
                true
            }
            _ => false,
        },
        "flex-shrink" => match tokens.first() {
            Some(Token::Number(n)) => {
                d.flex_shrink = Some(*n);
                true
            }
            _ => false,
        },
        "flex-basis" => match tokens.first() {
            Some(Token::Ident(s)) if s.eq_ignore_ascii_case("auto") => {
                d.flex_basis = Some(RawLengthAuto::Auto);
                true
            }
            Some(t) => token_to_raw_length(t).map(|l| d.flex_basis = Some(RawLengthAuto::Length(l))).is_some(),
            None => false,
        },
        "gap" => match tokens.first().and_then(token_to_raw_length) {
            // Percent gap needs a containing-block size this layer doesn't
            // have; out of scope for v0 (unlike width/margin/padding, gap
            // has no percent-carrying computed representation to defer to).
            Some(RawLength::Percent(_)) | None => false,
            Some(l) => {
                d.gap = Some(l);
                true
            }
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::tokenizer::tokenize;

    fn toks(css_value: &str) -> Vec<Token> {
        tokenize(css_value).into_iter().filter(|t| *t != Token::Whitespace).collect()
    }

    #[test]
    fn parses_named_hex_and_rgb_colors() {
        assert_eq!(parse_color(&toks("red")), Some(Color::rgb(255, 0, 0)));
        assert_eq!(parse_color(&toks("#f00")), Some(Color::rgb(255, 0, 0)));
        assert_eq!(parse_color(&toks("#ff0000")), Some(Color::rgb(255, 0, 0)));
        assert_eq!(parse_color(&toks("rgb(1, 2, 3)")), Some(Color::rgb(1, 2, 3)));
        assert_eq!(parse_color(&toks("transparent")), Some(Color::TRANSPARENT));
        assert_eq!(parse_color(&toks("not-a-color")), None);
    }

    #[test]
    fn classifies_generic_and_named_font_families() {
        assert_eq!(classify_font_family(&toks("monospace")), Some(FontFamily::Monospace));
        assert_eq!(classify_font_family(&toks("Arial, sans-serif")), Some(FontFamily::SansSerif));
        assert_eq!(classify_font_family(&toks("Courier New, monospace")), Some(FontFamily::Monospace));
        assert_eq!(classify_font_family(&toks("\"Times New Roman\", serif")), Some(FontFamily::Serif));
        assert_eq!(classify_font_family(&toks("")), None);
    }

    #[test]
    fn margin_shorthand_expands_1_2_3_4_values() {
        let mut d = Declarations::default();
        assert!(apply_property("margin", &toks("5px"), &mut d));
        assert_eq!(d.margin.top, Some(RawLengthAuto::Length(RawLength::Px(5.0))));
        assert_eq!(d.margin.left, Some(RawLengthAuto::Length(RawLength::Px(5.0))));

        let mut d = Declarations::default();
        assert!(apply_property("margin", &toks("1px 2px"), &mut d));
        assert_eq!(d.margin.top, Some(RawLengthAuto::Length(RawLength::Px(1.0))));
        assert_eq!(d.margin.right, Some(RawLengthAuto::Length(RawLength::Px(2.0))));
        assert_eq!(d.margin.bottom, Some(RawLengthAuto::Length(RawLength::Px(1.0))));
        assert_eq!(d.margin.left, Some(RawLengthAuto::Length(RawLength::Px(2.0))));

        let mut d = Declarations::default();
        assert!(apply_property("margin", &toks("1px 2px 3px 4px"), &mut d));
        assert_eq!(d.margin.top, Some(RawLengthAuto::Length(RawLength::Px(1.0))));
        assert_eq!(d.margin.right, Some(RawLengthAuto::Length(RawLength::Px(2.0))));
        assert_eq!(d.margin.bottom, Some(RawLengthAuto::Length(RawLength::Px(3.0))));
        assert_eq!(d.margin.left, Some(RawLengthAuto::Length(RawLength::Px(4.0))));
    }

    #[test]
    fn margin_auto_is_recognized() {
        let mut d = Declarations::default();
        assert!(apply_property("margin", &toks("auto"), &mut d));
        assert_eq!(d.margin.top, Some(RawLengthAuto::Auto));
    }

    #[test]
    fn border_shorthand_parses_width_style_color_in_any_order() {
        let mut d = Declarations::default();
        assert!(apply_property("border", &toks("solid red 2px"), &mut d));
        let b = d.border.unwrap();
        assert_eq!(b.width, Some(RawLength::Px(2.0)));
        assert_eq!(b.style, Some(BorderStyle::Solid));
        assert_eq!(b.color, Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn unknown_property_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("flibbertigibbet", &toks("1"), &mut d));
    }

    #[test]
    fn unparseable_color_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("color", &toks("bogus"), &mut d));
    }

    #[test]
    fn margin_shorthand_rejects_the_whole_declaration_if_any_token_is_unrecognized() {
        // Real CSS invalidates the whole declaration if any component fails
        // to parse — it must not silently degrade into a 3-value shorthand.
        let mut d = Declarations::default();
        assert!(!apply_property("margin", &toks("1px bogus 2px 3px"), &mut d));
        assert_eq!(d.margin.top, None);
        assert_eq!(d.margin.right, None);
        assert_eq!(d.margin.bottom, None);
        assert_eq!(d.margin.left, None);
    }

    #[test]
    fn padding_shorthand_rejects_the_whole_declaration_if_any_token_is_unrecognized() {
        let mut d = Declarations::default();
        assert!(!apply_property("padding", &toks("1px bogus"), &mut d));
        assert_eq!(d.padding.top, None);
        assert_eq!(d.padding.left, None);
    }

    #[test]
    fn border_shorthand_rejects_the_whole_declaration_on_a_trailing_unrecognized_token() {
        let mut d = Declarations::default();
        assert!(!apply_property("border", &toks("2px solid red garbage"), &mut d));
        assert!(d.border.is_none());
    }

    #[test]
    fn border_shorthand_rejects_a_percentage_width() {
        // border-width has no percentage form in CSS; it must not silently
        // become width 0 — the whole shorthand is invalid.
        let mut d = Declarations::default();
        assert!(!apply_property("border", &toks("5% solid red"), &mut d));
        assert!(d.border.is_none());
    }

    #[test]
    fn overlay_lets_later_values_win_per_field() {
        let mut base = Declarations::default();
        apply_property("color", &toks("red"), &mut base);
        apply_property("display", &toks("block"), &mut base);

        let mut later = Declarations::default();
        apply_property("color", &toks("blue"), &mut later);

        base.overlay(&later);
        assert_eq!(base.color, Some(Color::rgb(0, 0, 255)));
        assert_eq!(base.display, Some(Display::Block)); // untouched field survives
    }
}
