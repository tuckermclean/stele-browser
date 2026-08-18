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
    // CSS table display values (freeze amendment, packet P8 follow-up): the
    // marker the layout engine keys off to run the bespoke table column
    // solver (`layout::table::solve_table`). This packet only lands the
    // marker + UA-sheet wiring; every consumer still falls back to
    // block-equivalent behavior (see `layout::block::map_display`) until the
    // real table-layout packet lands.
    Table,
    TableRow,
    TableCell,
    TableRowGroup,
    /// `display: list-item` (freeze amendment, packet/display-list-item):
    /// real CSS's own value for `<li>` (`src/style/ua.rs`'s `li { display:
    /// list-item; }`), and the ONLY value `layout::box_tree::
    /// build_list_container_node` now treats as "still list-item-shaped" for
    /// marker synthesis (bullet/ordinal) + ordinal-counter advancement. This
    /// replaces packet #58's stopgap `tag_is_li && display == Display::
    /// Block` guard, which could not distinguish an ordinary `<li>` (UA
    /// default, now `ListItem`) from `<li>` with author CSS `display: block`
    /// re-asserted on purpose (both used to resolve to the identical
    /// `Display::Block` — see `fixtures/evidence/css1-float-5526c.
    /// diagnosis.md`, now resolved). For layout purposes a list-item box is
    /// otherwise ORDINARY block flow — `layout::block::map_display` maps it
    /// straight to `TDisplay::Block`, so `<li>` occupies the exact same
    /// position/size a `Display::Block` box would; only marker emission
    /// differs.
    ListItem,
    /// `display: grid` (packet/css-grid). Mirrors how `Display::Flex` maps
    /// straight onto taffy's own `Display::Flex` (`layout::block::
    /// map_display`) — `Display::Grid` maps onto taffy's `Display::Grid`,
    /// letting taffy's own grid algorithm (not this engine) place items.
    /// `grid-template-columns`/`grid-template-rows` (below) are the only
    /// two grid properties this packet parses; explicit item placement
    /// (`grid-column`/`grid-row`), `grid-template-areas`, and
    /// `grid-auto-flow` are all unparsed — every item uses taffy's default
    /// auto-placement (row-major, CSS's own `grid-auto-flow: row` initial
    /// value). See this packet's PR description for the full list of
    /// deferred grid features.
    Grid,
}

/// One grid track's size (packet/css-grid): the parsed form of a bare
/// `<length>` / `<percentage>` / `<flex>` (`fr`) value wherever a track
/// size can appear — as a whole track (`GridTrack::Bare`) or as either half
/// of a `minmax()` (`GridTrack::MinMax`). Mirrors taffy's own
/// `MinTrackSizingFunction`/`MaxTrackSizingFunction`/`TrackSizingFunction`
/// split (`layout::block::map_grid_track` maps this onto those) without
/// depending on taffy's types here — `ComputedStyle` stays taffy-agnostic,
/// same as every other field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridTrackSize {
    Length(f32),
    Percent(f32),
    Fr(f32),
}

/// One grid track definition (packet/css-grid): either a bare size (`1fr`,
/// `200px`) or an explicit `minmax(<min>, <max>)`. A bare `<flex>` track
/// (`GridTrack::Bare(GridTrackSize::Fr(_))`) is NOT the same as
/// `MinMax(Fr(_), Fr(_))` — real CSS's own `<flex>` grammar production
/// implies an automatic minimum (`minmax(auto, Nfr)`), which is exactly
/// what taffy's generic `fr()` helper produces for a `TrackSizingFunction`
/// (see `layout::block::map_grid_track`'s doc comment) — keeping `Bare`
/// distinct from `MinMax` lets that mapping reach the correct taffy helper
/// rather than reimplementing the auto-minimum rule here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridTrack {
    Bare(GridTrackSize),
    MinMax(GridTrackSize, GridTrackSize),
}

/// `repeat()`'s first argument (packet/css-grid): an explicit count, or one
/// of the two CSS auto-repeat keywords (`auto-fill`/`auto-fit`) — see MDN's
/// `repeat()` reference for the (subtle) difference between the two; this
/// engine doesn't need to distinguish them itself, it only forwards
/// whichever was declared straight to taffy's own `RepetitionCount`
/// (`layout::block::map_grid_template_component`), which implements both.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridRepetitionCount {
    Count(u16),
    AutoFill,
    AutoFit,
}

/// One component of a `grid-template-columns`/`grid-template-rows` value
/// (packet/css-grid): either a single track, or a `repeat(<count>,
/// <track>+)` — CSS allows the whole value to be a mix of both (`200px
/// repeat(2, 1fr) 100px`), so this is stored as a `Vec` of components on
/// `ComputedStyle`, same shape as taffy's own `Style.grid_template_columns`
/// (`GridTrackVec<GridTemplateComponent<S>>`) it maps onto.
#[derive(Debug, Clone, PartialEq)]
pub enum GridTemplateComponent {
    Single(GridTrack),
    Repeat(GridRepetitionCount, Vec<GridTrack>),
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

/// CSS `border-collapse: separate | collapse` (FREEZE AMENDMENT, packet/
/// border-collapse — see `ComputedStyle::border_collapse`'s own doc comment
/// for the full rationale). `Separate` is the CSS initial value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderCollapse {
    Separate,
    Collapse,
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
    /// `background-image: url(...)` (or the `background` shorthand's own
    /// `url(...)` component) — the RAW, unresolved URL string exactly as
    /// written in the stylesheet (freeze amendment, packet bg-image: brief
    /// §10 sanctions this ONE field addition to the otherwise-frozen
    /// `ComputedStyle`). Deliberately not a decoded image or a resolved
    /// `Url`: resolving/fetching/decoding is driver-level work (`bg_images`
    /// module) that only the pixel backends run — the tty backend has no use
    /// for it (a char grid can't show an image; the tty already shows
    /// `background_color` via ANSI). CSS initial value is `none`, so `None`
    /// here. Like `background_color`, this is NOT inherited (see `cascade::
    /// resolve`'s `own!` treatment below) — a child with no `background-
    /// image` declaration of its own never picks up its parent's.
    pub background_image: Option<Box<str>>,
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
    /// `border-spacing` (FREEZE AMENDMENT, packet/table-spacing): CSS
    /// `border-spacing: <length-x> <length-y>?` — the gap the bespoke table
    /// solver (`layout::table::solve_table`) inserts *between* adjacent
    /// columns (`x`) and rows (`y`). This is the ONLY change this packet
    /// makes to the otherwise-frozen `ComputedStyle`: two `f32` fields,
    /// nothing else touched.
    ///
    /// Defaults are EXACTLY the pre-existing `layout::block::
    /// BORDER_SPACING_X/Y` constants (`8.0`/`0.0`) this packet replaces —
    /// see `ComputedStyle::default` below — so every existing table with no
    /// `border-spacing`/`cellspacing` of its own resolves to the identical
    /// numeric spacing it always has, and therefore renders byte-identically
    /// (no golden churn). `layout::block::compute_table_cache_entry` reads
    /// these straight off the table's own `LayoutNode.style` instead of the
    /// old module-private constants.
    ///
    /// Not inherited: real CSS `border-spacing` DOES inherit, but only a
    /// `Display::Table` box's own style is ever consulted by the solver (a
    /// descendant picking up an ancestor's `border-spacing` is otherwise
    /// unobservable in this engine), so `cascade::resolve` resolves this as
    /// a plain non-inherited ("own") box property — the simplest correct
    /// choice, documented here per the packet brief rather than left
    /// implicit.
    pub border_spacing_x: f32,
    pub border_spacing_y: f32,
    /// `border-collapse: separate | collapse` (FREEZE AMENDMENT, packet/
    /// border-collapse): selects the table border model the bespoke table
    /// solver (`layout::block::compute_table_cache_entry`) and the box-tree
    /// builder's collapse-dedup step (`layout::box_tree`'s post-stamp walk)
    /// key off. `Separate` (the CSS initial value, and this field's default)
    /// is the pre-existing model — untouched by this packet, byte-identical
    /// to every table's rendering before it landed. `Collapse` makes the
    /// solver ignore `border_spacing_x/y` (feeds it `0.0` regardless of what
    /// they resolved to) and makes the box-tree builder dedup each cell's
    /// right/bottom borders against its neighbor's top/left, so adjacent
    /// cells share one border line instead of doubling it with a gap between
    /// (real CSS `border-collapse`). This is the ONLY other change this
    /// packet makes to the otherwise-frozen `ComputedStyle` — one enum, one
    /// field.
    ///
    /// Not inherited: real CSS `border-collapse` DOES inherit, but (exactly
    /// like `border_spacing_x/y` above) only a `Display::Table` box's own
    /// value is ever consulted by the solver/box-tree builder — a descendant
    /// picking up an ancestor's `border-collapse` is otherwise unobservable
    /// in this engine — so `cascade::resolve` resolves this as a plain
    /// non-inherited ("own") box property, the same documented-simplest
    /// choice `border_spacing_x/y` already made.
    pub border_collapse: BorderCollapse,
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
    /// The distinct COLUMN-gap value from a two-value `gap: <row-gap>
    /// <column-gap>` shorthand (packet/t3-inline-spacing, the D3 fix).
    /// `None` when only one value was ever declared (or none at all) --
    /// `gap` alone then governs both axes, unchanged pre-existing
    /// behavior. Kept as a SEPARATE `Option<f32>` rather than folding into
    /// `gap` itself (which would need to become a `(f32, f32)` pair, a much
    /// bigger ripple through every existing `gap`-reading call site and
    /// test) -- see `layout::block::apply_flex`, the only reader, which
    /// uses `column_gap.unwrap_or(gap)` for taffy's WIDTH axis and `gap`
    /// unconditionally for the HEIGHT axis -- real CSS's own `column-gap`/
    /// `row-gap` always mean "horizontal"/"vertical" spacing respectively,
    /// regardless of `flex-direction` (taffy's own layout algorithm is what
    /// maps a container's WIDTH-axis gap onto its actual main/cross axis,
    /// not this translation layer), so this mapping needs no
    /// `flex-direction`-dependent branching.
    pub column_gap: Option<f32>,

    // Grid (packet/css-grid). Empty `Vec` (this field's default, same as
    // `ComputedStyle::default()` below) means "no explicit template" —
    // taffy's own `Style::DEFAULT` already treats an empty
    // `grid_template_columns`/`rows` as CSS's own `none` initial value (an
    // implicit, content-auto-sized grid), so an empty `Vec` here is a
    // faithful "unset" rather than a special-cased sentinel.
    pub grid_template_columns: Vec<GridTemplateComponent>,
    pub grid_template_rows: Vec<GridTemplateComponent>,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            color: Color::BLACK,
            background_color: Color::TRANSPARENT,
            background_image: None,
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
            // Freeze amendment defaults (packet/table-spacing) — MUST match
            // the pre-existing `layout::block::BORDER_SPACING_X/Y` constants
            // exactly (8.0/0.0), see the field doc comment above.
            border_spacing_x: 8.0,
            border_spacing_y: 0.0,
            // Freeze amendment default (packet/border-collapse): CSS's own
            // initial value, and the only value a table without an explicit
            // `border-collapse`/`<table border>`-presentational-hint resolves
            // to — see the field's own doc comment.
            border_collapse: BorderCollapse::Separate,
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
            column_gap: None,

            grid_template_columns: Vec::new(),
            grid_template_rows: Vec::new(),
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
        assert_eq!(s.background_image, None);
        assert_eq!(s.flex_shrink, 1.0);
        assert_eq!(s.flex_grow, 0.0);
        assert_eq!(s.margin.top, LengthPercentageAuto::Px(0.0));
        // packet/table-spacing freeze amendment: defaults MUST match the
        // pre-existing `layout::block::BORDER_SPACING_X/Y` constants exactly
        // (8.0/0.0), so no existing table's rendering shifts.
        assert_eq!(s.border_spacing_x, 8.0);
        assert_eq!(s.border_spacing_y, 0.0);
        // packet/border-collapse freeze amendment: default MUST be
        // `Separate` (CSS's own initial value), so no existing table's
        // rendering shifts.
        assert_eq!(s.border_collapse, BorderCollapse::Separate);
        // packet/t3-inline-spacing: no distinct column-gap by default --
        // `gap` alone governs both axes until a two-value shorthand says
        // otherwise (see the field's own doc comment).
        assert_eq!(s.column_gap, None);
    }
}
