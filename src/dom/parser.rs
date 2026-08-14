//! The bespoke frozen-dialect HTML parser with 1996-grade tag-soup recovery.
//!
//! P1 (Wave 1) owns this file. Its contract: consume arbitrary HTML text and
//! produce a [`Dom`]. Full syntax is parsed; a curated semantic set (brief §4)
//! is kept; the remainder is skipped per the standards' forward-compat rules.
//!
//! Two consumed-at-parse elements deserve note here, since this is where the
//! covenant is actually applied (the AST cannot express what this file refuses
//! to build): `<style>` contents are handed to the CSS layer, and executable
//! wire elements are discarded outright — no node is ever constructed for them,
//! which is exactly why `dom::ast` has no variant to hold one. `<noscript>`
//! content, by contrast, is rendered first-class (charter C3, the JS treaty).

use crate::dom::{Dom, ElementName, Node, NodeId};

/// Elements with no content model and no end tag (brief §4/dialect note).
const VOID_ELEMENTS: &[&str] = &[
    "br", "hr", "img", "meta", "link", "input", "area", "base", "col", "wbr", "embed", "frame",
];

/// Elements whose contents are raw text, not markup. `script` is additionally
/// discarded entirely (see [`parse`] and charter C3); the other three are kept
/// with a single raw `Text` child.
const RAWTEXT_ELEMENTS: &[&str] = &["script", "style", "textarea", "title"];

/// Tags that get implicitly closed when a new tag from `targets` opens, unless
/// a `stoppers` element is encountered first while scanning up the open-element
/// stack (a lightweight stand-in for HTML5's "scope" concept — sufficient for
/// 1996-grade tag soup without the full adoption-agency algorithm).
fn implied_close_rule(new_tag: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    match new_tag {
        "p" => Some((
            &["p"],
            &["div", "table", "td", "th", "body", "html", "ul", "ol", "dl", "blockquote", "form"],
        )),
        "li" => Some((&["li"], &["ul", "ol", "body", "html"])),
        "dt" | "dd" => Some((&["dt", "dd"], &["dl", "body", "html"])),
        "tr" => Some((&["tr"], &["table", "body", "html"])),
        "td" | "th" => Some((&["td", "th"], &["tr", "table", "body", "html"])),
        "option" => Some((&["option"], &["select", "optgroup", "body", "html"])),
        _ => None,
    }
}

/// Parse a document. Recovery rules (implied close for `p`/`li`/`td`/`tr`,
/// b/i mis-nesting tolerance, unclosed-everything at EOF) are P1's remit.
///
/// This parser is TOTAL: it never panics, on any input, including truncated
/// or otherwise hostile bytes. It is a hand-rolled single-pass tokenizer over
/// `input`'s `char`s (never raw bytes, so no UTF-8 boundary hazards) driving a
/// stack of currently-open elements. Nodes are appended to their parent the
/// moment they are created, so "closing" an element is just popping the stack
/// back to (and including) it — which is also how mis-nested close tags are
/// tolerated: the nearest matching ancestor is found and everything above it
/// (already-attached, so no text is lost) is popped along with it.
pub fn parse(input: &str) -> Dom {
    let mut dom = Dom::new();
    let root = dom.root();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut pos: usize = 0;

    // Open-element stack: (node id, lowercased tag name). Always starts with
    // the frozen root ("html"), which is never actually popped off the end.
    let mut stack: Vec<(NodeId, String)> = vec![(root, "html".to_string())];
    let mut text_buf = String::new();

    macro_rules! flush_text {
        () => {
            if !text_buf.is_empty() {
                let decoded = decode_entities(&text_buf);
                let parent = stack.last().unwrap().0;
                let node = dom.new_text(decoded);
                dom.append_child(parent, node);
                text_buf.clear();
            }
        };
    }

    while pos < len {
        let c = chars[pos];
        if c != '<' {
            text_buf.push(c);
            pos += 1;
            continue;
        }

        // c == '<'; decide what kind of token follows.
        match chars.get(pos + 1).copied() {
            Some('!') => {
                flush_text!();
                pos = skip_comment_or_doctype(&chars, pos);
            }
            Some('/') => {
                flush_text!();
                pos += 2;
                let name = read_name(&chars, &mut pos);
                skip_to_gt(&chars, &mut pos);
                if !name.is_empty() {
                    handle_end_tag(&mut stack, &name.to_ascii_lowercase());
                }
            }
            Some(nc) if nc.is_ascii_alphabetic() => {
                flush_text!();
                pos += 1; // consume '<', leaving pos at the name's first char
                let raw_name = read_name(&chars, &mut pos);
                let (attrs, self_close) = parse_attributes(&chars, &mut pos);
                let lname = raw_name.to_ascii_lowercase();
                process_start_tag(&mut dom, &mut stack, &chars, &mut pos, root, &lname, attrs, self_close);
            }
            _ => {
                // A lone '<' not starting a real tag (EOF, whitespace, digit,
                // another '<', ...) is just a literal character in 1996 soup.
                text_buf.push('<');
                pos += 1;
            }
        }
    }
    flush_text!();

    dom
}

/// Apply the implied-close rule (if any) for an about-to-open `new_tag`,
/// popping the open-element stack down through the nearest matching ancestor
/// found before a scope-stopping element.
fn apply_implied_close(stack: &mut Vec<(NodeId, String)>, new_tag: &str) {
    let Some((targets, stoppers)) = implied_close_rule(new_tag) else {
        return;
    };
    for i in (1..stack.len()).rev() {
        let name = stack[i].1.as_str();
        if targets.contains(&name) {
            stack.truncate(i);
            return;
        }
        if stoppers.contains(&name) {
            return;
        }
    }
}

/// Close the nearest open element named `name`, if one is open. Anything
/// still open above it is popped too (already attached to its parent, so no
/// content is lost) — this is what makes `<b><i>x</b>y</i>` tolerable without
/// a full adoption-agency algorithm. A stray end tag with no open match is a
/// silent no-op, per 1996 tag-soup convention. Closing `</html>` (or any
/// stray close found at the sentinel root entry) collapses back to the root
/// without ever popping the root itself.
fn handle_end_tag(stack: &mut Vec<(NodeId, String)>, name: &str) {
    if let Some(i) = stack.iter().rposition(|(_, n)| n == name) {
        let cut = if i == 0 { 1 } else { i };
        stack.truncate(cut);
    }
}

/// Handle a start tag once its name/attrs/self-close flag are known: apply
/// implied close, create+attach the element (unless it's `<script>`, which is
/// discarded per charter C3), then either leave it as a leaf (void/self-close),
/// slurp its raw-text contents (`style`/`textarea`/`title`), or push it onto
/// the open-element stack for normal children.
#[allow(clippy::too_many_arguments)]
fn process_start_tag(
    dom: &mut Dom,
    stack: &mut Vec<(NodeId, String)>,
    chars: &[char],
    pos: &mut usize,
    root: NodeId,
    lname: &str,
    attrs: Vec<(String, String)>,
    self_close: bool,
) {
    // A literal <html ...> tag folds into the frozen root element rather than
    // nesting a second "html" node, so long as nothing has been opened yet.
    if lname == "html" && stack.len() == 1 {
        set_attrs(dom, root, &attrs);
        return;
    }

    apply_implied_close(stack, lname);

    let is_void = VOID_ELEMENTS.contains(&lname);
    let is_raw = RAWTEXT_ELEMENTS.contains(&lname);

    if lname == "script" {
        // Discarded at parse: no node is ever constructed for it or its
        // contents (the covenant, charter C3). Still consume its raw text so
        // the tokenizer's position stays correct past any markup-looking
        // characters inside (e.g. `if (1 < 2)`).
        if !(is_void || self_close) {
            let _ = read_raw_text(chars, pos, lname);
        }
        return;
    }

    let parent = stack.last().unwrap().0;
    let node = dom.new_element(ElementName::new(lname));
    set_attrs(dom, node, &attrs);
    dom.append_child(parent, node);

    if is_void {
        // Leaf: no children, no stack push. HTML has no true self-closing
        // tags outside VOID_ELEMENTS -- a trailing '/' on any other element
        // (XHTML-style, common in transitional markup) is tolerated but
        // ignored, exactly as real browsers do: the element still opens
        // normally and waits for its content/close tag (or EOF).
    } else if is_raw {
        let content = read_raw_text(chars, pos, lname);
        if !content.is_empty() {
            // style is raw CSS text (no HTML entity decoding); textarea/title
            // are RCDATA and do get entities decoded, per the HTML spec split
            // between "raw text" and "RCDATA" content models.
            let decoded = if lname == "style" { content } else { decode_entities(&content) };
            let text_node = dom.new_text(decoded);
            dom.append_child(node, text_node);
        }
        // Raw-text elements are self-contained: never pushed onto the stack.
    } else {
        stack.push((node, lname.to_string()));
    }
}

fn set_attrs(dom: &mut Dom, node: NodeId, attrs: &[(String, String)]) {
    if let Node::Element(el) = dom.node_mut(node) {
        for (k, v) in attrs {
            el.attrs.set(k, v);
        }
    }
}

/// Read an ASCII tag/attribute-name-shaped run: letters, digits, `-`, `_`,
/// `:`. Stops (possibly immediately, yielding an empty string) at the first
/// character outside that set or at EOF.
fn read_name(chars: &[char], pos: &mut usize) -> String {
    let start = *pos;
    let len = chars.len();
    while *pos < len {
        let c = chars[*pos];
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' {
            *pos += 1;
        } else {
            break;
        }
    }
    chars[start..*pos].iter().collect()
}

fn skip_whitespace(chars: &[char], pos: &mut usize) {
    let len = chars.len();
    while *pos < len && chars[*pos].is_whitespace() {
        *pos += 1;
    }
}

/// Advance past the next `>` (consuming it), or to EOF if none exists. Used
/// for end tags and other places where we tolerate but ignore trailing junk.
fn skip_to_gt(chars: &[char], pos: &mut usize) {
    let len = chars.len();
    while *pos < len && chars[*pos] != '>' {
        *pos += 1;
    }
    if *pos < len {
        *pos += 1;
    }
}

/// Consume a `<!-- ... -->` comment or a bogus/doctype `<! ... >` marker
/// starting at `pos` (which must point at the `<`), returning the position
/// just past it. Total-safe: an unterminated comment/doctype consumes to EOF
/// rather than looping or panicking.
fn skip_comment_or_doctype(chars: &[char], pos: usize) -> usize {
    let len = chars.len();
    if pos + 3 < len && chars[pos + 2] == '-' && chars[pos + 3] == '-' {
        let mut i = pos + 4;
        while i + 2 < len {
            if chars[i] == '-' && chars[i + 1] == '-' && chars[i + 2] == '>' {
                return i + 3;
            }
            i += 1;
        }
        len
    } else {
        let mut i = pos + 2;
        while i < len && chars[i] != '>' {
            i += 1;
        }
        if i < len {
            i += 1;
        }
        i
    }
}

/// Parse the attribute list of a start tag, `pos` positioned just after the
/// tag name. Returns (attrs in source order, whether a self-closing `/>` was
/// seen) and leaves `pos` just past the terminating `>` (or at EOF if the tag
/// was truncated). Every loop iteration provably advances `pos`, so this
/// cannot spin even on adversarial input (guarantees totality).
fn parse_attributes(chars: &[char], pos: &mut usize) -> (Vec<(String, String)>, bool) {
    let len = chars.len();
    let mut attrs = Vec::new();
    loop {
        skip_whitespace(chars, pos);
        if *pos >= len {
            return (attrs, false); // truncated tag at EOF
        }
        match chars[*pos] {
            '>' => {
                *pos += 1;
                return (attrs, false);
            }
            '/' => {
                if chars.get(*pos + 1) == Some(&'>') {
                    *pos += 2;
                    return (attrs, true);
                }
                *pos += 1; // stray slash, ignore
                continue;
            }
            _ => {}
        }

        let name_start = *pos;
        while *pos < len && !matches!(chars[*pos], ' ' | '\t' | '\n' | '\r' | '=' | '>' | '/') {
            *pos += 1;
        }
        if *pos == name_start {
            // Current char (must be '=', since others are handled above) has
            // no attribute name before it; skip it to guarantee progress.
            *pos += 1;
            continue;
        }
        let aname: String = chars[name_start..*pos].iter().collect();

        skip_whitespace(chars, pos);
        let mut value = String::new();
        if *pos < len && chars[*pos] == '=' {
            *pos += 1;
            skip_whitespace(chars, pos);
            if *pos < len && (chars[*pos] == '"' || chars[*pos] == '\'') {
                let quote = chars[*pos];
                *pos += 1;
                let vstart = *pos;
                while *pos < len && chars[*pos] != quote {
                    *pos += 1;
                }
                value = chars[vstart..*pos].iter().collect();
                if *pos < len {
                    *pos += 1; // closing quote
                }
            } else {
                let vstart = *pos;
                while *pos < len && !matches!(chars[*pos], ' ' | '\t' | '\n' | '\r' | '>') {
                    *pos += 1;
                }
                value = chars[vstart..*pos].iter().collect();
            }
        }
        attrs.push((aname, decode_entities(&value)));
    }
}

/// Read the raw-text contents of a `script`/`style`/`textarea`/`title`
/// element: everything up to (not including) a matching `</name` close tag,
/// with no markup interpretation in between. `pos` starts just past the open
/// tag's `>` and ends just past the close tag's `>` (or at EOF if none was
/// found — total-safe: the rest of the document becomes the element's text).
fn read_raw_text(chars: &[char], pos: &mut usize, tag: &str) -> String {
    let start = *pos;
    let len = chars.len();
    let tag_len = tag.len();
    while *pos < len {
        if chars[*pos] == '<' && chars.get(*pos + 1) == Some(&'/') {
            let name_start = *pos + 2;
            let name_end = name_start + tag_len;
            if name_end <= len {
                let candidate: String = chars[name_start..name_end].iter().collect();
                let boundary_ok = name_end == len
                    || matches!(chars[name_end], '>' | '/' | ' ' | '\t' | '\n' | '\r');
                if boundary_ok && candidate.eq_ignore_ascii_case(tag) {
                    let content: String = chars[start..*pos].iter().collect();
                    let mut end = name_end;
                    skip_to_gt(chars, &mut end);
                    *pos = end;
                    return content;
                }
            }
        }
        *pos += 1;
    }
    let content: String = chars[start..len].iter().collect();
    *pos = len;
    content
}

/// Decode HTML character references (numeric `&#169;`/`&#xA9;` and the named
/// HTML 4.01 set) in text or an attribute value. Anything that doesn't parse
/// as a recognized reference is left as a literal `&` followed by the rest of
/// the text (never panics, never drops input).
fn decode_entities(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < len {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }

        if chars.get(i + 1) == Some(&'#') {
            let mut j = i + 2;
            let hex = matches!(chars.get(j), Some('x') | Some('X'));
            if hex {
                j += 1;
            }
            let digits_start = j;
            if hex {
                while j < len && chars[j].is_ascii_hexdigit() {
                    j += 1;
                }
            } else {
                while j < len && chars[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if j > digits_start && j < len && chars[j] == ';' {
                let digits: String = chars[digits_start..j].iter().collect();
                let radix = if hex { 16 } else { 10 };
                let ch = u32::from_str_radix(&digits, radix)
                    .ok()
                    // &#0; is a well-formed reference to the NUL codepoint,
                    // but a bare NUL leaking into a text node would bite the
                    // render layer downstream -- the HTML spec itself treats
                    // codepoint 0 as a parse error remapped to U+FFFD, so we
                    // do the same rather than passing it through.
                    .filter(|&cp| cp != 0)
                    .and_then(char::from_u32)
                    .unwrap_or('\u{FFFD}');
                out.push(ch);
                i = j + 1;
                continue;
            }
        } else {
            let name_start = i + 1;
            let mut j = name_start;
            while j < len && j - name_start < 32 && chars[j].is_ascii_alphanumeric() {
                j += 1;
            }
            if j > name_start && j < len && chars[j] == ';' {
                let name: String = chars[name_start..j].iter().collect();
                if let Some(c) = named_entity(&name) {
                    out.push(c);
                    i = j + 1;
                    continue;
                }
            }
        }

        // Not a recognized reference: emit the '&' literally and move on.
        out.push('&');
        i += 1;
    }
    out
}

/// The HTML 4.01 named character reference set (ISO 8859-1 Latin-1 block,
/// the special markup characters, and the symbols/mathematical/Greek block),
/// mapped to their Unicode scalar values.
fn named_entity(name: &str) -> Option<char> {
    let cp: u32 = match name {
        // Special markup characters (+ apos, technically XHTML/HTML5 but
        // harmless and expected by authors).
        "quot" => 34,
        "amp" => 38,
        "apos" => 39,
        "lt" => 60,
        "gt" => 62,

        // Latin-1 Supplement (ISO 8859-1), 160-255.
        "nbsp" => 160,
        "iexcl" => 161,
        "cent" => 162,
        "pound" => 163,
        "curren" => 164,
        "yen" => 165,
        "brvbar" => 166,
        "sect" => 167,
        "uml" => 168,
        "copy" => 169,
        "ordf" => 170,
        "laquo" => 171,
        "not" => 172,
        "shy" => 173,
        "reg" => 174,
        "macr" => 175,
        "deg" => 176,
        "plusmn" => 177,
        "sup2" => 178,
        "sup3" => 179,
        "acute" => 180,
        "micro" => 181,
        "para" => 182,
        "middot" => 183,
        "cedil" => 184,
        "sup1" => 185,
        "ordm" => 186,
        "raquo" => 187,
        "frac14" => 188,
        "frac12" => 189,
        "frac34" => 190,
        "iquest" => 191,
        "Agrave" => 192,
        "Aacute" => 193,
        "Acirc" => 194,
        "Atilde" => 195,
        "Auml" => 196,
        "Aring" => 197,
        "AElig" => 198,
        "Ccedil" => 199,
        "Egrave" => 200,
        "Eacute" => 201,
        "Ecirc" => 202,
        "Euml" => 203,
        "Igrave" => 204,
        "Iacute" => 205,
        "Icirc" => 206,
        "Iuml" => 207,
        "ETH" => 208,
        "Ntilde" => 209,
        "Ograve" => 210,
        "Oacute" => 211,
        "Ocirc" => 212,
        "Otilde" => 213,
        "Ouml" => 214,
        "times" => 215,
        "Oslash" => 216,
        "Ugrave" => 217,
        "Uacute" => 218,
        "Ucirc" => 219,
        "Uuml" => 220,
        "Yacute" => 221,
        "THORN" => 222,
        "szlig" => 223,
        "agrave" => 224,
        "aacute" => 225,
        "acirc" => 226,
        "atilde" => 227,
        "auml" => 228,
        "aring" => 229,
        "aelig" => 230,
        "ccedil" => 231,
        "egrave" => 232,
        "eacute" => 233,
        "ecirc" => 234,
        "euml" => 235,
        "igrave" => 236,
        "iacute" => 237,
        "icirc" => 238,
        "iuml" => 239,
        "eth" => 240,
        "ntilde" => 241,
        "ograve" => 242,
        "oacute" => 243,
        "ocirc" => 244,
        "otilde" => 245,
        "ouml" => 246,
        "divide" => 247,
        "oslash" => 248,
        "ugrave" => 249,
        "uacute" => 250,
        "ucirc" => 251,
        "uuml" => 252,
        "yacute" => 253,
        "thorn" => 254,
        "yuml" => 255,

        // Symbols, mathematical symbols, Greek letters.
        "fnof" => 402,
        "Alpha" => 913,
        "Beta" => 914,
        "Gamma" => 915,
        "Delta" => 916,
        "Epsilon" => 917,
        "Zeta" => 918,
        "Eta" => 919,
        "Theta" => 920,
        "Iota" => 921,
        "Kappa" => 922,
        "Lambda" => 923,
        "Mu" => 924,
        "Nu" => 925,
        "Xi" => 926,
        "Omicron" => 927,
        "Pi" => 928,
        "Rho" => 929,
        "Sigma" => 931,
        "Tau" => 932,
        "Upsilon" => 933,
        "Phi" => 934,
        "Chi" => 935,
        "Psi" => 936,
        "Omega" => 937,
        "alpha" => 945,
        "beta" => 946,
        "gamma" => 947,
        "delta" => 948,
        "epsilon" => 949,
        "zeta" => 950,
        "eta" => 951,
        "theta" => 952,
        "iota" => 953,
        "kappa" => 954,
        "lambda" => 955,
        "mu" => 956,
        "nu" => 957,
        "xi" => 958,
        "omicron" => 959,
        "pi" => 960,
        "rho" => 961,
        "sigmaf" => 962,
        "sigma" => 963,
        "tau" => 964,
        "upsilon" => 965,
        "phi" => 966,
        "chi" => 967,
        "psi" => 968,
        "omega" => 969,
        "thetasym" => 977,
        "upsih" => 978,
        "piv" => 982,
        "bull" => 8226,
        "hellip" => 8230,
        "prime" => 8242,
        "Prime" => 8243,
        "oline" => 8254,
        "frasl" => 8260,
        "weierp" => 8472,
        "image" => 8465,
        "real" => 8476,
        "trade" => 8482,
        "alefsym" => 8501,
        "larr" => 8592,
        "uarr" => 8593,
        "rarr" => 8594,
        "darr" => 8595,
        "harr" => 8596,
        "crarr" => 8629,
        "lArr" => 8656,
        "uArr" => 8657,
        "rArr" => 8658,
        "dArr" => 8659,
        "hArr" => 8660,
        "forall" => 8704,
        "part" => 8706,
        "exist" => 8707,
        "empty" => 8709,
        "nabla" => 8711,
        "isin" => 8712,
        "notin" => 8713,
        "ni" => 8715,
        "prod" => 8719,
        "sum" => 8721,
        "minus" => 8722,
        "lowast" => 8727,
        "radic" => 8730,
        "prop" => 8733,
        "infin" => 8734,
        "ang" => 8736,
        "and" => 8743,
        "or" => 8744,
        "cap" => 8745,
        "cup" => 8746,
        "int" => 8747,
        "there4" => 8756,
        "sim" => 8764,
        "cong" => 8773,
        "asymp" => 8776,
        "ne" => 8800,
        "equiv" => 8801,
        "le" => 8804,
        "ge" => 8805,
        "sub" => 8834,
        "sup" => 8835,
        "nsub" => 8836,
        "sube" => 8838,
        "supe" => 8839,
        "oplus" => 8853,
        "otimes" => 8855,
        "perp" => 8869,
        "sdot" => 8901,
        "lceil" => 8968,
        "rceil" => 8969,
        "lfloor" => 8970,
        "rfloor" => 8971,
        "lang" => 9001,
        "rang" => 9002,
        "loz" => 9674,
        "spades" => 9824,
        "clubs" => 9827,
        "hearts" => 9829,
        "diams" => 9830,

        // Internationalization / markup-significant additions.
        "OElig" => 338,
        "oelig" => 339,
        "Scaron" => 352,
        "scaron" => 353,
        "Yuml" => 376,
        "circ" => 710,
        "tilde" => 732,
        "ensp" => 8194,
        "emsp" => 8195,
        "thinsp" => 8201,
        "zwnj" => 8204,
        "zwj" => 8205,
        "lrm" => 8206,
        "rlm" => 8207,
        "ndash" => 8211,
        "mdash" => 8212,
        "lsquo" => 8216,
        "rsquo" => 8217,
        "sbquo" => 8218,
        "ldquo" => 8220,
        "rdquo" => 8221,
        "bdquo" => 8222,
        "dagger" => 8224,
        "Dagger" => 8225,
        "permil" => 8240,
        "lsaquo" => 8249,
        "rsaquo" => 8250,
        "euro" => 8364,

        _ => return None,
    };
    char::from_u32(cp)
}

// ---------------------------------------------------------------------------
// Tests (strict test-first: committed against the `todo!()` stub above, red
// until `parse` is implemented — brief §10 TDD protocol).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::{Dom, Node, NodeId};

    // ---- tree-walk helpers (no pixel goldens; structural assertions only) --

    /// The element children of `id` (empty slice if `id` is text or absent).
    fn children_of(dom: &Dom, id: NodeId) -> Vec<NodeId> {
        match dom.node(id) {
            Node::Element(e) => e.children.clone(),
            Node::Text(_) => Vec::new(),
        }
    }

    fn elem_name<'a>(dom: &'a Dom, id: NodeId) -> Option<&'a str> {
        dom.node(id).element().map(|e| e.name.as_str())
    }

    /// First direct child element named `name`, if any.
    fn find_child(dom: &Dom, parent: NodeId, name: &str) -> Option<NodeId> {
        children_of(dom, parent)
            .into_iter()
            .find(|&c| elem_name(dom, c) == Some(name))
    }

    /// All direct child elements named `name`, in order.
    fn find_children(dom: &Dom, parent: NodeId, name: &str) -> Vec<NodeId> {
        children_of(dom, parent)
            .into_iter()
            .filter(|&c| elem_name(dom, c) == Some(name))
            .collect()
    }

    /// Depth-first search for the first descendant element named `name`.
    fn find_descendant(dom: &Dom, start: NodeId, name: &str) -> Option<NodeId> {
        if elem_name(dom, start) == Some(name) {
            return Some(start);
        }
        for c in children_of(dom, start) {
            if let Some(found) = find_descendant(dom, c, name) {
                return Some(found);
            }
        }
        None
    }

    /// Concatenated text of every text-node descendant of `id`.
    fn text_of(dom: &Dom, id: NodeId) -> String {
        let mut out = String::new();
        collect_text(dom, id, &mut out);
        out
    }

    fn collect_text(dom: &Dom, id: NodeId, out: &mut String) {
        match dom.node(id) {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) => {
                for &c in &e.children {
                    collect_text(dom, c, out);
                }
            }
        }
    }

    // ------------------------------- well-formed nesting --------------------

    #[test]
    fn well_formed_nesting() {
        let dom = parse("<html><body><div><p>hello <b>world</b></p></div></body></html>");
        let root = dom.root();
        let body = find_descendant(&dom, root, "body").expect("body");
        let div = find_child(&dom, body, "div").expect("div");
        let p = find_child(&dom, div, "p").expect("p");
        assert_eq!(text_of(&dom, p), "hello world");
        let b = find_child(&dom, p, "b").expect("b");
        assert_eq!(text_of(&dom, b), "world");
    }

    // ------------------------------- implied close ---------------------------

    #[test]
    fn implied_close_p_in_p() {
        // <p>a<p>b -- second <p> implicitly closes the first.
        let dom = parse("<body><p>a<p>b</body>");
        let root = dom.root();
        let body = find_descendant(&dom, root, "body").expect("body");
        let ps = find_children(&dom, body, "p");
        assert_eq!(ps.len(), 2, "expected two sibling <p>s, not nested");
        assert_eq!(text_of(&dom, ps[0]), "a");
        assert_eq!(text_of(&dom, ps[1]), "b");
    }

    #[test]
    fn implied_close_li_runs() {
        let dom = parse("<ul><li>one<li>two<li>three</ul>");
        let root = dom.root();
        let ul = find_descendant(&dom, root, "ul").expect("ul");
        let lis = find_children(&dom, ul, "li");
        assert_eq!(lis.len(), 3, "each <li> should close the previous one");
        assert_eq!(text_of(&dom, lis[0]), "one");
        assert_eq!(text_of(&dom, lis[1]), "two");
        assert_eq!(text_of(&dom, lis[2]), "three");
    }

    #[test]
    fn implied_close_tr_td() {
        let dom = parse("<table><tr><td>A<td>B<tr><td>C<td>D</table>");
        let root = dom.root();
        let table = find_descendant(&dom, root, "table").expect("table");
        let trs = find_children(&dom, table, "tr");
        assert_eq!(trs.len(), 2, "second <tr> should close the first");
        let tds0 = find_children(&dom, trs[0], "td");
        assert_eq!(tds0.len(), 2, "second <td> should close the first within a row");
        assert_eq!(text_of(&dom, tds0[0]), "A");
        assert_eq!(text_of(&dom, tds0[1]), "B");
        let tds1 = find_children(&dom, trs[1], "td");
        assert_eq!(tds1.len(), 2);
        assert_eq!(text_of(&dom, tds1[0]), "C");
        assert_eq!(text_of(&dom, tds1[1]), "D");
    }

    #[test]
    fn implied_close_dt_dd() {
        let dom = parse("<dl><dt>Term<dd>Definition</dl>");
        let root = dom.root();
        let dl = find_descendant(&dom, root, "dl").expect("dl");
        let dt = find_child(&dom, dl, "dt").expect("dt");
        let dd = find_child(&dom, dl, "dd").expect("dd");
        assert_eq!(text_of(&dom, dt), "Term");
        assert_eq!(text_of(&dom, dd), "Definition");
    }

    #[test]
    fn implied_close_option() {
        let dom = parse("<select><option>a<option>b<option>c</select>");
        let root = dom.root();
        let select = find_descendant(&dom, root, "select").expect("select");
        let opts = find_children(&dom, select, "option");
        assert_eq!(opts.len(), 3);
    }

    // ------------------------------- mis-nesting ------------------------------

    #[test]
    fn b_i_misnesting_keeps_all_text() {
        // <b><i>...</b>...</i> -- overlapping close tags. Text must survive
        // even though the nesting cannot be made well-formed without a full
        // adoption-agency algorithm.
        let dom = parse("<p><b>bold <i>both</b> only-i</i> tail</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        assert_eq!(text_of(&dom, p), "bold both only-i tail");
    }

    // ------------------------------- EOF recovery -----------------------------

    #[test]
    fn unclosed_everything_at_eof() {
        let dom = parse("<div><p>a<span>b");
        let root = dom.root();
        let div = find_descendant(&dom, root, "div").expect("div");
        let p = find_child(&dom, div, "p").expect("p");
        let span = find_child(&dom, p, "span").expect("span");
        assert_eq!(text_of(&dom, span), "b");
        assert_eq!(text_of(&dom, div), "ab");
    }

    // ------------------------------- pathological depth -----------------------

    /// Hostile/generated input (quote threads, WYSIWYG exports, nested-table
    /// markup) can nest thousands of tags deep. Unlike `style::cascade`'s
    /// `visit` walk (which DID recurse via plain Rust calls and reliably
    /// SIGABRTs at this depth), this parser drives an explicit `Vec`-backed
    /// open-element stack rather than recursing per nesting level -- these
    /// tests are a totality guard confirming that holds, not a fix for a
    /// found bug (empirically the parser was already total at depth; see the
    /// recursion-hardening packet report for the before/after evidence on
    /// both stages).
    #[test]
    fn parse_is_total_on_5000_nested_divs() {
        let mut html = String::with_capacity(5000 * 11 + 16);
        for _ in 0..5000 {
            html.push_str("<div>");
        }
        html.push('x');
        for _ in 0..5000 {
            html.push_str("</div>");
        }
        let dom = parse(&html);
        // Prove the tree is actually as deep as expected (not silently
        // truncated) by walking down iteratively (no test-side recursion
        // either) counting nested <div> levels.
        let mut depth = 0usize;
        let mut id = dom.root();
        loop {
            let el = dom.node(id).element().expect("element");
            if el.children.is_empty() {
                break;
            }
            let child = el.children[0];
            if dom.node(child).text().is_some() {
                break;
            }
            id = child;
            depth += 1;
        }
        assert_eq!(depth, 5000);
    }

    #[test]
    fn parse_is_total_on_5000_deep_unclosed_tag_soup() {
        // Pathological unclosed-tag soup: 5000 opens, nothing ever closed,
        // then EOF. Exercises the same nesting depth without giving the
        // parser any `</div>` close tags to pop the open-element stack on.
        let mut html = String::with_capacity(5000 * 5);
        for _ in 0..5000 {
            html.push_str("<div>");
        }
        let dom = parse(&html);
        let _ = dom.node(dom.root());
    }

    #[test]
    fn does_not_panic_on_hostile_or_truncated_input() {
        let hostiles = [
            "",
            "<",
            "</",
            "<!--",
            "<!-- unterminated comment",
            "<div",
            "<div ",
            "<div attr",
            "<div attr=",
            "<div attr='unterminated",
            "<div attr=\"unterminated",
            "<a href=",
            "<&&&<<<>>>",
            "&",
            "&#",
            "&#x",
            "&amp",
            "&;",
            "&#zzzz;",
            "1 < 2 and 3 > 1",
            "<3 <<< weird ascii art >>>",
            "<script>",
            "<script><div>",
            "<style",
            "</html></html></html>",
            "<p><p><p><p><p><p>",
            "<<<<<<<<<<<<<<<<<<<<<<<<",
            "\0\0\0null bytes\0\0\0",
            "<div class=\"a\" class=\"b\" class",
        ];
        for h in hostiles {
            let dom = parse(h);
            // Just proving we returned a Dom without panicking is the point;
            // sanity-check root is still readable.
            let _ = dom.node(dom.root());
        }
    }

    // ------------------------------- entities ---------------------------------

    #[test]
    fn named_and_numeric_entities_in_text() {
        let dom = parse("<p>&amp; &lt; &gt; &quot; &nbsp; &copy; &#169; &#xA9; &#XA9;</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        assert_eq!(text_of(&dom, p), "& < > \" \u{00A0} \u{00A9} \u{00A9} \u{00A9} \u{00A9}");
    }

    #[test]
    fn entities_in_attribute_values() {
        let dom = parse("<a href=\"/x?a=1&amp;b=2\" title=\"&copy; 2026\">link</a>");
        let root = dom.root();
        let a = find_descendant(&dom, root, "a").expect("a");
        let el = dom.node(a).element().unwrap();
        assert_eq!(el.attrs.get("href"), Some("/x?a=1&b=2"));
        assert_eq!(el.attrs.get("title"), Some("\u{00A9} 2026"));
    }

    #[test]
    fn numeric_entity_nul_maps_to_replacement_char() {
        // &#0; is the classic "smuggle a NUL byte into text" trick; leaking a
        // literal NUL into a text node would bite the render layer, so it
        // must be remapped to U+FFFD like any other disallowed codepoint.
        let dom = parse("<p>a&#0;b</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        assert_eq!(text_of(&dom, p), "a\u{FFFD}b");
        assert!(!text_of(&dom, p).contains('\0'), "no literal NUL should survive decoding");
    }

    #[test]
    fn numeric_entity_surrogate_and_out_of_range_map_to_replacement_char() {
        // &#xD800; is a lone UTF-16 surrogate half -- not a valid Unicode
        // scalar value, so char::from_u32 returns None. &#99999999; is simply
        // out of Unicode's range. Both must fall back to U+FFFD rather than
        // panicking or being dropped.
        let dom = parse("<p>x&#xD800;y&#99999999;z</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        assert_eq!(text_of(&dom, p), "x\u{FFFD}y\u{FFFD}z");
    }

    // ------------------------------- script / style / noscript ---------------

    #[test]
    fn script_is_fully_discarded() {
        let dom = parse("<body><script>if (1 < 2) { alert('hi'); }</script><p>after</p></body>");
        let root = dom.root();
        let body = find_descendant(&dom, root, "body").expect("body");
        assert!(
            find_child(&dom, body, "script").is_none(),
            "no script element should ever be constructed"
        );
        // The parser must not have gotten confused by markup-looking text
        // inside the script and lost the sibling paragraph.
        let p = find_child(&dom, body, "p").expect("p survives after script");
        assert_eq!(text_of(&dom, p), "after");
        // Walk the WHOLE tree: nothing named "script" is reachable anywhere.
        assert!(find_descendant(&dom, root, "script").is_none());
    }

    #[test]
    fn style_is_kept_with_raw_text() {
        let dom = parse("<head><style>body { color: red; } /* <div>not a tag</div> */</style></head>");
        let root = dom.root();
        let style = find_descendant(&dom, root, "style").expect("style kept as an element");
        let raw = text_of(&dom, style);
        assert!(raw.contains("color: red"));
        assert!(raw.contains("<div>not a tag</div>"), "raw contents must not be tag-parsed");
    }

    #[test]
    fn noscript_is_first_class() {
        let dom = parse("<noscript><p>fallback content</p></noscript>");
        let root = dom.root();
        let noscript = find_descendant(&dom, root, "noscript").expect("noscript kept");
        let p = find_child(&dom, noscript, "p").expect("noscript's children parsed normally");
        assert_eq!(text_of(&dom, p), "fallback content");
    }

    // ------------------------------- void elements ----------------------------

    #[test]
    fn void_elements_have_no_children_and_no_end_tag_needed() {
        let dom = parse("<p>line one<br>line two<hr>after</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        let br = find_child(&dom, p, "br").expect("br");
        assert!(children_of(&dom, br).is_empty());
        let hr = find_child(&dom, p, "hr").expect("hr");
        assert!(children_of(&dom, hr).is_empty());
        // br/hr must not have swallowed "line two"/"after" as children.
        assert_eq!(text_of(&dom, p), "line oneline twoafter");
    }

    /// `<frame>` is EMPTY-content-model (void) per HTML 4.01, and real 1996
    /// framesets write `<frame src="a"><frame src="b">` with NO closing
    /// tags at all (unlike ordinary transitional markup's tolerated-but-
    /// unnecessary self-closing `/>`). Before `frame` is void, the second
    /// `<frame>` would nest INSIDE the first (since a non-void element stays
    /// open on the stack) rather than becoming its sibling — silently
    /// collapsing every real-world frameset to one visible frame. This is
    /// what the `frames` packet's review caught: its own fixture only
    /// worked because it hand-added `</frame>` close tags no real 1996 page
    /// would have written.
    #[test]
    fn frame_is_void_like_real_1996_framesets_write_it() {
        let dom = parse(r#"<frameset><frame src="a"><frame src="b"></frameset>"#);
        let root = dom.root();
        let frameset = find_descendant(&dom, root, "frameset").expect("frameset");
        let frames = find_children(&dom, frameset, "frame");
        assert_eq!(frames.len(), 2, "two <frame>s should be SIBLINGS under <frameset>, not nested");
        assert!(children_of(&dom, frames[0]).is_empty(), "a void <frame> has no children");
        assert!(children_of(&dom, frames[1]).is_empty(), "a void <frame> has no children");
        let el0 = dom.node(frames[0]).element().unwrap();
        let el1 = dom.node(frames[1]).element().unwrap();
        assert_eq!(el0.attrs.get("src"), Some("a"));
        assert_eq!(el1.attrs.get("src"), Some("b"));
    }

    #[test]
    fn img_is_void_with_attrs() {
        let dom = parse("<img src=\"pic.gif\" alt=\"a picture\">");
        let root = dom.root();
        let img = find_descendant(&dom, root, "img").expect("img");
        let el = dom.node(img).element().unwrap();
        assert_eq!(el.attrs.get("src"), Some("pic.gif"));
        assert_eq!(el.attrs.get("alt"), Some("a picture"));
        assert!(el.children.is_empty());
    }

    #[test]
    fn self_closing_slash_on_non_void_element_opens_normally() {
        // XHTML-style self-closing on an ordinary (non-void) element is
        // common in the transitional markup Stele targets, but HTML has no
        // real self-closing tags outside VOID_ELEMENTS: browsers ignore the
        // trailing '/' and keep the element open for content and its real
        // close tag. `<div/>x</div>` must NOT turn "x" into a sibling of an
        // empty <div>, and the following </div> must close that same div.
        let dom = parse("<div/>x</div>");
        let root = dom.root();
        let div = find_descendant(&dom, root, "div").expect("div");
        assert_eq!(text_of(&dom, div), "x", "\"x\" must be a child of the div, not its sibling");
        assert!(children_of(&dom, root).len() >= 1);
        // The div must be the only top-level element carrying "x" -- i.e. no
        // stray top-level text node holding "x" outside the div.
        let root_children = children_of(&dom, root);
        let stray_text: String = root_children
            .iter()
            .filter(|&&c| dom.node(c).text().is_some())
            .map(|&c| dom.node(c).text().unwrap().to_string())
            .collect();
        assert!(!stray_text.contains('x'), "\"x\" leaked out as a top-level sibling: {stray_text:?}");
    }

    // ------------------------------- comments / doctype -----------------------

    #[test]
    fn comments_and_doctype_are_dropped() {
        let dom = parse("<!doctype html><!-- top comment --><p>a<!-- mid --> b</p>");
        let root = dom.root();
        let p = find_descendant(&dom, root, "p").expect("p");
        assert_eq!(text_of(&dom, p), "a b");
        // No element anywhere should be named after doctype/comment markers.
        assert!(find_descendant(&dom, root, "!--").is_none());
        assert!(find_descendant(&dom, root, "!doctype").is_none());
    }

    // ------------------------------- attributes --------------------------------

    #[test]
    fn quoted_unquoted_and_empty_attributes() {
        let dom = parse(
            "<input type=\"text\" value=unquoted disabled placeholder='single quoted'>",
        );
        let root = dom.root();
        let input = find_descendant(&dom, root, "input").expect("input");
        let el = dom.node(input).element().unwrap();
        assert_eq!(el.attrs.get("type"), Some("text"));
        assert_eq!(el.attrs.get("value"), Some("unquoted"));
        assert_eq!(el.attrs.get("disabled"), Some(""));
        assert_eq!(el.attrs.get("placeholder"), Some("single quoted"));
    }

    #[test]
    fn attribute_names_fold_to_lowercase_and_first_wins() {
        let dom = parse("<a HREF=\"one\" href=\"two\">x</a>");
        let root = dom.root();
        let a = find_descendant(&dom, root, "a").expect("a");
        let el = dom.node(a).element().unwrap();
        assert_eq!(el.attrs.get("href"), Some("one"));
    }

    // ------------------------------- fixtures ----------------------------------

    fn fixture(name: &str) -> String {
        let path = format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {path}: {e}"))
    }

    #[test]
    fn fixture_basic_html_structure() {
        let src = fixture("basic.html");
        let dom = parse(&src);
        let root = dom.root();
        let head = find_descendant(&dom, root, "head").expect("head");
        let title = find_child(&dom, head, "title").expect("title");
        assert_eq!(text_of(&dom, title), "Basic Fixture");

        let body = find_descendant(&dom, root, "body").expect("body");
        let h1 = find_child(&dom, body, "h1").expect("h1");
        assert_eq!(text_of(&dom, h1), "Welcome");
        let h2 = find_child(&dom, body, "h2").expect("h2");
        assert_eq!(text_of(&dom, h2), "Section One");

        let ps = find_children(&dom, body, "p");
        assert_eq!(ps.len(), 2);
        let a = find_child(&dom, ps[0], "a").expect("link inside first paragraph");
        let el = dom.node(a).element().unwrap();
        assert_eq!(el.attrs.get("href"), Some("https://example.com/"));
        assert_eq!(text_of(&dom, a), "link");
    }

    #[test]
    fn fixture_soup_html_structure() {
        let src = fixture("soup.html");
        let dom = parse(&src);
        let root = dom.root();

        // script fully discarded, even though it contains "<b>" text soup.
        assert!(find_descendant(&dom, root, "script").is_none());

        // style kept with raw (untouched) CSS text.
        let style = find_descendant(&dom, root, "style").expect("style kept");
        assert!(text_of(&dom, style).contains("color: red"));

        let body = find_descendant(&dom, root, "body").expect("body");

        // Two <p>s at the top implied-close each other rather than nesting.
        let top_ps = find_children(&dom, body, "p");
        assert!(top_ps.len() >= 2, "implied-close should have produced sibling <p>s");

        // <li> run implicitly closes.
        let ul = find_descendant(&dom, root, "ul").expect("ul");
        assert_eq!(find_children(&dom, ul, "li").len(), 3);

        // <tr>/<td> implicit closes.
        let table = find_descendant(&dom, root, "table").expect("table");
        let trs = find_children(&dom, table, "tr");
        assert_eq!(trs.len(), 2);
        assert_eq!(find_children(&dom, trs[0], "td").len(), 2);
        assert_eq!(find_children(&dom, trs[1], "td").len(), 2);

        // <dt>/<dd> implicit close.
        let dl = find_descendant(&dom, root, "dl").expect("dl");
        assert!(find_child(&dom, dl, "dt").is_some());
        assert!(find_child(&dom, dl, "dd").is_some());

        // void elements present with no children.
        let img = find_descendant(&dom, root, "img").expect("img");
        assert!(dom.node(img).element().unwrap().children.is_empty());

        // entities decoded somewhere in the document text.
        let body_text = text_of(&dom, body);
        assert!(body_text.contains('\u{00A9}'), "copy entity should decode");
        assert!(body_text.contains('\u{00A0}'), "nbsp entity should decode");
    }
}
