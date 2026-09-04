//! HTML rendering.
//!
//! Everything is escaped on the way in. [`Element::text`] takes a string and
//! escapes it; there is deliberately no method that inserts raw markup, so a
//! value that came from a rationale, an agent id or an error message cannot
//! carry markup into the page. Cross-site scripting is not something this
//! module tries to catch — it is something it has no way to express.
//!
//! The pages carry no JavaScript at all. That is a decision, not an omission:
//! the content-security policy the API sets forbids script entirely, so a
//! page that needed it would not work, and the interactions here are links and
//! form submissions that a server can answer.

use std::fmt::Write;

/// Escape text for an HTML element body or attribute value.
///
/// Both contexts at once, which is stricter than either needs and removes the
/// question of which one a given call site is in.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // A slash inside an attribute can close a tag in some parsers.
            '/' => out.push_str("&#47;"),
            c => out.push(c),
        }
    }
    out
}

/// A node in the tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// Text, escaped when rendered.
    Text(String),
    Element(Element),
    /// Unescaped content, for `<style>` alone. Never exposed outside this
    /// crate: `Element::raw` that builds it is `pub(crate)`, so nothing an
    /// API layer assembles into a [`crate::view::ViewModel`] or
    /// [`crate::console::ConsoleModel`] can reach it, and the only caller is
    /// [`crate::style::STYLESHEET`], a compile-time constant.
    Raw(String),
}

impl Node {
    pub fn render(&self) -> String {
        match self {
            Self::Text(text) => escape(text),
            Self::Element(element) => element.render(),
            Self::Raw(text) => text.clone(),
        }
    }
}

/// An HTML element.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    tag: &'static str,
    attributes: Vec<(String, String)>,
    children: Vec<Node>,
    /// Whether the tag closes itself.
    void: bool,
}

impl Element {
    pub fn new(tag: &'static str) -> Self {
        Self {
            tag,
            attributes: Vec::new(),
            children: Vec::new(),
            void: matches!(tag, "meta" | "link" | "br" | "hr" | "img" | "input"),
        }
    }

    pub fn attr(mut self, name: &str, value: &str) -> Self {
        self.attributes.push((name.to_string(), value.to_string()));
        self
    }

    pub fn class(self, value: &str) -> Self {
        self.attr("class", value)
    }

    /// Add escaped text.
    pub fn text(mut self, text: impl AsRef<str>) -> Self {
        self.children.push(Node::Text(text.as_ref().to_string()));
        self
    }

    /// Add unescaped content. `pub(crate)` and used for exactly one call
    /// site: embedding [`crate::style::STYLESHEET`] into a `<style>`
    /// element.
    ///
    /// `<style>` is a "raw text" element in the HTML5 parsing model — a
    /// browser never decodes an entity reference inside one — so routing the
    /// stylesheet through [`Element::text`] did not protect anything and
    /// instead corrupted it: `"SF Mono"` shipped as the literal characters
    /// `&quot;SF Mono&quot;`, `/* ... */` comments shipped as `&#47;* ... *&#47;`
    /// (no longer a comment to a CSS parser), and `font: 15px/1.55` shipped
    /// as `font: 15px&#47;1.55`. This method exists so the stylesheet can be
    /// embedded as the CSS it actually is, without adding a raw-markup path
    /// any value from a platform record could reach — every other call site
    /// in this crate still goes through `text`, and this method is not
    /// re-exported.
    pub(crate) fn raw(mut self, text: &str) -> Self {
        self.children.push(Node::Raw(text.to_string()));
        self
    }

    pub fn child(mut self, element: Element) -> Self {
        self.children.push(Node::Element(element));
        self
    }

    pub fn children(mut self, elements: impl IntoIterator<Item = Element>) -> Self {
        self.children
            .extend(elements.into_iter().map(Node::Element));
        self
    }

    /// Render to HTML.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = write!(out, "<{}", self.tag);
        for (name, value) in &self.attributes {
            // The attribute name is a compile-time constant at every call
            // site; the value is escaped because it often is not.
            let _ = write!(out, " {}=\"{}\"", name, escape(value));
        }
        if self.void {
            out.push_str(" />");
            return out;
        }
        out.push('>');
        for child in &self.children {
            out.push_str(&child.render());
        }
        let _ = write!(out, "</{}>", self.tag);
        out
    }
}

/// Shorthands for the tags the pages actually use.
pub fn div() -> Element {
    Element::new("div")
}
pub fn span() -> Element {
    Element::new("span")
}
pub fn p() -> Element {
    Element::new("p")
}
pub fn h1() -> Element {
    Element::new("h1")
}
pub fn h2() -> Element {
    Element::new("h2")
}
pub fn h3() -> Element {
    Element::new("h3")
}
pub fn table() -> Element {
    Element::new("table")
}
pub fn thead() -> Element {
    Element::new("thead")
}
pub fn tbody() -> Element {
    Element::new("tbody")
}
pub fn tr() -> Element {
    Element::new("tr")
}
pub fn th() -> Element {
    Element::new("th")
}
pub fn td() -> Element {
    Element::new("td")
}
pub fn a(href: &str) -> Element {
    Element::new("a").attr("href", href)
}
pub fn ul() -> Element {
    Element::new("ul")
}
pub fn li() -> Element {
    Element::new("li")
}
pub fn section() -> Element {
    Element::new("section")
}
pub fn nav() -> Element {
    Element::new("nav")
}
pub fn header() -> Element {
    Element::new("header")
}
pub fn main_element() -> Element {
    Element::new("main")
}
pub fn code() -> Element {
    Element::new("code")
}
pub fn strong() -> Element {
    Element::new("strong")
}
/// A form. The only interactive element the console has, and the only one the
/// content-security policy permits: `form-action 'self'` allows a submission
/// back to the origin that served the page, and forbids everything else.
pub fn form(action: &str) -> Element {
    Element::new("form")
        .attr("method", "post")
        .attr("action", action)
}
pub fn button() -> Element {
    Element::new("button").attr("type", "submit")
}
pub fn small() -> Element {
    Element::new("small")
}
