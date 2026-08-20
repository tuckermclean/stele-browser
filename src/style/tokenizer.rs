//! CSS tokenizer: turns raw CSS text into a flat token stream. Total: never
//! panics on any input, including truncated/unterminated strings, comments,
//! or stray bytes. Whitespace is preserved as its own token because selector
//! parsing needs it to tell a descendant combinator from a compound boundary.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    Ident(String),
    /// An identifier immediately followed by `(` — the `(` is consumed.
    Function(String),
    /// `@` followed by an identifier — the `@` is not part of the name.
    AtKeyword(String),
    /// `#` followed by ident-ish characters — the `#` is not part of the name.
    Hash(String),
    Str(String),
    Number(f32),
    /// A number immediately followed by a unit identifier. The unit's
    /// original case is preserved (NOT lowercased) — see `tokenize`'s
    /// comment at the call site that constructs this. Every consumer that
    /// matches on the unit string must do so case-insensitively.
    Dimension(f32, String),
    Percentage(f32),
    Colon,
    Semicolon,
    Comma,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Dot,
    Star,
    Whitespace,
    /// Any other single-character punctuation (combinators, attr brackets, …).
    Delim(char),
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '-' || (!c.is_ascii() && !c.is_control())
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || (!c.is_ascii() && !c.is_control())
}

pub(crate) fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut pos: usize = 0;
    let mut tokens = Vec::new();

    while pos < len {
        let c = chars[pos];

        // Comments: `/* ... */`, tolerated unterminated (runs to EOF).
        if c == '/' && chars.get(pos + 1) == Some(&'*') {
            pos += 2;
            while pos < len && !(chars[pos] == '*' && chars.get(pos + 1) == Some(&'/')) {
                pos += 1;
            }
            pos = if pos < len { pos + 2 } else { len };
            continue;
        }

        if c.is_whitespace() {
            while pos < len && chars[pos].is_whitespace() {
                pos += 1;
            }
            tokens.push(Token::Whitespace);
            continue;
        }

        if c == '"' || c == '\'' {
            let quote = c;
            pos += 1;
            let mut s = String::new();
            while pos < len && chars[pos] != quote {
                if chars[pos] == '\\' && pos + 1 < len {
                    s.push(chars[pos + 1]);
                    pos += 2;
                } else {
                    s.push(chars[pos]);
                    pos += 1;
                }
            }
            if pos < len {
                pos += 1; // closing quote; tolerated missing at EOF
            }
            tokens.push(Token::Str(s));
            continue;
        }

        if c == '#' {
            pos += 1;
            let start = pos;
            while pos < len && is_ident_continue(chars[pos]) {
                pos += 1;
            }
            tokens.push(Token::Hash(chars[start..pos].iter().collect()));
            continue;
        }

        if c == '@' {
            pos += 1;
            let start = pos;
            while pos < len && is_ident_continue(chars[pos]) {
                pos += 1;
            }
            tokens.push(Token::AtKeyword(chars[start..pos].iter().collect()));
            continue;
        }

        let starts_number = c.is_ascii_digit()
            || (c == '.' && chars.get(pos + 1).is_some_and(|d| d.is_ascii_digit()))
            || ((c == '-' || c == '+')
                && (chars.get(pos + 1).is_some_and(|d| d.is_ascii_digit())
                    || (chars.get(pos + 1) == Some(&'.')
                        && chars.get(pos + 2).is_some_and(|d| d.is_ascii_digit()))));
        if starts_number {
            let start = pos;
            if c == '-' || c == '+' {
                pos += 1;
            }
            while pos < len && chars[pos].is_ascii_digit() {
                pos += 1;
            }
            if pos < len && chars[pos] == '.' && chars.get(pos + 1).is_some_and(|d| d.is_ascii_digit()) {
                pos += 1;
                while pos < len && chars[pos].is_ascii_digit() {
                    pos += 1;
                }
            }
            let num_str: String = chars[start..pos].iter().collect();
            let value: f32 = num_str.parse().unwrap_or(0.0);
            if pos < len && chars[pos] == '%' {
                pos += 1;
                tokens.push(Token::Percentage(value));
            } else if pos < len && is_ident_start(chars[pos]) {
                let ustart = pos;
                while pos < len && is_ident_continue(chars[pos]) {
                    pos += 1;
                }
                // Unit case is preserved verbatim here (NOT lowercased): an
                // unquoted `url(...)` is reconstructed byte-for-byte from its
                // token stream (see `value.rs`'s `unquoted_url_token_text`),
                // and `data:` URIs frequently contain a dimension-shaped run
                // (a digit run immediately followed by base64 letters, e.g.
                // `2F58BAAT` after a `%2F` escape) whose "unit" is actually
                // case-sensitive payload — lowercasing it here used to
                // silently corrupt the base64 and break decoding (Acid2 eyes
                // packet). CSS units themselves ARE case-insensitive per
                // spec, so every consumer that matches on this string must
                // compare case-insensitively (`eq_ignore_ascii_case` /
                // `.to_ascii_lowercase()` before matching) instead of relying
                // on this function to normalize case for it.
                let unit: String = chars[ustart..pos].iter().collect();
                tokens.push(Token::Dimension(value, unit));
            } else {
                tokens.push(Token::Number(value));
            }
            continue;
        }

        if is_ident_start(c) {
            let start = pos;
            while pos < len && is_ident_continue(chars[pos]) {
                pos += 1;
            }
            let name: String = chars[start..pos].iter().collect();
            if pos < len && chars[pos] == '(' {
                pos += 1;
                tokens.push(Token::Function(name));
            } else {
                tokens.push(Token::Ident(name));
            }
            continue;
        }

        pos += 1;
        match c {
            ':' => tokens.push(Token::Colon),
            ';' => tokens.push(Token::Semicolon),
            ',' => tokens.push(Token::Comma),
            '{' => tokens.push(Token::LBrace),
            '}' => tokens.push(Token::RBrace),
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            '.' => tokens.push(Token::Dot),
            '*' => tokens.push(Token::Star),
            other => tokens.push(Token::Delim(other)),
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_a_simple_rule() {
        let t = tokenize("p { color: red; }");
        assert_eq!(
            t,
            vec![
                Token::Ident("p".into()),
                Token::Whitespace,
                Token::LBrace,
                Token::Whitespace,
                Token::Ident("color".into()),
                Token::Colon,
                Token::Whitespace,
                Token::Ident("red".into()),
                Token::Semicolon,
                Token::Whitespace,
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn tokenizes_lengths_and_percentages() {
        let t = tokenize("10px 1.5em 50% -3pt 0");
        assert_eq!(
            t,
            vec![
                Token::Dimension(10.0, "px".into()),
                Token::Whitespace,
                Token::Dimension(1.5, "em".into()),
                Token::Whitespace,
                Token::Percentage(50.0),
                Token::Whitespace,
                Token::Dimension(-3.0, "pt".into()),
                Token::Whitespace,
                Token::Number(0.0),
            ]
        );
    }

    #[test]
    fn tokenizes_hash_class_id_and_function() {
        let t = tokenize("#id .cls rgb(1,2,3)");
        assert_eq!(
            t,
            vec![
                Token::Hash("id".into()),
                Token::Whitespace,
                Token::Dot,
                Token::Ident("cls".into()),
                Token::Whitespace,
                Token::Function("rgb".into()),
                Token::Number(1.0),
                Token::Comma,
                Token::Number(2.0),
                Token::Comma,
                Token::Number(3.0),
                Token::RParen,
            ]
        );
    }

    #[test]
    fn tokenizes_at_keyword_and_strings() {
        let t = tokenize("@media 'x' \"y\"");
        assert_eq!(
            t,
            vec![
                Token::AtKeyword("media".into()),
                Token::Whitespace,
                Token::Str("x".into()),
                Token::Whitespace,
                Token::Str("y".into()),
            ]
        );
    }

    #[test]
    fn strips_comments() {
        let t = tokenize("p/* comment */{color:red}");
        assert_eq!(
            t,
            vec![
                Token::Ident("p".into()),
                Token::LBrace,
                Token::Ident("color".into()),
                Token::Colon,
                Token::Ident("red".into()),
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn never_panics_on_unterminated_constructs() {
        for input in ["/* unterminated", "\"unterminated", "'unterminated", "#", "@", "\0\0"] {
            let _ = tokenize(input);
        }
    }

    #[test]
    fn custom_property_name_lexes_as_one_ident_token() {
        // CSS custom-property names (`--name`) must lex as a single Ident token
        // so the parser can recognize a declaration as custom-property vs.
        // ordinary property purely by checking `Ident::starts_with("--")` — no
        // tokenizer change turned out to be needed for this (packet T1a): `-` is
        // already a valid ident-start/continue character (see `is_ident_start`/
        // `is_ident_continue` above), and the numeric-literal fast path only
        // claims a leading `-`/`+` when immediately followed by a digit (or
        // `.digit`), which `--bg` never is.
        let t = tokenize("--bg: red;");
        assert_eq!(t[0], Token::Ident("--bg".into()));
    }
}
