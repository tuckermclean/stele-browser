# D02 — html-parser

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01 | none | M |

**Owns:** `src/dom/parser.rs`

## Purpose

Imported from the code. `stele::dom::parser::parse(&str) -> Dom` is the bespoke, frozen-dialect HTML parser
with 1996-grade tag-soup recovery. It is where the covenant is applied: the AST (D01) cannot hold what this
file refuses to build.

- **Totality.** `parse` never panics. It is one pass over the input's `char`s (never raw bytes), driving an
  explicit stack of open elements instead of recursion, so nesting depth costs heap, not native stack. Every
  node is attached to its parent when it is created, so closing an element only pops the stack.
- **The root.** The tree starts with D01's `html` root. A literal `<html ...>` start tag, while nothing else
  is open, gives its attributes to that root instead of nesting a second `html` element.
- **Start tags.** Tag names are runs of ASCII letters, digits, `-`, `_` and `:`, folded to lowercase in start
  and end tags alike, so every rule below applies whatever case the page writes. `<` followed by anything but a
  letter, `/` or `!` is literal text.
- **Implied close.** A new `p` closes an open `p`, `li` an `li`, `dt`/`dd` a `dt`/`dd`, `tr` a `tr`, `td`/`th`
  a `td`/`th`, `option` an `option`, scanning up the stack and stopping at a scope element: for `p`, `div`,
  `table`, `td`, `th`, `body`, `html`, `ul`, `ol`, `dl`, `blockquote` or `form`; for `li`, `ul` or `ol`; for
  `dt`/`dd`, `dl`; for `tr`, `table`; for `td`/`th`, `tr` or `table`; for `option`, `select` or `optgroup`;
  and `body` or `html` for all of them. It stands in for HTML5's scope rules without the adoption-agency
  algorithm.
- **End tags.** An end tag closes the nearest open element of its name and everything still open above it,
  which tolerates `<b><i>x</b>y</i>` without losing text. An end tag with no open match is ignored, and the
  root is never popped. At end of input everything still open stays where it is, and pending text is kept.
- **Void elements.** `br`, `hr`, `img`, `meta`, `link`, `input`, `area`, `base`, `col`, `wbr`, `embed` and
  `frame` are leaves. A trailing `/>` on any other element is ignored and the element opens as usual, except on
  `script`: `<script .../>` is discarded with nothing consumed after it.
- **Raw text.** `script` is discarded with its contents: no node is built, but its raw text is consumed,
  unless its start tag ends in `/>`, so markup-looking characters inside cannot confuse what follows (charter
  C3). `style` keeps its raw CSS, not entity-decoded, as one `Text` child, which the style layer reads.
  `textarea` and `title` keep theirs as one `Text` child, entity-decoded (RCDATA). An empty one of the three
  gets no child at all. Raw text runs to the matching end tag, case-insensitively, or to end of input.
  `noscript` is an ordinary element whose content is parsed and rendered (the JS treaty).
- **Comments and doctype.** `<!-- ... -->` and any other `<! ... >` are dropped; an unterminated one runs to
  end of input.
- **Attributes.** Double-quoted, single-quoted, unquoted and valueless (value `""`) attributes, values
  entity-decoded, stored through `AttrMap::set` (first value wins). A truncated tag at end of input keeps the
  attributes read so far.
- **Character references.** Decimal and hexadecimal (`&#x`/`&#X`) numeric references, and the HTML 4.01 named
  set (Latin-1, symbols and Greek, special characters, plus `apos`), each needing its `;`. `&#0;`, surrogates
  and values past Unicode become U+FFFD. Anything else stays literal, `&` included.

## Tests

The inline module `dom::parser::tests` in `src/dom/parser.rs` (27 tests: nesting, implied close, mis-nesting,
end of input, depth, hostile input, entities, script/style/noscript, void elements, self-closing, comments,
attributes, and the fixtures `fixtures/basic.html` and `fixtures/soup.html`). The golden tests parse their
fixtures through `parse` (`tests/*_golden.rs`, `./accept.sh --tty-only`), `tests/entities_golden.rs` checks
the entity table against `goldens/`, and `tests/fuzz_totality.rs` feeds mutated fixtures and random bytes
through `parse` and the rest of the pipeline.

## Changes

none: imported

## Gate

- **D02-AC-1** — Start and end tags nest elements as written: `<html><body><div><p>hello <b>world</b></p></div></body></html>` builds `body > div > p > b`. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::well_formed_nesting -- --test-threads=2` exits 0.
- **D02-AC-2** — An end tag closes the nearest open element of its name (in `fixtures/basic.html`, `</h1>` closes the `h1`, so the `h2` is its sibling). *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::fixture_basic_html_structure -- --test-threads=2` exits 0.
- **D02-AC-3** — A mis-nested end tag (`<p><b>bold <i>both</b> only-i</i> tail</p>`) closes nothing below its match: all of the text stays inside the `p`. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::b_i_misnesting_keeps_all_text -- --test-threads=2` exits 0.
- **D02-AC-4** — Elements still open at end of input stay in the tree, and the text pending at end of input is kept. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::unclosed_everything_at_eof -- --test-threads=2` exits 0.
- **D02-AC-5** — Nesting depth is not capped: 5,000 nested `div`s parse to a tree 5,000 levels deep, and 5,000 unclosed ones do not panic. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::parse_is_total_on_5000 -- --test-threads=2` exits 0.
- **D02-AC-6** — `parse` returns without panicking on empty, truncated and hostile input (unterminated tags, attributes, comments and references, stray `<` and `&`, NUL bytes, repeated `</html>`). *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::does_not_panic_on_hostile_or_truncated_input -- --test-threads=2` exits 0.
- **D02-AC-7** — A new `p` closes an open `p`, a new `li` an `li`, a `dt` or `dd` an open `dt`/`dd`, a `tr` a `tr`, a `td` a `td`, and an `option` an `option`. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::implied_close -- --test-threads=2` exits 0.
- **D02-AC-8** — Implied close stops at the list: an `li` of a nested list does not close the outer list's `li`. *Verify:* `cargo test --locked --jobs 2 --test lists_golden -- --test-threads=2` exits 0.
- **D02-AC-9** — Implied close stops at the table: a `td` of a nested table does not close the outer table's cell. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 layout::box_tree::tests::table_border_nested_table_border_zero_does_not_inherit_outer_cells layout::box_tree::tests::table_cellpadding_nested_table_padding_is_independent` exits 0.
- **D02-AC-10** — A start tag and its end tag match whatever case each is written in (`<A HREF=...>` … `</A>` in `fixtures/css1-float-5526c.html`). *Verify:* `cargo test --locked --jobs 2 --test css1_float_golden -- --test-threads=2` exits 0.
- **D02-AC-11** — `br` and `hr` are void: they get no children and the text after them stays in their parent. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::void_elements_have_no_children_and_no_end_tag_needed -- --test-threads=2` exits 0.
- **D02-AC-12** — `frame` is void: two `<frame>` tags without end tags are siblings under the `frameset`, each keeping its `src`. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::frame_is_void_like_real_1996_framesets_write_it -- --test-threads=2` exits 0.
- **D02-AC-13** — `img` is void: in `fixtures/soup.html` it has no children. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::fixture_soup_html_structure -- --test-threads=2` exits 0.
- **D02-AC-14** — A trailing `/>` on a non-void element other than `script` is ignored: `<div/>x</div>` puts `x` inside the `div`. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::self_closing_slash_on_non_void_element_opens_normally -- --test-threads=2` exits 0.
- **D02-AC-15** — No `script` element is ever built, and the elements after a `script` are parsed as usual. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::script_is_fully_discarded -- --test-threads=2` exits 0.
- **D02-AC-16** — `style` is kept as an element whose one `Text` child is its raw content: markup inside it is not parsed. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::style_is_kept_with_raw_text -- --test-threads=2` exits 0.
- **D02-AC-17** — `noscript` is an ordinary element: its children are parsed and kept. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::noscript_is_first_class -- --test-threads=2` exits 0.
- **D02-AC-18** — Comments and the doctype are dropped, in the document and inside a `p`. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::comments_and_doctype_are_dropped -- --test-threads=2` exits 0.
- **D02-AC-19** — Attribute values may be double-quoted, single-quoted or unquoted; a valueless attribute has the value `""`. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::quoted_unquoted_and_empty_attributes -- --test-threads=2` exits 0.
- **D02-AC-20** — When a tag repeats an attribute in another case (`HREF="one" href="two"`), the first value is kept. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::attribute_names_fold_to_lowercase_and_first_wins -- --test-threads=2` exits 0.
- **D02-AC-21** — Text decodes the named references (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&nbsp;`, `&copy;`) and decimal and hexadecimal numeric references, `&#x` and `&#X` alike. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::named_and_numeric_entities_in_text -- --test-threads=2` exits 0.
- **D02-AC-22** — Attribute values are entity-decoded (`href="/x?a=1&amp;b=2"` gives `/x?a=1&b=2`). *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::entities_in_attribute_values -- --test-threads=2` exits 0.
- **D02-AC-23** — `&#0;` decodes to U+FFFD, never to a NUL. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::numeric_entity_nul_maps_to_replacement_char -- --test-threads=2` exits 0.
- **D02-AC-24** — A surrogate (`&#xD800;`) or a value past Unicode (`&#99999999;`) decodes to U+FFFD. *Verify:* `cargo test --locked --jobs 2 --lib dom::parser::tests::numeric_entity_surrogate_and_out_of_range_map_to_replacement_char -- --test-threads=2` exits 0.
- **D02-AC-25** — A reference that is not recognised stays literal, its `&` included. *Verify:* `cargo test --locked --jobs 2 --test entities_golden -- --test-threads=2` exits 0.
- **D02-AC-26** — A literal `<html>` start tag, while nothing else is open, folds into the root: no second `html` element is built. *Verify:* `CARGO_BUILD_JOBS=2 ./accept.sh --tty-only` prints `PASS A5w` (Acid2 composes at the window top).
- **D02-AC-27** — Unless its start tag ends in `/>`, a `script`'s content is consumed as raw text up to its `</script>`, so markup inside it builds no node and its text is not kept. *Verify:* unpinned (no test fails when the content is parsed as markup instead).
- **D02-AC-28** — `style` content is not entity-decoded. *Verify:* unpinned.
- **D02-AC-29** — `textarea` and `title` keep their raw text, entity-decoded, as one `Text` child (none when they are empty): markup inside them is not parsed. *Verify:* unpinned.
- **D02-AC-30** — A `<` that is not followed by a letter, `/` or `!` is literal text. *Verify:* unpinned.
- **D02-AC-31** — Implied close of a `p` stops at `div`, `table`, `td`, `th`, `ul`, `ol`, `dl`, `blockquote` or `form`, and a `th` closes an open `td`/`th` as a `td` does. *Verify:* unpinned.
- **D02-AC-32** — An end tag with no open element of its name is ignored, and `</html>` never pops the root. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 dom::parser::tests::b_i_misnesting_keeps_all_text dom::parser::tests::fixture_basic_html_structure` exits 0.
- **D02-AC-33** — The attributes of that `<html ...>` start tag are set on the root element. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::attribute_equals_selector_matches_the_stamped_root_html_element style::cascade::tests::attribute_scoped_custom_property_override_gates_on_the_attribute` exits 0.
- **D02-AC-34** — The void, raw-text, script-discard and implied-close rules apply whatever case the tag is written in (`<BR>`, `<SCRIPT>`, `<P>`). *Verify:* unpinned.
- **D02-AC-35** — An end tag also closes every element still open above its match. *Verify:* `CARGO_BUILD_JOBS=2 ./accept.sh --tty-only` prints `PASS A3r`, `PASS A3s` and `PASS A5w`.
- **D02-AC-36** — `<script .../>` builds no node and consumes no content: the text after it up to `</script>` is kept as ordinary text, and that `</script>` is ignored as a stray end tag. *Verify:* unpinned.
- **D02-AC-37** — A tag truncated by the end of input keeps the attributes read so far. *Verify:* unpinned.
- **D02-AC-38** — An unterminated comment or `<!` marker runs to the end of input: nothing after it is parsed. *Verify:* unpinned.
- **D02-AC-39** — Raw text ends at its end tag whatever case that is written in (`</STYLE>`, `</TITLE>`, `</SCRIPT>`). *Verify:* unpinned.
- **D02-AC-40** — A character reference without its `;` (`&copy 2026`, `&#169 2026`) is never decoded: it stays literal. *Verify:* unpinned.
- **D02-AC-41** — `&apos;` decodes to `'`: `apos` is in the named-reference table. *Verify:* unpinned.

## Risks

- The parse holds the whole input as a `Vec<char>` (4 bytes a character) next to the input string, and an
  attribute value or raw-text run is collected into a new `String`. A large page costs several times its
  size in memory on a 486.
- A page that writes `<script src=... />` followed by inline code renders that code as text: the `/>` makes the
  parser consume nothing, and the `</script>` after the code is dropped as a stray end tag. No node is built,
  so the covenant holds.
- A `script` with no end tag swallows the rest of the document, which then renders as nothing; the same
  holds for an unterminated `style`, `textarea`, `title` or comment. This is the tolerant reading, not a bug,
  but a page can lose its body to it.
- Implied close is a short table, not HTML5's tree construction: markup that relies on HTML5's adoption agency
  or foster parenting (a `div` inside a `table` row, say) builds another tree than a current browser's.
- The named-entity table is HTML 4.01's, plus `apos`. HTML5-only names (`&NewLine;`, `&rarrw;`) stay
  literal, and a reference without its `;` (`&copy 2026`) is never decoded.
- What the dialect speaks is fixed by the charter's C2 record. A new element rule here is a dialect change:
  its amendment and size price land in the spec PR that plans the node (`SPEC.md` principle 6).
