//! Fallback parser for models that write tool calls as text instead of native function calls.
//! Supports `<tool_call>{json}</tool_call>` (Hermes/Qwen), `<function=name>{json}</function>`,
//! and Qwen3-coder XML `<function=name><parameter=k>v</parameter></function>`.
use crate::types::Part;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};

static TOOL_CALL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap());
static FUNC: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<function=([\w.\-]+)>\s*(.*?)\s*</function>").unwrap());
static PARAM: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<parameter=([\w.\-]+)>\n?(.*?)\n?</parameter>").unwrap());

/// Returns (remaining_text, tool_call_parts). Only names in `known` are accepted.
pub fn extract(text: &str, known: &[String]) -> (String, Vec<Part>) {
    let mut calls = vec![];
    let mut rest = text.to_string();
    let ok = |n: &str| known.iter().any(|k| k == n);

    for cap in TOOL_CALL.captures_iter(text) {
        let inner = cap[1].trim();
        if let Some((name, args)) = parse_json_call(inner).or_else(|| parse_func(inner)) {
            if ok(&name) {
                calls.push(mk(name, args));
                rest = rest.replace(&cap[0], "");
            }
        }
    }
    if calls.is_empty() {
        for cap in FUNC.captures_iter(text) {
            let name = cap[1].to_string();
            if !ok(&name) {
                continue;
            }
            let body = cap[2].trim();
            let args = if body.starts_with('{') {
                serde_json::from_str(body).unwrap_or(Value::Object(Map::new()))
            } else {
                params(body)
            };
            calls.push(mk(name, args));
            rest = rest.replace(&cap[0], "");
        }
    }
    (rest.trim().to_string(), calls)
}

fn mk(name: String, args: Value) -> Part {
    Part::ToolCall { id: format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]), name, args }
}

fn parse_json_call(s: &str) -> Option<(String, Value)> {
    let v: Value = serde_json::from_str(s).ok()?;
    let name = v.get("name")?.as_str()?.to_string();
    let args = v.get("arguments").or_else(|| v.get("parameters")).cloned().unwrap_or(Value::Object(Map::new()));
    let args = match args {
        Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
        a => a,
    };
    Some((name, args))
}

fn parse_func(s: &str) -> Option<(String, Value)> {
    let c = FUNC.captures(s)?;
    Some((c[1].to_string(), params(&c[2])))
}

fn params(body: &str) -> Value {
    let mut m = Map::new();
    for p in PARAM.captures_iter(body) {
        let raw = p[2].to_string();
        let v = serde_json::from_str::<Value>(&raw)
            .ok()
            .filter(|v| !v.is_string())
            .unwrap_or(Value::String(raw));
        m.insert(p[1].to_string(), v);
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hermes() {
        let (t, c) = extract(
            "ok\n<tool_call>\n{\"name\": \"read\", \"arguments\": {\"path\": \"a.rs\"}}\n</tool_call>",
            &["read".into()],
        );
        assert_eq!(t, "ok");
        assert_eq!(c.len(), 1);
    }
    #[test]
    fn qwen_xml() {
        let (_, c) = extract(
            "<tool_call>\n<function=shell>\n<parameter=command>\nls -la\n</parameter>\n<parameter=timeout>\n30\n</parameter>\n</function>\n</tool_call>",
            &["shell".into()],
        );
        match &c[0] {
            Part::ToolCall { name, args, .. } => {
                assert_eq!(name, "shell");
                assert_eq!(args["command"], "ls -la");
                assert_eq!(args["timeout"], 30);
            }
            _ => panic!(),
        }
    }
    #[test]
    fn unknown_ignored() {
        let (t, c) = extract("<tool_call>{\"name\":\"x\",\"arguments\":{}}</tool_call>", &["read".into()]);
        assert!(c.is_empty());
        assert!(t.contains("tool_call"));
    }
}
