# S05 — css-parser

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01, S01, S04 | S06, S07, S08 | L |

**Owns:** `src/style/parser.rs`, `src/style/selector.rs`, `src/style/media.rs`

## Purpose

Imported from the code. The stylesheet grammar: rules, selectors and `@media`. `parser.rs` and `media.rs` use
each other (a `Stylesheet` holds parsed media queries; flattening builds a `Stylesheet`), so they are one node,
with the selectors they parse.

- **`parser::parse(&str) -> Stylesheet`** (`stele::style::parser::parse`) is total: it tokenizes (S01) and walks
  rules, never panicking. A rule is a comma-separated selector list and a `{ ... }` block; each selector of a
  list becomes its own `StyleRule` with the same declarations and source-order number. A declaration
  `name: value` is handed to `value::apply_property` (S04) with its whitespace removed; property names fold
  to lowercase, custom-property names (`--x`) do not and are stored raw, and a value holding `var()` anywhere is
  deferred to the cascade. Recovery: a declaration that fails or names an unknown property counts in
  `ignored_declarations` and skips to its `;`; a rule whose selector reaches no `{` skips to the next `}`.
  The value scan tracks parentheses, so a `;` inside `url(data:...;base64,...)` does not end the declaration.
  `@media` blocks are parsed and stored, unevaluated, in `media_rules` (counted in `media_at_rules`); every other
  at-rule (`@import`, `@font-face`, `@charset`, a nested at-rule inside `@media`) is skipped to its `;` or its
  balanced block and counted in `ignored_at_rules`. `parse_inline` parses a `style="..."` attribute with the
  same declaration grammar. `matching_rules` lists a sheet's rules whose selector matches an element.
- **Selectors** (`selector.rs`): a descendant chain of compounds, each an optional element name or `*`, an id,
  classes, `[name="value"]`/`[name=value]` attribute tests (exact match), and the pseudo-classes `:link` (an
  element with an `href`), `:visited` (never: there is no history) and `:root`. Names, ids and classes are
  folded to lowercase on both sides, so they match whatever case the page writes. A trailing `::before`/
  `::after` (or legacy `:before`/`:after`) names a generated box. Everything else (child and sibling
  combinators, other attribute operators, functional and unknown pseudo-classes, a pseudo-element that is not
  last) parses but marks the selector unsupported, so it never matches (charter C2). Specificity is (ids,
  classes + attributes + pseudo-classes, elements + pseudo-element), compared in that order.
- **Media queries** (`media.rs`): `MediaQuery::parse` reads a comma-separated OR of items, each
  `[only] [all|screen|print] [and (feature)]...` or bare features (implicit `all`). Features: `min-width`,
  `max-width` and `width` in px (or a unitless 0) and `prefers-color-scheme: dark|light`. `print`, unknown
  types, unknown features or values, a leading `not`, a dangling `and`, trailing tokens and an empty
  condition never match: unsupported fails closed. `flatten_media(sheet, width, scheme)` folds the rules of
  every matching block into an ordinary sheet, keeping the counters; the global source-order numbers place
  them correctly. `ColorScheme::parse` reads the `--color-scheme` value: `dark` is dark, and `light`, `auto` and
  anything else are light.

## Tests

The inline modules `style::parser::tests` (41 tests), `style::selector::tests` (12) and `style::media::tests`
(26), in the node's three files. The cascade tests in `src/style/cascade.rs`, the author-sheet tests in
`src/style/author.rs` and `src/stylesheets.rs`, `tests/media_query_golden.rs` and the golden tests run
stylesheets through it.

## Changes

none: imported

## Gate

- **S05-AC-1** — `p { color: red; }` parses to one rule with nothing ignored. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::parsing_a_clean_rule_ignores_nothing -- --test-threads=2` exits 0.
- **S05-AC-2** — An element selector matches its element. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::element_selector_matches style::selector::tests::element_compound_matches_by_name` exits 0.
- **S05-AC-3** — A class selector matches, and a compound of several classes needs all of them. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::class_selector_matches style::selector::tests::class_compound_requires_all_classes_present` exits 0.
- **S05-AC-4** — An id selector matches exactly that id. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::id_selector_matches style::selector::tests::id_compound_matches_exact_id` exits 0.
- **S05-AC-5** — `div p` matches only a `p` with a `div` ancestor, and the ancestors must appear in order. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::descendant_selector_matches_only_nested style::selector::tests::descendant_selector_requires_ancestor_in_order` exits 0.
- **S05-AC-6** — Every selector of a comma-separated list gets its own rule, at the top level and inside `@media`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::grouped_selectors_both_match style::parser::tests::grouped_selectors_inside_media_each_get_a_stored_rule` exits 0.
- **S05-AC-7** — `*` matches every element (`* { box-sizing: border-box }` reaches a `div`). *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::box_sizing_border_box_can_be_declared_via_a_universal_selector -- --test-threads=2` exits 0.
- **S05-AC-8** — `:link` matches only an element with an `href`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::selector::tests::link_pseudo_requires_href style::parser::tests::pseudo_link_matches_anchor_with_href_only` exits 0.
- **S05-AC-9** — `:visited` never matches. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::selector::tests::visited_pseudo_never_matches_without_history style::parser::tests::pseudo_visited_never_matches_without_history` exits 0.
- **S05-AC-10** — `:root` matches only the root element. *Verify:* `cargo test --locked --jobs 2 --lib style::selector::tests::root_pseudo_matches_only_the_root_element -- --test-threads=2` exits 0.
- **S05-AC-11** — An id outranks a class (`#x` beats `.a`). *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::specificity_id_beats_class -- --test-threads=2` exits 0.
- **S05-AC-12** — A class outranks an element. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::specificity_class_beats_element -- --test-threads=2` exits 0.
- **S05-AC-13** — An attribute test counts like a class in specificity. *Verify:* `cargo test --locked --jobs 2 --lib style::selector::tests::specificity_counts_attribute_selectors_like_classes -- --test-threads=2` exits 0.
- **S05-AC-14** — The id and classes an element is matched by are folded to lowercase, as is its name, and any `href` counts. *Verify:* `cargo test --locked --jobs 2 --lib style::selector::tests::element_info_reads_id_class_and_href_case_insensitively -- --test-threads=2` exits 0.
- **S05-AC-15** — `[name="value"]` matches only an element carrying that attribute with exactly that value (`html[data-theme="dark"] p`). *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::attribute_equals_selector_matches_the_stamped_root_html_element style::parser::tests::attribute_equals_selector_does_not_match_a_different_value style::selector::tests::attribute_selector_matches_exact_name_value_pair` exits 0.
- **S05-AC-16** — Another attribute operator (`[class~="x"]`) makes the selector unsupported. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::attribute_selector_with_an_unsupported_operator_fails_closed -- --test-threads=2` exits 0.
- **S05-AC-17** — An unterminated attribute selector does not panic. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::unterminated_attribute_selector_does_not_panic -- --test-threads=2` exits 0.
- **S05-AC-18** — An unsupported pseudo-class (`p:hover`) never matches, and an unsupported selector never matches. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::unsupported_selector_kinds_parse_without_choking_and_do_not_match style::selector::tests::unsupported_selector_never_matches` exits 0.
- **S05-AC-19** — `::before`, `::after`, `:before` and `:after` all name their generated box and keep the selector supported. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::parses_pseudo_elements_before_after_both_colon_forms -- --test-threads=2` exits 0.
- **S05-AC-20** — A pseudo-element followed by more selector (`p::before span`) makes the selector unsupported. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::pseudo_element_must_be_last_simple_selector -- --test-threads=2` exits 0.
- **S05-AC-21** — An unknown property counts as one ignored declaration. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::ignore_unknown_property_increments_counter -- --test-threads=2` exits 0.
- **S05-AC-22** — A shorthand with a bad token counts as ignored and applies nothing. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::bad_shorthand_token_counts_as_ignored_and_does_not_apply_partially -- --test-threads=2` exits 0.
- **S05-AC-23** — A declaration with an empty value is skipped and the next declaration in the block still applies. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::malformed_declaration_is_skipped_and_rest_still_parses -- --test-threads=2` exits 0.
- **S05-AC-24** — A rule whose selector reaches no `{` is skipped to the next `}`, and the rule after it still parses. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::malformed_rule_is_skipped_to_next_brace_and_next_rule_still_parses -- --test-threads=2` exits 0.
- **S05-AC-25** — An unknown at-rule with a block (`@font-face { ... }`) is skipped and counted, and the rule after it parses. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::ignore_unknown_at_rule -- --test-threads=2` exits 0.
- **S05-AC-26** — An at-rule ending in `;` (`@import url(...);`) is skipped to its `;` and counted: the import is not followed, and the rule after it applies. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::at_import_inside_a_fetched_sheet_is_ignored_not_followed -- --test-threads=2` exits 0.
- **S05-AC-27** — An `@media` block's rules are stored with it and never reach the plain rules until flattened. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::media_query_is_parsed_and_never_leaks_into_rules_without_flattening -- --test-threads=2` exits 0.
- **S05-AC-28** — A stored `@media` block keeps its parsed condition: `(min-width: 800px)` matches at 1024px and not at 640px. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::media_query_is_now_stored_not_just_counted -- --test-threads=2` exits 0.
- **S05-AC-29** — An `@media` that reaches no `{` (at end of input or ended by `;`) is counted and stores no block. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::malformed_media_at_eof_does_not_panic_and_stores_nothing -- --test-threads=2` exits 0.
- **S05-AC-30** — 5,000 `@media` blocks are each counted and stored. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::pathological_5000_media_rules_does_not_panic -- --test-threads=2` exits 0.
- **S05-AC-31** — `parse` does not panic on truncated or malformed CSS (an open block at end of input, a lone `{`, stray at-keywords, unterminated strings, comments and `url(`). *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::parser::tests::does_not_panic_on_malformed_css_sweep style::parser::tests::unterminated_url_function_does_not_hang_or_panic` exits 0.
- **S05-AC-32** — A `;` inside `url(...)` does not end the declaration: `background: red url(data:image/png;base64,AAAA)` keeps the whole URL, and nothing is ignored. *Verify:* `cargo test --locked --jobs 2 --lib style::parser::tests::url_with_internal_semicolon_does_not_truncate_the_declaration -- --test-threads=2` exits 0.
- **S05-AC-33** — Resyncing after a declaration with no `name:` tracks parentheses, so a `;` inside a `url(...)` in the skipped tokens does not end the skip early. *Verify:* unpinned (`skip_to_decl_boundary_also_tracks_depth_past_an_unrecognized_leading_token` skips only `123;`, which holds no parenthesis).
- **S05-AC-34** — A custom-property declaration (`--ink: #123456`) is kept for `var()` rather than applied or ignored. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::root_custom_property_resolves_via_var_on_a_descendant -- --test-threads=2` exits 0.
- **S05-AC-35** — A declaration whose value holds `var()` is deferred to the cascade. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::var_fallback_is_used_when_the_custom_property_is_never_declared_anywhere -- --test-threads=2` exits 0.
- **S05-AC-36** — A `style="..."` attribute parses with the declaration grammar. *Verify:* `cargo test --locked --jobs 2 --lib style::cascade::tests::inline_style_applies_with_no_author_sheet -- --test-threads=2` exits 0.
- **S05-AC-37** — `(max-width: 500px)` matches at 320px, not at 640px. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::max_width_matches_narrower_viewport_not_wider -- --test-threads=2` exits 0.
- **S05-AC-38** — `(min-width: 800px)` matches at 1024px, not at 640px. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::min_width_matches_wider_viewport_not_narrower -- --test-threads=2` exits 0.
- **S05-AC-39** — `(width: 640px)` matches at 640px only. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::exact_width_matches_only_that_width -- --test-threads=2` exits 0.
- **S05-AC-40** — `screen and (max-width: 500px)` needs both. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::screen_and_max_width_requires_both -- --test-threads=2` exits 0.
- **S05-AC-41** — `all and (...)` matches exactly where the bare feature does. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::all_type_is_equivalent_to_no_type -- --test-threads=2` exits 0.
- **S05-AC-42** — A comma-separated list matches when any item does. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::comma_separated_list_is_or -- --test-threads=2` exits 0.
- **S05-AC-43** — `print` never matches, even with a matching feature. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::media::tests::print_never_matches_a_screen_render style::media::tests::print_with_a_matching_width_feature_still_never_matches` exits 0.
- **S05-AC-44** — An unknown media type (`speech`) never matches. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::unknown_media_type_never_matches -- --test-threads=2` exits 0.
- **S05-AC-45** — An unknown feature (`orientation`) never matches. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::unknown_feature_name_fails_closed -- --test-threads=2` exits 0.
- **S05-AC-46** — An empty or malformed condition (garbage, unbalanced parentheses, a dangling `and`) never matches and does not panic. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::malformed_condition_never_matches_and_never_panics -- --test-threads=2` exits 0.
- **S05-AC-47** — A `not` query never matches. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::not_prefix_negation_is_unsupported_and_fails_closed -- --test-threads=2` exits 0.
- **S05-AC-48** — `(prefers-color-scheme: dark)` matches only the dark scheme and `light` only the light one, and joins width features with `and`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::media::tests::prefers_color_scheme_dark_matches_only_under_dark_scheme style::media::tests::prefers_color_scheme_light_matches_only_under_light_scheme style::media::tests::prefers_color_scheme_combines_with_width_via_and` exits 0.
- **S05-AC-49** — An unknown `prefers-color-scheme` value never matches. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::prefers_color_scheme_unknown_value_fails_closed -- --test-threads=2` exits 0.
- **S05-AC-50** — `ColorScheme::parse` reads `dark` and `light` in any case. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::color_scheme_parse_dark_and_light_case_insensitively -- --test-threads=2` exits 0.
- **S05-AC-51** — `ColorScheme::parse` gives light for `auto` and for anything it does not know. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::media::tests::color_scheme_parse_auto_resolves_to_light style::media::tests::color_scheme_parse_garbage_falls_back_to_light_not_a_panic` exits 0.
- **S05-AC-52** — `flatten_media` folds the rules of a matching block into the sheet's rules and leaves no media blocks. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::media::tests::flatten_media_includes_matching_block_rules_at_narrow_viewport style::media::tests::flatten_of_a_huge_sheet_is_bounded_and_does_not_panic` exits 0.
- **S05-AC-53** — `flatten_media` drops a block that does not match the width or the colour scheme. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::media::tests::flatten_media_excludes_non_matching_block_rules_at_wide_viewport style::media::tests::flatten_media_respects_color_scheme` exits 0.
- **S05-AC-54** — `flatten_media` keeps the sheet's counters. *Verify:* `cargo test --locked --jobs 2 --lib style::media::tests::flatten_media_preserves_counters -- --test-threads=2` exits 0.
- **S05-AC-55** — Property names match in any case (`COLOR: red`). *Verify:* unpinned.
- **S05-AC-56** — Custom-property names keep their case: `--Ink` and `--ink` are two properties. *Verify:* unpinned.
- **S05-AC-57** — Every rule gets the next number of one source-order counter shared with the rules inside `@media`, so a flattened block's rules keep their place. *Verify:* unpinned.
- **S05-AC-58** — A `::before`/`::after` adds one to the element count of specificity. *Verify:* unpinned.
- **S05-AC-59** — A child or sibling combinator (`p > span`) makes the selector unsupported. *Verify:* unpinned.
- **S05-AC-60** — A width feature takes only px or a unitless 0; `em` and `%` fail closed. *Verify:* unpinned.
- **S05-AC-61** — A leading `only` is ignored. *Verify:* unpinned.
- **S05-AC-62** — A token after the last feature makes the query item fail closed. *Verify:* unpinned.

## Risks

- There is no `!important`: it is neither parsed nor honoured (S04), so an author's `!important` loses to
  any later or more specific rule.
- Child (`>`) and sibling (`+`, `~`) combinators, `:hover`, `:first-child`, `:nth-child()` and the other
  attribute operators make a whole selector inert: a rule written with them applies to nothing, not to a
  looser match.
- Ids and classes match case-insensitively (quirks-mode behaviour); attribute values match case-sensitively.
- A `not` media query never matches, so a page that writes `@media not print { ... }` loses those rules.
  Height features and `em` widths are unsupported for the same reason (fail closed).
- `matching_rules` tests every rule of a sheet against every element: no selector index, so the cascade costs
  rules × elements.
- What the dialect speaks is fixed by the charter's C2 record: a new selector, at-rule or media feature is a
  dialect change whose amendment and size price land in the spec PR that plans the node (`SPEC.md`
  principle 6).
