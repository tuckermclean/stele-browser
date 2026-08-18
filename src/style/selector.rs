//! Selector matching (brief §4): element, `.class`, `#id`, descendant,
//! grouping, `a:link`/`a:visited`, and (packet t1b-color-scheme)
//! `[attr]`/`[attr=value]` attribute selectors. Anything else parses without
//! choking (the tokenizer/parser never rejects it) but is marked unsupported
//! so it simply never matches — charter C2's ignore-unknown treaty applied
//! to selectors instead of declarations.

use crate::dom::{AttrMap, ElementName};

/// (id count, class+pseudo count, element count). Field order is precedence
/// order: `#[derive(Ord)]` compares top-to-bottom, so this sorts exactly the
/// way CSS specificity does (id beats class beats element).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) struct Specificity {
    pub ids: u32,
    pub classes: u32,
    pub elements: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pseudo {
    Link,
    Visited,
}

/// How one `[attr...]` selector's value clause tests an attribute (packet
/// t1b-color-scheme). Only the two simplest CSS attribute-selector forms are
/// in the curated subset — `[attr]` (presence) and `[attr=value]` (exact,
/// case-SENSITIVE value match, no `i` flag) — mirroring `media.rs`'s own
/// "curated subset, everything else fails closed" posture: `~=`/`^=`/`$=`/
/// `*=`/`|=` and the `i` case-insensitivity flag all parse without choking
/// (`parser::parse_selector`'s `[` handling) but mark the whole selector
/// `supported: false` rather than being evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttrMatch {
    Present,
    Equals(String),
}

/// One `[attr...]` clause within a [`Compound`] (packet t1b-color-scheme) —
/// e.g. `[data-theme="dark"]` is `AttrSelector { name: "data-theme", match_:
/// AttrMatch::Equals("dark".into()) }`. `name` is stored already lowercased
/// (attribute names are ASCII-case-insensitive in HTML); the VALUE is kept
/// exactly as written — real CSS attribute-value matching is case-sensitive
/// by default (only an explicit trailing `i` flag, which this curated subset
/// doesn't support, would fold it).
#[derive(Debug, Clone)]
pub(crate) struct AttrSelector {
    pub name: String,
    pub match_: AttrMatch,
}

/// One simple selector: an optional element name, id, classes, pseudo
/// classes, and attribute selectors, all of which must match the same
/// element.
#[derive(Debug, Clone, Default)]
pub(crate) struct Compound {
    pub element: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub pseudo: Vec<Pseudo>,
    pub attrs: Vec<AttrSelector>,
}

impl Compound {
    fn specificity(&self) -> Specificity {
        Specificity {
            ids: self.id.is_some() as u32,
            // Attribute selectors carry class-level specificity, same as
            // real CSS (a `.class`, a `[attr]`, and a `:pseudo-class` are
            // all specificity (0,1,0) each).
            classes: (self.classes.len() + self.pseudo.len() + self.attrs.len()) as u32,
            elements: self.element.is_some() as u32,
        }
    }

    fn matches(&self, info: &ElementInfo) -> bool {
        if let Some(el) = &self.element {
            if el != &info.name {
                return false;
            }
        }
        if let Some(id) = &self.id {
            if Some(id.as_str()) != info.id.as_deref() {
                return false;
            }
        }
        if !self.classes.iter().all(|c| info.classes.iter().any(|ic| ic == c)) {
            return false;
        }
        for p in &self.pseudo {
            match p {
                Pseudo::Link => {
                    if !info.has_href {
                        return false;
                    }
                }
                // No history in v0 — nothing is ever :visited.
                Pseudo::Visited => return false,
            }
        }
        for a in &self.attrs {
            match &a.match_ {
                AttrMatch::Present => {
                    if info.attrs.get(&a.name).is_none() {
                        return false;
                    }
                }
                AttrMatch::Equals(want) => match info.attrs.get(&a.name) {
                    Some(v) if v == want.as_str() => {}
                    _ => return false,
                },
            }
        }
        true
    }
}

/// A descendant chain: `compounds[0]` is the outermost ancestor constraint,
/// `compounds.last()` is the subject that must match the target element.
/// `supported` is false for constructs outside brief §4's scope (child/
/// sibling combinators, attribute selectors, unknown pseudo-classes, …) —
/// such selectors still parse cleanly, they just never match anything.
#[derive(Debug, Clone)]
pub(crate) struct Selector {
    pub compounds: Vec<Compound>,
    pub supported: bool,
}

impl Selector {
    pub fn specificity(&self) -> Specificity {
        let mut total = Specificity::default();
        for c in &self.compounds {
            let s = c.specificity();
            total.ids += s.ids;
            total.classes += s.classes;
            total.elements += s.elements;
        }
        total
    }

    /// Does this selector match `target`, given its ancestor chain (root
    /// first, immediate parent last)? Only the descendant combinator is
    /// implemented: each non-subject compound just needs *some* matching
    /// ancestor, in order.
    pub fn matches(&self, ancestors: &[ElementInfo], target: &ElementInfo) -> bool {
        if !self.supported {
            return false;
        }
        let Some((last, rest)) = self.compounds.split_last() else {
            return false;
        };
        if !last.matches(target) {
            return false;
        }
        let mut idx = ancestors.len();
        for compound in rest.iter().rev() {
            let mut found = false;
            while idx > 0 {
                idx -= 1;
                if compound.matches(&ancestors[idx]) {
                    found = true;
                    break;
                }
            }
            if !found {
                return false;
            }
        }
        true
    }
}

/// The bits of an element the cascade needs to test selectors against,
/// captured once per node so matching doesn't repeatedly re-walk `AttrMap`.
/// `attrs` (packet t1b-color-scheme) is a full clone of the element's own
/// `AttrMap` — cheap (an `AttrMap` is just a small `Vec` of interned
/// strings) and lets [`Compound`]'s new `[attr...]` matching reuse
/// `AttrMap::get`'s existing case-insensitive-by-name lookup rather than
/// duplicating it.
#[derive(Debug, Clone)]
pub(crate) struct ElementInfo {
    name: String,
    id: Option<String>,
    classes: Vec<String>,
    has_href: bool,
    attrs: AttrMap,
}

impl ElementInfo {
    pub fn from_element(name: &ElementName, attrs: &AttrMap) -> Self {
        let id = attrs.get("id").map(|s| s.trim().to_ascii_lowercase());
        let classes = attrs
            .get("class")
            .map(|c| c.split_whitespace().map(|s| s.to_ascii_lowercase()).collect())
            .unwrap_or_default();
        let has_href = attrs.get("href").is_some();
        ElementInfo {
            name: name.as_str().to_string(),
            id,
            classes,
            has_href,
            attrs: attrs.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::AttrMap;

    fn info(name: &str, id: Option<&str>, classes: &[&str], has_href: bool) -> ElementInfo {
        ElementInfo {
            name: name.to_string(),
            id: id.map(|s| s.to_string()),
            classes: classes.iter().map(|s| s.to_string()).collect(),
            has_href,
            attrs: AttrMap::new(),
        }
    }

    fn compound(element: Option<&str>) -> Compound {
        Compound {
            element: element.map(|s| s.to_string()),
            ..Compound::default()
        }
    }

    #[test]
    fn element_compound_matches_by_name() {
        let c = compound(Some("p"));
        assert!(c.matches(&info("p", None, &[], false)));
        assert!(!c.matches(&info("div", None, &[], false)));
    }

    #[test]
    fn class_compound_requires_all_classes_present() {
        let c = Compound {
            classes: vec!["a".into(), "b".into()],
            ..Compound::default()
        };
        assert!(c.matches(&info("div", None, &["a", "b", "c"], false)));
        assert!(!c.matches(&info("div", None, &["a"], false)));
    }

    #[test]
    fn id_compound_matches_exact_id() {
        let c = Compound {
            id: Some("x".into()),
            ..Compound::default()
        };
        assert!(c.matches(&info("div", Some("x"), &[], false)));
        assert!(!c.matches(&info("div", Some("y"), &[], false)));
        assert!(!c.matches(&info("div", None, &[], false)));
    }

    #[test]
    fn link_pseudo_requires_href() {
        let c = Compound {
            element: Some("a".into()),
            pseudo: vec![Pseudo::Link],
            ..Compound::default()
        };
        assert!(c.matches(&info("a", None, &[], true)));
        assert!(!c.matches(&info("a", None, &[], false)));
    }

    #[test]
    fn visited_pseudo_never_matches_without_history() {
        let c = Compound {
            element: Some("a".into()),
            pseudo: vec![Pseudo::Visited],
            ..Compound::default()
        };
        assert!(!c.matches(&info("a", None, &[], true)));
    }

    #[test]
    fn descendant_selector_requires_ancestor_in_order() {
        let sel = Selector {
            compounds: vec![compound(Some("div")), compound(Some("p"))],
            supported: true,
        };
        let ancestors = vec![info("body", None, &[], false), info("div", None, &[], false)];
        assert!(sel.matches(&ancestors, &info("p", None, &[], false)));

        let wrong_ancestors = vec![info("body", None, &[], false), info("span", None, &[], false)];
        assert!(!sel.matches(&wrong_ancestors, &info("p", None, &[], false)));
    }

    #[test]
    fn unsupported_selector_never_matches() {
        let sel = Selector {
            compounds: vec![compound(Some("p"))],
            supported: false,
        };
        assert!(!sel.matches(&[], &info("p", None, &[], false)));
    }

    #[test]
    fn specificity_orders_id_over_class_over_element() {
        let id_sel = Specificity { ids: 1, classes: 0, elements: 0 };
        let class_sel = Specificity { ids: 0, classes: 1, elements: 0 };
        let el_sel = Specificity { ids: 0, classes: 0, elements: 1 };
        assert!(id_sel > class_sel);
        assert!(class_sel > el_sel);
    }

    #[test]
    fn element_info_reads_id_class_and_href_case_insensitively() {
        let mut attrs = AttrMap::new();
        attrs.set("ID", "Foo");
        attrs.set("class", "Bar Baz");
        attrs.set("HREF", "x");
        let info = ElementInfo::from_element(&ElementName::new("A"), &attrs);
        assert_eq!(info.name, "a");
        assert_eq!(info.id.as_deref(), Some("foo"));
        assert_eq!(info.classes, vec!["bar".to_string(), "baz".to_string()]);
        assert!(info.has_href);
    }

    // ---- T1b: attribute selectors ([attr], [attr=value]) ---------------------
    // (packet t1b-color-scheme: `html[data-theme="dark"]` needs to actually
    // match once `main.rs` stamps that attribute pre-cascade -- see
    // `Compound.attrs`/`AttrSelector`/`AttrMatch` below.)

    fn info_with_attr(name: &str, attr_name: &str, attr_value: &str) -> ElementInfo {
        let mut attrs = AttrMap::new();
        attrs.set(attr_name, attr_value);
        ElementInfo::from_element(&ElementName::new(name), &attrs)
    }

    #[test]
    fn attr_present_selector_matches_when_the_attribute_exists() {
        let c = Compound {
            attrs: vec![AttrSelector { name: "data-theme".into(), match_: AttrMatch::Present }],
            ..Compound::default()
        };
        assert!(c.matches(&info_with_attr("html", "data-theme", "dark")));
        assert!(!c.matches(&info("html", None, &[], false)));
    }

    #[test]
    fn attr_equals_selector_matches_only_the_exact_value() {
        let c = Compound {
            element: Some("html".into()),
            attrs: vec![AttrSelector { name: "data-theme".into(), match_: AttrMatch::Equals("dark".into()) }],
            ..Compound::default()
        };
        assert!(c.matches(&info_with_attr("html", "data-theme", "dark")));
        assert!(!c.matches(&info_with_attr("html", "data-theme", "light")));
        assert!(!c.matches(&info("html", None, &[], false)));
    }
}
