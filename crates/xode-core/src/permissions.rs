use crate::config::{Perm, Permissions, Rule};
use crate::types::Mode;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Ask(String),
    Deny(String),
}

/// `*` = any run of chars, `?` = one char. Case-insensitive.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn match_rules(rules: &[Rule], s: &str) -> Option<Perm> {
    // Most restrictive matching rule wins.
    let mut best: Option<Perm> = None;
    for r in rules {
        if !r.pattern.is_empty() && wildcard(&r.pattern, s) {
            best = Some(match (best, r.perm) {
                (Some(Perm::Deny), _) | (_, Perm::Deny) => Perm::Deny,
                (Some(Perm::Ask), _) | (_, Perm::Ask) => Perm::Ask,
                _ => Perm::Allow,
            });
        }
    }
    best
}

fn to_decision(p: Perm, why: &str) -> Decision {
    match p {
        Perm::Allow => Decision::Allow,
        Perm::Ask => Decision::Ask(why.to_string()),
        Perm::Deny => Decision::Deny(why.to_string()),
    }
}

pub struct CallInfo<'a> {
    pub tool: &'a str,
    pub args: &'a Value,
    pub read_only: bool,
    pub project_root: &'a Path,
    /// Absolute path targeted by file tools, if any.
    pub path: Option<&'a Path>,
}

pub fn decide(cfg: &Permissions, mode: Mode, c: &CallInfo) -> Decision {
    if mode == Mode::Plan && !c.read_only {
        return Decision::Deny("plan mode is read-only; describe the change in the plan instead".into());
    }
    // Explicit rules always apply.
    if c.tool == "shell" {
        if let Some(cmd) = c.args.get("command").and_then(|v| v.as_str()) {
            if let Some(p) = match_rules(&cfg.shell_rules, cmd.trim()) {
                if p != Perm::Allow || !cfg.full_access {
                    return to_decision(p, &format!("shell rule: {cmd}"));
                }
            }
        }
    }
    if let Some(path) = c.path {
        let ps = path.to_string_lossy().replace('\\', "/");
        if let Some(p) = match_rules(&cfg.path_rules, &ps) {
            if p != Perm::Allow {
                return to_decision(p, &format!("path rule: {ps}"));
            }
        }
    }
    if cfg.full_access {
        return Decision::Allow;
    }
    if let Some(path) = c.path {
        if !c.read_only && !path.starts_with(c.project_root) {
            let d = to_decision(cfg.allow_outside_project, "outside project");
            if d != Decision::Allow {
                return d;
            }
        }
    }
    let cat = match c.tool {
        "web_search" | "web_fetch" => Some(cfg.network),
        "browser" if !c.read_only => Some(cfg.browser),
        _ => None,
    };
    if let Some(p) = cat {
        if p != Perm::Allow {
            return to_decision(p, c.tool);
        }
    }
    let base = c.tool.split("__").next().unwrap_or(c.tool);
    let default = if c.read_only { Perm::Allow } else { Perm::Ask };
    let p = cfg.tools.get(c.tool).or_else(|| cfg.tools.get(base)).copied().unwrap_or(default);
    to_decision(p, c.tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wc() {
        assert!(wildcard("git push*", "git push origin main"));
        assert!(wildcard("*rm -rf*", "cd x && rm -rf build"));
        assert!(!wildcard("git push*", "git status"));
        assert!(wildcard("C:/Users/*", "c:/users/max/a.txt"));
    }
    #[test]
    fn plan_denies_writes() {
        let cfg = Permissions::default();
        let args = serde_json::json!({});
        let c = CallInfo { tool: "write", args: &args, read_only: false, project_root: Path::new("/p"), path: None };
        assert!(matches!(decide(&cfg, Mode::Plan, &c), Decision::Deny(_)));
        assert_eq!(decide(&cfg, Mode::Normal, &c), Decision::Allow);
    }
    #[test]
    fn shell_rule_asks_even_full_access() {
        let cfg = Permissions::default();
        let args = serde_json::json!({"command": "git push origin"});
        let c = CallInfo { tool: "shell", args: &args, read_only: false, project_root: Path::new("/p"), path: None };
        assert!(matches!(decide(&cfg, Mode::Normal, &c), Decision::Ask(_)));
    }
}
