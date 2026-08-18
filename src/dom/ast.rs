//! The document as a closed sum type — the covenant's enforcement point.
//!
//! Charter C3: uninvited, executable wire content is unrepresentable. The node
//! kinds below (`Node`) are the ENTIRE vocabulary a fetched page can become,
//! and not one of them can carry code. Material the wire tries to smuggle in as
//! an executable payload has no home in this type; the parser drops it at the
//! door. `accept.sh` greps THIS file to guarantee the vocabulary never quietly
//! grows a variant that could hold such a payload.
//!
//! The tree is arena-allocated: every node lives in one flat `Vec` and refers
//! to others by index (`NodeId`). That is cache-kind on a 486 and dodges the
//! ownership knots that tag-soup recovery would otherwise tie.

/// Index of a node within a [`Dom`]'s arena.
pub type NodeId = usize;

/// A whole parsed document: a flat arena of nodes plus the id of the root.
#[derive(Debug, Clone)]
pub struct Dom {
    nodes: Vec<Node>,
    root: NodeId,
}

/// The closed set of node kinds. Two shapes, neither of them executable:
/// a named element, or a run of character data.
#[derive(Debug, Clone)]
pub enum Node {
    Element(Element),
    Text(String),
}

impl Node {
    /// Borrow the element within, if this node is one.
    pub fn element(&self) -> Option<&Element> {
        match self {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        }
    }

    /// Borrow the character data within, if this node is one.
    pub fn text(&self) -> Option<&str> {
        match self {
            Node::Text(t) => Some(t),
            Node::Element(_) => None,
        }
    }
}

/// A named box: its tag name, its attributes, and its children (by id).
#[derive(Debug, Clone)]
pub struct Element {
    pub name: ElementName,
    pub attrs: AttrMap,
    pub children: Vec<NodeId>,
}

/// A lowercased element or attribute name.
///
/// Semantic classification — which tags are blocks, which are replaced, their
/// default `display`, and so on — is NOT baked in here. It lives in the
/// user-agent stylesheet and the style layer, exactly as it does on the real
/// web. Keeping names as interned text keeps this file tiny and keeps tag-soup
/// tolerant of names we do not implement.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ElementName(Box<str>);

impl ElementName {
    /// Intern a raw name, folded to ASCII lowercase and trimmed.
    pub fn new(raw: &str) -> Self {
        ElementName(raw.trim().to_ascii_lowercase().into_boxed_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Element attributes in source order, looked up case-insensitively. 1996 pages
/// repeat and mis-case attributes freely; first value wins, comparisons fold
/// ASCII case.
#[derive(Debug, Clone, Default)]
pub struct AttrMap {
    pairs: Vec<(Box<str>, Box<str>)>,
}

impl AttrMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_ref())
    }

    /// Record an attribute. First value for a given name wins (tag-soup rule).
    pub fn set(&mut self, name: &str, value: &str) {
        if self.get(name).is_none() {
            self.pairs
                .push((name.trim().to_ascii_lowercase().into_boxed_str(), value.into()));
        }
    }

    /// Insert or overwrite an attribute's value — unlike [`set`](Self::set)
    /// (first-value-wins, the tag-soup PARSE-TIME rule), this always takes
    /// the NEW value: replacing an existing pair's value case-insensitively,
    /// or appending a fresh pair if the name wasn't present. Built for
    /// post-parse mutation (packet t1b-color-scheme's `--color-scheme`
    /// pre-cascade `data-theme`/`data-mode` root stamp — see
    /// `Dom::set_attribute`), where "the caller's new value wins" is the
    /// correct semantics, not "the document's original value wins".
    pub fn set_overwrite(&mut self, name: &str, value: &str) {
        if let Some(pair) = self.pairs.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            pair.1 = value.into();
        } else {
            self.pairs
                .push((name.trim().to_ascii_lowercase().into_boxed_str(), value.into()));
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.pairs.iter().map(|(k, v)| (k.as_ref(), v.as_ref()))
    }

    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

impl Dom {
    /// A new arena seeded with a root element (conventionally `<html>`).
    pub fn new() -> Self {
        let root = Node::Element(Element {
            name: ElementName::new("html"),
            attrs: AttrMap::new(),
            children: Vec::new(),
        });
        Dom {
            nodes: vec![root],
            root: 0,
        }
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Insert or overwrite one attribute on `id`'s element (see
    /// [`AttrMap::set_overwrite`]) — a total no-op, never a panic, if `id`
    /// is out of range or names a `Text` node rather than an `Element`.
    /// Built for packet t1b-color-scheme's `--color-scheme` pre-cascade
    /// `data-theme`/`data-mode` root stamp (`main.rs`'s `stamp_color_scheme`)
    /// but deliberately general (any node, any attribute) rather than
    /// hardcoded to that one caller.
    pub fn set_attribute(&mut self, id: NodeId, name: &str, value: &str) {
        let Some(node) = self.nodes.get_mut(id) else {
            return;
        };
        if let Node::Element(el) = node {
            el.attrs.set_overwrite(name, value);
        }
    }

    /// Allocate an element node, returning its id. Not yet attached to a parent.
    pub fn new_element(&mut self, name: ElementName) -> NodeId {
        self.nodes.push(Node::Element(Element {
            name,
            attrs: AttrMap::new(),
            children: Vec::new(),
        }));
        self.nodes.len() - 1
    }

    /// Allocate a character-data node, returning its id.
    pub fn new_text(&mut self, text: String) -> NodeId {
        self.nodes.push(Node::Text(text));
        self.nodes.len() - 1
    }

    /// Attach `child` under `parent`. Panics if `parent` is character data.
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        match &mut self.nodes[parent] {
            Node::Element(el) => el.children.push(child),
            Node::Text(_) => panic!("append_child: parent {parent} is character data, not an element"),
        }
    }
}

impl Default for Dom {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_builds_and_reads_back_a_small_tree() {
        let mut dom = Dom::new();
        let para = dom.new_element(ElementName::new("P")); // folds to "p"
        let words = dom.new_text("hello".to_string());
        dom.append_child(para, words);
        dom.append_child(dom.root(), para);

        let root = dom.root();
        let root_el = dom.node(root).element().expect("root is an element");
        assert_eq!(root_el.name.as_str(), "html");
        assert_eq!(root_el.children, vec![para]);
        assert_eq!(dom.node(para).element().unwrap().name.as_str(), "p");
        assert_eq!(dom.node(words).text(), Some("hello"));
    }

    #[test]
    fn attrs_are_case_insensitive_and_first_wins() {
        let mut a = AttrMap::new();
        a.set("HREF", "one");
        a.set("href", "two");
        assert_eq!(a.get("href"), Some("one"));
        assert_eq!(a.get("HrEf"), Some("one"));
        assert_eq!(a.get("missing"), None);
        assert_eq!(a.len(), 1);
    }

    // ---- T1b: AttrMap::set_overwrite / Dom::set_attribute --------------------
    // (packet t1b-color-scheme: the `--color-scheme` pre-cascade `data-theme`/
    // `data-mode` root stamp needs an OVERWRITE-semantics attribute setter,
    // unlike `AttrMap::set`'s parse-time first-value-wins rule.)

    #[test]
    fn set_overwrite_replaces_an_existing_value_case_insensitively() {
        let mut a = AttrMap::new();
        a.set("data-theme", "light");
        a.set_overwrite("DATA-THEME", "dark");
        assert_eq!(a.get("data-theme"), Some("dark"));
        assert_eq!(a.len(), 1, "overwrite must not add a duplicate pair");
    }

    #[test]
    fn set_overwrite_inserts_when_the_attribute_is_absent() {
        let mut a = AttrMap::new();
        a.set_overwrite("data-mode", "dark");
        assert_eq!(a.get("data-mode"), Some("dark"));
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn dom_set_attribute_overwrites_the_root_elements_attribute() {
        let mut dom = Dom::new();
        let root = dom.root();
        dom.set_attribute(root, "data-theme", "dark");
        dom.set_attribute(root, "data-theme", "light");
        let el = dom.node(root).element().expect("root is an element");
        assert_eq!(el.attrs.get("data-theme"), Some("light"));
    }

    #[test]
    fn dom_set_attribute_sets_a_fresh_attribute_when_absent() {
        let mut dom = Dom::new();
        let root = dom.root();
        dom.set_attribute(root, "data-mode", "dark");
        let el = dom.node(root).element().expect("root is an element");
        assert_eq!(el.attrs.get("data-mode"), Some("dark"));
    }

    #[test]
    fn dom_set_attribute_is_a_total_noop_on_an_out_of_range_id() {
        let mut dom = Dom::new();
        dom.set_attribute(9999, "data-theme", "dark"); // must not panic
    }

    #[test]
    fn dom_set_attribute_is_a_noop_on_a_text_node() {
        let mut dom = Dom::new();
        let text = dom.new_text("hi".to_string());
        dom.set_attribute(text, "data-theme", "dark"); // must not panic
        assert_eq!(dom.node(text).text(), Some("hi"));
    }
}
