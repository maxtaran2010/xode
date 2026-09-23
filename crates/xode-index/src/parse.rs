//! Symbol + reference extraction: tree-sitter for major languages, regex heuristics otherwise.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    C,
    Cpp,
    CSharp,
    Java,
    Markdown,
    Json,
    Toml,
    Yaml,
    Sql,
    Shell,
    Generic,
}

const SKIP_FILES: &[&str] = &["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "Cargo.lock", "composer.lock", "poetry.lock"];

impl Lang {
    pub fn from_path(p: &Path) -> Option<Lang> {
        let fname = p.file_name()?.to_str()?;
        if SKIP_FILES.contains(&fname) || fname.ends_with(".min.js") || fname.ends_with(".min.css") {
            return None;
        }
        let ext = p.extension()?.to_str()?.to_ascii_lowercase();
        use Lang::*;
        Some(match ext.as_str() {
            "rs" => Rust,
            "ts" | "mts" | "cts" => TypeScript,
            "tsx" => Tsx,
            "js" | "jsx" | "mjs" | "cjs" => JavaScript,
            "py" | "pyi" | "pyw" => Python,
            "go" => Go,
            "c" => C,
            "h" | "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "ino" | "inl" | "ipp" => Cpp,
            "cs" => CSharp,
            "java" => Java,
            "md" | "markdown" | "mdx" => Markdown,
            "json" | "jsonc" | "json5" => Json,
            "toml" => Toml,
            "yaml" | "yml" => Yaml,
            "sql" => Sql,
            "sh" | "bash" | "zsh" => Shell,
            "lua" | "php" | "kt" | "kts" | "swift" | "rb" | "scala" | "sc" | "dart" | "ex" | "exs" | "zig"
            | "ps1" | "psm1" | "vue" | "svelte" | "r" | "pl" | "pm" | "groovy" | "gradle" | "fs" | "fsx"
            | "nim" | "jl" | "hs" | "ml" | "clj" | "erl" | "m" | "mm" | "vb" | "pas" | "cr" | "d" => Generic,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        use Lang::*;
        match self {
            Rust => "rust",
            TypeScript => "ts",
            Tsx => "tsx",
            JavaScript => "js",
            Python => "py",
            Go => "go",
            C => "c",
            Cpp => "cpp",
            CSharp => "cs",
            Java => "java",
            Markdown => "md",
            Json => "json",
            Toml => "toml",
            Yaml => "yaml",
            Sql => "sql",
            Shell => "sh",
            Generic => "text",
        }
    }

    fn ts(&self) -> Option<Language> {
        use Lang::*;
        Some(match self {
            Rust => tree_sitter_rust::LANGUAGE.into(),
            TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Python => tree_sitter_python::LANGUAGE.into(),
            Go => tree_sitter_go::LANGUAGE.into(),
            C => tree_sitter_c::LANGUAGE.into(),
            Cpp => tree_sitter_cpp::LANGUAGE.into(),
            CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Java => tree_sitter_java::LANGUAGE.into(),
            _ => return None,
        })
    }

    fn is_js(&self) -> bool {
        matches!(self, Lang::TypeScript | Lang::Tsx | Lang::JavaScript)
    }
    fn is_c(&self) -> bool {
        matches!(self, Lang::C | Lang::Cpp)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sym {
    pub name: String,
    pub kind: &'static str,
    pub parent: Option<String>,
    pub depth: u32,
    /// 1-based inclusive.
    pub line_start: u32,
    pub line_end: u32,
    pub signature: String,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub syms: Vec<Sym>,
    /// (identifier, 1-based line), deduped.
    pub refs: Vec<(String, u32)>,
    /// First line with a syntax error (tree-sitter languages only).
    pub error_line: Option<u32>,
}

pub fn parse(lang: Lang, src: &str) -> Parsed {
    match lang.ts() {
        Some(l) => parse_ts(lang, l, src).unwrap_or_default(),
        None => Parsed { syms: fallback(lang, src), ..Default::default() },
    }
}

/// First line of a declaration, trimmed, without trailing `{`, max 120 chars.
fn signature_at(src: &str, line_starts: &[usize], row: usize) -> String {
    let s = line_starts.get(row).copied().unwrap_or(0);
    let e = line_starts.get(row + 1).copied().unwrap_or(src.len());
    let mut l = src[s..e].trim();
    if let Some(x) = l.strip_suffix('{') {
        l = x.trim_end();
    }
    clip(l, 120)
}

pub fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut o: String = s.chars().take(n - 1).collect();
    o.push('…');
    o
}

fn line_starts(src: &str) -> Vec<usize> {
    let mut v = vec![0];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

enum Act {
    /// Symbol kind, is container (children are members).
    Sym(&'static str, bool),
    /// Variable-like declarations (handled by `vars`).
    Vars,
    /// Don't descend (function bodies, closures).
    Skip,
    Descend,
}

fn act(lang: Lang, n: Node) -> Act {
    use Act::*;
    let k = n.kind();
    match lang {
        Lang::Rust => match k {
            "function_item" | "function_signature_item" => Sym("fn", false),
            "struct_item" | "union_item" => Sym("struct", false),
            "enum_item" => Sym("enum", false),
            "type_item" => Sym("type", false),
            "const_item" => Sym("const", false),
            "static_item" => Sym("static", false),
            "macro_definition" => Sym("macro", false),
            "trait_item" => Sym("trait", true),
            "impl_item" => Sym("impl", true),
            "mod_item" => Sym("mod", true),
            "closure_expression" | "block" => Skip,
            _ => Descend,
        },
        l if l.is_js() => match k {
            "function_declaration" | "generator_function_declaration" | "function_signature" => Sym("fn", false),
            "class_declaration" | "abstract_class_declaration" => Sym("class", true),
            "method_definition" | "method_signature" | "abstract_method_signature" => Sym("method", false),
            "interface_declaration" => Sym("interface", true),
            "type_alias_declaration" => Sym("type", false),
            "enum_declaration" => Sym("enum", false),
            "internal_module" | "module" => Sym("mod", true),
            "lexical_declaration" | "variable_declaration" | "public_field_definition" => Vars,
            "arrow_function" | "function_expression" | "function" | "generator_function" | "statement_block"
            | "class" => Skip,
            _ => Descend,
        },
        Lang::Python => match k {
            "function_definition" => Sym("fn", false),
            "class_definition" => Sym("class", true),
            "expression_statement" => Vars,
            "lambda" => Skip,
            _ => Descend,
        },
        Lang::Go => match k {
            "function_declaration" => Sym("fn", false),
            "method_declaration" => Sym("method", false),
            "type_spec" | "type_alias" => Sym("type", false),
            "const_spec" | "var_spec" => Vars,
            "func_literal" | "block" => Skip,
            _ => Descend,
        },
        l if l.is_c() => match k {
            "function_definition" => Sym("fn", false),
            "struct_specifier" | "union_specifier" if n.child_by_field_name("body").is_some() => Sym("struct", true),
            "class_specifier" if n.child_by_field_name("body").is_some() => Sym("class", true),
            "enum_specifier" if n.child_by_field_name("body").is_some() => Sym("enum", false),
            "namespace_definition" => Sym("mod", true),
            "type_definition" | "alias_declaration" => Sym("type", false),
            "declaration" | "field_declaration" => Vars,
            "compound_statement" | "lambda_expression" | "struct_specifier" | "union_specifier" | "class_specifier"
            | "enum_specifier" => Skip,
            _ => Descend,
        },
        Lang::CSharp => match k {
            "class_declaration" => Sym("class", true),
            "struct_declaration" | "record_struct_declaration" => Sym("struct", true),
            "record_declaration" => Sym("record", true),
            "interface_declaration" => Sym("interface", true),
            "enum_declaration" => Sym("enum", false),
            "method_declaration" | "constructor_declaration" | "destructor_declaration" | "operator_declaration"
            | "local_function_statement" => Sym("method", false),
            "property_declaration" => Sym("prop", false),
            "delegate_declaration" => Sym("type", false),
            "namespace_declaration" | "file_scoped_namespace_declaration" => Sym("mod", true),
            "block" | "lambda_expression" | "anonymous_method_expression" | "accessor_list" => Skip,
            _ => Descend,
        },
        Lang::Java => match k {
            "class_declaration" => Sym("class", true),
            "interface_declaration" | "annotation_type_declaration" => Sym("interface", true),
            "enum_declaration" => Sym("enum", true),
            "record_declaration" => Sym("record", true),
            "method_declaration" | "constructor_declaration" | "compact_constructor_declaration" => {
                Sym("method", false)
            }
            "block" | "lambda_expression" | "constructor_body" => Skip,
            _ => Descend,
        },
        _ => Descend,
    }
}

fn text<'a>(n: Node, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

fn strip_generics(s: &str) -> String {
    let s = s.split('<').next().unwrap_or(s).trim();
    s.rsplit("::").next().unwrap_or(s).to_string()
}

/// Follow C/C++ declarator chains to the declared name; returns (name, is_function).
fn decl_name(n: Node, src: &str) -> Option<(String, bool)> {
    let mut cur = n;
    let mut is_fn = false;
    for _ in 0..32 {
        match cur.kind() {
            "identifier" | "field_identifier" | "type_identifier" | "qualified_identifier" | "destructor_name"
            | "operator_name" | "template_function" | "primitive_type" => {
                return Some((text(cur, src).to_string(), is_fn))
            }
            "function_declarator" => is_fn = true,
            _ => {}
        }
        if let Some(d) = cur.child_by_field_name("declarator") {
            cur = d;
            continue;
        }
        let mut c = cur.walk();
        let next = cur.named_children(&mut c).find(|c| {
            let k = c.kind();
            k.ends_with("declarator") || k.contains("identifier") || k == "destructor_name" || k == "operator_name"
        });
        cur = next?;
    }
    None
}

fn sym_name(lang: Lang, n: Node, src: &str) -> Option<String> {
    match (lang, n.kind()) {
        (Lang::Rust, "impl_item") => n.child_by_field_name("type").map(|t| strip_generics(text(t, src))),
        (l, "function_definition") if l.is_c() => n.child_by_field_name("declarator").and_then(|d| decl_name(d, src)).map(|x| x.0),
        (l, "type_definition") if l.is_c() => n.child_by_field_name("declarator").and_then(|d| decl_name(d, src)).map(|x| x.0),
        _ => n.child_by_field_name("name").map(|x| text(x, src).to_string()),
    }
    .filter(|s| !s.is_empty())
}

fn go_type_kind(n: Node) -> &'static str {
    match n.child_by_field_name("type").map(|t| t.kind()) {
        Some("struct_type") => "struct",
        Some("interface_type") => "interface",
        _ => "type",
    }
}

fn go_receiver(n: Node, src: &str) -> Option<String> {
    let r = n.child_by_field_name("receiver")?;
    let mut stack = vec![r];
    while let Some(x) = stack.pop() {
        if x.kind() == "type_identifier" {
            return Some(text(x, src).to_string());
        }
        let mut c = x.walk();
        let kids: Vec<_> = x.named_children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    None
}

/// Variable-like declarations → (name, kind, span node).
fn vars<'t>(lang: Lang, n: Node<'t>, src: &str, top: bool, in_class: bool) -> Vec<(String, &'static str, Node<'t>)> {
    let mut out = vec![];
    match lang {
        l if l.is_js() => {
            if n.kind() == "public_field_definition" {
                let is_fn = n
                    .child_by_field_name("value")
                    .map(|v| matches!(v.kind(), "arrow_function" | "function_expression" | "function"))
                    .unwrap_or(false);
                if is_fn {
                    if let Some(nm) = n.child_by_field_name("name") {
                        out.push((text(nm, src).to_string(), "method", n));
                    }
                }
                return out;
            }
            if !top {
                return out;
            }
            let is_const = text(n, src).trim_start().starts_with("const");
            let mut c = n.walk();
            let decls: Vec<_> = n.named_children(&mut c).filter(|d| d.kind() == "variable_declarator").collect();
            let single = decls.len() == 1;
            for d in decls {
                let Some(nm) = d.child_by_field_name("name") else { continue };
                if nm.kind() != "identifier" {
                    continue;
                }
                let is_fn = d
                    .child_by_field_name("value")
                    .map(|v| matches!(v.kind(), "arrow_function" | "function_expression" | "function" | "generator_function"))
                    .unwrap_or(false);
                let kind = if is_fn { "fn" } else if is_const { "const" } else { "var" };
                out.push((text(nm, src).to_string(), kind, if single { n } else { d }));
            }
        }
        Lang::Python => {
            if !top || n.parent().map(|p| p.kind()) != Some("module") {
                return out;
            }
            if let Some(a) = n.named_child(0) {
                if a.kind() == "assignment" {
                    if let Some(l) = a.child_by_field_name("left") {
                        if l.kind() == "identifier" {
                            out.push((text(l, src).to_string(), "var", n));
                        }
                    }
                }
            }
        }
        Lang::Go => {
            if !top {
                return out;
            }
            let kind = if n.kind() == "const_spec" { "const" } else { "var" };
            if let Some(nm) = n.child_by_field_name("name") {
                out.push((text(nm, src).to_string(), kind, n));
            }
        }
        l if l.is_c() => {
            let is_field = n.kind() == "field_declaration";
            if !(top || in_class) {
                return out;
            }
            let Some(d) = n.child_by_field_name("declarator") else { return out };
            let Some((name, is_fn)) = decl_name(d, src) else { return out };
            if is_field || in_class {
                if is_fn {
                    out.push((name, "method", n));
                }
            } else {
                out.push((name, if is_fn { "fn" } else { "var" }, n));
            }
        }
        _ => {}
    }
    out
}

type Frame<'t> = (Node<'t>, Option<(String, &'static str)>, u32);

fn descend<'t>(n: Node<'t>, parent: Option<(String, &'static str)>, depth: u32, stack: &mut Vec<Frame<'t>>) {
    let mut c = n.walk();
    let kids: Vec<_> = n.named_children(&mut c).collect();
    for k in kids.into_iter().rev() {
        stack.push((k, parent.clone(), depth));
    }
}

fn parse_ts(lang: Lang, language: Language, src: &str) -> Option<Parsed> {
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(src, None)?;
    let root = tree.root_node();
    let ls = line_starts(src);
    let mut out = Parsed::default();

    let line_range = |n: Node| -> (u32, u32) {
        let s = n.start_position().row as u32 + 1;
        let ep = n.end_position();
        let mut e = ep.row as u32 + 1;
        if ep.column == 0 && e > s {
            e -= 1;
        }
        (s, e)
    };

    // Symbols: explicit stack of (node, parent (name, kind), depth).
    let mut stack: Vec<Frame> = vec![(root, None, 0)];
    while let Some((n, parent, depth)) = stack.pop() {
        let top = parent.as_ref().map(|p| p.1 == "mod").unwrap_or(true);
        let in_class = parent.as_ref().map(|p| p.1 != "mod").unwrap_or(false);
        match act(lang, n) {
            Act::Skip => {}
            Act::Descend => descend(n, parent, depth, &mut stack),
            Act::Vars => {
                for (name, kind, span) in vars(lang, n, src, top, in_class) {
                    let (s, e) = line_range(span);
                    out.syms.push(Sym {
                        name,
                        kind,
                        parent: parent.as_ref().map(|p| p.0.clone()),
                        depth,
                        line_start: s,
                        line_end: e,
                        signature: signature_at(src, &ls, span.start_position().row),
                    });
                }
            }
            Act::Sym(kind, container) => {
                let Some(mut name) = sym_name(lang, n, src) else {
                    if container {
                        descend(n, parent, depth, &mut stack);
                    }
                    continue;
                };
                let mut kind = kind;
                let mut par = parent.as_ref().map(|p| p.0.clone());
                if kind == "fn" && in_class {
                    kind = "method";
                }
                if lang == Lang::Go {
                    if n.kind() == "type_spec" {
                        kind = go_type_kind(n);
                    }
                    if kind == "method" {
                        par = go_receiver(n, src);
                    }
                }
                if lang.is_c() {
                    if let Some(i) = name.rfind("::") {
                        par = Some(strip_generics(&name[..i]));
                        name = name[i + 2..].to_string();
                        if kind == "fn" {
                            kind = "method";
                        }
                    }
                }
                let (s, e) = line_range(n);
                out.syms.push(Sym {
                    name: name.clone(),
                    kind,
                    parent: par,
                    depth,
                    line_start: s,
                    line_end: e,
                    signature: signature_at(src, &ls, n.start_position().row),
                });
                if container {
                    descend(n, Some((name, kind)), depth + 1, &mut stack);
                }
            }
        }
    }
    out.syms.sort_by_key(|s| (s.line_start, s.depth));

    // References: every identifier leaf that isn't a definition's own name.
    let mut seen: HashSet<(String, u32)> = HashSet::new();
    let mut cursor = root.walk();
    let mut first_err: Option<u32> = None;
    'walk: loop {
        let n = cursor.node();
        if first_err.is_none() && (n.is_error() || n.is_missing()) {
            first_err = Some(n.start_position().row as u32 + 1);
        }
        if n.child_count() == 0 && n.kind().contains("identifier") {
            let t = text(n, src);
            let is_def = n
                .parent()
                .map(|p| matches!(act(lang, p), Act::Sym(..)) && p.child_by_field_name("name") == Some(n))
                .unwrap_or(false);
            if !is_def && t.len() >= 2 && t.len() <= 80 && !t.bytes().all(|b| b == b'_') {
                let line = n.start_position().row as u32 + 1;
                let key = (t.to_string(), line);
                if !seen.contains(&key) {
                    seen.insert(key.clone());
                    out.refs.push(key);
                }
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    if root.has_error() {
        out.error_line = first_err.or(Some(1));
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Heuristic fallback outlines.

static RE_MD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*#*\s*$").unwrap());
static RE_JSON: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^(\s*)"([^"]+)"\s*:"#).unwrap());
static RE_TOML_T: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*\[\[?\s*([^\]]+?)\s*\]\]?").unwrap());
static RE_TOML_K: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^([A-Za-z0-9_\-."]+)\s*="#).unwrap());
static RE_YAML: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^([A-Za-z0-9_\-."'][^:#]*?)\s*:(\s|$)"#).unwrap());
static RE_SQL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)^\s*create\s+(?:or\s+replace\s+)?(?:temp\s+|temporary\s+|unique\s+)?(table|view|function|procedure|index|trigger|type)\s+(?:if\s+not\s+exists\s+)?([\w."`\[\]]+)"#).unwrap()
});
static RE_SH: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\s*)(?:function\s+([A-Za-z_][\w:-]*)|([A-Za-z_][\w:-]*)\s*\(\s*\))").unwrap());
static RE_GEN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(\s*)(?:(?:export|public|private|protected|internal|static|final|abstract|open|override|async|inline|suspend|data|sealed|local|pub|default|fileprivate|mutating|partial|virtual|unsafe|extern|const|global|Public|Private|Friend|Shared)\s+)*(class|struct|enum|interface|trait|protocol|extension|object|module|def|defp|defmodule|fun|func|function|Function|fn|sub|Sub|procedure|record|impl|namespace)\s+([A-Za-z_$][\w$.:!?-]*)").unwrap()
});

fn indent_of(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

fn fallback(lang: Lang, src: &str) -> Vec<Sym> {
    let lines: Vec<&str> = src.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    // Last content line (1-based); JSON's closing root bracket doesn't belong to the last key.
    let mut n = lines.iter().rposition(|l| !l.trim().is_empty()).map(|i| i as u32 + 1).unwrap_or(0);
    if lang == Lang::Json && n > 1 && matches!(lines[n as usize - 1].trim(), "}" | "]") {
        n -= 1;
    }
    let sig = |i: usize| {
        let mut l = lines[i].trim();
        if let Some(x) = l.strip_suffix('{') {
            l = x.trim_end();
        }
        clip(l, 120)
    };
    let mut out: Vec<Sym> = vec![];
    // Close each symbol at the line before the next symbol with depth <= its own.
    let finish = |out: &mut Vec<Sym>| {
        for i in 0..out.len() {
            if out[i].line_end != 0 {
                continue;
            }
            let d = out[i].depth;
            let next = out[i + 1..].iter().find(|s| s.depth <= d).map(|s| s.line_start - 1).unwrap_or(n);
            out[i].line_end = next.max(out[i].line_start);
        }
    };
    match lang {
        Lang::Markdown => {
            let mut fence = false;
            let mut stack: Vec<(usize, String)> = vec![];
            for (i, l) in lines.iter().enumerate() {
                if l.trim_start().starts_with("```") || l.trim_start().starts_with("~~~") {
                    fence = !fence;
                    continue;
                }
                if fence {
                    continue;
                }
                if let Some(c) = RE_MD.captures(l) {
                    let level = c[1].len();
                    while stack.last().map(|x| x.0 >= level).unwrap_or(false) {
                        stack.pop();
                    }
                    out.push(Sym {
                        name: c[2].to_string(),
                        kind: "heading",
                        parent: stack.last().map(|x| x.1.clone()),
                        depth: stack.len() as u32,
                        line_start: i as u32 + 1,
                        line_end: 0,
                        signature: sig(i),
                    });
                    stack.push((level, c[2].to_string()));
                }
            }
            finish(&mut out);
        }
        Lang::Json => {
            let caps: Vec<(usize, usize, String)> = lines
                .iter()
                .enumerate()
                .filter_map(|(i, l)| RE_JSON.captures(l).map(|c| (i, c[1].len(), c[2].to_string())))
                .collect();
            let min = caps.iter().map(|c| c.1).filter(|x| *x > 0).min().unwrap_or(0);
            for (i, ind, k) in caps {
                if ind == min {
                    out.push(Sym { name: k, kind: "key", parent: None, depth: 0, line_start: i as u32 + 1, line_end: 0, signature: sig(i) });
                }
            }
            finish(&mut out);
        }
        Lang::Toml => {
            let mut in_table = false;
            for (i, l) in lines.iter().enumerate() {
                if let Some(c) = RE_TOML_T.captures(l) {
                    in_table = true;
                    out.push(Sym { name: c[1].to_string(), kind: "table", parent: None, depth: 0, line_start: i as u32 + 1, line_end: 0, signature: sig(i) });
                } else if !in_table {
                    if let Some(c) = RE_TOML_K.captures(l) {
                        out.push(Sym { name: c[1].to_string(), kind: "key", parent: None, depth: 0, line_start: i as u32 + 1, line_end: 0, signature: sig(i) });
                    }
                }
            }
            finish(&mut out);
        }
        Lang::Yaml => {
            for (i, l) in lines.iter().enumerate() {
                if l.starts_with(' ') || l.starts_with('-') || l.starts_with('#') {
                    continue;
                }
                if let Some(c) = RE_YAML.captures(l) {
                    let k = c[1].trim_matches(|c| c == '"' || c == '\'').to_string();
                    out.push(Sym { name: k, kind: "key", parent: None, depth: 0, line_start: i as u32 + 1, line_end: 0, signature: sig(i) });
                }
            }
            finish(&mut out);
        }
        Lang::Sql => {
            for (i, l) in lines.iter().enumerate() {
                if let Some(c) = RE_SQL.captures(l) {
                    let kind = match c[1].to_ascii_lowercase().as_str() {
                        "table" => "table",
                        "view" => "view",
                        "index" => "index",
                        "trigger" => "trigger",
                        "type" => "type",
                        _ => "fn",
                    };
                    let name = c[2].trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']').to_string();
                    out.push(Sym { name, kind, parent: None, depth: 0, line_start: i as u32 + 1, line_end: 0, signature: sig(i) });
                }
            }
            finish(&mut out);
        }
        _ => {
            // Generic code: keyword declarations, nesting by indentation, extent by braces/indent.
            let mut stack: Vec<(usize, String, &'static str)> = vec![];
            for (i, l) in lines.iter().enumerate() {
                let (ind, kw, name) = if lang == Lang::Shell {
                    match RE_SH.captures(l) {
                        Some(c) => (
                            c[1].len(),
                            "function".to_string(),
                            c.get(2).or(c.get(3)).map(|m| m.as_str().to_string()).unwrap_or_default(),
                        ),
                        None => continue,
                    }
                } else {
                    match RE_GEN.captures(l) {
                        Some(c) => (c[1].len(), c[2].to_string(), c[3].to_string()),
                        None => continue,
                    }
                };
                if name.is_empty() {
                    continue;
                }
                while stack.last().map(|x| x.0 >= ind).unwrap_or(false) {
                    stack.pop();
                }
                let container = matches!(
                    kw.as_str(),
                    "class" | "struct" | "enum" | "interface" | "trait" | "protocol" | "extension" | "object" | "module"
                        | "defmodule" | "record" | "impl" | "namespace"
                );
                let in_class = stack.last().map(|x| x.2 != "mod").unwrap_or(false);
                let kind: &'static str = match kw.as_str() {
                    "module" | "defmodule" | "namespace" => "mod",
                    "class" => "class",
                    "struct" => "struct",
                    "enum" => "enum",
                    "interface" | "protocol" => "interface",
                    "trait" => "trait",
                    "extension" | "impl" => "impl",
                    "object" => "object",
                    "record" => "record",
                    _ if in_class => "method",
                    _ => "fn",
                };
                let end = block_end(&lines, i, ind);
                out.push(Sym {
                    name: name.clone(),
                    kind,
                    parent: stack.last().map(|x| x.1.clone()),
                    depth: stack.len() as u32,
                    line_start: i as u32 + 1,
                    line_end: end as u32 + 1,
                    signature: sig(i),
                });
                if container {
                    stack.push((ind, name, kind));
                }
            }
        }
    }
    out
}

/// Heuristic end line (0-based) of a block starting at `start`: brace matching if a `{` opens
/// on the first two lines, otherwise the last line before dedent (including a closing `end`).
fn block_end(lines: &[&str], start: usize, ind: usize) -> usize {
    let opens = lines[start..lines.len().min(start + 2)].iter().any(|l| l.contains('{'));
    if opens {
        let mut depth = 0i32;
        let mut seen = false;
        for (j, l) in lines.iter().enumerate().skip(start) {
            for ch in l.chars() {
                if ch == '{' {
                    depth += 1;
                    seen = true;
                } else if ch == '}' {
                    depth -= 1;
                }
            }
            if seen && depth <= 0 {
                return j;
            }
            if j - start > 5000 {
                break;
            }
        }
        return start;
    }
    let mut last = start;
    for (j, l) in lines.iter().enumerate().skip(start + 1) {
        if l.trim().is_empty() {
            continue;
        }
        if indent_of(l) <= ind {
            let t = l.trim_start();
            if t.starts_with("end") || t.starts_with('}') || t.starts_with("End ") {
                return j;
            }
            return last;
        }
        last = j;
    }
    last
}
