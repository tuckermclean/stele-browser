//! The built-in user-agent stylesheet (brief §4, cascade.rs doc comment):
//! element semantics that `dom::ast` deliberately leaves out of the DOM
//! itself — block vs inline defaults, replaced-ish defaults, classic
//! browser margins — live here, expressed as ordinary CSS and parsed by our
//! own [`super::parser::parse`]. Dogfooding the parser on its own UA sheet
//! keeps this list honest against the curated property set.

pub(crate) const UA_CSS: &str = r#"
html, body, div, p,
h1, h2, h3, h4, h5, h6,
ul, ol, li, dl, dt, dd,
blockquote, pre, table, caption,
form, fieldset, hr,
article, section, nav, header, footer, main, aside,
figure, figcaption, details, summary, address, center {
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
li { display: block; }

blockquote { margin: 1em 40px; }
hr { margin: 0.5em 0; }
"#;
