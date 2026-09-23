//! `web_search` and `web_fetch` tools.

use crate::html;
use crate::util::{cap_tokens, clip, collapse_ws};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use scraper::{Html, Selector};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use xode_core::config::Config;
use xode_core::tokens;
use xode_core::tool::{arg_str, arg_u64, Tool, ToolCtx, ToolOutput, ToolRef};
use xode_core::types::ToolSpec;

pub const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

pub(crate) static HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(UA)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .gzip(true)
        .build()
        .expect("http client")
});

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub fn format_hits(hits: &[Hit]) -> String {
    let mut s = String::new();
    for (i, h) in hits.iter().enumerate() {
        s.push_str(&format!("{}. {} — {}\n", i + 1, clip(&h.title, 120), h.url));
        let sn = clip(&h.snippet, 200);
        if !sn.is_empty() {
            s.push_str(&format!("   {sn}\n"));
        }
    }
    s.trim_end().to_string()
}

// ---------------------------------------------------------------- web_search

struct WebSearch;

pub fn web_search_tool() -> ToolRef {
    Arc::new(WebSearch)
}

#[async_trait]
impl Tool for WebSearch {
    fn name(&self) -> &str {
        "web_search"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_search".into(),
            description: "Search the web. Returns title, url, snippet.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "n": {"type": "integer", "description": "max results"}
                },
                "required": ["query"]
            }),
        }
    }
    fn read_only(&self, _args: &Value) -> bool {
        true
    }
    fn summary(&self, args: &Value) -> String {
        format!("search {}", arg_str(args, "query").unwrap_or(""))
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let Some(q) = arg_str(&args, "query").map(str::trim).filter(|q| !q.is_empty()) else {
            return ToolOutput::err("query required");
        };
        let n = arg_u64(&args, "n").map(|n| n as usize).unwrap_or(ctx.config.tools.search_results).clamp(1, 20);
        tokio::select! {
            r = search(&ctx.config, q, n) => match r {
                Ok(h) => ToolOutput::ok(format_hits(&h)),
                Err(e) => ToolOutput::err(e.to_string()),
            },
            _ = ctx.cancel.cancelled() => ToolOutput::err("cancelled"),
        }
    }
}

/// Run the configured engines in order until one returns results.
pub async fn search(cfg: &Config, q: &str, n: usize) -> Result<Vec<Hit>> {
    let t = &cfg.tools;
    let mut errs = vec![];
    for eng in &t.search_order {
        let r = match eng.as_str() {
            "duckduckgo" | "ddg" => ddg(q, n).await,
            "searxng" if !t.searxng_url.trim().is_empty() => searxng(&t.searxng_url, q, n).await,
            "brave" if !t.brave_key.trim().is_empty() => brave(&t.brave_key, q, n).await,
            "tavily" if !t.tavily_key.trim().is_empty() => tavily(&t.tavily_key, q, n).await,
            "browser" => crate::browser::google_search(&cfg.browser, q, n).await,
            _ => continue,
        };
        match r {
            Ok(h) if !h.is_empty() => {
                return Ok(h.into_iter().take(n).collect());
            }
            Ok(_) => errs.push(format!("{eng}: no results")),
            Err(e) => {
                tracing::debug!("search {eng} failed: {e:#}");
                errs.push(format!("{eng}: {}", clip(&format!("{e:#}"), 120)))
            }
        }
    }
    if errs.is_empty() {
        bail!("no search engine configured");
    }
    bail!("search failed: {}", errs.join("; "))
}

async fn ddg(q: &str, n: usize) -> Result<Vec<Hit>> {
    let body = HTTP
        .post("https://html.duckduckgo.com/html/")
        .header("Referer", "https://html.duckduckgo.com/")
        .header("Accept-Language", "en-US,en;q=0.9")
        .form(&[("q", q), ("b", ""), ("kl", "")])
        .send()
        .await?
        .text()
        .await?;
    match parse_ddg(&body, n) {
        Ok(h) if !h.is_empty() => Ok(h),
        first => {
            // Lite endpoint is sometimes spared by the bot check.
            let url = reqwest::Url::parse_with_params("https://lite.duckduckgo.com/lite/", &[("q", q)])?;
            let body = HTTP.get(url).header("Accept-Language", "en-US,en;q=0.9").send().await?.text().await?;
            match parse_ddg_lite(&body, n) {
                Ok(h) if !h.is_empty() => Ok(h),
                Ok(h) => first.map(|_| h),
                Err(e) => first.and(Err(e)),
            }
        }
    }
}

fn is_ddg_anomaly(body: &str) -> bool {
    body.contains("anomaly-modal") || body.contains("challenge-form") || body.contains("anomaly.js")
}

/// Decode `//duckduckgo.com/l/?uddg=<url>&rut=...` redirect links.
pub fn ddg_url(href: &str) -> String {
    let full = if href.starts_with("//") { format!("https:{href}") } else { href.to_string() };
    if let Ok(u) = reqwest::Url::parse(&full) {
        if u.path().starts_with("/l/") {
            if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "uddg") {
                return v.into_owned();
            }
        }
        return u.to_string();
    }
    full
}

pub fn parse_ddg(body: &str, n: usize) -> Result<Vec<Hit>> {
    if is_ddg_anomaly(body) {
        bail!("captcha");
    }
    let doc = Html::parse_document(body);
    let res = Selector::parse("div.result").unwrap();
    let a = Selector::parse("a.result__a").unwrap();
    let sn = Selector::parse(".result__snippet").unwrap();
    let mut out = vec![];
    for r in doc.select(&res) {
        let cls = r.value().attr("class").unwrap_or("");
        if cls.contains("result--ad") {
            continue;
        }
        let Some(link) = r.select(&a).next() else { continue };
        let url = ddg_url(link.value().attr("href").unwrap_or(""));
        if url.is_empty() || url.contains("duckduckgo.com/y.js") {
            continue;
        }
        out.push(Hit {
            title: collapse_ws(&link.text().collect::<String>()),
            url,
            snippet: r.select(&sn).next().map(|s| collapse_ws(&s.text().collect::<String>())).unwrap_or_default(),
        });
        if out.len() >= n {
            break;
        }
    }
    Ok(out)
}

pub fn parse_ddg_lite(body: &str, n: usize) -> Result<Vec<Hit>> {
    if is_ddg_anomaly(body) {
        bail!("captcha");
    }
    let doc = Html::parse_document(body);
    let a = Selector::parse("a.result-link").unwrap();
    let sn = Selector::parse("td.result-snippet").unwrap();
    let snippets: Vec<String> = doc.select(&sn).map(|s| collapse_ws(&s.text().collect::<String>())).collect();
    Ok(doc
        .select(&a)
        .enumerate()
        .map(|(i, l)| Hit {
            title: collapse_ws(&l.text().collect::<String>()),
            url: ddg_url(l.value().attr("href").unwrap_or("")),
            snippet: snippets.get(i).cloned().unwrap_or_default(),
        })
        .filter(|h| !h.url.contains("duckduckgo.com/y.js"))
        .take(n)
        .collect())
}

async fn searxng(base: &str, q: &str, n: usize) -> Result<Vec<Hit>> {
    let url = reqwest::Url::parse_with_params(
        &format!("{}/search", base.trim_end_matches('/')),
        &[("q", q), ("format", "json")],
    )?;
    let v: Value = HTTP.get(url).send().await?.error_for_status()?.json().await?;
    Ok(json_hits(&v["results"], "title", "url", "content", n))
}

async fn brave(key: &str, q: &str, n: usize) -> Result<Vec<Hit>> {
    let url = reqwest::Url::parse_with_params(
        "https://api.search.brave.com/res/v1/web/search",
        &[("q", q), ("count", &n.to_string())],
    )?;
    let v: Value = HTTP
        .get(url)
        .header("X-Subscription-Token", key.trim())
        .header("Accept", "application/json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(json_hits(&v["web"]["results"], "title", "url", "description", n))
}

async fn tavily(key: &str, q: &str, n: usize) -> Result<Vec<Hit>> {
    let v: Value = HTTP
        .post("https://api.tavily.com/search")
        .bearer_auth(key.trim())
        .json(&json!({"api_key": key.trim(), "query": q, "max_results": n}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(json_hits(&v["results"], "title", "url", "content", n))
}

fn json_hits(arr: &Value, t: &str, u: &str, s: &str, n: usize) -> Vec<Hit> {
    arr.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(Hit {
                        title: html::fragment_text(r[t].as_str().unwrap_or("")),
                        url: r[u].as_str()?.to_string(),
                        snippet: html::fragment_text(r[s].as_str().unwrap_or("")),
                    })
                })
                .take(n)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------- web_fetch

struct WebFetch;

pub fn web_fetch_tool() -> ToolRef {
    Arc::new(WebFetch)
}

#[async_trait]
impl Tool for WebFetch {
    fn name(&self) -> &str {
        "web_fetch"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "web_fetch".into(),
            description: "Fetch a URL as text. query: keep the most relevant parts.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "query": {"type": "string"}
                },
                "required": ["url"]
            }),
        }
    }
    fn read_only(&self, _args: &Value) -> bool {
        true
    }
    fn summary(&self, args: &Value) -> String {
        format!("fetch {}", arg_str(args, "url").unwrap_or(""))
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let Some(url) = arg_str(&args, "url").map(str::trim).filter(|u| !u.is_empty()) else {
            return ToolOutput::err("url required");
        };
        let query = arg_str(&args, "query").map(str::trim).filter(|q| !q.is_empty());
        let max = ctx.config.tools.fetch_max_tokens.max(200);
        tokio::select! {
            r = fetch(url, query, max) => match r {
                Ok(s) => ToolOutput::ok(s),
                Err(e) => ToolOutput::err(format!("{e:#}")),
            },
            _ = ctx.cancel.cancelled() => ToolOutput::err("cancelled"),
        }
    }
}

const MAX_BODY: usize = 8 * 1024 * 1024;

/// Fetch `url` and return compact text capped at `max_tokens`.
pub async fn fetch(url: &str, query: Option<&str>, max_tokens: u64) -> Result<String> {
    let url = if url.contains("://") { url.to_string() } else { format!("https://{url}") };
    let mut resp = HTTP
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml,application/json,text/plain;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            break;
        }
    }
    let mut head = String::new();
    if !status.is_success() {
        head.push_str(&format!("HTTP {status}\n"));
    }
    if final_url.trim_end_matches('/') != url.trim_end_matches('/') {
        head.push_str(&format!("-> {final_url}\n"));
    }
    let text_like = ctype.is_empty()
        || ctype.starts_with("text/")
        || ctype.contains("json")
        || ctype.contains("xml")
        || ctype.contains("javascript");
    if !text_like {
        return Ok(format!("{head}[binary {ctype}, {} bytes]", body.len()));
    }
    let raw = String::from_utf8_lossy(&body);
    let looks_html = ctype.contains("html") || (ctype.is_empty() && raw.trim_start().starts_with('<'));
    let text = if looks_html {
        let p = html::html_to_text(&raw);
        if !p.title.is_empty() {
            head.push_str(&format!("# {}\n", p.title));
        }
        p.text
    } else if ctype.contains("json") {
        serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| raw.into_owned())
    } else {
        raw.into_owned()
    };
    if text.trim().is_empty() {
        return Ok(format!("{head}[empty page; may need JS — try the browser tool]"));
    }
    let budget = max_tokens.saturating_sub(tokens::count(&head));
    let body = match query {
        Some(q) => select_relevant(&text, q, budget),
        None => cap_tokens(&text, budget),
    };
    Ok(format!("{head}\n{body}").trim().to_string())
}

/// Keep the intro plus the paragraphs that best match `query`, in document
/// order, until `budget` tokens are filled.
pub fn select_relevant(text: &str, query: &str, budget: u64) -> String {
    let total = tokens::count(text);
    if total <= budget {
        return text.to_string();
    }
    let terms: Vec<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.chars().count() >= 2)
        .map(|w| w.to_lowercase())
        .collect();
    let phrase = query.trim().to_lowercase();
    let paras = split_blocks(text);
    let lower: Vec<String> = paras.iter().map(|p| p.to_lowercase()).collect();
    let toks: Vec<u64> = paras.iter().map(|p| tokens::count(p) + 1).collect();
    let n = paras.len() as f64;
    // Rare terms weigh more (IDF), so "mut" in "retain_mut" won't match everything.
    let idf: Vec<f64> = terms
        .iter()
        .map(|t| {
            let df = lower.iter().filter(|l| l.contains(t.as_str())).count() as f64;
            ((n + 1.0) / (df + 1.0)).ln()
        })
        .collect();
    let max_idf = idf.iter().cloned().fold(0.0, f64::max);
    let scores: Vec<f64> = lower
        .iter()
        .zip(&paras)
        .map(|(l, p)| {
            let mut s = 0.0;
            for (t, w) in terms.iter().zip(&idf) {
                let c = l.matches(t.as_str()).count();
                if c > 0 {
                    s += w * (1.0 + (c as f64).ln());
                }
            }
            if phrase.len() >= 3 && l.contains(&phrase) {
                s += 2.0 * max_idf.max(0.5);
            }
            if p.starts_with('#') && s > 0.0 {
                s *= 1.5;
            }
            s
        })
        .collect();
    let mut keep = vec![false; paras.len()];
    let mut used = 0u64;
    // Intro: up to 15% of budget.
    for i in 0..paras.len() {
        if used + toks[i] > budget * 15 / 100 {
            break;
        }
        keep[i] = true;
        used += toks[i];
    }
    let mut order: Vec<usize> = (0..paras.len()).filter(|i| scores[*i] > 1e-9).collect();
    order.sort_by(|a, b| scores[*b].partial_cmp(&scores[*a]).unwrap().then(a.cmp(b)));
    let reserve = 10 * (order.len() as u64).min(20);
    for i in order {
        if keep[i] {
            continue;
        }
        if used + toks[i] + reserve > budget {
            continue;
        }
        keep[i] = true;
        used += toks[i];
        // Include the heading just above a matching paragraph, or the body under a matching heading.
        let nb = if paras[i].starts_with('#') { i + 1 } else { i.wrapping_sub(1) };
        if nb < paras.len()
            && !keep[nb]
            && (paras[i].starts_with('#') || paras[nb].starts_with('#'))
            && used + toks[nb] + reserve <= budget
        {
            keep[nb] = true;
            used += toks[nb];
        }
    }
    let mut out = String::new();
    let mut skipped = 0u64;
    for i in 0..paras.len() {
        if keep[i] {
            if skipped > 0 {
                out.push_str(&format!("[{skipped} tokens omitted]\n\n"));
                skipped = 0;
            }
            out.push_str(&cap_tokens(&paras[i], budget));
            out.push_str("\n\n");
        } else {
            skipped += toks[i];
        }
    }
    if skipped > 0 {
        out.push_str(&format!("[{skipped} tokens omitted]"));
    }
    if !keep.iter().any(|k| *k) {
        return cap_tokens(text, budget);
    }
    out.trim_end().to_string()
}

fn split_blocks(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if line.trim().is_empty() && !in_code {
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur).trim_end().to_string());
            }
            cur.clear();
            continue;
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim_end().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DDG: &str = r##"<!DOCTYPE html><html><body><div id="links" class="results">
<div class="result results_links results_links_deep result--ad">
 <div class="links_main links_deep result__body"><h2 class="result__title">
 <a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_domain=x.com&amp;u3=abc">Ad Title</a></h2>
 <a class="result__snippet" href="#">Buy stuff</a></div></div>
<div class="result results_links results_links_deep web-result ">
 <div class="links_main links_deep result__body">
  <h2 class="result__title">
   <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2Ftokio%2Ftutorial&amp;rut=6c1d2">Tutorial | <b>Tokio</b> - An asynchronous Rust runtime</a>
  </h2>
  <div class="result__extras"><div class="result__extras__url"><a class="result__url" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs">tokio.rs/tokio/tutorial</a></div></div>
  <a class="result__snippet" href="//duckduckgo.com/l/?uddg=x">Tokio is an asynchronous runtime for the <b>Rust</b> programming language.
   It provides the building blocks.</a>
  <div class="clear"></div>
 </div>
</div>
<div class="result results_links results_links_deep web-result ">
 <div class="links_main links_deep result__body">
  <h2 class="result__title"><a rel="nofollow" class="result__a" href="https://docs.rs/tokio/latest/tokio/?a=1&amp;b=2">tokio - Rust</a></h2>
  <a class="result__snippet" href="#">A runtime for writing reliable network applications.</a>
 </div>
</div>
</div></body></html>"##;

    #[test]
    fn ddg_parse() {
        let h = parse_ddg(DDG, 10).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].title, "Tutorial | Tokio - An asynchronous Rust runtime");
        assert_eq!(h[0].url, "https://tokio.rs/tokio/tutorial");
        assert_eq!(h[0].snippet, "Tokio is an asynchronous runtime for the Rust programming language. It provides the building blocks.");
        assert_eq!(h[1].url, "https://docs.rs/tokio/latest/tokio/?a=1&b=2");
        assert_eq!(parse_ddg(DDG, 1).unwrap().len(), 1);
        let f = format_hits(&h);
        assert!(f.starts_with("1. Tutorial | Tokio - An asynchronous Rust runtime — https://tokio.rs/tokio/tutorial\n   Tokio is"));
    }

    #[test]
    fn ddg_captcha() {
        let b = r#"<html><body><div class="anomaly-modal__mask"></div><form id="challenge-form"></form></body></html>"#;
        assert!(parse_ddg(b, 5).is_err());
        assert!(parse_ddg_lite(b, 5).is_err());
    }

    #[test]
    fn ddg_lite_parse() {
        let b = r#"<html><body><table>
<tr><td>1.&nbsp;</td><td><a rel="nofollow" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fx%3D1&amp;rut=z" class='result-link'>Example A</a></td></tr>
<tr><td></td><td class='result-snippet'>Snippet <b>A</b> text</td></tr>
<tr><td>2.&nbsp;</td><td><a rel="nofollow" href="https://example.org/" class='result-link'>Example B</a></td></tr>
<tr><td></td><td class='result-snippet'>Snippet B</td></tr>
</table></body></html>"#;
        let h = parse_ddg_lite(b, 5).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].url, "https://example.com/a?x=1");
        assert_eq!(h[0].snippet, "Snippet A text");
        assert_eq!(h[1].title, "Example B");
    }

    #[test]
    fn relevant_selection() {
        let mut t = String::from("# Intro\n\nThis page is about many things.\n\n");
        for i in 0..300 {
            t.push_str(&format!("Paragraph {i} talks about gardening and weather and nothing else at all.\n\n"));
        }
        t.push_str("## Install\n\nRun cargo install xode to install the binary.\n\n");
        for i in 0..300 {
            t.push_str(&format!("Filler {i} lorem ipsum dolor sit amet consectetur.\n\n"));
        }
        let s = select_relevant(&t, "cargo install", 300);
        assert!(s.contains("# Intro"));
        assert!(s.contains("Run cargo install xode"), "{s}");
        assert!(s.contains("## Install"));
        assert!(s.contains("tokens omitted]"));
        assert!(tokens::count(&s) < 400);
    }

    #[test]
    fn json_hits_parse() {
        let v = json!([{"title": "A <strong>b</strong>", "url": "https://a", "description": "d"}, {"title": "no url"}]);
        let h = json_hits(&v, "title", "url", "description", 5);
        assert_eq!(h, vec![Hit { title: "A b".into(), url: "https://a".into(), snippet: "d".into() }]);
    }

    /// Live smoke test; never fails on network problems.
    #[tokio::test]
    #[ignore]
    async fn live_search_fetch() {
        let mut cfg = Config::default();
        cfg.tools.search_order = vec!["duckduckgo".into()];
        match search(&cfg, "tokio rust tutorial", 5).await {
            Ok(h) => println!("{}", format_hits(&h)),
            Err(e) => println!("search error: {e}"),
        }
        match fetch("https://doc.rust-lang.org/std/vec/struct.Vec.html", Some("retain_mut"), 800).await {
            Ok(s) => println!("{s}\n-----"),
            Err(e) => println!("fetch error: {e}"),
        }
        match fetch("https://tokio.rs/tokio/tutorial", Some("spawn"), 1500).await {
            Ok(s) => println!("{s}"),
            Err(e) => println!("fetch error: {e}"),
        }
    }
}
