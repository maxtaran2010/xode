//! HTML -> compact markdown-ish text.

use ego_tree::NodeRef;
use scraper::{Html, Node, Selector};

const SKIP: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "canvas", "iframe", "object", "embed", "nav", "footer", "header",
    "aside", "form", "button", "select", "input", "textarea", "head", "meta", "link", "dialog", "video", "audio", "map",
];
const BLOCK: &[&str] = &[
    "p", "div", "section", "article", "main", "ul", "ol", "table", "thead", "tbody", "tfoot", "tr", "dl", "dt", "dd",
    "figure", "figcaption", "blockquote", "address", "details", "summary", "center", "hr", "body", "html",
];

pub struct Page {
    pub title: String,
    pub text: String,
}

/// Convert an HTML document to readable text. Uses `<main>`/`<article>` when it
/// holds most of the content.
pub fn html_to_text(html: &str) -> Page {
    let doc = Html::parse_document(html);
    let title = Selector::parse("title")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|t| crate::util::collapse_ws(&t.text().collect::<String>()))
        .unwrap_or_default();

    let mut root: NodeRef<Node> = doc.tree.root();
    for sel in ["main", "[role=main]", "article"] {
        if let Ok(s) = Selector::parse(sel) {
            let found: Vec<_> = doc.select(&s).collect();
            if found.len() == 1 {
                let len: usize = found[0].text().map(|t| t.trim().len()).sum();
                if len > 400 {
                    root = *found[0];
                    break;
                }
            }
        }
    }
    let mut w = Writer::default();
    w.walk(root, 0);
    Page { title, text: tidy(&w.out) }
}

/// Convert an HTML fragment (e.g. a snippet with <b> tags) to plain text.
pub fn fragment_text(html: &str) -> String {
    let f = Html::parse_fragment(html);
    crate::util::collapse_ws(&f.root_element().text().collect::<String>())
}

#[derive(Default)]
struct Writer {
    out: String,
    list: Vec<Option<usize>>, // None = ul, Some(n) = ol counter
    pre: bool,
}

impl Writer {
    fn brk(&mut self, n: usize) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if self.out.is_empty() {
            return;
        }
        let have = self.out.chars().rev().take_while(|c| *c == '\n').count();
        for _ in have..n {
            self.out.push('\n');
        }
    }

    fn text(&mut self, t: &str) {
        if self.pre {
            self.out.push_str(t);
            return;
        }
        let lead = t.starts_with(char::is_whitespace);
        let trail = t.ends_with(char::is_whitespace);
        let c = crate::util::collapse_ws(t);
        if c.is_empty() {
            if (lead || trail) && !self.out.is_empty() && !self.out.ends_with(['\n', ' ']) {
                self.out.push(' ');
            }
            return;
        }
        if lead && !self.out.is_empty() && !self.out.ends_with(['\n', ' ']) {
            self.out.push(' ');
        }
        self.out.push_str(&c);
        if trail {
            self.out.push(' ');
        }
    }

    fn children(&mut self, n: NodeRef<Node>, depth: usize) {
        for c in n.children() {
            self.walk(c, depth + 1);
        }
    }

    fn walk(&mut self, n: NodeRef<Node>, depth: usize) {
        if depth > 200 {
            return;
        }
        match n.value() {
            Node::Text(t) => self.text(t),
            Node::Element(e) => {
                let tag = e.name();
                if SKIP.contains(&tag) {
                    return;
                }
                if e.attr("hidden").is_some() || e.attr("aria-hidden") == Some("true") {
                    return;
                }
                if let Some(r) = e.attr("role") {
                    if matches!(r, "navigation" | "banner" | "contentinfo" | "search" | "dialog") {
                        return;
                    }
                }
                match tag {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let lvl = tag[1..].parse::<usize>().unwrap_or(1);
                        self.brk(2);
                        self.out.push_str(&"#".repeat(lvl));
                        self.out.push(' ');
                        self.children(n, depth);
                        self.brk(2);
                    }
                    "br" => {
                        while self.out.ends_with(' ') {
                            self.out.pop();
                        }
                        self.out.push('\n');
                    }
                    "pre" => {
                        self.brk(2);
                        self.out.push_str("```\n");
                        let was = self.pre;
                        self.pre = true;
                        self.children(n, depth);
                        self.pre = was;
                        if !self.out.ends_with('\n') {
                            self.out.push('\n');
                        }
                        self.out.push_str("```");
                        self.brk(2);
                    }
                    "code" if !self.pre => {
                        self.out.push('`');
                        self.children(n, depth);
                        while self.out.ends_with(' ') {
                            self.out.pop();
                        }
                        self.out.push('`');
                    }
                    "ul" | "ol" => {
                        self.brk(if self.list.is_empty() { 2 } else { 1 });
                        self.list.push(if tag == "ol" { Some(0) } else { None });
                        self.children(n, depth);
                        self.list.pop();
                        self.brk(if self.list.is_empty() { 2 } else { 1 });
                    }
                    "li" => {
                        self.brk(1);
                        let ind = "  ".repeat(self.list.len().saturating_sub(1));
                        let mark = match self.list.last_mut() {
                            Some(Some(k)) => {
                                *k += 1;
                                format!("{k}. ")
                            }
                            _ => "- ".to_string(),
                        };
                        self.out.push_str(&ind);
                        self.out.push_str(&mark);
                        self.children(n, depth);
                        self.brk(1);
                    }
                    "td" | "th" => {
                        if !self.out.ends_with('\n') && !self.out.is_empty() {
                            while self.out.ends_with(' ') {
                                self.out.pop();
                            }
                            self.out.push_str(" | ");
                        }
                        self.children(n, depth);
                    }
                    "img" => {
                        if let Some(a) = e.attr("alt").map(str::trim).filter(|a| !a.is_empty() && a.len() < 120) {
                            self.text(&format!(" [{a}] "));
                        }
                    }
                    "blockquote" => {
                        self.brk(2);
                        self.out.push_str("> ");
                        self.children(n, depth);
                        self.brk(2);
                    }
                    _ if BLOCK.contains(&tag) => {
                        self.brk(1);
                        self.children(n, depth);
                        self.brk(if tag == "p" { 2 } else { 1 });
                    }
                    _ => self.children(n, depth),
                }
            }
            Node::Document | Node::Fragment => self.children(n, depth),
            _ => {}
        }
    }
}

/// Trim lines, drop runs of blank lines and lines that are just punctuation.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    let mut in_code = false;
    for line in s.lines() {
        let fence = line.trim_start().starts_with("```");
        if fence {
            in_code = !in_code;
        }
        let l = if in_code {
            line.trim_end()
        } else {
            let t = line.trim();
            let li = t.starts_with("- ") || t.split_once(". ").is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()));
            if li { line.trim_end() } else { t }
        };
        let meaningful = in_code || fence || l.chars().any(|c| c.is_alphanumeric());
        if l.is_empty() || !meaningful {
            blank += 1;
            continue;
        }
        if blank > 0 && !out.is_empty() {
            out.push('\n');
        }
        blank = 0;
        out.push_str(l);
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_basic() {
        let h = r#"<html><head><title>My  Page</title><style>body{}</style></head><body>
<nav><a href="/">Home</a><a href="/x">Nav link</a></nav>
<header><div>site header</div></header>
<h1>Main   Title</h1>
<p>Hello <b>bold</b> and <a href="https://x.y">a link</a>.</p>
<ul><li>one</li><li>two<ul><li>nested</li></ul></li></ul>
<ol><li>first</li><li>second</li></ol>
<pre><code>fn main() {
    println!("hi");
}</code></pre>
<p>Use <code>cargo build</code> now.</p>
<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>
<script>var x = 1;</script>
<footer>copyright</footer>
</body></html>"#;
        let p = html_to_text(h);
        assert_eq!(p.title, "My Page");
        let t = p.text;
        assert!(t.contains("# Main Title"), "{t}");
        assert!(t.contains("Hello bold and a link."), "{t}");
        assert!(t.contains("- one\n- two\n  - nested"), "{t}");
        assert!(t.contains("1. first\n2. second"), "{t}");
        assert!(t.contains("```\nfn main() {\n    println!(\"hi\");\n}\n```"), "{t}");
        assert!(t.contains("Use `cargo build` now."), "{t}");
        assert!(t.contains("A | B\n1 | 2"), "{t}");
        assert!(!t.contains("var x"));
        assert!(!t.contains("copyright"));
        assert!(!t.contains("Nav link"));
        assert!(!t.contains("site header"));
    }

    #[test]
    fn prefers_main() {
        let body = "word ".repeat(200);
        let h = format!("<body><div>sidebar junk</div><main><h2>Doc</h2><p>{body}</p></main></body>");
        let t = html_to_text(&h).text;
        assert!(t.starts_with("## Doc"));
        assert!(!t.contains("sidebar"));
    }

    #[test]
    fn fragment() {
        assert_eq!(fragment_text("Rust <strong>async</strong> &amp; more"), "Rust async & more");
    }
}
