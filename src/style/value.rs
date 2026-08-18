//! Curated property/value parsing (brief §4): colors, lengths, and the
//! declaration accumulator the cascade folds rules into. Everything here is
//! total — an unparseable value simply fails to apply (the caller counts it
//! against `Stylesheet::ignored_declarations`), it never panics.

use crate::dom::AttrMap;
use crate::style::computed::{
    AlignItems, AlignSelf, BorderCollapse, BorderStyle, Clear, Display, FlexDirection, FlexWrap, Float, FontFamily,
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

/// packet/css-grid: a raw grid-track size — a bare `<length>`/`<percentage>`
/// (via [`RawLength`], resolved to px by `cascade::resolve` exactly like
/// `width`/`gap`) or a `<flex>` (`fr`) value, which has no `em` component and
/// so needs no cascade resolution at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RawGridTrackSize {
    Length(RawLength),
    Fr(f32),
}

/// packet/css-grid: a raw grid track — either a bare size or an explicit
/// `minmax(<min>, <max>)`. Mirrors `computed::GridTrack`; see that type's
/// doc comment for why `Bare` and `MinMax` are kept distinct rather than
/// folding `Bare` into `MinMax(size, size)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RawGridTrack {
    Bare(RawGridTrackSize),
    MinMax(RawGridTrackSize, RawGridTrackSize),
}

/// packet/css-grid: `repeat()`'s raw first argument. Mirrors
/// `computed::GridRepetitionCount`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RawGridRepetitionCount {
    Count(u16),
    AutoFill,
    AutoFit,
}

/// packet/css-grid: one raw component of a `grid-template-columns`/
/// `grid-template-rows` value. Mirrors `computed::GridTemplateComponent`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RawGridTemplateComponent {
    Single(RawGridTrack),
    Repeat(RawGridRepetitionCount, Vec<RawGridTrack>),
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
    /// Raw (unresolved) `background-image`/`background` shorthand URL —
    /// mirrors `ComputedStyle::background_image`'s own doc comment (packet
    /// bg-image). Not `Copy` (heap-allocated `Box<str>`), so it's overlaid
    /// by hand in `Declarations::overlay` below rather than through the
    /// `ov!` macro (which every OTHER field here can use only because every
    /// other `Option<T>` in this struct is `Copy`).
    pub background_image: Option<Box<str>>,
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
    /// `border-top` (packet/hr-rule): a distinct field from `border` above
    /// rather than folded into it, because they mean different things to
    /// the cascade -- `border` (the shorthand) resolves to all four
    /// `Edges<BorderSide>` sides uniformly (`cascade::resolve_border`'s
    /// `Edges::all`), while this is a true CSS longhand that must override
    /// ONLY `Edges.top`. The curated dialect has no `border-right`/`-bottom`
    /// /`-left` (out of scope -- only `<hr>`'s UA rule needs a single-side
    /// border today); add them the same way if a future packet needs them.
    pub border_top: Option<BorderRaw>,
    /// `border-width: <length>{1,4}` (packet/acid1-content-box:
    /// `fixtures/css1-float-5526c.html`'s `blockquote` declares
    /// `border-width: 1em 1.5em 2em .5em` as its OWN longhand, never a
    /// `border` shorthand at all -- before this packet, `apply_property`
    /// had no `"border-width"` arm, so this fell through to the catch-all
    /// `_ => false` and the whole border silently never applied (`style`/
    /// `width` both stayed the CSS-initial `none`/`0`). A per-edge
    /// `EdgesRaw<RawLength>`, same 1-4-value TRBL grammar as `margin`/
    /// `padding` above (`apply_edges_shorthand`), because — unlike
    /// `border`/`border-top` (curated as ONE uniform width/style/color
    /// triple per brief §4) — THIS fixture needs four genuinely different
    /// widths on one element and there is no shorthand token in its CSS to
    /// carry them. `cascade::resolve_border` layers this on top of
    /// whatever `border`/`border-top` already resolved, side by side,
    /// touching a side's width only when this field actually set it (never
    /// a behavior change for the many existing fixtures that only ever use
    /// the `border` shorthand).
    pub border_width: EdgesRaw<RawLength>,
    /// `border-style: <keyword>` (packet/acid1-content-box, sibling to
    /// `border_width` above — same fixture, same gap). Curated to a single
    /// uniform value (real CSS allows 1-4, but no fixture needs more than
    /// one), reusing the border shorthand's own solid-vs-everything-else
    /// mapping (`parse_border_raw`'s doc comment) so `none`/`dashed`/
    /// `dotted`/... all collapse to `BorderStyle::None` exactly like they
    /// do inside the `border`/`border-top` shorthand grammar.
    pub border_style: Option<BorderStyle>,
    /// `border-color: <color>` (packet/acid1-content-box, sibling to
    /// `border_width`/`border_style` above). Single uniform value, same
    /// scope note as `border_style`.
    pub border_color: Option<Color>,
    /// `border-spacing: <length-x> <length-y>?` (packet/table-spacing,
    /// `ComputedStyle`'s own freeze-amendment doc comment has the full
    /// rationale). Two independent fields (not an `EdgesRaw`-style pair —
    /// there's no "4 sides" here, just x/y) so `overlay`'s `ov!` macro can
    /// treat them exactly like every other `Copy` field. `None` means
    /// "this declaration didn't set it" -- NOT "zero"; `cascade::resolve`
    /// falls back to `ComputedStyle::default`'s `8.0`/`0.0` the same way
    /// every other `own!`-resolved field does.
    pub border_spacing_x: Option<RawLength>,
    pub border_spacing_y: Option<RawLength>,
    /// `border-collapse: separate | collapse` (packet/border-collapse,
    /// `ComputedStyle::border_collapse`'s own doc comment has the full
    /// rationale). `Copy`, so it goes through the `overlay` `ov!` macro like
    /// every other simple enum field here.
    pub border_collapse: Option<BorderCollapse>,
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
    /// The SECOND value of a two-value `gap: <row-gap> <column-gap>`
    /// shorthand (packet/t3-inline-spacing, the D3 fix). `None` when only
    /// one value was ever declared, in which case `gap` alone governs both
    /// axes (unchanged pre-existing behavior) -- see
    /// `ComputedStyle::column_gap`'s own doc comment for why this needs its
    /// own field rather than overwriting `gap`.
    pub column_gap: Option<RawLength>,

    /// packet/css-grid. `None` means "no `grid-template-columns` declared"
    /// (not "declared empty" -- CSS has no way to declare an
    /// explicitly-empty template); `cascade::resolve` falls back to
    /// `ComputedStyle::default`'s empty `Vec` the same way every other
    /// `Option`-shaped field here does.
    pub grid_template_columns: Option<Vec<RawGridTemplateComponent>>,
    pub grid_template_rows: Option<Vec<RawGridTemplateComponent>>,

    /// Custom-property (`--name: <tokens>;`) declarations captured by this
    /// block (packet T1a — CSS custom properties + `var()`). Stored as RAW
    /// tokens, never eagerly resolved: a custom property's value may itself
    /// reference `var()` (a chain), and — unlike every other field in this
    /// struct — its name is CASE-SENSITIVE (`--Foo` and `--foo` are distinct
    /// custom properties; see `parser::parse_declaration_block`, which is
    /// careful NOT to lowercase a name starting with `--` the way it
    /// lowercases every ordinary property name). A flat `Vec` rather than a
    /// map: a single declaration block's custom-property count is small in
    /// practice, and this keeps the type as simple/dependency-free as every
    /// other field here. `overlay` below merges these last-write-wins by
    /// name, exactly like every other property.
    pub custom: Vec<(Box<str>, Vec<Token>)>,
    /// Ordinary (non-custom) declarations whose value tokens contained a
    /// `var()` function at parse time (packet T1a) — `parser::
    /// parse_declaration_block` defers these instead of calling
    /// `apply_property` immediately, because resolving `var()` needs the
    /// element's full custom-property environment, which only exists at
    /// cascade time (`cascade::resolve` substitutes these against that
    /// environment via `substitute_vars` and, on success, calls
    /// `apply_property` itself to fill in the typed field above). Every
    /// OTHER declaration (no `var()` present) keeps going through the
    /// existing eager `apply_property` fast path unchanged, so this packet
    /// causes zero golden/behavior churn for stylesheets that don't use
    /// custom properties. `Box<str>` keys here ARE lowercased (these are
    /// ordinary property names, unlike `custom`'s case-sensitive keys).
    pub deferred: Vec<(Box<str>, Vec<Token>)>,
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
        ov!(border_top);
        ov!(border_style);
        ov!(border_color);
        ov!(border_spacing_x);
        ov!(border_spacing_y);
        ov!(border_collapse);
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
        ov!(column_gap);
        self.margin.overlay(&other.margin);
        self.padding.overlay(&other.padding);
        self.border_width.overlay(&other.border_width);
        // `background_image` isn't `Copy` (see its field doc comment), so it
        // can't go through the `ov!` macro above -- same last-writer-wins
        // semantics, just spelled with an explicit `.clone()`.
        if other.background_image.is_some() {
            self.background_image = other.background_image.clone();
        }
        // packet/css-grid: `grid_template_columns`/`rows` are `Vec`-shaped
        // (not `Copy`), same reason as `background_image` right above.
        if other.grid_template_columns.is_some() {
            self.grid_template_columns = other.grid_template_columns.clone();
        }
        if other.grid_template_rows.is_some() {
            self.grid_template_rows = other.grid_template_rows.clone();
        }
        // Custom properties and var()-bearing deferred declarations both
        // cascade with the same last-write-wins-by-name semantics as every
        // typed field above (packet T1a) — can't go through the `ov!` macro
        // (that macro is `Option<Copy>`-only; these are `Vec`s of name/token
        // pairs), so it's spelled out by hand, mirroring `background_image`'s
        // explicit merge just above. Custom-property names are matched
        // CASE-SENSITIVELY (`custom`'s own field doc comment); deferred
        // (ordinary) property names are already lowercased by the time they
        // land here.
        for (name, tokens) in &other.custom {
            match self.custom.iter_mut().find(|(n, _)| n == name) {
                Some(entry) => entry.1 = tokens.clone(),
                None => self.custom.push((name.clone(), tokens.clone())),
            }
        }
        for (name, tokens) in &other.deferred {
            match self.deferred.iter_mut().find(|(n, _)| n == name) {
                Some(entry) => entry.1 = tokens.clone(),
                None => self.deferred.push((name.clone(), tokens.clone())),
            }
        }
    }
}

/// The custom-property environment visible to one element during `var()`
/// substitution (packet T1a): every `--name -> raw tokens` pair inherited
/// from the parent element, overlaid with this element's own `--name`
/// declarations (`Declarations::custom`) — exactly mirroring how every other
/// property cascades and inherits, just carried as a side channel alongside
/// `ComputedStyle` rather than as a typed field ON it (the brief for this
/// packet is explicit that `ComputedStyle` — the frozen contract with
/// layout/paint — must NOT grow a field for this). `cascade::visit` threads
/// one `Env` per element down its walk (in `Frame::Enter`), builds each
/// child's `Env` via `child` below, and discards it once `cascade::resolve`
/// has used it to substitute that element's `deferred` declarations — an
/// `Env` never outlives the cascade pass that built it.
///
/// Values are stored UNSUBSTITUTED (exactly as declared) rather than
/// pre-resolved: a chain like `--a: var(--b); --b: blue;` only makes sense
/// if looking up `--a` returns the raw `var(--b)` tokens, which
/// `substitute_vars` then resolves recursively at the point of use — trying
/// to pre-resolve `custom` entries up front would need the SAME recursive
/// substitution machinery anyway, with no benefit (every custom property
/// gets looked up at most a handful of times per cascade pass in practice).
#[derive(Debug, Clone, Default)]
pub(crate) struct Env(Vec<(Box<str>, Vec<Token>)>);

impl Env {
    /// Build a child element's environment from this (parent) one: clone the
    /// inherited chain and overlay `own` (that child's own `Declarations::
    /// custom`), last-write-wins by name — the same semantics `Declarations::
    /// overlay` already uses for the custom-property side of a single
    /// declaration block, just extended across the inheritance chain here.
    /// Siblings must never see each other's custom properties, so this
    /// always returns a fresh, independent `Env` rather than mutating `self`.
    pub(crate) fn child(&self, own: &[(Box<str>, Vec<Token>)]) -> Env {
        let mut next = self.0.clone();
        for (name, tokens) in own {
            match next.iter_mut().find(|(n, _)| n == name) {
                Some(entry) => entry.1 = tokens.clone(),
                None => next.push((name.clone(), tokens.clone())),
            }
        }
        Env(next)
    }

    /// Look up a custom property's raw (unsubstituted) token value by its
    /// CASE-SENSITIVE name, if it's defined anywhere in the inheritance
    /// chain up to and including this element.
    fn get(&self, name: &str) -> Option<&[Token]> {
        self.0.iter().find(|(n, _)| n.as_ref() == name).map(|(_, t)| t.as_slice())
    }
}

/// Hard recursion-depth backstop for `var()` substitution (packet T1a): a
/// document-controlled stylesheet is hostile input, and while `substitute_
/// vars` below also does PRECISE cycle detection (a name that references
/// itself directly or transitively is caught immediately via `visiting`,
/// well before this cap could matter), this cap exists as a backstop for any
/// pathological shape that tracking doesn't anticipate — e.g. a very long
/// but strictly acyclic chain, or thousands of nested `var(--x, var(--x,
/// var(--x, ...)))` fallbacks all missing the same `--x`. 64 is generous for
/// any real stylesheet's custom-property chain while keeping the native
/// recursion this function uses (bounded by this constant, unlike
/// `cascade::visit`'s explicit-stack walk) provably shallow.
const VAR_SUBSTITUTION_DEPTH_CAP: u32 = 64;

/// Substitute every `var(--name)` / `var(--name, <fallback>)` function in
/// `tokens` against `env`, recursively (a custom property's own value may
/// itself contain `var()` — see `Env`'s doc comment). Returns `None` — CSS's
/// own term for this is "invalid at computed-value time" — if ANY `var()` in
/// `tokens` fails to resolve: `--name` is undefined in `env` and has no
/// fallback, `--name` is malformed (`var()` with no leading custom-property
/// ident), OR resolving it would exceed `VAR_SUBSTITUTION_DEPTH_CAP` nested
/// substitutions. The caller (`cascade::resolve`) treats `None` as "this
/// declaration doesn't apply" — the property falls back to its ordinary
/// inherited-or-initial value, never a crash, never a garbage value.
///
/// `visiting` carries the chain of custom-property names CURRENTLY being
/// substituted (i.e. the names on the path from the top-level `var()` down
/// to this recursive call) — a direct or transitive self-reference (`--a:
/// var(--b); --b: var(--a)`) is caught the instant the cycle closes, by
/// name, rather than only by exhausting `depth`. `depth` is a separate
/// counter (not just `visiting.len()`) because it also grows through
/// FALLBACK substitution, which doesn't add a name to `visiting` at all
/// (fallback tokens aren't a custom-property lookup) but still must be
/// depth-capped against something like `var(--missing, var(--missing,
/// var(--missing, ...)))` nested thousands deep.
///
/// Total: `tokens` is only ever scanned forward (see `parse_var_args`), and
/// native recursion depth is hard-capped by `VAR_SUBSTITUTION_DEPTH_CAP` —
/// this can never panic, hang, or overflow the stack, on any input.
pub(crate) fn substitute_vars(tokens: &[Token], env: &Env, visiting: &[&str], depth: u32) -> Option<Vec<Token>> {
    if depth > VAR_SUBSTITUTION_DEPTH_CAP {
        return None;
    }
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Function(f) if f.eq_ignore_ascii_case("var") => {
                let (name_opt, fallback, next_i) = parse_var_args(tokens, i + 1);
                i = next_i;
                let name = name_opt?; // malformed `var()` (no leading ident): invalid
                if visiting.contains(&name.as_str()) {
                    return None; // direct or transitive cycle
                }
                match env.get(&name) {
                    Some(value_tokens) => {
                        let mut next_visiting: Vec<&str> = visiting.to_vec();
                        next_visiting.push(&name);
                        out.extend(substitute_vars(value_tokens, env, &next_visiting, depth + 1)?);
                    }
                    None => match fallback {
                        Some(fallback_tokens) => {
                            out.extend(substitute_vars(&fallback_tokens, env, visiting, depth + 1)?);
                        }
                        None => return None, // undefined custom property, no fallback: invalid
                    },
                }
            }
            other => {
                out.push(other.clone());
                i += 1;
            }
        }
    }
    Some(out)
}

/// Parse one `var(...)` function's arguments, given `tokens[i]` is the first
/// token AFTER the already-consumed `Function("var")` token. Returns
/// `(name, fallback, next)`: `name` is `Some(ident text)` iff the first
/// (non-whitespace) token is a `Token::Ident` — real CSS requires it to
/// start with `--`, but this simply doesn't matter here: if it doesn't, `Env
/// ::get` will never find a match for it (custom-property names are exactly
/// what `parser::parse_declaration_block` stores under `Declarations::
/// custom`, all of which DO start with `--`), so a malformed name behaves
/// identically to an undefined one — consistent with charter C2's "unknown/
/// unrecognized input fails to apply, never guessed at". `fallback` is
/// `Some(tokens)` iff a top-level comma follows the name — everything from
/// just after that comma up to the function's own matching `RParen`,
/// UNSPLIT on any inner commas (CSS custom-property fallbacks may themselves
/// contain commas, e.g. `var(--x, 1px, 2px)` is a single two-token fallback,
/// not three arguments). `next` is the index just past the matching
/// `RParen`, or `tokens.len()` if the function is unterminated (tolerated,
/// never panics — matches every other tolerant-of-truncation parser in this
/// module, e.g. `parse_url_function`). Total: the scan index only ever
/// advances, so this always terminates, regardless of nesting depth (nested
/// functions are skipped via a simple depth counter, not recursion).
fn parse_var_args(tokens: &[Token], start: usize) -> (Option<String>, Option<Vec<Token>>, usize) {
    let len = tokens.len();
    let mut j = start;
    while j < len && tokens[j] == Token::Whitespace {
        j += 1;
    }
    let name = match tokens.get(j) {
        Some(Token::Ident(s)) => {
            j += 1;
            Some(s.clone())
        }
        _ => None,
    };
    while j < len && tokens[j] == Token::Whitespace {
        j += 1;
    }
    let mut fallback: Option<Vec<Token>> = None;
    if j < len && tokens[j] == Token::Comma {
        j += 1;
        while j < len && tokens[j] == Token::Whitespace {
            j += 1;
        }
        fallback = Some(Vec::new());
    }
    let mut depth = 0i32;
    while j < len {
        match &tokens[j] {
            Token::RParen if depth == 0 => break,
            Token::LParen | Token::Function(_) => depth += 1,
            Token::RParen => depth -= 1,
            _ => {}
        }
        if let Some(fb) = fallback.as_mut() {
            fb.push(tokens[j].clone());
        }
        j += 1;
    }
    let next = if j < len && tokens[j] == Token::RParen { j + 1 } else { len };
    (name, fallback, next)
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

/// Parse an HTML color ATTRIBUTE value (as opposed to a CSS token stream --
/// `parse_color` above): `#rgb`/`#rrggbb`, the same hex WITHOUT a leading
/// `#` (vintage markup writes `bgcolor="FF0000"` freely), or a CSS named
/// color, tried in that order. Total: anything else (garbage, empty, a
/// partial hex run) simply fails to parse and the caller ignores the whole
/// attribute (charter C2 -- unknown/unhandled input is ignored, never
/// guessed at), never panics.
fn parse_html_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let hex_candidate = s.strip_prefix('#').unwrap_or(s);
    hex_color(hex_candidate).or_else(|| named_color(s))
}

/// The HTML4 `<font size>` scale (curated dialect: absolute values `1..=7`,
/// `3` the default) -- `resolve_font_size_attr` below maps an already-
/// resolved size 1..=7 to its pixel value; index 7 (and anything the caller
/// already clamped above it) falls through to the top of the scale.
fn html_font_size_scale_px(size: i32) -> f32 {
    match size {
        1 => 10.0,
        2 => 13.0,
        3 => 16.0,
        4 => 18.0,
        5 => 24.0,
        6 => 32.0,
        _ => 48.0, // 7, and anything clamped to it
    }
}

/// Parse a `<font size="...">` attribute value into a resolved pixel size,
/// per the HTML4 font-size scale (brief: packet/presentational-attrs).
/// Absolute values (`"1"`..`"7"`, no leading sign) map directly and OUT-OF-
/// RANGE absolute values are ignored outright (no clamping) -- `"0"`/`"8"`
/// are simply invalid, not "meant" 1 or 7. A leading `"+"`/`"-"` makes the
/// value RELATIVE: N steps from the default size 3, clamped into `[1, 7]`
/// before mapping (real HTML: relative sizes always resolve to something,
/// they just saturate at the scale's ends). Total: any non-digit run after
/// an optional sign (or no digits at all) fails to parse and the whole
/// attribute is ignored -- never panics.
fn font_size_attr_px(raw: &str) -> Option<f32> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let (sign, digits): (i32, &str) = if let Some(rest) = s.strip_prefix('+') {
        (1, rest)
    } else if let Some(rest) = s.strip_prefix('-') {
        (-1, rest)
    } else {
        (0, s)
    };
    // Digits only from here -- rejects garbage like "3.5", "++1" (the
    // remainder after one stripped sign still starts with a sign), or a
    // bare "+"/"-" with nothing after it, rather than letting `i32::parse`
    // itself tolerate a second leading sign it otherwise would.
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i32 = digits.parse().ok()?;
    if sign == 0 {
        // Absolute: exactly 1..=7, no clamping -- "0"/"8" are invalid, not
        // "meant" 1 or 7.
        (1..=7).contains(&n).then(|| html_font_size_scale_px(n))
    } else {
        // Relative: N steps from the default size 3, saturating into [1, 7].
        let size = (3 + sign * n).clamp(1, 7);
        Some(html_font_size_scale_px(size))
    }
}

/// The presentational-hint cascade tier (packet/presentational-attrs): the
/// vintage HTML presentational attributes a given element carries, folded
/// into `Declarations` exactly the way a matched CSS rule would be. The
/// cascade (`cascade::fold_matching_declarations`) slots this in as its own
/// tier -- above the UA sheet, below author CSS and inline `style=` -- so
/// these participate in real inheritance and can be overridden by author
/// CSS, unlike a post-cascade `ComputedStyle` mutation.
///
/// Curated to the handful vintage markup actually leans on (brief): `bgcolor`
/// on ANY element, `<font color>`/`<font size>`, `align=` on any element
/// EXCEPT `<img>` (whose `align` stays a float hint -- see
/// `layout::box_tree::apply_align_float_hint`, deliberately left untouched by
/// this packet), `<body text>`, and (packet/table-spacing) `<table
/// cellspacing>` (a non-inherited `border-spacing` this function CAN handle,
/// unlike `cellpadding` -- see `layout::box_tree::
/// apply_table_cellpadding_attribute`'s own doc comment for why that one is
/// stamped post-cascade instead). `<table border>` is also handled outside
/// this function, in `layout::box_tree`, for the same ancestor-relationship
/// reason as `cellpadding`. Everything else (and any element/attribute combo
/// not listed above) yields an empty `Declarations` -- including unparseable
/// attribute values, which are silently ignored rather than guessed at.
/// DEFERRED (not implemented; follow-up packets): `<body link/vlink/alink>`,
/// `<td width/height/valign/nowrap>`, `<font face>`.
pub(crate) fn presentational_hints(tag: &str, attrs: &AttrMap) -> Declarations {
    let mut d = Declarations::default();

    if let Some(bg) = attrs.get("bgcolor") {
        d.background_color = parse_html_color(bg);
    }

    if tag.eq_ignore_ascii_case("font") {
        if let Some(color) = attrs.get("color") {
            d.color = parse_html_color(color);
        }
        if let Some(size) = attrs.get("size") {
            d.font_size = font_size_attr_px(size).map(RawLength::Px);
        }
    }

    if tag.eq_ignore_ascii_case("body") {
        if let Some(text) = attrs.get("text") {
            d.color = parse_html_color(text);
        }
    }

    if tag.eq_ignore_ascii_case("table") {
        // `cellspacing="N"` (packet/table-spacing): a non-negative integer
        // pixel count -- same HTML4 plain-integer grammar as `border`
        // (`layout::box_tree::table_border_attribute`), so `"6.5"` is
        // unparseable (ignored), not rounded. `0` is a valid explicit value
        // (distinct from "absent"), same as `RawLength::Px(0.0)` for any
        // other length -- both axes get the same value, matching real
        // HTML4's `cellspacing` (there is no separate x/y attribute).
        if let Some(raw) = attrs.get("cellspacing") {
            if let Ok(n) = raw.trim().parse::<u32>() {
                d.border_spacing_x = Some(RawLength::Px(n as f32));
                d.border_spacing_y = Some(RawLength::Px(n as f32));
            }
        }

        // `<table border>` defaults to `border-collapse: collapse` (packet/
        // border-collapse) -- but ONLY when there's no `cellspacing`
        // attribute too: a bare `<table border=1>` should render as a clean
        // single-line grid (real vintage rendering), while `<table border=1
        // cellspacing=N>` explicitly asks for gaps between cells, so it must
        // stay `separate`. Presence of `border` alone is the signal (its
        // value isn't parsed/validated here -- `border="0"`/an unparseable
        // value still sets the collapse hint; `layout::box_tree`'s own
        // `table_border_attribute` independently decides whether any VISIBLE
        // border actually gets stamped, a fully orthogonal concern from the
        // border MODEL this hint selects). This is a presentational-tier
        // hint (`fold_matching_declarations`'s tier `1`), so author CSS
        // `border-collapse: separate` on the `<table>` itself still wins
        // (tier `2` beats tier `1` per property, `Declarations::overlay`).
        if attrs.get("border").is_some() && attrs.get("cellspacing").is_none() {
            d.border_collapse = Some(BorderCollapse::Collapse);
        }
    }

    if !tag.eq_ignore_ascii_case("img") {
        if let Some(align) = attrs.get("align") {
            d.text_align = match align.trim().to_ascii_lowercase().as_str() {
                "left" => Some(TextAlign::Left),
                "right" => Some(TextAlign::Right),
                "center" => Some(TextAlign::Center),
                "justify" => Some(TextAlign::Justify),
                _ => None, // top/middle/bottom/unknown: vertical-align territory, not this packet's scope.
            };
        }
    }

    d
}

/// Whether `name` (a `Token::Function` name) is a CSS gradient function
/// this packet recognizes (packet T1c): the three gradient shapes, plus
/// their `-webkit-`/`-moz-` legacy-prefixed spellings (still common in the
/// wild on 1996-2010s-era pages this browser targets) -- matched case-
/// insensitively, mirroring every other function-name check in this module
/// (`rgb`/`rgba`/`url`).
fn is_gradient_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "linear-gradient"
            | "radial-gradient"
            | "conic-gradient"
            | "-webkit-linear-gradient"
            | "-webkit-radial-gradient"
            | "-webkit-conic-gradient"
            | "-moz-linear-gradient"
            | "-moz-radial-gradient"
            | "-moz-conic-gradient"
    )
}

/// Extract a REPRESENTATIVE SOLID color from a CSS gradient function's
/// color stops (packet T1c): Stele has no gradient renderer (brief §4
/// curates gradients out entirely -- see this module's own top-level doc
/// comment), so rather than dropping a `background`/`background-image`/
/// `background-color` declaration whose value is a gradient outright
/// (real-world path: a `var()` custom property that resolves to one,
/// packet T1a -- see `apply_property`'s own `"background-color"` doc
/// comment), this picks the FIRST color stop that parses as a color and
/// uses THAT as a flat fallback -- closer to what the page's author
/// actually intended (SOME color, usually the gradient's own "start") than
/// falling back to no background at all, which is exactly the motivating
/// bug this packet fixes (light text vanishing against the default white
/// canvas because the background meant to sit behind it got silently
/// dropped).
///
/// `tokens[i]` must be the gradient's own `Token::Function(name)` (every
/// caller checks `is_gradient_function` first). Direction/shape/position
/// tokens (`180deg`, `to right`, `120% 140% at 50% -20%`, ...) and stop-
/// position percentages (`45%`) are simply skipped -- this only cares
/// about the FIRST token run that resolves to an actual color, the same
/// left-to-right color hunt `parse_background_color_component` already
/// does at the top level, just scoped to this one function's own argument
/// tokens.
///
/// Returns `(color, next)`: `next` is the index just past the function's
/// matching `RParen` (or `tokens.len()` if unterminated -- tolerated,
/// never panics, mirrors every other tolerant-of-truncation scan in this
/// module) regardless of whether a color was found, so callers can keep
/// scanning past a no-color-stops gradient rather than getting stuck.
/// Total: a nested function (e.g. an `rgba(...)` color stop) is skipped
/// via a depth counter, not recursion, and the scan index only ever
/// advances -- always terminates.
fn parse_gradient_first_color_stop(tokens: &[Token], i: usize) -> (Option<Color>, usize) {
    let mut j = i + 1;
    let mut depth = 0i32;
    let mut found: Option<Color> = None;
    while j < tokens.len() {
        match &tokens[j] {
            Token::RParen if depth == 0 => {
                j += 1;
                break;
            }
            Token::RParen => {
                depth -= 1;
                j += 1;
            }
            Token::Function(name) if found.is_none() && (name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba")) => {
                let start = j;
                let mut k = j + 1;
                while k < tokens.len() && tokens[k] != Token::RParen {
                    k += 1;
                }
                let end = (k + 1).min(tokens.len());
                found = parse_color(&tokens[start..end]);
                j = end;
            }
            Token::Function(_) => {
                depth += 1;
                j += 1;
            }
            Token::Hash(h) if found.is_none() => {
                found = hex_color(h);
                j += 1;
            }
            Token::Ident(name) if found.is_none() && named_color(name).is_some() => {
                found = named_color(name);
                j += 1;
            }
            _ => j += 1,
        }
    }
    (found, j)
}

/// `background-color`'s own value parser (packet T1c): ordinary `parse_
/// color` first (the overwhelming common case -- a plain color), falling
/// back to [`parse_gradient_first_color_stop`] when the first token is a
/// gradient function. Real CSS's `background-color` doesn't even accept a
/// gradient -- the path that actually reaches this is `var()` substitution
/// (packet T1a's `substitute_vars`, which runs BEFORE `apply_property`)
/// leaving `background-color`'s value holding a full gradient function
/// verbatim, e.g. `background-color: var(--brand-bg)` where `--brand-bg`
/// was itself declared as `radial-gradient(...)`. Charter C2's "extract
/// something useful rather than silently dropping" spirit (and this
/// packet's own motivating bug) makes that worth a defined fallback rather
/// than counting the whole declaration as ignored.
fn parse_background_color_value(tokens: &[Token]) -> Option<Color> {
    if let Some(c) = parse_color(tokens) {
        return Some(c);
    }
    match tokens.first() {
        Some(Token::Function(name)) if is_gradient_function(name) => parse_gradient_first_color_stop(tokens, 0).0,
        _ => None,
    }
}

/// Extract just the color component of a `background` shorthand value
/// (image/position/repeat/size are curated-out for now — image is a later
/// packet's scope, brief §4). Scans left to right for the first token that
/// resolves to a color, skipping anything that doesn't: a `url(...)`
/// function's contents (consumed to its matching `RParen` so a stray color-
/// looking token inside a URL, e.g. `url(red.png)`, can never be
/// misidentified as `background-color`), an `rgb()`/`rgba()` function
/// (consumed as one unit and handed to `parse_color`), a bare `Hash`, or a
/// color-named `Ident` — every other token (keywords like `no-repeat`,
/// `none`, `left`/`center`, or plain lengths for `background-position`) is
/// simply skipped. Total: the scan index only ever advances, so this always
/// terminates; `None` (not just "ignored image/position") is the correct
/// result for `background: none` or any value with no color component at
/// all — `apply_property` then counts the whole declaration against
/// `ignored_declarations`, matching charter C2 (unknown/unhandled parts are
/// ignored, never guessed at).
///
/// Packet T1c amendment: a gradient function (`linear-gradient`/`radial-
/// gradient`/`conic-gradient`, optionally `-webkit-`/`-moz-` prefixed) is
/// its own explicit arm rather than falling through to the generic
/// catch-all below — see [`parse_gradient_first_color_stop`]'s own doc
/// comment for the full rationale (Stele has no gradient renderer, so this
/// extracts a REPRESENTATIVE SOLID from the gradient's first parseable
/// color stop instead of silently dropping the declaration).
fn parse_background_color_component(tokens: &[Token]) -> Option<Color> {
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Function(name) if name.eq_ignore_ascii_case("url") => {
                i += 1;
                while i < tokens.len() && tokens[i] != Token::RParen {
                    i += 1;
                }
                i += 1; // step past the RParen (or past `len` if unterminated)
            }
            Token::Function(name) if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") => {
                let start = i;
                i += 1;
                while i < tokens.len() && tokens[i] != Token::RParen {
                    i += 1;
                }
                let end = (i + 1).min(tokens.len());
                if let Some(c) = parse_color(&tokens[start..end]) {
                    return Some(c);
                }
                i = end;
            }
            Token::Function(name) if is_gradient_function(name) => {
                let (color, next) = parse_gradient_first_color_stop(tokens, i);
                if let Some(c) = color {
                    return Some(c);
                }
                i = next;
            }
            Token::Hash(_) => {
                if let Some(c) = parse_color(&tokens[i..i + 1]) {
                    return Some(c);
                }
                i += 1;
            }
            Token::Ident(name) if named_color(name).is_some() => {
                return named_color(name);
            }
            _ => i += 1,
        }
    }
    None
}

/// The literal text one CSS token contributes inside an UNQUOTED `url(...)`
/// value (packet bg-image). The tokenizer (`style::tokenizer`) has no
/// special-cased `url-token` production — an unquoted URL like
/// `images/tile.png` tokenizes into a run of ordinary tokens (`Ident`,
/// `Dot`, `Delim('/')`, ...), so reconstructing the original text means
/// concatenating each token's own text back together. Covers exactly the
/// token kinds a bare, unescaped URL can plausibly tokenize into; anything
/// else (`Whitespace`, a nested `Function`, a brace/semicolon, ...) returns
/// `None` and the caller treats the whole `url(...)` as malformed (charter
/// C2: fails to parse, never guessed at) rather than silently dropping a
/// token.
fn unquoted_url_token_text(t: &Token) -> Option<String> {
    Some(match t {
        Token::Ident(s) => s.clone(),
        Token::Hash(s) => format!("#{s}"),
        Token::Number(n) => format!("{n}"),
        Token::Dimension(n, unit) => format!("{n}{unit}"),
        Token::Percentage(n) => format!("{n}%"),
        Token::Colon => ":".to_string(),
        Token::Comma => ",".to_string(),
        Token::Dot => ".".to_string(),
        Token::Star => "*".to_string(),
        Token::Delim(c) => c.to_string(),
        _ => return None,
    })
}

/// Parse one `url(...)` function, given `tokens[i] == Token::Function("url")`
/// (case-insensitive — checked by every caller before calling this). Returns
/// the URL string and the index just past the matching `RParen`, or `None`
/// if the parens don't hold a single quoted string or a well-formed run of
/// unquoted url tokens (see `unquoted_url_token_text`) — including an
/// unterminated `url(` with no matching `RParen` at all, or an empty
/// `url()`. Total: `k` only ever advances, so this always terminates.
fn parse_url_function(tokens: &[Token], i: usize) -> Option<(String, usize)> {
    let mut j = i + 1;
    while j < tokens.len() && tokens[j] == Token::Whitespace {
        j += 1;
    }
    if let Some(Token::Str(s)) = tokens.get(j) {
        let mut k = j + 1;
        while k < tokens.len() && tokens[k] == Token::Whitespace {
            k += 1;
        }
        return if tokens.get(k) == Some(&Token::RParen) { Some((s.clone(), k + 1)) } else { None };
    }
    let mut s = String::new();
    let mut k = j;
    while k < tokens.len() && tokens[k] != Token::RParen {
        s.push_str(&unquoted_url_token_text(&tokens[k])?);
        k += 1;
    }
    if k >= tokens.len() || s.is_empty() {
        return None; // unterminated `url(` or an empty `url()`
    }
    Some((s, k + 1))
}

/// Extract just the `url(...)` component of a `background` shorthand value
/// (color/position/repeat/size are parsed elsewhere or curated-out — see
/// `parse_background_color_component`'s own doc comment; this is that
/// function's packet-bg-image companion). Scans left to right for the FIRST
/// `url(...)` function; every other token (a color, `no-repeat`, a length
/// for `background-position`, ...) is simply skipped, including another
/// function's own contents (e.g. `rgb(...)`, consumed to its matching
/// `RParen` so it can never be mistaken for a url). `None` for `background:
/// none` or any value with no (well-formed) `url(...)` at all — matching
/// `parse_background_color_component`'s own "ignored, not guessed at"
/// contract (charter C2). Total: the scan index only ever advances.
fn parse_background_image_component(tokens: &[Token]) -> Option<String> {
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Function(name) if name.eq_ignore_ascii_case("url") => {
                if let Some((url, _next)) = parse_url_function(tokens, i) {
                    return Some(url);
                }
                // A malformed url(...) here means this whole declaration is
                // malformed too -- real CSS doesn't keep scanning past a
                // broken function looking for another one.
                return None;
            }
            Token::Function(_) => {
                i += 1;
                while i < tokens.len() && tokens[i] != Token::RParen {
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
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

/// Diagnosis note (packet/t3-inline-spacing, the D3 investigation): `rem`
/// is deliberately NOT one of the recognized units below -- this engine has
/// no root-element-font-size plumbing through `cascade::resolve` (only the
/// current/parent node's own already-cascaded font size is threaded down
/// the walk), so a real, correctly-scoped `rem` (relative to the ROOT
/// element's font-size, not the current element's) can't be resolved here
/// without restructuring that walk. A `<N>rem` token therefore falls
/// through to `_ => None` below exactly like any other unrecognized unit,
/// and the WHOLE declaration it appears in is dropped (this function's own
/// caller convention: an unparseable value simply fails to apply, module
/// docs). This is real, observable scope -- e.g. every `rem`-based
/// `margin`/`padding`/`font-size`/... declaration in `fixtures/
/// httpforever.html` silently falls back to its CSS initial value today.
///
/// [`token_to_gap_length`] right below is a SEPARATE, narrowly-scoped
/// carve-out of this rule for `gap` alone (an `em`≈`rem` approximation,
/// not real root-relative `rem`) -- see that function's own doc comment
/// for why `gap` specifically can afford the approximation and every other
/// property here still can't.
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

/// `gap`-only length parsing (packet/t3-inline-spacing): [`token_to_raw_
/// length`] plus ONE scoped extra case -- a `rem` dimension token maps to
/// `RawLength::Em`. This is a deliberate approximation, NOT real `rem`
/// support: it resolves against the CURRENT element's font-size (an `em`),
/// not the root element's, so it's only correct when the two coincide (no
/// `html { font-size: ... }` override -- true of every fixture in this
/// repo today, `fixtures/httpforever.html` included). Real root-relative
/// `rem` needs `cascade::resolve` to thread a second, root-pinned font
/// size down its walk (see [`token_to_raw_length`]'s own doc comment) --
/// out of scope for a spacing packet.
///
/// Scoped to `gap` alone, not folded into `token_to_raw_length` itself:
/// every OTHER `rem`-based declaration on a real page (margin, padding,
/// font-size, ...) stays exactly as unparseable as before this packet --
/// widening this to the general parser would reflow the WHOLE page against
/// an approximation, a much bigger and unrelated blast radius than "make a
/// flex row's gap advance by roughly the right amount." A gap only ever
/// needs a plausible-magnitude advance to keep two elements from visually
/// fusing (and this packet's zero-advance synthesis rule already covers
/// gap magnitudes it still gets wrong) -- correctness bar low enough that
/// the em/rem conflation is an acceptable, narrowly-scoped approximation
/// here specifically.
fn token_to_gap_length(t: &Token) -> Option<RawLength> {
    // `tokenizer::tokenize` already lowercases every unit string (see its
    // own doc comment), so a plain `==` matches the same way every other
    // unit arm in `token_to_raw_length` does -- no case-insensitive
    // comparison needed here either.
    if let Token::Dimension(v, unit) = t {
        if unit == "rem" {
            return Some(RawLength::Em(*v));
        }
    }
    token_to_raw_length(t)
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

/// Shared grammar for `border` and `border-top` (packet/hr-rule):
/// width/style/color in any order. Real CSS invalidates the whole shorthand
/// if any single component is unrecognized — it does not silently apply the
/// components it understood and drop the rest. `border-width` also has no
/// percentage form in CSS (unlike margin/padding), so a `%` token must fail
/// to parse as a width here rather than resolve to `0px` later. `None` for
/// an empty/entirely-unrecognized value (nothing set at all) or the first
/// unrecognized token (whole declaration invalid) — the caller (`apply_
/// property`) treats either the same way: not applied.
fn parse_border_raw(tokens: &[Token]) -> Option<BorderRaw> {
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
        return None; // unrecognized token: the whole shorthand is invalid
    }
    if b.width.is_none() && b.style.is_none() && b.color.is_none() {
        return None;
    }
    Some(b)
}

/// `font` shorthand (CSS2.1 §15.3): `[ <font-style> || <font-weight> ]?
/// <font-size> [ / <line-height> ]? <font-family>`. Curated like `border`/
/// `background` above: extracts font-size (this shorthand's mandatory
/// component, and the whole reason this function exists — see this
/// function's caller-site comment in `apply_property` for the bug this
/// closes), plus font-family/line-height/font-style/font-weight wherever
/// present. `font-variant` and the CSS2.1 system-font keywords (`caption`/
/// `icon`/`menu`/`message-box`/`small-caption`/`status-bar`) are NOT
/// recognized — no fixture in this repo uses them, and (unlike `border`'s
/// "any bad token invalidates everything" rule) an unrecognized token here
/// before the size is found is just skipped, not fatal: font-family lists
/// routinely carry punctuation/keywords this loop doesn't special-case, and
/// being lenient about the leading style/weight/variant slot matches
/// `background`'s "apply what we understood" precedent instead of
/// `border`'s stricter one.
///
/// A missing font-size fails the WHOLE shorthand (`None`) — real CSS
/// invalidates `font: bold sans-serif` outright rather than half-applying
/// it, since font-size has no default to fall back to within the
/// shorthand's own grammar.
fn apply_font_shorthand(tokens: &[Token], d: &mut Declarations) -> bool {
    let mut size: Option<RawLength> = None;
    let mut weight: Option<FontWeight> = None;
    let mut style: Option<FontStyle> = None;
    let mut line_height: Option<RawLineHeight> = None;
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        if let Token::Ident(s) = t {
            match s.to_ascii_lowercase().as_str() {
                "italic" | "oblique" => {
                    style = Some(FontStyle::Italic);
                    i += 1;
                    continue;
                }
                "bold" | "bolder" => {
                    weight = Some(FontWeight::Bold);
                    i += 1;
                    continue;
                }
                "lighter" => {
                    weight = Some(FontWeight::Normal);
                    i += 1;
                    continue;
                }
                _ => {} // font-variant/"normal"/system-font keyword/garbage: skip, see doc comment
            }
        } else if let Token::Number(n) = t {
            // A bare number before the size is a numeric font-weight
            // (100-900) — `token_to_raw_length` never parses a nonzero
            // `Number` token (only Dimension/Percentage/unitless-0), so
            // this is unambiguous with the size search below.
            weight = Some(if *n >= 600.0 { FontWeight::Bold } else { FontWeight::Normal });
            i += 1;
            continue;
        } else if let Some(l) = token_to_raw_length(t) {
            size = Some(l);
            i += 1;
            if i < tokens.len() && tokens[i] == Token::Delim('/') {
                i += 1;
                if let Some(lh_tok) = tokens.get(i) {
                    line_height = match lh_tok {
                        Token::Ident(s) if s.eq_ignore_ascii_case("normal") => Some(RawLineHeight::Normal),
                        Token::Number(n) => Some(RawLineHeight::Number(*n)),
                        other => token_to_raw_length(other).map(RawLineHeight::Length),
                    };
                    i += 1;
                }
            }
            break; // everything after size[/line-height] is the font-family list
        }
        i += 1;
    }
    let Some(size) = size else { return false }; // font-size is mandatory
    d.font_size = Some(size);
    if let Some(w) = weight {
        d.font_weight = Some(w);
    }
    if let Some(s) = style {
        d.font_style = Some(s);
    }
    if let Some(lh) = line_height {
        d.line_height = Some(lh);
    }
    if let Some(f) = classify_font_family(&tokens[i..]) {
        d.font_family = Some(f);
    }
    true
}

/// Apply one already-lowercased property `name` with its (whitespace-
/// filtered) value tokens onto `d`. Returns whether it was recognized *and*
/// parsed successfully — the caller counts `false` against
/// `Stylesheet::ignored_declarations` (charter C2's ignore-unknown treaty).
pub(crate) fn apply_property(name: &str, tokens: &[Token], d: &mut Declarations) -> bool {
    match name {
        "color" => parse_color(tokens).map(|c| d.color = Some(c)).is_some(),
        // packet T1c: `parse_background_color_value` is `parse_color` plus
        // a gradient-first-stop fallback — see its own doc comment for why
        // `background-color` (unlike `color`, `border-color`, ...) needs
        // that fallback and the others deliberately don't.
        "background-color" => parse_background_color_value(tokens).map(|c| d.background_color = Some(c)).is_some(),
        // Packet bg-image: `background-image: url(...)` sets the raw URL;
        // anything else (`none`, a garbage value, a malformed `url(...)`) is
        // simply not applied — see `parse_url_function`'s doc comment.
        //
        // Packet T1c: a gradient function IS valid real CSS here (`<image>`
        // includes `<gradient>`) — Stele has no gradient renderer, so it
        // sets `background_color` to the gradient's first parseable color
        // stop instead (see `parse_gradient_first_color_stop`'s own doc
        // comment), leaving `background_image` itself unset (a gradient
        // isn't a fetchable URL for `bg_images::collect_bg_images`).
        "background-image" => match tokens.first() {
            Some(Token::Function(name)) if name.eq_ignore_ascii_case("url") => {
                parse_url_function(tokens, 0).map(|(url, _)| d.background_image = Some(url.into_boxed_str())).is_some()
            }
            Some(Token::Function(name)) if is_gradient_function(name) => {
                parse_gradient_first_color_stop(tokens, 0).0.map(|c| d.background_color = Some(c)).is_some()
            }
            _ => false,
        },
        // `background` shorthand: extracts BOTH the color (packet D36) AND
        // the image (packet bg-image) components — `#fff url(x.png)
        // no-repeat` sets both fields; `position`/`size`/`repeat` remain
        // curated-out. Succeeds (and is NOT counted against
        // `ignored_declarations`) if EITHER half parsed, matching real CSS's
        // "the shorthand succeeds if any of its longhands got a value" spirit
        // without this codebase's simplification of tracking per-longhand
        // validity.
        "background" => {
            let color = parse_background_color_component(tokens).map(|c| d.background_color = Some(c)).is_some();
            let image = parse_background_image_component(tokens).map(|u| d.background_image = Some(u.into_boxed_str())).is_some();
            color || image
        }
        // `font` shorthand (packet/acid1-coherence): see `apply_font_
        // shorthand`'s own doc comment above for the grammar it covers and
        // the bug this closes (`html { font: 10px/1 ... }` used to parse as
        // nothing at all, leaving font-size at the UA default).
        "font" => apply_font_shorthand(tokens, d),
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
            // packet/css-grid: see `Display::Grid`'s own doc comment
            // (`style/computed.rs`) for the full contract.
            Some("grid") => {
                d.display = Some(Display::Grid);
                true
            }
            Some("table") => {
                d.display = Some(Display::Table);
                true
            }
            Some("table-row") => {
                d.display = Some(Display::TableRow);
                true
            }
            Some("table-cell") => {
                d.display = Some(Display::TableCell);
                true
            }
            Some("table-row-group") => {
                d.display = Some(Display::TableRowGroup);
                true
            }
            // packet/display-list-item: real CSS's own value for generating
            // a list marker (bullet/ordinal) — see `Display::ListItem`'s own
            // doc comment (`style/computed.rs`) for the full contract.
            Some("list-item") => {
                d.display = Some(Display::ListItem);
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
        "border" => match parse_border_raw(tokens) {
            Some(b) => {
                d.border = Some(b);
                true
            }
            None => false,
        },
        // `border-top` (packet/hr-rule): same shorthand grammar as `border`
        // (width/style/color in any order, whole declaration invalid on any
        // unrecognized token), but lands in its own `Declarations` field --
        // see that field's own doc comment for why it can't just reuse
        // `d.border`.
        "border-top" => match parse_border_raw(tokens) {
            Some(b) => {
                d.border_top = Some(b);
                true
            }
            None => false,
        },
        // packet/acid1-content-box: `border-width`/`border-style`/
        // `border-color` as their OWN longhands (not folded into `border`/
        // `border-top`'s shorthand grammar above) -- see `Declarations::
        // border_width`'s own doc comment for why `fixtures/css1-float-
        // 5526c.html`'s `blockquote` needs exactly this shape.
        // `token_to_border_width`'s percent-rejecting conversion is reused
        // here too (same reasoning as the `border-spacing` 1/2-value arm
        // below): `border-width` has no percentage form in CSS either.
        "border-width" => apply_edges_shorthand(tokens, &mut d.border_width, token_to_border_width),
        "border-style" => match keyword(tokens).as_deref() {
            Some("solid") => {
                d.border_style = Some(BorderStyle::Solid);
                true
            }
            // Curated set is solid-only (brief §4), matching `parse_border_
            // raw`'s own style-keyword mapping exactly: every other named
            // style is a real CSS keyword that resolves to "no visible
            // border" here rather than being rejected outright.
            Some("none" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset") => {
                d.border_style = Some(BorderStyle::None);
                true
            }
            _ => false,
        },
        "border-color" => parse_color(tokens).map(|c| d.border_color = Some(c)).is_some(),
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
        // `border-spacing: <length> <length>?` (packet/table-spacing): 1
        // value sets both axes, 2 values set x then y -- CSS's own grammar
        // for this property. No percentage form (unlike margin/padding), so
        // `token_to_border_width`'s percent-rejecting conversion is reused
        // here rather than `token_to_raw_length` (which accepts `%`). Any
        // other token count (0 or 3+) or an unrecognized/percent token
        // invalidates the WHOLE declaration -- same "no partial application"
        // rule every other shorthand in this file follows.
        "border-spacing" => match tokens.len() {
            1 => match token_to_border_width(&tokens[0]) {
                Some(l) => {
                    d.border_spacing_x = Some(l);
                    d.border_spacing_y = Some(l);
                    true
                }
                None => false,
            },
            2 => match (token_to_border_width(&tokens[0]), token_to_border_width(&tokens[1])) {
                (Some(x), Some(y)) => {
                    d.border_spacing_x = Some(x);
                    d.border_spacing_y = Some(y);
                    true
                }
                _ => false,
            },
            _ => false,
        },
        // `border-collapse: separate | collapse` (packet/border-collapse):
        // plain keyword property, same shape as `float`/`clear`/etc. above.
        // Any other keyword (or no keyword at all) is unrecognized and the
        // whole declaration is simply not applied (charter C2).
        "border-collapse" => match keyword(tokens).as_deref() {
            Some("collapse") => {
                d.border_collapse = Some(BorderCollapse::Collapse);
                true
            }
            Some("separate") => {
                d.border_collapse = Some(BorderCollapse::Separate);
                true
            }
            _ => false,
        },
        // `gap: <row-gap>` or `gap: <row-gap> <column-gap>` (packet/
        // t3-inline-spacing, one half of the D3 fix). Same one-or-two-value
        // shape as `border-spacing` right above. Before this packet, this
        // arm read only `tokens.first()`, silently dropping a declared
        // column-gap -- for the common `flex-direction: row` case (CSS's
        // own default) the horizontal advance BETWEEN adjacent flex items
        // is governed by column-gap, not row-gap, so a two-value px/pt/em
        // declaration like `gap: 2px 20px` used to silently render with
        // only 2px of item-to-item spacing. See `layout::block::
        // apply_flex`, which now reads `column_gap` (falling back to `gap`
        // when unset) for the taffy width axis.
        //
        // NOTE on `fixtures/httpforever.html`'s own `.footer__projects {
        // gap: .35rem 1.1rem; }`: this arm parses through [`token_to_gap_
        // length`], not the general [`token_to_raw_length`] -- see that
        // helper's own doc comment for why `gap` alone gets a scoped `rem`
        // ≈ `em` approximation (real root-relative `rem` needs `cascade::
        // resolve` to thread a second, root-pinned font size down its
        // walk, out of scope here) rather than every other rem-based
        // property on the page also being reflowed. So `.35rem 1.1rem`
        // DOES parse today (row-gap ≈ 5.6px, column-gap ≈ 17.6px at this
        // fixture's 16px root font-size) -- this arm's fix alone already
        // moves that fixture's footer links apart. The OTHER half of the
        // D3 fix (`backend::tty::synthesize_gap_start_col` / `backend::
        // raster::synthesize_gap_rect`) is what guarantees distinct
        // adjacent flex items never visually fuse EVEN when a gap can't be
        // resolved at all (e.g. a `calc()`/`clamp()`-wrapped gap, or any
        // other value shape this parser doesn't understand) -- a second,
        // independent safety net, not the only thing standing between this
        // fixture and a jammed footer.
        "gap" => match tokens.len() {
            1 => match tokens.first().and_then(token_to_gap_length) {
                // Percent gap needs a containing-block size this layer
                // doesn't have; out of scope for v0 (unlike width/margin/
                // padding, gap has no percent-carrying computed
                // representation to defer to).
                Some(RawLength::Percent(_)) | None => false,
                Some(l) => {
                    d.gap = Some(l);
                    d.column_gap = None;
                    true
                }
            },
            2 => match (tokens.first().and_then(token_to_gap_length), tokens.get(1).and_then(token_to_gap_length)) {
                (Some(row), Some(col))
                    if !matches!(row, RawLength::Percent(_)) && !matches!(col, RawLength::Percent(_)) =>
                {
                    d.gap = Some(row);
                    d.column_gap = Some(col);
                    true
                }
                _ => false,
            },
            _ => false,
        },
        // packet/css-grid: a track list of `<length>`/`<percentage>`/
        // `<flex>` (`fr`) values and/or `repeat(<count>, <track>+)` (with
        // `minmax(<min>, <max>)` inside either position) -- see
        // [`parse_grid_template`]'s own doc comment for the exact grammar
        // subset this covers. Real CSS invalidates the WHOLE declaration on
        // any unparseable component (same rule `apply_edges_shorthand`
        // already follows for margin/padding/border shorthands) — never a
        // partial/best-effort template.
        "grid-template-columns" => match parse_grid_template(tokens) {
            Some(v) => {
                d.grid_template_columns = Some(v);
                true
            }
            None => false,
        },
        "grid-template-rows" => match parse_grid_template(tokens) {
            Some(v) => {
                d.grid_template_rows = Some(v);
                true
            }
            None => false,
        },
        _ => false,
    }
}

/// packet/css-grid: convert one track-size token into a
/// [`RawGridTrackSize`] -- a bare `<flex>` (`fr`) dimension, or anything
/// [`token_to_raw_length`] already understands (`<length>`/`<percentage>`).
/// Negative `fr` values are invalid (CSS Grid §7.2.3) and rejected here
/// (falls through to `token_to_raw_length`, which doesn't understand a
/// `"fr"` unit either, so the whole token is correctly unparseable rather
/// than silently clamped).
fn token_to_grid_track_size(t: &Token) -> Option<RawGridTrackSize> {
    if let Token::Dimension(v, unit) = t {
        // `tokenizer::tokenize` already lowercases every unit string, so a
        // plain `==` is enough (same reasoning `token_to_gap_length`'s own
        // doc comment gives for its `"rem"` check).
        if unit == "fr" && *v >= 0.0 {
            return Some(RawGridTrackSize::Fr(*v));
        }
    }
    token_to_raw_length(t).map(RawGridTrackSize::Length)
}

/// packet/css-grid: parse one grid track starting at `tokens[i]` -- either
/// `minmax(<min>, <max>)` or a bare track size -- returning the parsed
/// [`RawGridTrack`] and the index just past it. `None` on any malformed
/// shape (missing comma/paren, unparseable size, `tokens` too short),
/// matching this module's total/never-panics contract.
fn parse_grid_track(tokens: &[Token], i: usize) -> Option<(RawGridTrack, usize)> {
    match tokens.get(i)? {
        Token::Function(name) if name.eq_ignore_ascii_case("minmax") => {
            let min = token_to_grid_track_size(tokens.get(i + 1)?)?;
            if tokens.get(i + 2)? != &Token::Comma {
                return None;
            }
            let max = token_to_grid_track_size(tokens.get(i + 3)?)?;
            if tokens.get(i + 4)? != &Token::RParen {
                return None;
            }
            Some((RawGridTrack::MinMax(min, max), i + 5))
        }
        t => token_to_grid_track_size(t).map(|size| (RawGridTrack::Bare(size), i + 1)),
    }
}

/// packet/css-grid: parse `repeat(<count>, <track>+)` starting right AFTER
/// the already-consumed `repeat(` function token (i.e. `tokens[i]` is the
/// count argument) -- `<count>` is `auto-fill`, `auto-fit`, or a positive
/// integer (CSS Grid §7.2.3.1); the track list is one or more tracks (a
/// nested `repeat()` is real CSS's own restriction too, so [`parse_grid_
/// track`] not accepting one here is correct, not a limitation). Returns
/// the parsed component and the index just past the closing `)`.
fn parse_grid_repeat(tokens: &[Token], i: usize) -> Option<(RawGridTemplateComponent, usize)> {
    let count = match tokens.get(i)? {
        Token::Ident(s) if s.eq_ignore_ascii_case("auto-fill") => RawGridRepetitionCount::AutoFill,
        Token::Ident(s) if s.eq_ignore_ascii_case("auto-fit") => RawGridRepetitionCount::AutoFit,
        Token::Number(n) if *n >= 1.0 && n.fract() == 0.0 => RawGridRepetitionCount::Count(*n as u16),
        _ => return None,
    };
    if tokens.get(i + 1)? != &Token::Comma {
        return None;
    }
    let mut idx = i + 2;
    let mut tracks = Vec::new();
    loop {
        if tokens.get(idx)? == &Token::RParen {
            break;
        }
        let (track, next_i) = parse_grid_track(tokens, idx)?;
        tracks.push(track);
        idx = next_i;
    }
    idx += 1; // consume the closing `)`
    if tracks.is_empty() {
        return None;
    }
    Some((RawGridTemplateComponent::Repeat(count, tracks), idx))
}

/// packet/css-grid: `grid-template-columns`/`grid-template-rows`. Supports
/// a track list mixing bare tracks and `repeat(<count>, <track>+)`
/// (`minmax()` valid in either position) -- e.g. `1fr 1fr 1fr`, `200px
/// 1fr`, `repeat(auto-fill, minmax(200px, 1fr))`, `200px repeat(2, 1fr)`.
/// NOT covered (real CSS grid features this packet leaves unparsed --
/// documented as deferred in the PR description): named lines
/// (`[line-name]`), the `subgrid`/`masonry` keywords, and `grid-template-
/// areas`' string-literal row syntax. `None` (declaration doesn't apply) on
/// empty input or any unparseable component -- never a partial template.
fn parse_grid_template(tokens: &[Token]) -> Option<Vec<RawGridTemplateComponent>> {
    if tokens.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Function(name) if name.eq_ignore_ascii_case("repeat") => {
                let (component, next_i) = parse_grid_repeat(tokens, i + 1)?;
                out.push(component);
                i = next_i;
            }
            _ => {
                let (track, next_i) = parse_grid_track(tokens, i)?;
                out.push(RawGridTemplateComponent::Single(track));
                i = next_i;
            }
        }
    }
    Some(out)
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
    fn border_top_longhand_parses_into_its_own_field_leaving_border_unset() {
        // packet/hr-rule: `<hr>`'s UA rule needs a solid TOP-only border,
        // distinct from the `border` shorthand (which sets all four sides
        // uniformly via `resolve_border`'s `Edges::all`) -- `border-top`
        // must land in its own `Declarations` field so the cascade can
        // override just the top `BorderSide`.
        let mut d = Declarations::default();
        assert!(apply_property("border-top", &toks("1px solid #808080"), &mut d));
        let b = d.border_top.unwrap();
        assert_eq!(b.width, Some(RawLength::Px(1.0)));
        assert_eq!(b.style, Some(BorderStyle::Solid));
        assert_eq!(b.color, Some(Color::rgb(0x80, 0x80, 0x80)));
        assert!(d.border.is_none(), "border-top must not also set the all-sides `border` field");
    }

    #[test]
    fn border_top_garbage_token_invalidates_the_whole_declaration() {
        // Same "whole shorthand invalid on one bad token" rule as `border`
        // itself (see `border_shorthand_...`'s sibling tests below).
        let mut d = Declarations::default();
        assert!(!apply_property("border-top", &toks("2px solid red garbage"), &mut d));
        assert!(d.border_top.is_none());
    }

    // ---- packet/acid1-content-box: `border-width`/`border-style`/
    // `border-color` longhands (fixtures/css1-float-5526c.html's
    // `blockquote` uses exactly this triple, never the `border` shorthand)
    // ----

    #[test]
    fn border_width_four_values_expand_top_right_bottom_left() {
        let mut d = Declarations::default();
        assert!(apply_property("border-width", &toks("1em 1.5em 2em .5em"), &mut d));
        assert_eq!(d.border_width.top, Some(RawLength::Em(1.0)));
        assert_eq!(d.border_width.right, Some(RawLength::Em(1.5)));
        assert_eq!(d.border_width.bottom, Some(RawLength::Em(2.0)));
        assert_eq!(d.border_width.left, Some(RawLength::Em(0.5)));
    }

    #[test]
    fn border_width_rejects_percentage_like_the_border_shorthand_does() {
        // `token_to_border_width` (shared with the `border` shorthand's own
        // width sub-token) rejects `%` -- CSS has no percentage form for
        // `border-width`. A single bad token invalidates the whole 1-4-value
        // declaration, same as `apply_edges_shorthand`'s existing contract.
        let mut d = Declarations::default();
        assert!(!apply_property("border-width", &toks("10%"), &mut d));
    }

    #[test]
    fn border_style_solid_is_recognized() {
        let mut d = Declarations::default();
        assert!(apply_property("border-style", &toks("solid"), &mut d));
        assert_eq!(d.border_style, Some(BorderStyle::Solid));
    }

    #[test]
    fn border_style_non_solid_keywords_map_to_none_not_rejected() {
        // Matches `parse_border_raw`'s own curated mapping: `dashed`/
        // `dotted`/... are recognized CSS keywords, just not ones this
        // engine paints (brief §4, solid-only) -- so they resolve to "no
        // visible border" rather than failing the whole declaration.
        let mut d = Declarations::default();
        assert!(apply_property("border-style", &toks("dashed"), &mut d));
        assert_eq!(d.border_style, Some(BorderStyle::None));
    }

    #[test]
    fn border_color_parses_a_named_or_hex_color() {
        let mut d = Declarations::default();
        assert!(apply_property("border-color", &toks("black"), &mut d));
        assert_eq!(d.border_color, Some(Color::rgb(0, 0, 0)));
    }

    #[test]
    fn border_width_style_color_longhands_do_not_touch_the_border_shorthand_field() {
        let mut d = Declarations::default();
        assert!(apply_property("border-width", &toks("1em 1.5em 2em .5em"), &mut d));
        assert!(apply_property("border-style", &toks("solid"), &mut d));
        assert!(apply_property("border-color", &toks("black"), &mut d));
        assert!(d.border.is_none(), "border-width/-style/-color longhands must not also set the `border` shorthand field");
        assert!(d.border_top.is_none());
    }

    // ---- packet/table-spacing: `border-spacing` shorthand ----

    #[test]
    fn border_spacing_one_value_sets_both_axes() {
        let mut d = Declarations::default();
        assert!(apply_property("border-spacing", &toks("4px"), &mut d));
        assert_eq!(d.border_spacing_x, Some(RawLength::Px(4.0)));
        assert_eq!(d.border_spacing_y, Some(RawLength::Px(4.0)));
    }

    #[test]
    fn border_spacing_two_values_set_x_then_y() {
        let mut d = Declarations::default();
        assert!(apply_property("border-spacing", &toks("4px 2px"), &mut d));
        assert_eq!(d.border_spacing_x, Some(RawLength::Px(4.0)));
        assert_eq!(d.border_spacing_y, Some(RawLength::Px(2.0)));
    }

    #[test]
    fn border_spacing_rejects_percent_and_garbage_and_too_many_values() {
        for bad in ["10%", "not-a-length", "4px 2px 1px", ""] {
            let mut d = Declarations::default();
            assert!(!apply_property("border-spacing", &toks(bad), &mut d), "expected {bad:?} to be rejected");
            assert_eq!(d.border_spacing_x, None);
            assert_eq!(d.border_spacing_y, None);
        }
    }

    // ---- packet/t3-inline-spacing: `gap` shorthand (D3 fix) ----

    #[test]
    fn gap_one_value_sets_row_gap_only_leaving_column_gap_unset() {
        let mut d = Declarations::default();
        assert!(apply_property("gap", &toks("20px"), &mut d));
        assert_eq!(d.gap, Some(RawLength::Px(20.0)));
        assert_eq!(d.column_gap, None, "single-value gap must not set a distinct column-gap");
    }

    #[test]
    fn gap_two_values_set_row_then_column_distinctly() {
        // The exact shape of `fixtures/httpforever.html`'s
        // `.footer__projects { gap: .35rem 1.1rem; }` -- row-gap .35rem,
        // column-gap 1.1rem, and (before this packet) the column-gap value
        // that actually governs the row-direction footer's item-to-item
        // spacing was the one silently dropped.
        let mut d = Declarations::default();
        assert!(apply_property("gap", &toks(".35rem 1.1rem"), &mut d));
        assert_eq!(d.gap, Some(RawLength::Em(0.35)));
        assert_eq!(d.column_gap, Some(RawLength::Em(1.1)));
    }

    #[test]
    fn gap_rejects_percent_and_garbage_and_too_many_values() {
        for bad in ["10%", "not-a-length", "4px 10%", "10% 4px", "4px 2px 1px", ""] {
            let mut d = Declarations::default();
            assert!(!apply_property("gap", &toks(bad), &mut d), "expected {bad:?} to be rejected");
            assert_eq!(d.gap, None);
            assert_eq!(d.column_gap, None);
        }
    }

    // ---- packet/border-collapse: `border-collapse` property ----

    #[test]
    fn border_collapse_property_parses_collapse_and_separate() {
        let mut d = Declarations::default();
        assert!(apply_property("border-collapse", &toks("collapse"), &mut d));
        assert_eq!(d.border_collapse, Some(BorderCollapse::Collapse));

        let mut d = Declarations::default();
        assert!(apply_property("border-collapse", &toks("separate"), &mut d));
        assert_eq!(d.border_collapse, Some(BorderCollapse::Separate));
    }

    #[test]
    fn border_collapse_unknown_value_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("border-collapse", &toks("bogus"), &mut d));
        assert_eq!(d.border_collapse, None);
    }

    #[test]
    fn unknown_property_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("flibbertigibbet", &toks("1"), &mut d));
    }

    #[test]
    fn parses_table_display_values() {
        let mut d = Declarations::default();
        assert!(apply_property("display", &toks("table"), &mut d));
        assert_eq!(d.display, Some(Display::Table));

        let mut d = Declarations::default();
        assert!(apply_property("display", &toks("table-row"), &mut d));
        assert_eq!(d.display, Some(Display::TableRow));

        let mut d = Declarations::default();
        assert!(apply_property("display", &toks("table-cell"), &mut d));
        assert_eq!(d.display, Some(Display::TableCell));

        let mut d = Declarations::default();
        assert!(apply_property("display", &toks("table-row-group"), &mut d));
        assert_eq!(d.display, Some(Display::TableRowGroup));

        // Case-insensitive, like the other display keywords.
        let mut d = Declarations::default();
        assert!(apply_property("display", &toks("TABLE-CELL"), &mut d));
        assert_eq!(d.display, Some(Display::TableCell));
    }

    #[test]
    fn parses_list_item_display_value() {
        // packet/display-list-item: `display: list-item` must parse to the
        // dedicated `Display::ListItem` variant, distinct from `Block` — see
        // `Display::ListItem`'s own doc comment for why the distinction
        // matters (list-marker emission).
        let mut d = Declarations::default();
        assert!(apply_property("display", &toks("list-item"), &mut d));
        assert_eq!(d.display, Some(Display::ListItem));

        // Case-insensitive, like the other display keywords.
        let mut d = Declarations::default();
        assert!(apply_property("display", &toks("LIST-ITEM"), &mut d));
        assert_eq!(d.display, Some(Display::ListItem));
    }

    #[test]
    fn unknown_display_value_is_still_ignored() {
        // Charter C2: an unrecognized value must not apply, and must not be
        // confused with any table variant.
        let mut d = Declarations::default();
        assert!(!apply_property("display", &toks("table-column"), &mut d));
        assert_eq!(d.display, None);
    }

    #[test]
    fn unparseable_color_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("color", &toks("bogus"), &mut d));
    }

    // ------------------------------------- background shorthand (P7 tty-color)

    #[test]
    fn background_shorthand_extracts_a_bare_color() {
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("navy"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(0, 0, 128)));
    }

    #[test]
    fn background_shorthand_extracts_hex_color() {
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("#fff"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(255, 255, 255)));
    }

    #[test]
    fn background_shorthand_extracts_color_from_amid_image_and_repeat() {
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("red url(x.png) no-repeat"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn background_shorthand_extracts_color_regardless_of_token_order() {
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("url(x.png) no-repeat red"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn background_shorthand_rgb_function_color() {
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("rgb(1, 2, 3) no-repeat"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(1, 2, 3)));
    }

    #[test]
    fn background_shorthand_with_no_color_or_url_is_not_applied() {
        // `background: none` (no color, no image component) is total (never
        // panics) but sets neither field — charter C2: unrecognized/
        // uncovered parts of a value are ignored, not guessed at.
        let mut d = Declarations::default();
        assert!(!apply_property("background", &toks("none"), &mut d));
        assert_eq!(d.background_color, None);
        assert_eq!(d.background_image, None);
    }

    #[test]
    fn background_shorthand_garbage_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("background", &toks("bogus"), &mut d));
        assert_eq!(d.background_color, None);
        assert_eq!(d.background_image, None);
    }

    // ------------------------------------------------- font shorthand (D-acid1)
    //
    // packet/acid1-coherence: `html { font: 10px/1 Verdana, sans-serif }`
    // (fixtures/css1-float-5526c.html) used to be silently DROPPED whole —
    // `apply_property` had longhands for `font-family`/`font-size`/`font-
    // weight`/`font-style` but no `"font"` arm at all, so it fell to the
    // catch-all `_ => false` and counted as an ignored declaration. The
    // practical effect: `html`'s font-size never left the UA default
    // (16px), so EVERY `em` length in the whole document (including this
    // fixture's own `width: 10.638%`/`41.17%`, which resolve against an
    // `em`-sized ancestor) computed 1.6x too large against an 800px fixed
    // viewport — see fixtures/evidence/css1-float-5526c.diagnosis.md's
    // follow-up note for the pixel-measurement trail that found this.

    #[test]
    fn font_shorthand_extracts_size_and_family() {
        let mut d = Declarations::default();
        assert!(apply_property("font", &toks("10px/1 Verdana, sans-serif"), &mut d));
        assert_eq!(d.font_size, Some(RawLength::Px(10.0)));
        assert_eq!(d.line_height, Some(RawLineHeight::Number(1.0)));
        assert_eq!(d.font_family, Some(FontFamily::SansSerif));
    }

    #[test]
    fn font_shorthand_size_without_line_height_or_family() {
        let mut d = Declarations::default();
        assert!(apply_property("font", &toks("1em"), &mut d));
        assert_eq!(d.font_size, Some(RawLength::Em(1.0)));
        assert_eq!(d.line_height, None);
        assert_eq!(d.font_family, None);
    }

    #[test]
    fn font_shorthand_extracts_leading_style_and_weight() {
        let mut d = Declarations::default();
        assert!(apply_property("font", &toks("italic bold 12px sans-serif"), &mut d));
        assert_eq!(d.font_style, Some(FontStyle::Italic));
        assert_eq!(d.font_weight, Some(FontWeight::Bold));
        assert_eq!(d.font_size, Some(RawLength::Px(12.0)));
        assert_eq!(d.font_family, Some(FontFamily::SansSerif));
    }

    #[test]
    fn font_shorthand_percent_line_height() {
        let mut d = Declarations::default();
        assert!(apply_property("font", &toks("12px/150% serif"), &mut d));
        assert_eq!(d.line_height, Some(RawLineHeight::Length(RawLength::Percent(150.0))));
    }

    #[test]
    fn font_shorthand_without_a_size_is_not_applied() {
        // Real CSS: font-size is mandatory in the `font` shorthand — a
        // value with none invalidates the whole declaration (same
        // total-parse-failure rule `parse_border_raw` documents for
        // `border`, just triggered by a missing mandatory component instead
        // of a bad token).
        let mut d = Declarations::default();
        assert!(!apply_property("font", &toks("bold sans-serif"), &mut d));
        assert_eq!(d.font_size, None);
        assert_eq!(d.font_family, None);
    }

    // ------------------------------------------- background-image (bg-image)

    #[test]
    fn background_image_parses_a_bare_url() {
        let mut d = Declarations::default();
        assert!(apply_property("background-image", &toks("url(x.png)"), &mut d));
        assert_eq!(d.background_image.as_deref(), Some("x.png"));
    }

    #[test]
    fn background_image_parses_a_quoted_url() {
        let mut d = Declarations::default();
        assert!(apply_property("background-image", &toks(r#"url("x.png")"#), &mut d));
        assert_eq!(d.background_image.as_deref(), Some("x.png"));

        let mut d = Declarations::default();
        assert!(apply_property("background-image", &toks("url('images/tile.png')"), &mut d));
        assert_eq!(d.background_image.as_deref(), Some("images/tile.png"));
    }

    #[test]
    fn background_image_parses_an_unquoted_url_with_a_path() {
        // Unquoted url() contents tokenize into several tokens (Ident, Dot,
        // Delim('/'), ...) rather than one Str -- must be reconstructed.
        let mut d = Declarations::default();
        assert!(apply_property("background-image", &toks("url(images/tile.png)"), &mut d));
        assert_eq!(d.background_image.as_deref(), Some("images/tile.png"));
    }

    #[test]
    fn background_image_none_clears_and_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("background-image", &toks("none"), &mut d));
        assert_eq!(d.background_image, None);
    }

    #[test]
    fn background_image_garbage_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("background-image", &toks("bogus"), &mut d));
        assert_eq!(d.background_image, None);
    }

    #[test]
    fn background_image_unterminated_url_does_not_panic_and_is_not_applied() {
        let mut d = Declarations::default();
        assert!(!apply_property("background-image", &toks("url(x.png"), &mut d));
        assert_eq!(d.background_image, None);
    }

    #[test]
    fn background_shorthand_extracts_both_color_and_image() {
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("#fff url(x.png) no-repeat"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(255, 255, 255)));
        assert_eq!(d.background_image.as_deref(), Some("x.png"));
    }

    #[test]
    fn background_shorthand_extracts_image_regardless_of_token_order() {
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("url(x.png) no-repeat red"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(255, 0, 0)));
        assert_eq!(d.background_image.as_deref(), Some("x.png"));
    }

    #[test]
    fn background_shorthand_with_a_url_but_no_color_sets_image_only() {
        // Packet bg-image extends the `background` shorthand (previously
        // color-only, packet D36) to also extract the image component --
        // `background: url(x.png) no-repeat` now succeeds (returns `true`,
        // no longer counted against `ignored_declarations`) even with no
        // color component, since the image half DID parse.
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("url(x.png) no-repeat"), &mut d));
        assert_eq!(d.background_color, None);
        assert_eq!(d.background_image.as_deref(), Some("x.png"));
    }

    #[test]
    fn background_shorthand_url_inside_never_misidentified_as_color() {
        // `url(red.png)` -- "red" inside the url() must not be picked up as
        // a named color by the color half of the shorthand scan.
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("url(red.png)"), &mut d));
        assert_eq!(d.background_color, None);
        assert_eq!(d.background_image.as_deref(), Some("red.png"));
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

    // ---- packet/presentational-attrs: vintage HTML attrs -> Declarations ---

    fn attrs(pairs: &[(&str, &str)]) -> AttrMap {
        let mut a = AttrMap::new();
        for (k, v) in pairs {
            a.set(k, v);
        }
        a
    }

    #[test]
    fn font_color_attribute_maps_to_color_declaration() {
        let d = presentational_hints("font", &attrs(&[("color", "red")]));
        assert_eq!(d.color, Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn color_attribute_on_a_non_font_element_is_ignored() {
        let d = presentational_hints("div", &attrs(&[("color", "red")]));
        assert_eq!(d.color, None);
    }

    #[test]
    fn bgcolor_accepts_hash_bare_hex_and_named_colors_identically() {
        let expect = Some(Color::rgb(0xff, 0xcc, 0x00));
        for val in ["#ffcc00", "ffcc00", "FFCC00", "#FFCC00"] {
            let d = presentational_hints("p", &attrs(&[("bgcolor", val)]));
            assert_eq!(d.background_color, expect, "bgcolor={val}");
        }
        let d = presentational_hints("p", &attrs(&[("bgcolor", "yellow")]));
        assert_eq!(d.background_color, Some(Color::rgb(255, 255, 0)));
    }

    #[test]
    fn bgcolor_applies_on_any_element_not_just_body() {
        let d = presentational_hints("span", &attrs(&[("bgcolor", "red")]));
        assert_eq!(d.background_color, Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn invalid_bgcolor_is_ignored() {
        let d = presentational_hints("p", &attrs(&[("bgcolor", "not-a-color")]));
        assert_eq!(d.background_color, None);
    }

    #[test]
    fn font_size_absolute_scale_1_through_7() {
        let expected = [(1, 10.0), (2, 13.0), (3, 16.0), (4, 18.0), (5, 24.0), (6, 32.0), (7, 48.0)];
        for (n, px) in expected {
            let d = presentational_hints("font", &attrs(&[("size", &n.to_string())]));
            assert_eq!(d.font_size, Some(RawLength::Px(px)), "size={n}");
        }
    }

    #[test]
    fn font_size_relative_plus_and_minus_one_step_from_default() {
        let d = presentational_hints("font", &attrs(&[("size", "+1")]));
        assert_eq!(d.font_size, Some(RawLength::Px(18.0)), "size 4");
        let d = presentational_hints("font", &attrs(&[("size", "-1")]));
        assert_eq!(d.font_size, Some(RawLength::Px(13.0)), "size 2");
    }

    #[test]
    fn font_size_relative_clamps_at_scale_bounds() {
        let d = presentational_hints("font", &attrs(&[("size", "+10")]));
        assert_eq!(d.font_size, Some(RawLength::Px(48.0)), "clamped to 7");
        let d = presentational_hints("font", &attrs(&[("size", "-10")]));
        assert_eq!(d.font_size, Some(RawLength::Px(10.0)), "clamped to 1");
    }

    #[test]
    fn font_size_garbage_or_absolute_out_of_range_is_ignored() {
        for v in ["abc", "0", "8", "", "3.5", "++1"] {
            let d = presentational_hints("font", &attrs(&[("size", v)]));
            assert_eq!(d.font_size, None, "size={v:?}");
        }
    }

    #[test]
    fn align_center_left_right_justify_map_to_text_align_on_non_img_elements() {
        for (val, expect) in [
            ("left", TextAlign::Left),
            ("right", TextAlign::Right),
            ("center", TextAlign::Center),
            ("justify", TextAlign::Justify),
        ] {
            for tag in ["p", "div", "td"] {
                let d = presentational_hints(tag, &attrs(&[("align", val)]));
                assert_eq!(d.text_align, Some(expect), "tag={tag} align={val}");
            }
        }
    }

    #[test]
    fn align_is_ignored_on_img_elements() {
        let d = presentational_hints("img", &attrs(&[("align", "center")]));
        assert_eq!(d.text_align, None);
    }

    #[test]
    fn align_top_middle_bottom_are_ignored_everywhere() {
        for v in ["top", "middle", "bottom", "nonsense"] {
            let d = presentational_hints("p", &attrs(&[("align", v)]));
            assert_eq!(d.text_align, None, "align={v}");
        }
    }

    #[test]
    fn body_text_attribute_maps_to_color() {
        let d = presentational_hints("body", &attrs(&[("text", "#333333")]));
        assert_eq!(d.color, Some(Color::rgb(0x33, 0x33, 0x33)));
    }

    #[test]
    fn text_attribute_on_a_non_body_element_is_ignored() {
        let d = presentational_hints("p", &attrs(&[("text", "#333333")]));
        assert_eq!(d.color, None);
    }

    #[test]
    fn irrelevant_element_attribute_combos_yield_empty_declarations() {
        let d = presentational_hints("div", &attrs(&[]));
        assert_eq!(d.color, None);
        assert_eq!(d.background_color, None);
        assert_eq!(d.font_size, None);
        assert_eq!(d.text_align, None);
    }

    #[test]
    fn presentational_hints_never_panics_on_garbage_attribute_values() {
        let d = presentational_hints(
            "font",
            &attrs(&[("color", "!!!"), ("size", "!!!"), ("bgcolor", "!!!"), ("align", "!!!"), ("text", "!!!")]),
        );
        assert_eq!(d.color, None);
        assert_eq!(d.font_size, None);
    }

    // ---- packet/table-spacing: `cellspacing="N"` -> border-spacing hint ----

    #[test]
    fn table_cellspacing_sets_both_border_spacing_axes() {
        let d = presentational_hints("table", &attrs(&[("cellspacing", "6")]));
        assert_eq!(d.border_spacing_x, Some(RawLength::Px(6.0)));
        assert_eq!(d.border_spacing_y, Some(RawLength::Px(6.0)));
    }

    #[test]
    fn table_cellspacing_zero_is_a_valid_explicit_value() {
        let d = presentational_hints("table", &attrs(&[("cellspacing", "0")]));
        assert_eq!(d.border_spacing_x, Some(RawLength::Px(0.0)));
        assert_eq!(d.border_spacing_y, Some(RawLength::Px(0.0)));
    }

    #[test]
    fn cellspacing_on_a_non_table_element_is_ignored() {
        let d = presentational_hints("div", &attrs(&[("cellspacing", "6")]));
        assert_eq!(d.border_spacing_x, None);
        assert_eq!(d.border_spacing_y, None);
    }

    #[test]
    fn cellspacing_garbage_or_negative_is_ignored_never_panics() {
        for v in ["abc", "-5", "", "1.5", "99999999999999999999"] {
            let d = presentational_hints("table", &attrs(&[("cellspacing", v)]));
            assert_eq!(d.border_spacing_x, None, "cellspacing={v:?}");
            assert_eq!(d.border_spacing_y, None, "cellspacing={v:?}");
        }
    }

    // ---- packet/border-collapse: `<table border>` -> border-collapse hint ---

    #[test]
    fn table_border_with_no_cellspacing_sets_collapse_hint() {
        let d = presentational_hints("table", &attrs(&[("border", "1")]));
        assert_eq!(d.border_collapse, Some(BorderCollapse::Collapse));
    }

    #[test]
    fn table_border_with_cellspacing_does_not_set_collapse_hint() {
        let d = presentational_hints("table", &attrs(&[("border", "1"), ("cellspacing", "4")]));
        assert_eq!(d.border_collapse, None);
    }

    #[test]
    fn table_with_no_border_attribute_does_not_set_collapse_hint() {
        let d = presentational_hints("table", &attrs(&[]));
        assert_eq!(d.border_collapse, None);
    }

    #[test]
    fn border_collapse_hint_only_applies_to_table_elements() {
        let d = presentational_hints("div", &attrs(&[("border", "1")]));
        assert_eq!(d.border_collapse, None);
    }

    // ---- packet T1a: CSS custom properties + var() substitution ----

    #[test]
    fn substitute_vars_resolves_a_defined_custom_property() {
        let env = Env::default().child(&[("--ink".into(), toks("blue"))]);
        let result = substitute_vars(&toks("var(--ink)"), &env, &[], 0);
        assert_eq!(result, Some(toks("blue")));
    }

    #[test]
    fn substitute_vars_uses_fallback_when_undefined() {
        let env = Env::default();
        let result = substitute_vars(&toks("var(--missing, green)"), &env, &[], 0);
        assert_eq!(result, Some(toks("green")));
    }

    #[test]
    fn substitute_vars_undefined_with_no_fallback_is_invalid() {
        // "Invalid at computed-value time" (CSS's own term): the caller
        // treats `None` as "this declaration doesn't apply", never a crash
        // and never a garbage value — see `substitute_vars`'s own doc
        // comment.
        let env = Env::default();
        let result = substitute_vars(&toks("var(--missing)"), &env, &[], 0);
        assert_eq!(result, None);
    }

    #[test]
    fn substitute_vars_resolves_a_chain_through_another_custom_property() {
        // `--a` doesn't hold a color directly — it holds a reference to
        // `--b`, which `Env::get` returns UNSUBSTITUTED (see `Env`'s own doc
        // comment); resolving `var(--a)` must recurse through that chain.
        let env = Env::default().child(&[("--a".into(), toks("var(--b)")), ("--b".into(), toks("blue"))]);
        let result = substitute_vars(&toks("var(--a)"), &env, &[], 0);
        assert_eq!(result, Some(toks("blue")));
    }

    #[test]
    fn substitute_vars_direct_self_cycle_is_invalid() {
        let env = Env::default().child(&[("--a".into(), toks("var(--a)"))]);
        let result = substitute_vars(&toks("var(--a)"), &env, &[], 0);
        assert_eq!(result, None);
    }

    #[test]
    fn substitute_vars_two_step_cycle_is_invalid() {
        let env = Env::default().child(&[("--a".into(), toks("var(--b)")), ("--b".into(), toks("var(--a)"))]);
        let result = substitute_vars(&toks("var(--a)"), &env, &[], 0);
        assert_eq!(result, None);
    }

    #[test]
    fn substitute_vars_pathologically_deep_nested_fallback_does_not_panic_or_hang() {
        // A few thousand nested `var(--missing, var(--missing, ...))`
        // fallbacks, all missing the same custom property: `--missing`'s
        // name never enters `visiting` on the fallback path (fallback
        // tokens aren't a custom-property lookup — see `substitute_vars`'s
        // own doc comment), so it's `VAR_SUBSTITUTION_DEPTH_CAP`, not cycle
        // detection, that has to stop this. This test's mere existence and
        // return (rather than a stack overflow or an infinite loop) IS the
        // totality proof.
        let depth = 5000;
        let mut css = String::new();
        for _ in 0..depth {
            css.push_str("var(--missing, ");
        }
        css.push_str("red");
        for _ in 0..depth {
            css.push(')');
        }
        let env = Env::default();
        let result = substitute_vars(&toks(&css), &env, &[], 0);
        assert_eq!(result, None, "depth cap must trip long before any real recursion overflow");
    }

    // ---- packet T1c: gradient -> representative-solid background fallback ----

    #[test]
    fn radial_gradient_background_shorthand_extracts_first_color_stop() {
        let mut d = Declarations::default();
        assert!(apply_property(
            "background",
            &toks("radial-gradient(120% 140% at 50% -20%, #46a35f 0%, #2f8f4e 45%, #206b39 100%)"),
            &mut d
        ));
        assert_eq!(d.background_color, Some(Color::rgb(0x46, 0xa3, 0x5f)));
    }

    #[test]
    fn linear_gradient_named_color_stops_picks_the_first() {
        let mut d = Declarations::default();
        assert!(apply_property("background", &toks("linear-gradient(180deg, red, blue)"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn webkit_prefixed_gradient_in_background_image_is_recognized() {
        let mut d = Declarations::default();
        assert!(apply_property("background-image", &toks("-webkit-linear-gradient(top, #112233, #445566)"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(0x11, 0x22, 0x33)));
    }

    #[test]
    fn moz_prefixed_gradient_in_background_image_is_recognized() {
        let mut d = Declarations::default();
        assert!(apply_property("background-image", &toks("-moz-radial-gradient(circle, green, yellow)"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(0, 128, 0)));
    }

    #[test]
    fn gradient_with_no_parseable_color_stops_leaves_background_unset() {
        let mut d = Declarations::default();
        assert!(!apply_property("background", &toks("linear-gradient(to right, nonsense-a, nonsense-b)"), &mut d));
        assert!(d.background_color.is_none());
    }

    #[test]
    fn background_color_property_itself_falls_back_to_a_gradients_first_stop() {
        // Real-world path (packet T1a): a var() substitution BEFORE
        // apply_property ever runs (cascade::resolve) can leave
        // `background-color`'s own value holding a full gradient function
        // -- real CSS wouldn't accept that for this property, but this
        // packet extracts a usable color anyway rather than dropping it
        // (the motivating bug: light text vanishing against the default
        // white canvas when the gradient it was meant to sit on is dropped
        // entirely).
        let mut d = Declarations::default();
        assert!(apply_property("background-color", &toks("linear-gradient(red, blue)"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(255, 0, 0)));
    }

    #[test]
    fn ordinary_background_color_values_are_unaffected_by_the_gradient_fallback() {
        let mut d = Declarations::default();
        assert!(apply_property("background-color", &toks("#123456"), &mut d));
        assert_eq!(d.background_color, Some(Color::rgb(0x12, 0x34, 0x56)));
    }

    #[test]
    fn garbage_gradient_like_function_name_does_not_panic() {
        // Not a real gradient function name at all -- must fall through to
        // "unrecognized declaration", never panic on the malformed function
        // body.
        let mut d = Declarations::default();
        assert!(!apply_property("background", &toks("not-a-gradient(oops"), &mut d));
        assert!(d.background_color.is_none());
    }

    // -----------------------------------------------------------------
    // packet/css-grid
    // -----------------------------------------------------------------

    #[test]
    fn display_grid_parses() {
        let mut d = Declarations::default();
        assert!(apply_property("display", &toks("grid"), &mut d));
        assert_eq!(d.display, Some(Display::Grid));
    }

    #[test]
    fn grid_template_columns_parses_a_bare_track_list() {
        let mut d = Declarations::default();
        assert!(apply_property("grid-template-columns", &toks("1fr 1fr 1fr"), &mut d));
        let fr = |f: f32| RawGridTemplateComponent::Single(RawGridTrack::Bare(RawGridTrackSize::Fr(f)));
        assert_eq!(d.grid_template_columns, Some(vec![fr(1.0), fr(1.0), fr(1.0)]));
    }

    #[test]
    fn grid_template_columns_parses_mixed_length_and_fr_tracks() {
        let mut d = Declarations::default();
        assert!(apply_property("grid-template-columns", &toks("200px 1fr"), &mut d));
        assert_eq!(
            d.grid_template_columns,
            Some(vec![
                RawGridTemplateComponent::Single(RawGridTrack::Bare(RawGridTrackSize::Length(RawLength::Px(200.0)))),
                RawGridTemplateComponent::Single(RawGridTrack::Bare(RawGridTrackSize::Fr(1.0))),
            ])
        );
    }

    #[test]
    fn grid_template_columns_parses_repeat_with_a_plain_count() {
        let mut d = Declarations::default();
        assert!(apply_property("grid-template-columns", &toks("repeat(3, 1fr)"), &mut d));
        assert_eq!(
            d.grid_template_columns,
            Some(vec![RawGridTemplateComponent::Repeat(
                RawGridRepetitionCount::Count(3),
                vec![RawGridTrack::Bare(RawGridTrackSize::Fr(1.0))]
            )])
        );
    }

    #[test]
    fn grid_template_columns_parses_repeat_auto_fill_with_minmax() {
        let mut d = Declarations::default();
        assert!(apply_property("grid-template-columns", &toks("repeat(auto-fill, minmax(200px, 1fr))"), &mut d));
        assert_eq!(
            d.grid_template_columns,
            Some(vec![RawGridTemplateComponent::Repeat(
                RawGridRepetitionCount::AutoFill,
                vec![RawGridTrack::MinMax(
                    RawGridTrackSize::Length(RawLength::Px(200.0)),
                    RawGridTrackSize::Fr(1.0)
                )]
            )])
        );
    }

    #[test]
    fn grid_template_columns_parses_repeat_auto_fit() {
        let mut d = Declarations::default();
        assert!(apply_property("grid-template-columns", &toks("repeat(auto-fit, minmax(10px, 1fr))"), &mut d));
        let Some(vec) = d.grid_template_columns else { panic!("expected Some") };
        assert_eq!(
            vec,
            vec![RawGridTemplateComponent::Repeat(
                RawGridRepetitionCount::AutoFit,
                vec![RawGridTrack::MinMax(RawGridTrackSize::Length(RawLength::Px(10.0)), RawGridTrackSize::Fr(1.0))]
            )]
        );
    }

    #[test]
    fn grid_template_columns_mixes_a_leading_bare_track_with_a_repeat() {
        let mut d = Declarations::default();
        assert!(apply_property("grid-template-columns", &toks("200px repeat(2, 1fr)"), &mut d));
        assert_eq!(
            d.grid_template_columns,
            Some(vec![
                RawGridTemplateComponent::Single(RawGridTrack::Bare(RawGridTrackSize::Length(RawLength::Px(200.0)))),
                RawGridTemplateComponent::Repeat(
                    RawGridRepetitionCount::Count(2),
                    vec![RawGridTrack::Bare(RawGridTrackSize::Fr(1.0))]
                ),
            ])
        );
    }

    #[test]
    fn grid_template_rows_parses_independently_of_columns() {
        // `grid-template-rows` goes through the exact same [`parse_grid_
        // template`] as `grid-template-columns` but lands in its own
        // `Declarations` field -- setting one must never touch the other.
        let mut d = Declarations::default();
        assert!(apply_property("grid-template-rows", &toks("100px minmax(0, 1fr)"), &mut d));
        assert_eq!(
            d.grid_template_rows,
            Some(vec![
                RawGridTemplateComponent::Single(RawGridTrack::Bare(RawGridTrackSize::Length(RawLength::Px(100.0)))),
                RawGridTemplateComponent::Single(RawGridTrack::MinMax(
                    RawGridTrackSize::Length(RawLength::Px(0.0)),
                    RawGridTrackSize::Fr(1.0)
                )),
            ])
        );
        assert_eq!(d.grid_template_columns, None, "setting rows must not touch columns");
    }

    #[test]
    fn grid_template_columns_rejects_an_unparseable_component_entirely() {
        // Real CSS invalidates the WHOLE declaration on one bad component
        // -- not a partial/best-effort template (see `parse_grid_template`'s
        // doc comment). `auto` is a real CSS Grid track keyword this
        // packet's parser deliberately does not implement.
        let mut d = Declarations::default();
        assert!(!apply_property("grid-template-columns", &toks("1fr auto"), &mut d));
        assert_eq!(d.grid_template_columns, None);
    }

    #[test]
    fn grid_template_columns_empty_value_does_not_apply() {
        let mut d = Declarations::default();
        assert!(!apply_property("grid-template-columns", &[], &mut d));
        assert_eq!(d.grid_template_columns, None);
    }

    #[test]
    fn grid_template_columns_rejects_malformed_minmax() {
        let mut d = Declarations::default();
        // Missing the comma between minmax's two arguments.
        assert!(!apply_property("grid-template-columns", &toks("minmax(200px 1fr)"), &mut d));
        assert_eq!(d.grid_template_columns, None);
    }

    #[test]
    fn negative_fr_is_rejected() {
        let mut d = Declarations::default();
        assert!(!apply_property("grid-template-columns", &toks("-1fr"), &mut d));
        assert_eq!(d.grid_template_columns, None);
    }
}
