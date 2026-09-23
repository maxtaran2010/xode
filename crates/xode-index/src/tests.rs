use super::*;
use crate::parse::{parse, Lang};
use crate::tool::run_sync;
use serde_json::json;
use std::time::Instant;
use xode_core::tool::{SessionState, ToolCtx};
use xode_core::Config;

const LIB_RS: &str = "pub struct Engine {
    pub n: u32,
}

impl Engine {
    pub fn new() -> Self {
        Engine { n: 0 }
    }

    pub fn step(&mut self) -> u32 {
        self.n += 1;
        helper(self.n)
    }
}

pub fn helper(x: u32) -> u32 {
    x * 2
}

pub const LIMIT: usize = 10;
";

const MAIN_RS: &str = "use crate::lib::{Engine, helper};

fn main() {
    let mut e = Engine::new();
    e.step();
    println!(\"{}\", helper(1));
}
";

const APP_TS: &str = "import { helper } from './util';

export interface Props {
  size: number;
}

export class Widget {
  private el: HTMLElement;
  render(p: Props): string {
    return helper(p.size);
  }
  onClick = () => {
    this.render({ size: 1 });
  };
}

export function mount(el: HTMLElement): Widget {
  return new Widget();
}

export const makeId = (p: string) => p + '1';
";

const TOOL_PY: &str = "class Runner:\r\n    def run(self, x):\r\n        return helper(x)\r\n\r\n\r\ndef helper(x):\r\n    return x + 1\r\n\r\nMAX = 3\r\n";

const README: &str = "# Title\n\nintro\n\n## Usage\n\n```\n# not a heading\n```\n\n## License\nMIT\n";

fn setup() -> (tempfile::TempDir, Arc<ProjectIndex>) {
    setup_cfg(false)
}

fn setup_cfg(watch: bool) -> (tempfile::TempDir, Arc<ProjectIndex>) {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::create_dir_all(r.join("web")).unwrap();
    std::fs::create_dir_all(r.join("py")).unwrap();
    std::fs::create_dir_all(r.join("node_modules/x")).unwrap();
    std::fs::write(r.join("src/lib.rs"), LIB_RS).unwrap();
    std::fs::write(r.join("src/main.rs"), MAIN_RS).unwrap();
    std::fs::write(r.join("web/app.ts"), APP_TS).unwrap();
    std::fs::write(r.join("py/tool.py"), TOOL_PY).unwrap();
    std::fs::write(r.join("README.md"), README).unwrap();
    std::fs::write(r.join("node_modules/x/index.js"), "function ignored() {}\n").unwrap();
    std::fs::write(r.join("blob.rs"), b"fn x() {}\0\0\0").unwrap();
    let mut cfg = xode_core::config::Tools::default();
    cfg.index_watch = watch;
    let idx = ProjectIndex::open(r, &cfg).unwrap();
    wait_ready(&idx);
    (dir, idx)
}

fn wait_ready(idx: &ProjectIndex) {
    let t = Instant::now();
    while !idx.is_ready() {
        assert!(t.elapsed() < Duration::from_secs(20), "index never became ready");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn ctx(root: &Path) -> ToolCtx {
    ToolCtx {
        session_id: "t".into(),
        project_root: root.to_path_buf(),
        extra_roots: vec![],
        config: Arc::new(Config::default()),
        state: Arc::new(parking_lot::Mutex::new(SessionState::default())),
        cancel: tokio_util::sync::CancellationToken::new(),
        outliner: None,
    }
}

fn call(idx: &ProjectIndex, c: &ToolCtx, args: Value) -> (bool, String) {
    let o = run_sync(idx, &args, c);
    (o.is_error, o.content)
}

use serde_json::Value;

#[test]
fn indexes_and_skips() {
    let (_d, idx) = setup();
    let files = idx.files_under("");
    assert!(files.contains(&"src/lib.rs".to_string()));
    assert!(files.contains(&"py/tool.py".to_string()));
    assert!(files.contains(&"README.md".to_string()));
    assert!(!files.iter().any(|f| f.contains("node_modules")), "{files:?}");
    assert!(!files.contains(&"blob.rs".to_string()));
    let s = idx.stats();
    assert!(s.ready && s.files == 5 && s.symbols > 10, "{s:?}");
    assert!(std::path::Path::new(&_d.path().join(".xode/index.db")).exists());
}

#[test]
fn outline_rust_ts_py_md() {
    let (d, idx) = setup();
    let o = idx.outline(&d.path().join("src/lib.rs")).unwrap();
    assert!(o.contains("L1-3 pub struct Engine"), "{o}");
    assert!(o.contains("L5-14 impl Engine"), "{o}");
    assert!(o.contains("\n  L6-8 pub fn new() -> Self"), "{o}");
    assert!(o.contains("L16-18 pub fn helper(x: u32) -> u32"), "{o}");
    assert!(o.contains("L20 pub const LIMIT"), "{o}");

    let o = idx.outline(&d.path().join("web/app.ts")).unwrap();
    assert!(o.contains("L3-5 export interface Props"), "{o}");
    assert!(o.contains("L7-15 export class Widget"), "{o}");
    assert!(o.contains("  L9-11 render(p: Props): string"), "{o}");
    assert!(o.contains("  L12-14 onClick = () =>"), "{o}");
    assert!(o.contains("L17-19 export function mount"), "{o}");
    assert!(o.contains("L21 export const makeId"), "{o}");

    let o = idx.outline(&d.path().join("py/tool.py")).unwrap();
    assert!(o.contains("L1-3 class Runner:"), "{o}");
    assert!(o.contains("  L2-3 def run(self, x):"), "{o}");
    assert!(o.contains("L6-7 def helper(x):"), "{o}");
    assert!(o.contains("L9 MAX = 3"), "{o}");

    let o = idx.outline(&d.path().join("README.md")).unwrap();
    assert!(o.contains("L1-12 # Title") && o.contains("  L5-10 ## Usage") && !o.contains("not a heading"), "{o}");
}

#[test]
fn find_refs_via_tool() {
    let (d, idx) = setup();
    let c = ctx(d.path());
    let (e, o) = call(&idx, &c, json!({"action":"find","symbol":"helper"}));
    assert!(!e, "{o}");
    assert!(o.contains("src/lib.rs:L16-18 fn pub fn helper"), "{o}");
    assert!(o.contains("py/tool.py:L6-7 fn def helper"), "{o}");
    let (_, o) = call(&idx, &c, json!({"action":"find","symbol":"engi"}));
    assert!(o.lines().next().unwrap().contains("Engine"), "{o}");
    let (_, o) = call(&idx, &c, json!({"action":"find","symbol":"step"}));
    assert!(o.contains("method pub fn step(&mut self) -> u32 [in Engine]"), "{o}");

    let (_, o) = call(&idx, &c, json!({"action":"refs","symbol":"helper"}));
    assert!(o.contains("src/lib.rs\n  12: helper(self.n)"), "{o}");
    assert!(o.contains("src/main.rs\n  1: use crate::lib::{Engine, helper};"), "{o}");
    assert!(o.contains("py/tool.py\n  3: return helper(x)"), "{o}");
    assert!(!o.contains("16:"), "definition should not be a ref: {o}");

    let (_, o) = call(&idx, &c, json!({"action":"outline","path":"."}));
    assert!(o.contains("src/lib.rs: Engine, Engine, helper, LIMIT"), "{o}");
    assert!(!o.contains("node_modules"));
    let (_, o) = call(&idx, &c, json!({"action":"outline","path":"src/main.rs"}));
    assert!(o.starts_with("src/main.rs (8 lines)\nL3-7 fn main()"), "{o}");
}

#[test]
fn read_symbol() {
    let (d, idx) = setup();
    let c = ctx(d.path());
    let (e, o) = call(&idx, &c, json!({"action":"read","symbol":"Engine.step"}));
    assert!(!e, "{o}");
    assert_eq!(
        o,
        "src/lib.rs:L10-13 method pub fn step(&mut self) -> u32 [in Engine]\n10│    pub fn step(&mut self) -> u32 {\n11│        self.n += 1;\n12│        helper(self.n)\n13│    }"
    );
    let st = c.state.lock();
    let t = st.touched.get("src/lib.rs").unwrap();
    assert_eq!(t.how, "read");
    assert_eq!(t.ranges, vec!["10-13".to_string()]);
    drop(st);
    // Ambiguous: struct + impl named Engine in same file.
    let (e, o) = call(&idx, &c, json!({"action":"read","symbol":"Engine"}));
    assert!(!e && o.contains("1│pub struct Engine") && o.contains("5│impl Engine"), "{o}");
    let (e, o) = call(&idx, &c, json!({"action":"read","symbol":"Engine","path":"src/lib.rs:6"}));
    assert!(!e && o.starts_with("src/lib.rs:L5-14 impl"), "{o}");
    // Not found with suggestion.
    let (e, o) = call(&idx, &c, json!({"action":"read","symbol":"helpr"}));
    assert!(e && o.contains("not found"), "{o}");
    let (e, o) = call(&idx, &c, json!({"action":"read","symbol":"help"}));
    assert!(e && o.contains("similar") && o.contains("helper"), "{o}");
}

#[test]
fn edit_symbol_lf_and_crlf() {
    let (d, idx) = setup();
    let c = ctx(d.path());
    let tool = code_tool(idx.clone());
    assert!(tool.read_only(&json!({"action":"read"})));
    assert!(!tool.read_only(&json!({"action":"edit"})));

    // Rust: grow `new` by two lines; later symbols must shift.
    let (e, o) = call(
        &idx,
        &c,
        json!({"action":"edit","symbol":"Engine::new","content":"    pub fn new() -> Self {\n        let n = 0;\n        // start\n        Engine { n }\n    }\n"}),
    );
    assert!(!e, "{o}");
    assert!(o.contains("src/lib.rs: method new L6-8 → L6-10; changed L7-9 (-1 +3)"), "{o}");
    let src = std::fs::read_to_string(d.path().join("src/lib.rs")).unwrap();
    assert!(src.contains("        let n = 0;\n        // start\n        Engine { n }\n    }\n\n    pub fn step"));
    let rows = idx.defs("helper", None);
    let r = rows.iter().find(|r| r.file == "src/lib.rs").unwrap();
    assert_eq!((r.line_start, r.line_end), (18, 20), "reindexed after edit");
    assert_eq!(c.state.lock().touched["src/lib.rs"].how, "edit");
    assert_eq!(c.state.lock().undo.len(), 1);
    assert_eq!(c.state.lock().undo[0].1.as_deref(), Some(LIB_RS.as_bytes()));

    // Python CRLF file.
    let (e, o) = call(
        &idx,
        &c,
        json!({"action":"edit","symbol":"helper","path":"py/tool.py","content":"def helper(x, y=1):\n    return x + y\n"}),
    );
    assert!(!e, "{o}");
    let py = std::fs::read_to_string(d.path().join("py/tool.py")).unwrap();
    assert_eq!(
        py,
        "class Runner:\r\n    def run(self, x):\r\n        return helper(x)\r\n\r\n\r\ndef helper(x, y=1):\r\n    return x + y\r\n\r\nMAX = 3\r\n"
    );
    assert!(!py.replace("\r\n", "").contains('\n'));

    // Syntax error warning.
    let (e, o) = call(&idx, &c, json!({"action":"edit","symbol":"helper","path":"src/lib.rs","content":"pub fn helper(x: u32 -> u32 {\n    x\n}"}));
    assert!(!e && o.contains("warning: syntax error"), "{o}");

    // Stale line numbers: external change shifts the symbol; edit must re-locate it.
    let cur = std::fs::read_to_string(d.path().join("web/app.ts")).unwrap();
    std::fs::write(d.path().join("web/app.ts"), format!("// header\n// more\n{cur}")).unwrap();
    let (e, o) = call(&idx, &c, json!({"action":"edit","symbol":"mount","content":"export function mount(): void {}"}));
    assert!(!e, "{o}");
    let ts = std::fs::read_to_string(d.path().join("web/app.ts")).unwrap();
    assert!(ts.contains("}\n\nexport function mount(): void {}\n\nexport const makeId"), "{ts}");
}

#[test]
fn repo_map_budget_and_rank() {
    let (d, idx) = setup();
    let full = idx.repo_map(2000, &[]);
    assert!(full.starts_with("src/lib.rs:\n"), "{full}");
    assert!(full.contains("  pub fn helper(x: u32) -> u32"), "{full}");
    for b in [20u64, 60, 120] {
        let m = idx.repo_map(b, &[]);
        assert!(xode_core::tokens::count(&m) <= b, "budget {b}: {m}");
    }
    assert!(!idx.repo_map(60, &[]).is_empty());
    let c = ctx(d.path());
    let (e, o) = call(&idx, &c, json!({"action":"map"}));
    assert!(!e && o.contains("src/lib.rs:"), "{o}");
}

#[test]
fn incremental_reindex_and_reopen() {
    let (d, idx) = setup();
    let p = d.path().join("src/main.rs");
    std::fs::write(&p, format!("{MAIN_RS}\nfn added_later() {{}}\n")).unwrap();
    assert!(idx.find("added_later", 5).is_empty());
    idx.file_changed(&p);
    let r = idx.find("added_later", 5);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].line_start, 9);
    // Outline indexes on demand when mtime/size differ.
    let p2 = d.path().join("src/extra.rs");
    std::fs::write(&p2, "pub fn fresh() {}\n").unwrap();
    assert_eq!(idx.outline(&p2).unwrap(), "L1 pub fn fresh() {}");
    assert_eq!(idx.find("fresh", 5).len(), 1);
    // Deleted file is dropped.
    std::fs::remove_file(&p2).unwrap();
    idx.file_changed(&p2);
    assert!(idx.find("fresh", 5).is_empty());
    let before = idx.stats();
    drop(idx);
    // Reopen: unchanged files kept, stale rows removed, new file picked up.
    std::fs::write(d.path().join("src/new.rs"), "pub struct Newbie;\n").unwrap();
    let idx = ProjectIndex::open(d.path(), &xode_core::config::Tools { index_watch: false, ..Default::default() }).unwrap();
    wait_ready(&idx);
    assert_eq!(idx.stats().files, before.files + 1);
    assert_eq!(idx.find("Newbie", 5).len(), 1);
    assert_eq!(idx.find("added_later", 5).len(), 1);
}

#[test]
fn watcher_picks_up_changes() {
    let (d, idx) = setup_cfg(true);
    std::thread::sleep(Duration::from_millis(300));
    std::fs::write(d.path().join("src/watched.rs"), "pub fn watched_fn() {}\n").unwrap();
    let t = Instant::now();
    while idx.find("watched_fn", 5).is_empty() {
        assert!(t.elapsed() < Duration::from_secs(10), "watcher did not reindex");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn autocomplete() {
    let (_d, idx) = setup();
    let n = idx.symbol_names("he", 10);
    assert_eq!(n, vec!["helper".to_string()]);
    assert!(idx.symbol_names("", 100).len() > 5);
    assert_eq!(idx.file_paths("src/l", 10), vec!["src/lib.rs".to_string()]);
    assert_eq!(idx.file_paths("tool", 10), vec!["py/tool.py".to_string()]);
    assert_eq!(idx.file_paths("main", 10), vec!["src/main.rs".to_string()]);
}

fn names(lang: Lang, src: &str) -> Vec<String> {
    parse(lang, src)
        .syms
        .iter()
        .map(|s| format!("{}{}:{}", "  ".repeat(s.depth as usize), s.kind, s.name))
        .collect()
}

#[test]
fn languages() {
    let go = "package main\n\ntype Server struct {\n\tport int\n}\n\ntype Handler interface {\n\tServe()\n}\n\nconst Max = 3\n\nfunc (s *Server) Start() error {\n\treturn nil\n}\n\nfunc main() {\n\tx := 1\n\t_ = x\n}\n";
    assert_eq!(names(Lang::Go, go), vec!["struct:Server", "interface:Handler", "const:Max", "method:Start", "fn:main"]);
    let p = parse(Lang::Go, go);
    assert_eq!(p.syms[3].parent.as_deref(), Some("Server"));

    let c = "#include <stdio.h>\n\ntypedef struct { int a; } Pt;\n\nstruct Node {\n  int v;\n};\n\nint add(int a, int b);\n\nstatic int add(int a, int b) {\n  return a + b;\n}\n\nint counter = 0;\n";
    assert_eq!(names(Lang::C, c), vec!["type:Pt", "struct:Node", "fn:add", "fn:add", "var:counter"]);

    let cpp = "namespace app {\nclass Foo {\npublic:\n  Foo();\n  int bar(int x) const;\n};\n\nint Foo::bar(int x) const {\n  return x;\n}\n}\n\ntemplate <typename T>\nT id(T v) { return v; }\n";
    assert_eq!(names(Lang::Cpp, cpp), vec!["mod:app", "  class:Foo", "    method:Foo", "    method:bar", "  method:bar", "fn:id"]);

    let cs = "namespace App;\n\npublic class Svc : IDisposable {\n  public int Count { get; set; }\n  public Svc() {}\n  public void Run(int x) {\n    var y = x;\n  }\n}\n\npublic enum Color { Red, Green }\n";
    let n = names(Lang::CSharp, cs);
    assert!(n.contains(&"  class:Svc".to_string()) || n.contains(&"class:Svc".to_string()), "{n:?}");
    assert!(n.iter().any(|x| x.trim() == "prop:Count") && n.iter().any(|x| x.trim() == "method:Run") && n.iter().any(|x| x.trim() == "enum:Color"), "{n:?}");

    let java = "package a;\n\npublic class Main {\n  private int x;\n  public Main() {}\n  public static void main(String[] args) {\n    Runnable r = () -> {};\n  }\n  interface Inner { void go(); }\n}\n";
    assert_eq!(names(Lang::Java, java), vec!["class:Main", "  method:Main", "  method:main", "  interface:Inner", "    method:go"]);

    let js = "const a = require('x');\nfunction top() {\n  function inner() {}\n}\nclass K extends B {\n  constructor() { super(); }\n  get v() { return 1; }\n}\nmodule.exports = { top };\nconst f = async () => {};\n";
    assert_eq!(names(Lang::JavaScript, js), vec!["const:a", "fn:top", "class:K", "  method:constructor", "  method:v", "fn:f"]);

    let tsx = "export default function App() {\n  return <div className=\"x\">hi</div>;\n}\ntype P = { a: number };\n";
    assert_eq!(names(Lang::Tsx, tsx), vec!["fn:App", "type:P"]);

    let rs = "mod inner {\n    pub trait T {\n        fn a(&self);\n        fn b(&self) {}\n    }\n}\nmacro_rules! m { () => {} }\nstatic S: u8 = 1;\nenum E { A }\ntype X = u8;\n";
    assert_eq!(names(Lang::Rust, rs), vec!["mod:inner", "  trait:T", "    method:a", "    method:b", "macro:m", "static:S", "enum:E", "type:X"]);
}

#[test]
fn fallback_outlines() {
    let toml = "name = \"x\"\n\n[package]\nversion = \"1\"\n\n[[bin]]\nname = \"y\"\n";
    assert_eq!(names(Lang::Toml, toml), vec!["key:name", "table:package", "table:bin"]);
    let yaml = "version: 3\nservices:\n  web:\n    image: x\n# c\njobs:\n";
    assert_eq!(names(Lang::Yaml, yaml), vec!["key:version", "key:services", "key:jobs"]);
    let json = "{\n  \"name\": \"x\",\n  \"scripts\": {\n    \"build\": \"tsc\"\n  }\n}\n";
    assert_eq!(names(Lang::Json, json), vec!["key:name", "key:scripts"]);
    let p = parse(Lang::Json, json);
    assert_eq!((p.syms[1].line_start, p.syms[1].line_end), (3, 5));
    let kt = "package a\n\nclass Foo(val x: Int) {\n    fun bar(): Int {\n        return x\n    }\n}\n\nfun top() = 1\n";
    assert_eq!(names(Lang::Generic, kt), vec!["class:Foo", "  method:bar", "fn:top"]);
    let p = parse(Lang::Generic, kt);
    assert_eq!((p.syms[0].line_start, p.syms[0].line_end), (3, 7));
    let rb = "module M\n  class C\n    def go\n      1\n    end\n  end\nend\n";
    let p = parse(Lang::Generic, rb);
    assert_eq!(names(Lang::Generic, rb), vec!["mod:M", "  class:C", "    method:go"]);
    assert_eq!((p.syms[2].line_start, p.syms[2].line_end), (3, 5));
    let lua = "local function f(a)\n  return a\nend\nfunction M.g() end\n";
    assert_eq!(names(Lang::Generic, lua), vec!["fn:f", "fn:M.g"]);
    let sql = "CREATE TABLE IF NOT EXISTS users (id int);\ncreate view v as select 1;\n";
    assert_eq!(names(Lang::Sql, sql), vec!["table:users", "view:v"]);
    let sh = "#!/bin/sh\nbuild() {\n  echo\n}\nfunction deploy {\n  x\n}\n";
    assert_eq!(names(Lang::Shell, sh), vec!["fn:build", "fn:deploy"]);
}

#[test]
fn rel_paths_are_slash_separated() {
    let (d, idx) = setup();
    assert_eq!(idx.rel(&d.path().join("src").join("lib.rs")).as_deref(), Some("src/lib.rs"));
    assert_eq!(idx.rel(Path::new("src/../src/lib.rs")).as_deref(), Some("src/lib.rs"));
    assert_eq!(idx.rel(Path::new("/definitely/elsewhere.rs")), None);
    assert_eq!(idx.abs("src/lib.rs"), d.path().join("src").join("lib.rs"));
}

/// Manual smoke test: `XODE_INDEX_ROOT=/path cargo test -p xode-index smoke -- --ignored --nocapture`
#[test]
#[ignore]
fn smoke_real_repo() {
    let root = PathBuf::from(std::env::var("XODE_INDEX_ROOT").expect("XODE_INDEX_ROOT"));
    let t = Instant::now();
    let idx = ProjectIndex::open(&root, &xode_core::config::Tools { index_watch: false, ..Default::default() }).unwrap();
    while !idx.is_ready() {
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("scan {:?} {:?}", t.elapsed(), idx.stats());
    let t = Instant::now();
    let m = idx.repo_map(1200, &[]);
    println!("map {:?} {} tokens\n{m}", t.elapsed(), xode_core::tokens::count(&m));
    let t = Instant::now();
    let m2 = idx.repo_map(1200, &[]);
    assert_eq!(m, m2);
    println!("map cached {:?}", t.elapsed());
    let t = Instant::now();
    let f = idx.find("new", 30);
    let r = idx.refs("new");
    println!("find+refs {:?} {} {}", t.elapsed(), f.len(), r.len());
}
