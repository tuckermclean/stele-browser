//! The built-in user-agent stylesheet (brief §4, cascade.rs doc comment):
//! element semantics that `dom::ast` deliberately leaves out of the DOM
//! itself — block vs inline defaults, replaced-ish defaults, classic
//! browser margins — live here, expressed as ordinary CSS and parsed by our
//! own [`super::parser::parse`]. Dogfooding the parser on its own UA sheet
//! keeps this list honest against the curated property set.

pub(crate) const UA_CSS: &str = r#"
/* `noscript` (M5 dialect-completeness): Stele has no JavaScript by
   construction (charter C3), so a <noscript>'s content is exactly "what to
   render when scripting is unavailable" -- always, here. It needs its own
   `display: block` (the CSS initial value is `inline`, and no other rule
   below already covers it) so it reads as an ordinary block container,
   never like `<script>`/`<style>`/`<head>` (display: none, below). */
html, body, div, p,
h1, h2, h3, h4, h5, h6,
ul, ol, li, dl, dt, dd,
blockquote, pre, table, caption,
form, fieldset, hr,
article, section, nav, header, footer, main, aside,
figure, figcaption, details, summary, address, center, noscript {
  display: block;
}

head, style, title, script, meta, link, base {
  display: none;
}

/* CSS table display values (freeze amendment, packet P8 follow-up): these
   are the marker the layout engine keys off to run the bespoke table column
   solver (`layout::table::solve_table`). `table` above still gets the
   generic `display: block` rule, so it's overridden here to the correct
   `display: table`. */
table { display: table; }
tr { display: table-row; }
td, th { display: table-cell; }
thead, tbody, tfoot { display: table-row-group; }

body { margin: 8px; }
p { margin: 1em 0; }

/* `<center>` (packet `text-align`): the presentational HTML element that
   centers its content -- `text-align` is an inherited property (cascade.rs),
   so this one rule on `center` itself is enough for descendant blocks/inline
   content to center too, without repeating it per descendant. */
center { text-align: center; }

h1 { font-size: 2em; font-weight: bold; margin: 0.67em 0; }
h2 { font-size: 1.5em; font-weight: bold; margin: 0.75em 0; }
h3 { font-size: 1.17em; font-weight: bold; margin: 0.83em 0; }
h4 { font-size: 1em; font-weight: bold; margin: 1.12em 0; }
h5 { font-size: 0.83em; font-weight: bold; margin: 1.5em 0; }
h6 { font-size: 0.67em; font-weight: bold; margin: 1.67em 0; }

b, strong { font-weight: bold; }
i, em { font-style: italic; }
u { text-decoration: underline; }
a { text-decoration: underline; color: blue; }
tt, code { font-family: monospace; }
pre { font-family: monospace; white-space: pre; margin: 1em 0; }

ul, ol { margin: 1em 0; padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
/* packet/display-list-item: real CSS's own default for `<li>` is `display:
   list-item`, not `block` -- the distinction matters because only a
   `list-item` box gets a synthesized marker (bullet/ordinal), see
   `layout::box_tree::build_list_container_node`. `li` is still caught by the
   generic `display: block` rule above (same pattern the table elements use,
   `display: table`/`table-row`/... below); this later, more specific-in-
   source-order rule overrides it, exactly like the table rules do. Author
   CSS that overrides `display` away from `list-item` (`block`, `inline`,
   ...) now correctly loses the marker -- see the packet's own report for the
   contract this replaces (packet #58's `tag_is_li && display == Block`
   stopgap, which could not tell "author wrote block on purpose" apart from
   the UA default). */
li { display: list-item; }

blockquote { margin: 1em 40px; }
/* `<hr>` (packet/hr-rule): a void element with no content of its own -- a
   plain `display: block` box with nothing inside it renders as invisible
   blank space, which is not what a rule line means. `height: 0` collapses
   the (nonexistent) content box, and `border-top` alone (not the `border`
   shorthand, which would also draw right/bottom/left edges around that
   zero-height box) paints exactly one horizontal line at the top edge --
   both the pixel backend (`backend::raster::paint_box`, which already draws
   each `BorderSide` independently) and the tty backend (`backend::tty`'s
   own top-border-only rule-line support) key off `border.top` alone. */
hr { height: 0; border-top: 1px solid #808080; margin: 0.5em 0; }

/* Form controls (P-forms): `inline` is CSS's own initial `display` value
   already (`ComputedStyle::default().display == Display::Inline`), so this
   rule is redundant with that default -- it is spelled out anyway so the
   choice is a documented decision, not an accident of what the initial
   value happens to be. `layout::box_tree::build_form_control` synthesizes
   each control's visible placeholder text (real widget geometry is the fb
   backend's job, M4); `inline` here just governs how that placeholder
   flows relative to surrounding text -- letting `Name: [__________]` sit
   on one line after its label, rather than every control forcing its own
   line the way `block` would (real CSS's closest match, `inline-block`,
   isn't in this dialect's curated `Display` set -- see the packet report). */
input, select, textarea, button {
  display: inline;
}
"#;
