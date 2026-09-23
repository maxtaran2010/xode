//! RTK ("Rust Token Killer"): compress command output before it reaches the model.
//!
//! [`filter_output`] runs generic passes (ANSI stripping, progress collapsing, dedup) plus a
//! command-aware filter picked from the command string. [`cap_output`] then enforces the
//! per-result token budget with head/tail truncation, saving the full text to a file.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use xode_core::config::TokenSaving;
use xode_core::tokens;
use xode_core::tool::ToolCtx;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filtered {
    pub text: String,
    pub saved_tokens: u64,
    pub truncated: bool,
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: Lazy<Regex> = Lazy::new(|| Regex::new($pat).unwrap());
    };
}

fn saved(raw: &str, out: &str) -> u64 {
    tokens::count(raw).saturating_sub(tokens::count(out))
}

/// Filter command output. `command` is the shell command line (used to pick a filter).
pub fn filter_output(command: &str, raw: &str, cfg: &TokenSaving) -> Filtered {
    if !cfg.rtk_enabled || raw.is_empty() {
        return Filtered { text: raw.to_string(), saved_tokens: 0, truncated: false };
    }
    let s = if cfg.rtk_strip_ansi { strip_ansi(raw) } else { raw.to_string() };
    let mut lines = split_lines(&s, cfg.rtk_collapse_progress);
    if cfg.rtk_collapse_progress {
        lines = drop_progress(lines);
    }
    if cfg.rtk_command_filters {
        lines = powershell_errors(lines);
        if let Some(k) = detect(command) {
            lines = apply(k, lines);
        }
    }
    if cfg.rtk_dedup_lines {
        lines = dedup(lines);
    }
    let text = finish(lines);
    let saved_tokens = saved(raw, &text);
    Filtered { text, saved_tokens, truncated: false }
}

/// Head/tail truncate `text` to `cfg.max_tool_output_tokens`. When truncated and `save_to` is
/// given (and `cfg.save_full_output`), the full text is written there and its path is mentioned
/// so the model can `read` it with offset/limit.
pub fn cap_output(text: &str, cfg: &TokenSaving, save_to: Option<&Path>) -> Filtered {
    let max = cfg.max_tool_output_tokens;
    let total = tokens::count(text);
    if max == 0 || total <= max {
        return Filtered { text: text.to_string(), saved_tokens: 0, truncated: false };
    }
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    let half = (max * 45 / 100).max(50);
    let clip = |l: &str| crate::util::clip(l, 400);
    let mut head = vec![];
    let mut used = 0;
    for l in lines.iter().take(cfg.head_lines) {
        let c = clip(l);
        used += tokens::quick(&c) + 1;
        if used > half && !head.is_empty() {
            break;
        }
        head.push(c);
    }
    let mut tail = vec![];
    used = 0;
    for l in lines.iter().rev().take(cfg.tail_lines.min(n - head.len())) {
        let c = clip(l);
        used += tokens::quick(&c) + 1;
        if used > half && !tail.is_empty() {
            break;
        }
        tail.push(c);
    }
    tail.reverse();
    let (h, t) = (head.len(), tail.len());
    let omitted = n - h - t;
    let mut note = if omitted > 0 {
        format!("[… {omitted} lines omitted (lines {}-{} of {n})", h + 1, n - t)
    } else {
        format!("[… long lines clipped ({n} lines)")
    };
    let mut wrote = false;
    if let (Some(p), true) = (save_to, cfg.save_full_output) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if std::fs::write(p, text).is_ok() {
            note.push_str(&format!("; full output: {} (use read with offset/limit)", crate::util::fwd(p)));
            wrote = true;
        }
    }
    if !wrote {
        note.push_str("; re-run with a narrower command");
    }
    note.push(']');
    let mut out = head.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&note);
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(&tail.join("\n"));
    }
    let saved_tokens = total.saturating_sub(tokens::count(&out));
    Filtered { text: out, saved_tokens, truncated: true }
}

/// A fresh file path under `<project>/.xode/out` for a full tool output.
pub fn out_path(ctx: &ToolCtx, prefix: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    ctx.out_dir().join(format!("{prefix}-{ms}-{n}.txt"))
}

// ---------------------------------------------------------------- generic passes

re!(ANSI, r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[PX^_][^\x1b]*\x1b\\|\x1b[@-Z\\-_]|\x1b[()][0-9A-Za-z]");

/// Remove ANSI/VT escape sequences and stray control characters (keeps `\n`, `\r`, `\t`).
pub fn strip_ansi(s: &str) -> String {
    let s = ANSI.replace_all(s, "");
    s.chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t')).collect()
}

/// Split into trimmed lines; `\r` progress redraws collapse to their final state.
fn split_lines(s: &str, collapse_cr: bool) -> Vec<String> {
    s.trim_end_matches(['\n', '\r', ' ', '\t'])
        .split('\n')
        .map(|l| {
            let l = l.strip_suffix('\r').unwrap_or(l);
            let l = if l.contains('\r') {
                if collapse_cr {
                    l.split('\r').rev().find(|x| !x.trim().is_empty()).unwrap_or("")
                } else {
                    return l.replace('\r', "").trim_end().to_string();
                }
            } else {
                l
            };
            l.trim_end().to_string()
        })
        .collect()
}

re!(
    PROGRESS,
    r"(?x)
    ^\s*[\[(|]?[=\#>\-.*█▓▒░■□▏▎▍▌▋▊▉━─\s]{6,}[\])|]?\s*\d{1,3}(\.\d+)?\s*%   # [=====>   ] 45%
    | ^\s*\d{1,3}(\.\d+)?\s*%\s*[\[|]                                           # tqdm 45%|███
    | ^\s*\d{1,3}(\.\d+)?\s*%\s*$                                               # bare 45%
    | ^\s*[\x{2801}-\x{28FF}]\s                                                 # braille spinner
    | ^\s*━+                                                                    # pip rich bar
    | ^\s*(Compiling|Checking|Downloaded|Downloading|Documenting|Fresh|Locking|Adding|Updating|Blocking|Unpacking|Installing|Packaging|Verifying|Archiving|Dirty|Building)\s+(\S+\s+v\d|crates|crates\.io|git\s|waiting|\d+\s+(packages?|crates?))
    | ^(remote:\s*)?(Enumerating|Counting|Compressing|Receiving|Resolving|Writing|Unpacking)\s+(objects|deltas)
    | ^\s*(Collecting|Downloading|Using\ cached|Obtaining|Preparing\ metadata|Building\ wheels?\ for|Created\ wheel|Stored\ in\ directory|Getting\ requirements|Installing\ build\ dependencies|Installing\ backend\ dependencies)\b
    | ^npm\ (http|timing|sill|verb)\s
    | ^[0-9a-f]{12}:\ (Pulling\ fs\ layer|Waiting|Downloading|Verifying\ Checksum|Download\ complete|Extracting|Pull\ complete|Already\ exists)
    | ^Progress:\ resolved\ \d+
    | ^\[\d/\d\]\ .*\.\.\.$
    "
);

/// Drop progress/spinner/download spam. Keeps the last dropped line if nothing else remains.
fn drop_progress(lines: Vec<String>) -> Vec<String> {
    let mut last_dropped = None;
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        if PROGRESS.is_match(&l) {
            last_dropped = Some(l);
        } else {
            out.push(l);
        }
    }
    if out.iter().all(|l| l.trim().is_empty()) {
        if let Some(l) = last_dropped {
            return vec![l.trim().to_string()];
        }
    }
    out
}

re!(DIGITS, r"\d+");

/// Collapse identical consecutive lines (`line (xN)`) and runs of lines differing only in numbers.
fn dedup(lines: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let n = lines.len();
    let mut i = 0;
    while i < n {
        let l = &lines[i];
        if l.trim().is_empty() {
            out.push(l.clone());
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && lines[j] == *l {
            j += 1;
        }
        if j - i >= 2 {
            out.push(format!("{l} (x{})", j - i));
            i = j;
            continue;
        }
        if l.bytes().any(|b| b.is_ascii_digit()) {
            let shape = DIGITS.replace_all(l, "#");
            let mut j = i + 1;
            while j < n && DIGITS.replace_all(&lines[j], "#") == shape {
                j += 1;
            }
            if j - i >= 4 {
                out.push(l.clone());
                out.push(format!("… (x{} similar)", j - i - 2));
                out.push(lines[j - 1].clone());
                i = j;
                continue;
            }
        }
        out.push(l.clone());
        i += 1;
    }
    out
}

/// Collapse blank runs, trim leading/trailing blanks, join.
fn finish(lines: Vec<String>) -> String {
    let mut out = String::new();
    let mut blank = true;
    for l in lines {
        if l.trim().is_empty() {
            if !blank {
                out.push('\n');
            }
            blank = true;
        } else {
            out.push_str(&l);
            out.push('\n');
            blank = false;
        }
    }
    out.trim_end().to_string()
}

// ---------------------------------------------------------------- PowerShell error records

re!(PS_AT, r"^At (?:line:(\d+)|(.+?):(\d+)) char:\d+$");
re!(PS_NOISE, r"^\s*\+ (CategoryInfo|FullyQualifiedErrorId)\s*:|^\s*\+\s+~+\s*$");
re!(PS7_HEAD, r"^(\S+): (.+):(\d+)$");
re!(PS_LINE_BAR, r"^\s*Line \|$");
re!(PS_CODE, r"^\s*\d+ \| ");
re!(PS_BAR, r"^\s*\|(.*)$");

fn is_xode_script(p: &str) -> bool {
    let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
    name.starts_with("xode-") && name.ends_with(".ps1")
}

/// Collapse PowerShell error-record noise (`At line:`, `+ CategoryInfo`, `Line |` frames) into one line.
fn powershell_errors(lines: Vec<String>) -> Vec<String> {
    #[derive(PartialEq)]
    enum M {
        No,
        At,
        Bar,
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut mode = M::No;
    let append = |out: &mut Vec<String>, s: &str| {
        if let Some(last) = out.iter_mut().rev().find(|l| !l.trim().is_empty()) {
            last.push_str(s);
        }
    };
    for l in lines {
        if let Some(c) = PS_AT.captures(&l) {
            if !out.is_empty() {
                let loc = match (c.get(1), c.get(2), c.get(3)) {
                    (Some(n), _, _) => format!(" (line {})", n.as_str()),
                    // Our own temp script (the model's command): just the line number.
                    (_, Some(p), Some(n)) if is_xode_script(p.as_str()) => format!(" (line {})", n.as_str()),
                    (_, Some(p), Some(n)) => format!(" ({}:{})", p.as_str(), n.as_str()),
                    _ => String::new(),
                };
                append(&mut out, &loc);
                mode = M::At;
                continue;
            }
        }
        if PS_NOISE.is_match(&l) {
            continue;
        }
        if mode == M::At && l.trim_start().starts_with("+ ") {
            continue;
        }
        if let Some(c) = PS7_HEAD.captures(&l) {
            if is_xode_script(&c[2]) {
                out.push(format!("{} (line {}):", &c[1], &c[3]));
                continue;
            }
        }
        if PS_LINE_BAR.is_match(&l) {
            mode = M::Bar;
            continue;
        }
        if mode == M::Bar {
            if PS_CODE.is_match(&l) {
                continue;
            }
            if let Some(c) = PS_BAR.captures(&l) {
                let msg = c[1].trim();
                if !msg.is_empty() && !msg.chars().all(|ch| ch == '~' || ch == ' ') {
                    append(&mut out, &format!(" {msg}"));
                }
                continue;
            }
        }
        mode = M::No;
        out.push(l);
    }
    out
}

// ---------------------------------------------------------------- command detection

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    GitStatus,
    GitDiff,
    GitLog,
    CargoBuild,
    CargoTest,
    Pytest,
    Jest,
    GoTest,
    DotnetTest,
    DotnetBuild,
    Install,
    Tsc,
    Eslint,
    Ls,
    Docker,
}

/// Split a command line into pipeline stages: (segment, is_piped_from_previous).
fn segments(cmd: &str) -> Vec<(String, bool)> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut piped = false;
    let mut quote: Option<char> = None;
    let cs: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            cur.push(c);
            i += 1;
            continue;
        }
        let next = cs.get(i + 1).copied();
        match c {
            '"' | '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            '|' if next == Some('|') => {
                out.push((std::mem::take(&mut cur), piped));
                piped = false;
                i += 1;
            }
            '|' => {
                out.push((std::mem::take(&mut cur), piped));
                piped = true;
            }
            '&' if next == Some('&') => {
                out.push((std::mem::take(&mut cur), piped));
                piped = false;
                i += 1;
            }
            ';' | '\n' => {
                out.push((std::mem::take(&mut cur), piped));
                piped = false;
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    out.push((cur, piped));
    out.into_iter().filter(|(s, _)| !s.trim().is_empty()).collect()
}

fn base_name(t: &str) -> String {
    let t = t.trim_matches(|c| c == '"' || c == '\'' || c == '&');
    let t = t.rsplit(['/', '\\']).next().unwrap_or(t).to_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(s) = t.strip_suffix(ext) {
            return s.to_string();
        }
    }
    t
}

/// Commands that pass output through unchanged (so the upstream filter still applies).
const PASSTHROUGH: &[&str] = &["head", "tail", "select-object", "select", "out-string", "out-host", "more", "cat", "tee", "tee-object"];

/// Whitespace split that keeps quoted strings together (quotes removed).
fn tokenize(seg: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in seg.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn words(seg: &str) -> Vec<String> {
    let mut w = tokenize(seg);
    // Strip env assignments and wrappers.
    loop {
        let Some(f) = w.first() else { break };
        let b = base_name(f);
        if f == "&" || (f.contains('=') && !f.starts_with('-') && !f.starts_with('$')) || ["sudo", "time", "npx", "bunx", "env", "exec", "command", "nice"].contains(&b.as_str()) {
            w.remove(0);
        } else if ["python", "python3", "py"].contains(&b.as_str()) && w.get(1).map(|s| s.as_str()) == Some("-m") {
            w.drain(..2);
        } else if b == "uv" && w.get(1).map(|s| s.as_str()) == Some("run") || b == "poetry" && w.get(1).map(|s| s.as_str()) == Some("run") {
            w.drain(..2);
        } else if (b == "pnpm" || b == "yarn") && w.get(1).map(|s| s.as_str()) == Some("exec") {
            w.drain(..2);
        } else {
            break;
        }
    }
    w
}

fn detect_seg(seg: &str) -> Option<Kind> {
    let w = words(seg);
    let prog = base_name(w.first()?);
    let rest: Vec<&str> = w[1..].iter().map(|s| s.as_str()).filter(|s| !s.starts_with('+')).collect();
    let has = |f: &str| rest.iter().any(|x| *x == f || x.starts_with(&format!("{f}=")));
    let sub = rest.iter().find(|x| !x.starts_with('-')).copied().unwrap_or("");
    match prog.as_str() {
        "git" => {
            // Skip global options like `-C dir`, `-c k=v`, `--no-pager`.
            let mut it = rest.iter();
            let mut sub = "";
            while let Some(x) = it.next() {
                if *x == "-C" || *x == "-c" {
                    it.next();
                } else if !x.starts_with('-') {
                    sub = x;
                    break;
                }
            }
            match sub {
                "status" if !(has("-s") || has("--short") || has("--porcelain")) => Some(Kind::GitStatus),
                "diff" | "show" if !(has("--stat") || has("--name-only") || has("--name-status") || has("--numstat")) => {
                    Some(Kind::GitDiff)
                }
                "log" if !(has("--oneline") || has("--pretty") || has("--format") || has("-p") || has("--patch") || has("--stat") || has("--graph")) => {
                    Some(Kind::GitLog)
                }
                _ => None,
            }
        }
        "cargo" => match sub {
            "build" | "b" | "check" | "c" | "clippy" => Some(Kind::CargoBuild),
            "test" | "t" | "nextest" => Some(Kind::CargoTest),
            _ => None,
        },
        "pytest" | "py.test" => Some(Kind::Pytest),
        "jest" | "vitest" => Some(Kind::Jest),
        "npm" | "pnpm" | "yarn" | "bun" => match sub {
            "test" | "t" => Some(Kind::Jest),
            "run" if rest.iter().any(|x| *x == "test" || x.starts_with("test:")) => Some(Kind::Jest),
            "install" | "i" | "ci" | "add" | "update" | "up" | "upgrade" | "remove" | "rm" | "uninstall" => Some(Kind::Install),
            "" if prog == "yarn" => Some(Kind::Install),
            _ => None,
        },
        "pip" | "pip3" => matches!(sub, "install" | "uninstall").then_some(Kind::Install),
        "uv" => (sub == "pip" && rest.contains(&"install") || sub == "sync" || sub == "add").then_some(Kind::Install),
        "poetry" => matches!(sub, "install" | "add" | "update").then_some(Kind::Install),
        "go" => (sub == "test").then_some(Kind::GoTest),
        "dotnet" => match sub {
            "test" => Some(Kind::DotnetTest),
            "build" | "publish" | "restore" => Some(Kind::DotnetBuild),
            _ => None,
        },
        "msbuild" => Some(Kind::DotnetBuild),
        "tsc" | "vue-tsc" => Some(Kind::Tsc),
        "eslint" => Some(Kind::Eslint),
        "ls" | "dir" | "get-childitem" | "gci" => Some(Kind::Ls),
        "docker" | "podman" => {
            let second = rest.iter().filter(|x| !x.starts_with('-')).nth(1).copied().unwrap_or("");
            match (sub, second) {
                ("ps", _) | ("images", _) | ("container", "ls") | ("image", "ls") | ("container", "list") | ("image", "list") => {
                    Some(Kind::Docker)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Pick a command-specific filter. Returns None if ambiguous or piped into a transforming command.
pub(crate) fn detect(command: &str) -> Option<Kind> {
    let segs = segments(command);
    let mut kinds: Vec<Kind> = vec![];
    for (i, (s, _)) in segs.iter().enumerate() {
        // Output of this stage is transformed by a later pipe stage?
        let mut j = i + 1;
        let mut transformed = false;
        while j < segs.len() && segs[j].1 {
            let w = words(&segs[j].0);
            if !w.first().is_some_and(|p| PASSTHROUGH.contains(&base_name(p).as_str())) {
                transformed = true;
            }
            j += 1;
        }
        if segs[i].1 {
            continue;
        }
        if let Some(k) = detect_seg(s) {
            if transformed {
                return None;
            }
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
    }
    if kinds.len() == 1 {
        Some(kinds[0])
    } else {
        None
    }
}

pub(crate) fn apply(k: Kind, lines: Vec<String>) -> Vec<String> {
    match k {
        Kind::GitStatus => git_status(lines),
        Kind::GitDiff => git_diff(lines),
        Kind::GitLog => git_log(lines),
        Kind::CargoBuild => cargo_diagnostics(&lines, false),
        Kind::CargoTest => cargo_test(lines),
        Kind::Pytest => pytest(lines),
        Kind::Jest => jest(lines),
        Kind::GoTest => go_test(lines),
        Kind::DotnetTest | Kind::DotnetBuild => dotnet(lines, k == Kind::DotnetTest),
        Kind::Install => install(lines),
        Kind::Tsc => tsc(lines),
        Kind::Eslint => eslint(lines),
        Kind::Ls => ls(lines),
        Kind::Docker => docker(lines),
    }
}

// ---------------------------------------------------------------- git

fn git_status(lines: Vec<String>) -> Vec<String> {
    let mut head: Vec<String> = vec![];
    let mut sections: Vec<(String, Vec<String>)> = vec![];
    let mut other: Vec<String> = vec![];
    let mut recognized = false;
    for l in &lines {
        let t = l.trim();
        if t.is_empty() || t.starts_with("(use \"git") || t.starts_with("(commit or discard") || t.starts_with("(fix conflicts") {
            continue;
        }
        if let Some(b) = t.strip_prefix("On branch ") {
            head.push(format!("on {b}"));
            recognized = true;
        } else if t.starts_with("HEAD detached") {
            head.push(t.to_string());
            recognized = true;
        } else if let Some(r) = t.strip_prefix("Your branch ") {
            let r = r.trim_start_matches("is ").trim_end_matches('.').replace('\'', "");
            head.push(r);
        } else if t.starts_with("and have ") && t.contains("different commits") {
            head.push(t.trim_end_matches('.').to_string());
        } else if let Some(name) = match t {
            "Changes to be committed:" => Some("staged"),
            "Changes not staged for commit:" => Some("unstaged"),
            "Untracked files:" => Some("untracked"),
            "Unmerged paths:" => Some("conflicts"),
            "Ignored files:" => Some("ignored"),
            _ => None,
        } {
            sections.push((name.to_string(), vec![]));
            recognized = true;
        } else if t.starts_with("no changes added to commit") || t.starts_with("nothing added to commit but untracked") {
            continue;
        } else if t.starts_with("nothing to commit") {
            other.push("clean".into());
        } else if (l.starts_with('\t') || l.starts_with("  ")) && !sections.is_empty() {
            let entry = if let Some((st, path)) = t.split_once(':').filter(|(s, _)| {
                matches!(*s, "modified" | "new file" | "deleted" | "renamed" | "copied" | "typechange" | "both modified" | "both added" | "both deleted" | "added by us" | "added by them" | "deleted by us" | "deleted by them")
            }) {
                let code = match st {
                    "modified" => "M",
                    "new file" => "A",
                    "deleted" => "D",
                    "renamed" => "R",
                    "copied" => "C",
                    "typechange" => "T",
                    "both modified" => "UU",
                    "both added" => "AA",
                    "both deleted" => "DD",
                    "added by us" => "AU",
                    "added by them" => "UA",
                    "deleted by us" => "DU",
                    _ => "UD",
                };
                let path = path.trim();
                let path = path.strip_suffix(" (modified content)").or(path.strip_suffix(" (new commits)")).unwrap_or(path);
                format!("{code} {path}")
            } else {
                t.to_string()
            };
            sections.last_mut().unwrap().1.push(entry);
        } else {
            other.push(t.to_string());
        }
    }
    if !recognized {
        return lines;
    }
    let mut out = vec![head.join("; ")];
    for (name, entries) in sections {
        out.push(format!("{name} ({}):", entries.len()));
        let n = entries.len();
        out.extend(entries.into_iter().take(40));
        if n > 40 {
            out.push(format!("… +{} more", n - 40));
        }
    }
    out.extend(other);
    out
}

re!(DIFF_GIT, r"^diff --git a/(.+?) b/(.+)$");

fn git_diff(lines: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        if let Some(c) = DIFF_GIT.captures(&l) {
            if c[1] == c[2] {
                out.push(format!("=== {}", &c[1]));
            } else {
                out.push(format!("=== {} -> {}", &c[1], &c[2]));
            }
        } else if l.starts_with("index ")
            || l.starts_with("--- a/")
            || l.starts_with("+++ b/")
            || l == "--- /dev/null"
            || l == "+++ /dev/null"
            || l.starts_with("similarity index")
            || l.starts_with("rename from")
            || l.starts_with("rename to")
            || l.starts_with("old mode")
            || l.starts_with("new mode")
        {
            continue;
        } else if l.starts_with("new file mode") {
            out.push("(new file)".into());
        } else if l.starts_with("deleted file mode") {
            out.push("(deleted)".into());
        } else {
            out.push(l);
        }
    }
    out
}

re!(LOG_COMMIT, r"^commit ([0-9a-f]{7,40})");

fn git_log(lines: Vec<String>) -> Vec<String> {
    if !lines.first().is_some_and(|l| LOG_COMMIT.is_match(l)) || lines.iter().any(|l| l.starts_with("diff --git")) {
        return lines;
    }
    let mut out = vec![];
    let mut sha = String::new();
    let mut author = String::new();
    let mut date = String::new();
    let mut subject: Option<String> = None;
    let flush = |out: &mut Vec<String>, sha: &str, subject: &Option<String>, author: &str, date: &str| {
        if !sha.is_empty() {
            out.push(format!("{} {} ({author}, {date})", &sha[..sha.len().min(8)], subject.as_deref().unwrap_or("")));
        }
    };
    for l in &lines {
        if let Some(c) = LOG_COMMIT.captures(l) {
            flush(&mut out, &sha, &subject, &author, &date);
            sha = c[1].to_string();
            subject = None;
            author.clear();
            date.clear();
        } else if let Some(a) = l.strip_prefix("Author:") {
            author = a.split('<').next().unwrap_or("").trim().to_string();
        } else if let Some(d) = l.strip_prefix("Date:") {
            // "Mon Sep 1 12:00:00 2025 +0200" -> "Sep 1 2025"
            let p: Vec<&str> = d.split_whitespace().collect();
            date = if p.len() >= 5 { format!("{} {} {}", p[1], p[2], p[4]) } else { d.trim().to_string() };
        } else if subject.is_none() && l.starts_with("    ") && !l.trim().is_empty() {
            subject = Some(l.trim().to_string());
        }
    }
    flush(&mut out, &sha, &subject, &author, &date);
    out
}

// ---------------------------------------------------------------- cargo / rustc

re!(RUST_DIAG, r"^(warning|error)(\[\w+\])?: (.*)$");
re!(RUST_LOC, r"^\s*--> (.+)$");
re!(CARGO_SUMMARY, r"^(warning|error): .*(generated \d+ warnings?|could not compile|aborting due to|previous errors?|warnings? emitted|build failed)");
re!(CARGO_NOISE, r"^\s*(Compiling|Checking|Fresh|Documenting|Downloaded|Downloading|Locking|Updating|Adding|Blocking|Running|Doc-tests)\s");
re!(RUST_NOTE_NOISE, r"^\s*= (note: `#\[\w+\(.*\)\]`|note: `-\w .*` implied by|help: for further information visit|note: requested on the command line)");
const MAX_WARNINGS: usize = 30;
const MAX_ERROR_LINES: usize = 25;

/// Condense rustc diagnostics: warnings become one line, errors keep their block (capped).
/// With `keep_other`, non-diagnostic lines are kept (minus cargo progress noise).
fn cargo_diagnostics(lines: &[String], keep_other: bool) -> Vec<String> {
    let mut out = vec![];
    let mut summary = vec![];
    let mut seen = HashSet::new();
    let mut warnings = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        if CARGO_SUMMARY.is_match(l) {
            summary.push(l.clone());
            i += 1;
            continue;
        }
        if let Some(c) = RUST_DIAG.captures(l) {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim().is_empty() && !RUST_DIAG.is_match(&lines[j]) {
                j += 1;
            }
            let block = &lines[i..j];
            let loc = block.iter().find_map(|b| RUST_LOC.captures(b).map(|c| c[1].to_string()));
            let key = format!("{l}@{}", loc.clone().unwrap_or_default());
            i = j;
            if !seen.insert(key) {
                continue;
            }
            if &c[1] == "warning" {
                warnings += 1;
                if warnings <= MAX_WARNINGS {
                    match loc {
                        Some(loc) => out.push(format!("{l} @ {loc}")),
                        None => out.push(l.clone()),
                    }
                }
            } else {
                let kept: Vec<&String> = block.iter().filter(|b| !RUST_NOTE_NOISE.is_match(b)).collect();
                let n = kept.len();
                out.extend(kept.into_iter().take(MAX_ERROR_LINES).cloned());
                if n > MAX_ERROR_LINES {
                    out.push(format!("  … +{} lines", n - MAX_ERROR_LINES));
                }
            }
            continue;
        }
        if l.trim_start().starts_with("Finished ") {
            summary.push(l.trim().to_string());
        } else if keep_other && !CARGO_NOISE.is_match(l) {
            out.push(l.clone());
        }
        i += 1;
    }
    if warnings > MAX_WARNINGS {
        out.push(format!("… +{} more warnings", warnings - MAX_WARNINGS));
    }
    out.extend(summary);
    out
}

re!(CT_OK_LINE, r"^test .+ \.\.\. (ok|ignored)$");
re!(CT_RUNNING, r"^running \d+ tests?$");
re!(CT_RESULT, r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; \d+ measured; (\d+) filtered out(; finished in ([\d.]+)s)?");

fn cargo_test(lines: Vec<String>) -> Vec<String> {
    let lines = cargo_diagnostics(&lines, true);
    let failed = lines.iter().any(|l| l.contains("FAILED") || l.contains("panicked at") || l.starts_with("error"));
    let (mut passed, mut ignored, mut suites) = (0u64, 0u64, 0u64);
    let mut out = vec![];
    for l in lines {
        if CT_OK_LINE.is_match(&l) || CT_RUNNING.is_match(&l) || l.trim_start().starts_with("Finished ") {
            continue;
        }
        if let Some(c) = CT_RESULT.captures(&l) {
            if &c[1] == "ok" {
                let p: u64 = c[2].parse().unwrap_or(0);
                let ig: u64 = c[4].parse().unwrap_or(0);
                if p + ig > 0 {
                    suites += 1;
                }
                passed += p;
                ignored += ig;
                continue;
            }
        }
        if !failed && !(l.starts_with("warning") || l.starts_with("error")) {
            continue;
        }
        out.push(l);
    }
    let mut s = if failed { format!("other suites ok: {passed} passed") } else { format!("test result: ok. {passed} passed") };
    if ignored > 0 {
        s.push_str(&format!("; {ignored} ignored"));
    }
    s.push_str(&format!(" ({suites} suites)"));
    if !failed || passed > 0 {
        out.push(s);
    }
    out
}

// ---------------------------------------------------------------- pytest

re!(PY_SECTION, r"^=+ (.+?) =+$");
re!(PY_TEST_HEAD, r"^_+ (.+?) _+$");
re!(PY_PROGRESS, r"^\S+\.py [.FEsxX]+\s*(\[\s*\d+%\])?$|^\S+::\S+ (PASSED|SKIPPED|XFAIL|XPASS)");
re!(PY_HEADER, r"^(platform |rootdir:|configfile:|plugins:|cachedir:|collecting|collected \d+ items?|cachedir|testpaths:)");
re!(PY_FAIL, r"(?i)\b\d+ (failed|errors?)\b");

fn pytest(lines: Vec<String>) -> Vec<String> {
    let last_summary = lines.iter().rev().find_map(|l| PY_SECTION.captures(l).map(|c| c[1].to_string()));
    let failed = lines.iter().any(|l| PY_SECTION.captures(l).is_some_and(|c| PY_FAIL.is_match(&c[1])));
    if !failed {
        if let Some(s) = last_summary {
            return vec![s];
        }
    }
    let mut out = vec![];
    let mut skip_section = false;
    for l in lines {
        if let Some(c) = PY_SECTION.captures(&l) {
            let name = c[1].to_string();
            skip_section = name == "test session starts" || name.starts_with("warnings summary");
            if !skip_section {
                out.push(format!("== {name}"));
            }
            continue;
        }
        if skip_section || PY_HEADER.is_match(&l) || PY_PROGRESS.is_match(&l) {
            continue;
        }
        if let Some(c) = PY_TEST_HEAD.captures(&l) {
            out.push(format!("__ {}", &c[1]));
            continue;
        }
        out.push(l);
    }
    out
}

// ---------------------------------------------------------------- jest / vitest

re!(JS_DROP, r"^\s*(PASS\s|[✓√✔]\s|Start at|Duration\s|Time:|Ran all test suites|RUN\s+v\d|Snapshots:\s+0 total)|^\s*(✓|√|✔)$");
re!(JS_SUMMARY, r"^\s*(Tests|Test Suites|Test Files|Snapshots):?\s");
re!(JS_FAIL, r"FAIL|[✕×✗●]|\bfailed\b|Error:");
re!(WS, r"\s{2,}");

fn jest(lines: Vec<String>) -> Vec<String> {
    let failed = lines.iter().any(|l| JS_FAIL.is_match(l));
    if !failed {
        let s: Vec<String> = lines.iter().filter(|l| JS_SUMMARY.is_match(l)).map(|l| WS.replace_all(l.trim(), " ").into_owned()).collect();
        if !s.is_empty() {
            return s;
        }
    }
    lines
        .into_iter()
        .filter(|l| !JS_DROP.is_match(l))
        .map(|l| if JS_SUMMARY.is_match(&l) { WS.replace_all(l.trim(), " ").into_owned() } else { l })
        .collect()
}

// ---------------------------------------------------------------- go test

re!(GO_DROP, r"^(=== (RUN|PAUSE|CONT|NAME)|\s*--- PASS:|PASS$|\?\s+\S+\s+\[no test files\])");
re!(GO_OK, r"^ok\s+\S+");

fn go_test(lines: Vec<String>) -> Vec<String> {
    let mut ok = 0;
    let mut out = vec![];
    for l in lines {
        if GO_OK.is_match(&l) {
            ok += 1;
        } else if !GO_DROP.is_match(&l) {
            out.push(l);
        }
    }
    if ok > 0 {
        out.push(format!("ok: {ok} packages"));
    }
    out
}

// ---------------------------------------------------------------- dotnet / msbuild

re!(
    DOTNET_DROP,
    r"^\s*(Determining projects to restore|All projects are up-to-date|Restored |Restore complete|MSBuild version|Build started|Time Elapsed|Test run for |VSTest version|Microsoft \(R\) Test Execution|Copyright \(c\)|Starting test execution|A total of \d+ test files? matched|Passed \S+ \[|Workload updates are available|Run 'dotnet workload)|^\s*\S+ -> .+\.(dll|exe)$|^\s*\S+ net\S+ succeeded"
);
re!(DOTNET_DIAG, r": (error|warning) [A-Z]+\d+:");
re!(DOTNET_PROJ, r"\s+\[[^\]]+\.(cs|fs|vb)proj(::[^\]]*)?\]$");
re!(DOTNET_SUMMARY, r"^\s*(Passed|Failed)!\s+-");

fn dotnet(lines: Vec<String>, test: bool) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for l in lines {
        if DOTNET_DROP.is_match(&l) {
            continue;
        }
        if DOTNET_DIAG.is_match(&l) {
            let d = DOTNET_PROJ.replace(l.trim(), "").into_owned();
            if seen.insert(d.clone()) {
                out.push(d);
            }
            continue;
        }
        if DOTNET_SUMMARY.is_match(&l) {
            out.push(WS.replace_all(l.trim(), " ").into_owned());
            continue;
        }
        let t = l.trim();
        if !test && (t.ends_with("Warning(s)") || t.ends_with("Error(s)")) {
            out.push(t.to_string());
            continue;
        }
        out.push(l);
    }
    out
}

// ---------------------------------------------------------------- package installs

re!(
    INSTALL_KEEP,
    r"(?i)^(npm (err!|error|warn)|added \d|removed \d|changed \d|up to date|audited \d|.*vulnerabilit|successfully (installed|uninstalled)|error|warning|err_|done in|success |packages: |dependencies:|devdependencies:|[+-] \S+ \S+|already up.to.date|installed \d+ packages?|resolved \d+ packages?|uninstalled \d+|audited|\s*(fatal|critical))"
);
re!(INSTALL_DROP, r"(?i)^(\d+ packages? are looking for funding|\s*run `npm fund`|\[notice\]|to address (all )?issues|run `npm audit` for details|\s*npm audit fix)");
re!(NPM_DEPRECATED, r"(?i)^npm warn deprecated (\S+):");

fn install(lines: Vec<String>) -> Vec<String> {
    let mut out = vec![];
    let mut deprecated = vec![];
    let mut satisfied = 0;
    for l in &lines {
        let t = l.trim_start();
        if let Some(c) = NPM_DEPRECATED.captures(t) {
            deprecated.push(c[1].to_string());
            continue;
        }
        if t.starts_with("Requirement already satisfied") {
            satisfied += 1;
            continue;
        }
        if INSTALL_DROP.is_match(t) {
            continue;
        }
        if INSTALL_KEEP.is_match(t) {
            out.push(l.clone());
        }
    }
    if !deprecated.is_empty() {
        let n = deprecated.len();
        let mut s = format!("npm warn deprecated ({n}): {}", deprecated.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
        if n > 8 {
            s.push_str(", …");
        }
        out.insert(0, s);
    }
    if satisfied > 0 {
        out.push(format!("{satisfied} requirements already satisfied"));
    }
    if out.is_empty() {
        let tail: Vec<String> = lines.iter().filter(|l| !l.trim().is_empty()).rev().take(3).cloned().collect();
        return tail.into_iter().rev().collect();
    }
    out
}

// ---------------------------------------------------------------- tsc / eslint

re!(TSC_PAREN, r"^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.*)$");
re!(TSC_PRETTY, r"^(.+?):(\d+):(\d+) - (error|warning) (TS\d+): (.*)$");
re!(TSC_FRAME, r"^\d+\s|^\s*~+\s*$|^Errors\s+Files$|^\s+\d+\s+\S+:\d+$");

fn tsc(lines: Vec<String>) -> Vec<String> {
    let mut files: Vec<(String, Vec<String>)> = vec![];
    let mut other = vec![];
    let mut in_err = false;
    for l in lines {
        let c = TSC_PAREN.captures(&l).or_else(|| TSC_PRETTY.captures(&l));
        if let Some(c) = c {
            let f = c[1].replace('\\', "/");
            let row = format!("{}:{} {} {}", &c[2], &c[3], &c[5], &c[6]);
            match files.iter_mut().find(|(n, _)| *n == f) {
                Some((_, v)) => v.push(row),
                None => files.push((f, vec![row])),
            }
            in_err = true;
            continue;
        }
        if TSC_FRAME.is_match(&l) {
            continue;
        }
        if l.trim().is_empty() {
            continue;
        }
        if in_err && l.starts_with(' ') {
            if let Some((_, v)) = files.last_mut() {
                v.push(format!("  {}", l.trim()));
            }
            continue;
        }
        in_err = false;
        other.push(l);
    }
    if files.is_empty() {
        return other;
    }
    let mut out = vec![];
    for (f, rows) in files {
        out.push(f);
        out.extend(rows);
    }
    out.extend(other);
    out
}

re!(ESLINT_ROW, r"^\s+(\d+):(\d+)\s+(error|warning)\s+(.*?)\s{2,}(\S+)$");

fn eslint(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .filter(|l| !l.trim_start().starts_with("at "))
        .map(|l| match ESLINT_ROW.captures(&l) {
            Some(c) => format!("{}:{} {} {} ({})", &c[1], &c[2], &c[3], &c[4], &c[5]),
            None => l,
        })
        .collect()
}

// ---------------------------------------------------------------- ls / dir / Get-ChildItem

re!(
    LS_LONG,
    r"^([dlcbps-])[rwxsStT-]{9}[@+.]?\s+\d+\s+\S+\s+\S+\s+([\d,.]+[KMGTP]?)\s+(\w{3}\s+\d+\s+[\d:]+|\d+\s+\w{3}\s+[\d:]+|\d{4}-\d\d-\d\d\s+[\d:.]+(\s+[+-]\d{4})?)\s+(.+)$"
);
re!(GCI_ROW, r"^([dla-][a-z-]{4,5})\s+\d+[/.-]\d+[/.-]\d+\s+\d+:\d+(\s*[AP]M)?\s+(\d+\s+)?(.+)$");
re!(DIR_ROW, r"^\d\d[/.-]\d\d[/.-]\d{2,4}\s+\d\d:\d\d(\s*[AP]M)?\s+(<DIR>|<JUNCTION>|<SYMLINKD?>|[\d,.\u{a0}]+)\s+(.+)$");
re!(LS_DROP, r"^(total \d+|Mode\s+LastWriteTime|----\s+-|\s*Volume (in drive|Serial Number)|\s+\d+ (File|Dir)\(s\))");

fn human(n: &str) -> String {
    let v: u64 = n.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
    if n.chars().any(|c| c.is_ascii_alphabetic()) {
        return n.to_string();
    }
    match v {
        0..=1023 => format!("{v}B"),
        1024..=1_048_575 => format!("{:.1}K", v as f64 / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1}M", v as f64 / 1_048_576.0),
        _ => format!("{:.1}G", v as f64 / 1_073_741_824.0),
    }
}

fn ls(lines: Vec<String>) -> Vec<String> {
    let mut out = vec![];
    for l in lines {
        if LS_DROP.is_match(&l) || l.trim().is_empty() {
            continue;
        }
        if let Some(c) = LS_LONG.captures(&l) {
            let name = &c[5];
            if name == "." || name == ".." {
                continue;
            }
            out.push(match &c[1] {
                "d" => format!("{name}/"),
                "l" => name.to_string(),
                _ => format!("{name} {}", human(&c[2])),
            });
        } else if let Some(c) = GCI_ROW.captures(&l) {
            let name = c[4].trim();
            match c.get(3) {
                None if c[1].starts_with('d') => out.push(format!("{name}/")),
                Some(sz) => out.push(format!("{name} {}", human(sz.as_str().trim()))),
                None => out.push(name.to_string()),
            }
        } else if let Some(c) = DIR_ROW.captures(&l) {
            let name = c[3].trim();
            if name == "." || name == ".." {
                continue;
            }
            if c[2].starts_with('<') {
                out.push(format!("{name}/"));
            } else {
                out.push(format!("{name} {}", human(&c[2])));
            }
        } else {
            let t = l.trim();
            if let Some(d) = t.strip_prefix("Directory: ").or(t.strip_prefix("Directory of ")) {
                out.push(format!("{d}:"));
            } else {
                out.push(l);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- docker

re!(HEADER_COL, r"\S+(?: \S+)*");

fn docker(lines: Vec<String>) -> Vec<String> {
    let Some(hi) = lines.iter().position(|l| !l.trim().is_empty()) else { return lines };
    let header = &lines[hi];
    if !(header.starts_with("CONTAINER ID") || header.starts_with("REPOSITORY") || header.starts_with("IMAGE ID")) {
        return lines;
    }
    let cols: Vec<(usize, String)> = HEADER_COL.find_iter(header).map(|m| (header[..m.start()].chars().count(), m.as_str().to_string())).collect();
    let keep: Vec<usize> = (0..cols.len()).filter(|&i| cols[i].1 != "COMMAND").collect();
    let mut out = vec![keep.iter().map(|&i| cols[i].1.as_str()).collect::<Vec<_>>().join(" | ")];
    for l in &lines[hi + 1..] {
        if l.trim().is_empty() {
            continue;
        }
        let ch: Vec<char> = l.chars().collect();
        let cell = |i: usize| -> String {
            let a = cols[i].0.min(ch.len());
            let b = cols.get(i + 1).map(|c| c.0).unwrap_or(ch.len()).min(ch.len());
            let s: String = ch[a..b].iter().collect();
            let s = s.trim();
            if s.is_empty() {
                "-".into()
            } else {
                s.to_string()
            }
        };
        out.push(keep.iter().map(|&i| cell(i)).collect::<Vec<_>>().join(" | "));
    }
    out
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TokenSaving {
        TokenSaving::default()
    }
    fn f(cmd: &str, raw: &str) -> String {
        filter_output(cmd, raw, &cfg()).text
    }

    #[test]
    fn detects() {
        assert_eq!(detect("cargo build --release"), Some(Kind::CargoBuild));
        assert_eq!(detect("cd foo && cargo test 2>&1 | tail -50"), Some(Kind::CargoTest));
        assert_eq!(detect("cargo test | grep FAIL"), None);
        assert_eq!(detect("cargo +nightly clippy"), Some(Kind::CargoBuild));
        assert_eq!(detect("git -C x --no-pager status"), Some(Kind::GitStatus));
        assert_eq!(detect("git status --porcelain"), None);
        assert_eq!(detect("python -m pytest -x tests"), Some(Kind::Pytest));
        assert_eq!(detect("npx tsc --noEmit"), Some(Kind::Tsc));
        assert_eq!(detect("npm ci"), Some(Kind::Install));
        assert_eq!(detect("& \"C:\\Program Files\\dotnet\\dotnet.exe\" test"), Some(Kind::DotnetTest));
        assert_eq!(detect("Get-ChildItem -Force"), Some(Kind::Ls));
        assert_eq!(detect("cargo build; cargo test"), None);
        assert_eq!(detect("echo hi"), None);
    }

    #[test]
    fn ansi_progress_dedup() {
        let raw = "\x1b[32mok\x1b[0m\nDownloading foo 1.2MB\r[#####     ] 50%\r[##########] 100%\ndone\nsame\nsame\nsame\nitem 1\nitem 2\nitem 3\nitem 4\nitem 5\n\n\n\nend   \n";
        assert_eq!(f("echo", raw), "ok\ndone\nsame (x3)\nitem 1\n… (x3 similar)\nitem 5\n\nend");
    }

    #[test]
    fn disabled() {
        let mut c = cfg();
        c.rtk_enabled = false;
        let r = filter_output("cargo build", "\x1b[1mx\x1b[0m", &c);
        assert_eq!(r.text, "\x1b[1mx\x1b[0m");
        assert_eq!(r.saved_tokens, 0);
    }

    const CARGO_BUILD: &str = r#"   Compiling proc-macro2 v1.0.86
   Compiling unicode-ident v1.0.12
   Compiling serde v1.0.210
    Checking demo v0.1.0 (/home/u/demo)
warning: unused variable: `x`
 --> src/main.rs:2:9
  |
2 |     let x = 5;
  |         ^ help: if this is intentional, prefix it with an underscore: `_x`
  |
  = note: `#[warn(unused_variables)]` on by default

warning: function `helper` is never used
 --> src/lib.rs:10:4
   |
10 | fn helper() {}
   |    ^^^^^^
   |
   = note: `#[warn(dead_code)]` on by default

error[E0308]: mismatched types
 --> src/main.rs:3:18
  |
3 |     let y: u32 = "a";
  |            ---   ^^^ expected `u32`, found `&str`
  |            |
  |            expected due to this

For more information about this error, try `rustc --explain E0308`.
warning: `demo` (bin "demo") generated 2 warnings
error: could not compile `demo` (bin "demo") due to 1 previous error; 2 warnings emitted
"#;

    #[test]
    fn cargo_build_filter() {
        let r = filter_output("cargo build", CARGO_BUILD, &cfg());
        assert_eq!(
            r.text,
            "warning: unused variable: `x` @ src/main.rs:2:9\n\
             warning: function `helper` is never used @ src/lib.rs:10:4\n\
             error[E0308]: mismatched types\n --> src/main.rs:3:18\n  |\n3 |     let y: u32 = \"a\";\n  |            ---   ^^^ expected `u32`, found `&str`\n  |            |\n  |            expected due to this\n\
             warning: `demo` (bin \"demo\") generated 2 warnings\n\
             error: could not compile `demo` (bin \"demo\") due to 1 previous error; 2 warnings emitted"
        );
        assert!(r.saved_tokens > 50, "{}", r.saved_tokens);
    }

    const CARGO_TEST_FAIL: &str = r#"   Compiling demo v0.1.0 (/home/u/demo)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.02s
     Running unittests src/lib.rs (target/debug/deps/demo-1234)

running 4 tests
test tests::a ... ok
test tests::b ... ok
test tests::c ... ignored
test tests::bad ... FAILED

failures:

---- tests::bad stdout ----
thread 'tests::bad' panicked at src/lib.rs:20:9:
assertion `left == right` failed
  left: 1
 right: 2
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    tests::bad

test result: FAILED. 2 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/it.rs (target/debug/deps/it-5678)

running 3 tests
test x ... ok
test y ... ok
test z ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

error: test failed, to rerun pass `--lib`
"#;

    #[test]
    fn cargo_test_failure() {
        let t = f("cargo test", CARGO_TEST_FAIL);
        assert!(t.contains("test tests::bad ... FAILED"));
        assert!(t.contains("left: 1"));
        assert!(t.contains("test result: FAILED. 2 passed; 1 failed"));
        assert!(t.contains("error: test failed"));
        assert!(t.contains("other suites ok: 3 passed (1 suites)"), "{t}");
        assert!(!t.contains("tests::a ... ok"));
        assert!(!t.contains("Compiling"));
        assert!(!t.contains("running 3 tests"));
    }

    #[test]
    fn cargo_test_all_pass() {
        let raw = "   Compiling demo v0.1.0\n    Finished `test` profile in 1s\n     Running unittests src/lib.rs\n\nrunning 2 tests\ntest a ... ok\ntest b ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n   Doc-tests demo\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        assert_eq!(f("cargo test", raw), "test result: ok. 2 passed (1 suites)");
    }

    const PYTEST_FAIL: &str = r#"============================= test session starts ==============================
platform linux -- Python 3.11.4, pytest-7.4.0, pluggy-1.2.0
rootdir: /home/u/proj
plugins: anyio-3.7.1
collected 12 items

tests/test_a.py ..F.........                                            [100%]

=================================== FAILURES ===================================
___________________________________ test_add ___________________________________

    def test_add():
>       assert add(1, 2) == 4
E       assert 3 == 4
E        +  where 3 = add(1, 2)

tests/test_a.py:5: AssertionError
=============================== warnings summary ===============================
tests/test_a.py::test_old
  /home/u/proj/a.py:3: DeprecationWarning: old
    warnings.warn("old", DeprecationWarning)

-- Docs: https://docs.pytest.org/en/stable/how-to/capture-warnings.html
=========================== short test summary info ============================
FAILED tests/test_a.py::test_add - assert 3 == 4
=================== 1 failed, 11 passed, 1 warning in 0.12s ====================
"#;

    #[test]
    fn pytest_failure() {
        assert_eq!(
            f("pytest -q", PYTEST_FAIL),
            "== FAILURES\n__ test_add\n\n    def test_add():\n>       assert add(1, 2) == 4\nE       assert 3 == 4\nE        +  where 3 = add(1, 2)\n\ntests/test_a.py:5: AssertionError\n== short test summary info\nFAILED tests/test_a.py::test_add - assert 3 == 4\n== 1 failed, 11 passed, 1 warning in 0.12s"
        );
        let pass = "===== test session starts =====\nplatform x\ncollected 3 items\n\nt.py ...   [100%]\n\n===== 3 passed in 0.01s =====\n";
        assert_eq!(f("pytest", pass), "3 passed in 0.01s");
    }

    #[test]
    fn jest_filter() {
        let raw = "PASS src/a.test.js\nFAIL src/b.test.js\n  ● math › adds\n\n    expect(received).toBe(expected)\n\n    Expected: 4\n    Received: 3\n\n      at Object.<anonymous> (src/b.test.js:3:17)\n\nTest Suites: 1 failed, 1 passed, 2 total\nTests:       1 failed, 5 passed, 6 total\nSnapshots:   0 total\nTime:        1.2 s\nRan all test suites.\n";
        let t = f("npm test", raw);
        assert!(t.starts_with("FAIL src/b.test.js\n  ● math › adds"), "{t}");
        assert!(t.ends_with("Test Suites: 1 failed, 1 passed, 2 total\nTests: 1 failed, 5 passed, 6 total"), "{t}");
        let ok = " ✓ src/a.test.ts (3 tests) 5ms\n\n Test Files  1 passed (1)\n      Tests  3 passed (3)\n   Start at  10:00:00\n   Duration  300ms\n";
        assert_eq!(f("npx vitest run", ok), "Test Files 1 passed (1)\nTests 3 passed (3)");
    }

    #[test]
    fn go_and_dotnet() {
        let go = "=== RUN   TestA\n--- PASS: TestA (0.00s)\n=== RUN   TestB\n    b_test.go:9: want 2 got 1\n--- FAIL: TestB (0.00s)\nFAIL\nFAIL\texample.com/b\t0.01s\nok  \texample.com/a\t0.02s\n?   \texample.com/c\t[no test files]\n";
        assert_eq!(f("go test ./...", go), "    b_test.go:9: want 2 got 1\n--- FAIL: TestB (0.00s)\nFAIL\nFAIL\texample.com/b\t0.01s\nok: 1 packages");
        let dn = "  Determining projects to restore...\n  All projects are up-to-date for restore.\n  Lib -> C:\\p\\bin\\Debug\\net8.0\\Lib.dll\nC:\\p\\A.cs(3,5): warning CS0168: The variable 'e' is declared but never used [C:\\p\\Lib.csproj]\nTest run for C:\\p\\bin\\Tests.dll (.NETCoreApp,Version=v8.0)\nStarting test execution, please wait...\nA total of 1 test files matched the specified pattern.\n  Failed Tests.T.Adds [5 ms]\n  Error Message:\n   Assert.Equal() Failure\n  Passed Tests.T.Other [1 ms]\n\nFailed!  - Failed:     1, Passed:     5, Skipped:     0, Total:     6, Duration: 10 ms - Tests.dll (net8.0)\nC:\\p\\A.cs(3,5): warning CS0168: The variable 'e' is declared but never used [C:\\p\\Lib.csproj]\n";
        assert_eq!(
            f("dotnet test", dn),
            "C:\\p\\A.cs(3,5): warning CS0168: The variable 'e' is declared but never used\n  Failed Tests.T.Adds [5 ms]\n  Error Message:\n   Assert.Equal() Failure\n\nFailed! - Failed: 1, Passed: 5, Skipped: 0, Total: 6, Duration: 10 ms - Tests.dll (net8.0)"
        );
    }

    #[test]
    fn npm_install() {
        let raw = "npm warn deprecated inflight@1.0.6: This module is not supported, and leaks memory.\nnpm warn deprecated glob@7.2.3: Glob versions prior to v9 are no longer supported\nnpm warn deprecated rimraf@3.0.2: Rimraf versions prior to v4 are no longer supported\n\nadded 312 packages, and audited 313 packages in 12s\n\n58 packages are looking for funding\n  run `npm fund` for details\n\n2 moderate severity vulnerabilities\n\nTo address all issues, run:\n  npm audit fix\n\nRun `npm audit` for details.\n";
        assert_eq!(
            f("npm install", raw),
            "npm warn deprecated (3): inflight@1.0.6, glob@7.2.3, rimraf@3.0.2\nadded 312 packages, and audited 313 packages in 12s\n2 moderate severity vulnerabilities"
        );
        let pip = "Requirement already satisfied: requests in ./v/lib (2.31.0)\nRequirement already satisfied: idna in ./v/lib (3.4)\nCollecting flask\n  Downloading flask-3.0.0-py3-none-any.whl (99 kB)\n     ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ 99.7/99.7 kB 2.1 MB/s eta 0:00:00\nInstalling collected packages: flask\nSuccessfully installed flask-3.0.0\n\n[notice] A new release of pip is available: 23.2 -> 24.0\n";
        assert_eq!(f("pip install flask", pip), "Successfully installed flask-3.0.0\n2 requirements already satisfied");
    }

    #[test]
    fn git_status_filter() {
        let raw = "On branch main\nYour branch is ahead of 'origin/main' by 2 commits.\n  (use \"git push\" to publish your local commits)\n\nChanges to be committed:\n  (use \"git restore --staged <file>...\" to unstage)\n\tmodified:   src/a.rs\n\tnew file:   src/b.rs\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n  (use \"git restore <file>...\" to discard changes in working directory)\n\tmodified:   src/c.rs\n\tdeleted:    d.txt\n\nUntracked files:\n  (use \"git add <file>...\" to include in what will be committed)\n\te.txt\n\nno changes added to commit (use \"git add\" and/or \"git commit -a\")\n";
        assert_eq!(
            f("git status", raw),
            "on main; ahead of origin/main by 2 commits\nstaged (2):\nM src/a.rs\nA src/b.rs\nunstaged (2):\nM src/c.rs\nD d.txt\nuntracked (1):\ne.txt"
        );
        assert_eq!(f("git status", "On branch main\nnothing to commit, working tree clean\n"), "on main\nclean");
    }

    #[test]
    fn git_diff_and_log() {
        let d = "diff --git a/src/a.rs b/src/a.rs\nindex 83db48f..bf269f4 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@\n fn a() {\n-    1\n+    2\n }\n";
        assert_eq!(f("git diff", d), "=== src/a.rs\n@@ -1,3 +1,3 @@\n fn a() {\n-    1\n+    2\n }");
        let l = "commit 0123456789abcdef0123456789abcdef01234567\nAuthor: Jane Doe <jane@x.org>\nDate:   Mon Sep 1 12:00:00 2025 +0200\n\n    Fix the thing\n\n    Longer body.\n\ncommit fedcba9876543210fedcba9876543210fedcba98\nMerge: 1 2\nAuthor: Bob <b@x.org>\nDate:   Sun Aug 31 09:00:00 2025 +0000\n\n    Initial\n";
        assert_eq!(f("git log -n 2", l), "01234567 Fix the thing (Jane Doe, Sep 1 2025)\nfedcba98 Initial (Bob, Aug 31 2025)");
    }

    #[test]
    fn powershell_error_record() {
        let raw = "Get-Item : Cannot find path 'C:\\nope' because it does not exist.\nAt line:1 char:1\n+ Get-Item C:\\nope\n+ ~~~~~~~~~~~~~~~~\n    + CategoryInfo          : ObjectNotFound: (C:\\nope:String) [Get-Item], ItemNotFoundException\n    + FullyQualifiedErrorId : PathNotFound,Microsoft.PowerShell.Commands.GetItemCommand\n \nnext\n";
        assert_eq!(f("Get-Item C:\\nope", raw), "Get-Item : Cannot find path 'C:\\nope' because it does not exist. (line 1)\n\nnext");
        let pwsh7 = "Get-Item: C:\\s.ps1:3\nLine |\n   3 |  Get-Item nope\n     |  ~~~~~~~~~~~~~\n     | Cannot find path 'nope' because it does not exist.\n";
        assert_eq!(f("./s.ps1", pwsh7), "Get-Item: C:\\s.ps1:3 Cannot find path 'nope' because it does not exist.");
        let own = "Get-Item: C:\\T\\xode-12-0.ps1:1\nLine |\n   1 |  Get-Item nope\n     |  ~~~~~~~~~~~~~\n     | Cannot find path 'nope' because it does not exist.\n";
        assert_eq!(f("Get-Item nope", own), "Get-Item (line 1): Cannot find path 'nope' because it does not exist.");
    }

    #[test]
    fn tsc_eslint() {
        let t = "src/a.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.\nsrc/b.ts(1,1): error TS2304: Cannot find name 'foo'.\nsrc/a.ts(12,1): error TS2304: Cannot find name 'bar'.\n\nFound 3 errors in 2 files.\n";
        assert_eq!(
            f("tsc --noEmit", t),
            "src/a.ts\n10:5 TS2322 Type 'string' is not assignable to type 'number'.\n12:1 TS2304 Cannot find name 'bar'.\nsrc/b.ts\n1:1 TS2304 Cannot find name 'foo'.\nFound 3 errors in 2 files."
        );
        let e = "\n/p/src/a.js\n  1:10  error    'x' is defined but never used  no-unused-vars\n  2:1   warning  Unexpected console statement   no-console\n\n✖ 2 problems (1 error, 1 warning)\n";
        assert_eq!(
            f("eslint .", e),
            "/p/src/a.js\n1:10 error 'x' is defined but never used (no-unused-vars)\n2:1 warning Unexpected console statement (no-console)\n\n✖ 2 problems (1 error, 1 warning)"
        );
    }

    #[test]
    fn ls_variants() {
        let l = "total 16\ndrwxr-xr-x  5 u staff  160 Sep  1 12:00 .\ndrwxr-xr-x  5 u staff  160 Sep  1 12:00 ..\ndrwxr-xr-x  5 u staff  160 Sep  1 12:00 src\n-rw-r--r--  1 u staff 2048 Sep  1 12:00 Cargo.toml\n";
        assert_eq!(f("ls -la", l), "src/\nCargo.toml 2.0K");
        let g = "\n    Directory: C:\\proj\n\nMode                 LastWriteTime         Length Name\n----                 -------------         ------ ----\nd----          9/1/2025  12:00 PM                src\n-a---          9/1/2025  12:00 PM           1234 a.txt\n";
        assert_eq!(f("Get-ChildItem", g), "C:\\proj:\nsrc/\na.txt 1.2K");
        let d = " Volume in drive C has no label.\n Volume Serial Number is 1234-ABCD\n\n Directory of C:\\proj\n\n09/01/2025  12:00 PM    <DIR>          .\n09/01/2025  12:00 PM    <DIR>          src\n09/01/2025  12:00 PM             1,234 a.txt\n               1 File(s)          1,234 bytes\n";
        assert_eq!(f("dir", d), "C:\\proj:\nsrc/\na.txt 1.2K");
    }

    #[test]
    fn docker_ps() {
        let raw = "CONTAINER ID   IMAGE          COMMAND                  CREATED       STATUS       PORTS                    NAMES\n\
                   a1b2c3d4e5f6   nginx:latest   \"/docker-entrypoint.…\"   2 hours ago   Up 2 hours   0.0.0.0:8080->80/tcp     web\n\
                   0f9e8d7c6b5a   redis:7        \"docker-entrypoint.s…\"   3 days ago    Up 3 days                             cache\n";
        assert_eq!(
            f("docker ps", raw),
            "CONTAINER ID | IMAGE | CREATED | STATUS | PORTS | NAMES\na1b2c3d4e5f6 | nginx:latest | 2 hours ago | Up 2 hours | 0.0.0.0:8080->80/tcp | web\n0f9e8d7c6b5a | redis:7 | 3 days ago | Up 3 days | - | cache"
        );
    }

    #[test]
    fn cap_head_tail() {
        let mut c = cfg();
        c.max_tool_output_tokens = 200;
        c.head_lines = 5;
        c.tail_lines = 5;
        let text: String = (1..=1000).map(|i| format!("line number {i}\n")).collect();
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("full.txt");
        let r = cap_output(&text, &c, Some(&p));
        assert!(r.truncated);
        assert!(r.text.starts_with("line number 1\n"));
        assert!(r.text.ends_with("line number 1000"));
        assert!(r.text.contains("[… 990 lines omitted (lines 6-995 of 1000); full output: "), "{}", r.text);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), text);
        assert!(r.saved_tokens > 1000);
        let small = cap_output("hi", &c, None);
        assert!(!small.truncated);
        assert_eq!(small.text, "hi");
    }
}
