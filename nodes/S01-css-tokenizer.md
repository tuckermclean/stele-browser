# S01 — css-tokenizer

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | none | S04, S05, S07 | S |

**Owns:** `src/style/tokenizer.rs`

## Purpose

Imported from the code. `style::tokenizer::tokenize(&str) -> Vec<Token>` (crate-private) turns raw CSS text into
the flat token stream that the value layer (S04), the parser and media queries (S05) and the author-sheet
helpers (S07) all read. It is one forward pass over the input's `char`s and never panics.

- **Tokens.** `Ident`, `Function` (an identifier immediately followed by `(`; the `(` is consumed), `AtKeyword`
  and `Hash` (the `@`/`#` not part of the name), `Str`, `Number`, `Dimension(value, unit)`, `Percentage`, the
  punctuation tokens `Colon`, `Semicolon`, `Comma`, `LBrace`, `RBrace`, `LParen`, `RParen`, `Dot`, `Star`, and
  `Delim(char)` for any other single character (combinators, attribute brackets, `!`, `/`, ...).
- **Whitespace** is kept, one `Whitespace` token for each run, because the selector parser needs it to tell a
  descendant combinator from a compound boundary.
- **Comments** `/* ... */` produce no token; an unterminated one runs to end of input.
- **Strings** are single- or double-quoted, without their quotes; a backslash takes the next character
  literally. An unterminated string runs to end of input.
- **Numbers** start with a digit, a `.` before a digit, or a `+`/`-` before a digit or `.digit`, and take an
  optional fraction. A `%` after one makes a `Percentage`; an identifier after one makes a `Dimension` whose
  unit keeps its case (a `data:` URL rebuilt from tokens depends on it), so every consumer compares units
  case-insensitively.
- **Identifiers** start with a letter, `_`, `-` or any non-ASCII, non-control character, and continue with
  those and digits, so a custom-property name `--bg` is one `Ident`.

## Tests

The inline module `style::tokenizer::tests` in `src/style/tokenizer.rs` (7 tests: a simple rule, lengths and
percentages, hash/class/function, at-keywords and strings, comments, unterminated input, a custom-property
name). The value tests in `src/style/value.rs` rebuild an Acid2 `data:` URL from its tokens.

## Changes

none: imported

## Gate

- **S01-AC-1** — `p { color: red; }` tokenizes to `Ident(p)`, `Whitespace`, `LBrace`, `Whitespace`, `Ident(color)`, `Colon`, `Whitespace`, `Ident(red)`, `Semicolon`, `Whitespace`, `RBrace`: whitespace is kept as tokens. *Verify:* `cargo test --locked --jobs 2 --lib style::tokenizer::tests::tokenizes_a_simple_rule -- --test-threads=2` exits 0.
- **S01-AC-2** — `10px 1.5em 50% -3pt 0` tokenizes to `Dimension(10, "px")`, `Dimension(1.5, "em")`, `Percentage(50)`, `Dimension(-3, "pt")` and `Number(0)`, a leading `-` included in the number. *Verify:* `cargo test --locked --jobs 2 --lib style::tokenizer::tests::tokenizes_lengths_and_percentages -- --test-threads=2` exits 0.
- **S01-AC-3** — `#id` is `Hash("id")`, `.` is `Dot`, and `rgb(` is `Function("rgb")` with the `(` consumed, followed by its arguments and `RParen`. *Verify:* `cargo test --locked --jobs 2 --lib style::tokenizer::tests::tokenizes_hash_class_id_and_function -- --test-threads=2` exits 0.
- **S01-AC-4** — `@media` is `AtKeyword("media")`, and `'x'` and `"y"` are `Str("x")` and `Str("y")`, without their quotes. *Verify:* `cargo test --locked --jobs 2 --lib style::tokenizer::tests::tokenizes_at_keyword_and_strings -- --test-threads=2` exits 0.
- **S01-AC-5** — A `/* ... */` comment produces no token. *Verify:* `cargo test --locked --jobs 2 --lib style::tokenizer::tests::strips_comments -- --test-threads=2` exits 0.
- **S01-AC-6** — `tokenize` does not panic on an unterminated comment or string, a lone `#` or `@`, or NUL characters. *Verify:* `cargo test --locked --jobs 2 --lib style::tokenizer::tests::never_panics_on_unterminated_constructs -- --test-threads=2` exits 0.
- **S01-AC-7** — A custom-property name `--bg` is one `Ident("--bg")`. *Verify:* `cargo test --locked --jobs 2 --lib style::tokenizer::tests::custom_property_name_lexes_as_one_ident_token -- --test-threads=2` exits 0.
- **S01-AC-8** — A dimension's unit keeps its case, so an unquoted `data:` URL rebuilt from its tokens round-trips byte for byte. *Verify:* `cargo test --locked --jobs 2 --lib style::value::tests::background_image_round_trips_the_real_acid2_forehead_data_url_byte_for_byte -- --test-threads=2` exits 0.
- **S01-AC-9** — A run of several whitespace characters is one `Whitespace` token. *Verify:* unpinned.
- **S01-AC-10** — Inside a string a backslash takes the next character literally (`"a\"b"` gives the string `a"b`). *Verify:* unpinned.
- **S01-AC-11** — A non-ASCII, non-control character starts and continues an identifier. *Verify:* unpinned.

## Risks

- A backslash outside a string is not an escape: `\31 0` or an escaped identifier tokenizes as `Delim('\\')`
  and the characters after it, not as CSS's escaped code point. A backslash inside a string takes the next
  character literally, so a hex escape (`"\201C"`) gives `201C`, not the character.
- There is no `url(` token: an unquoted `url(...)` is a `Function` followed by ordinary tokens, and the value
  layer (S04) rebuilds its text from them. Characters that tokenize to nothing (comments) or to a token without
  text (`Whitespace`, `Str` with its quotes dropped) do not survive the rebuild.
- Numbers are parsed as `f32` with no exponent: `1e3` is `Dimension(1, "e3")`, which no property accepts.
- The whole input is held as a `Vec<char>` (4 bytes a character) next to the input string.
