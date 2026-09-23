//! `browser` tool: full control of a Chromium browser over CDP.

use crate::cdp::{remote_obj, Cdp};
use crate::util::{cap_tokens, clip};
use crate::web::Hit;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use xode_core::config::{data_dir, Browser};
use xode_core::tool::{arg_str, Tool, ToolCtx, ToolOutput, ToolRef};
use xode_core::types::ToolSpec;

pub const SNAPSHOT_JS: &str = include_str!("snapshot.js");

// ---------------------------------------------------------------- session

pub struct Session {
    cdp: Arc<Cdp>,
    port: u16,
    pub version: String,
    current: Mutex<Option<String>>,
    /// targetId -> sessionId
    attached: Mutex<HashMap<String, String>>,
}

static SESSION: Lazy<tokio::sync::Mutex<Option<Arc<Session>>>> = Lazy::new(|| tokio::sync::Mutex::new(None));

/// Shared lazily-connected session; reconnects when the connection died or the port changed.
pub async fn session(cfg: &Browser) -> Result<Arc<Session>> {
    let mut g = SESSION.lock().await;
    if let Some(s) = g.as_ref() {
        if s.cdp.alive() && s.port == cfg.debug_port {
            return Ok(s.clone());
        }
    }
    let s = connect(cfg).await?;
    *g = Some(s.clone());
    Ok(s)
}

/// Connect (attach or launch per cfg) and return the browser version string.
pub async fn browser_test(cfg: &Browser) -> Result<String> {
    let s = connect(cfg).await?;
    Ok(s.version.clone())
}

async fn ws_url(port: u16) -> Result<String> {
    let v: Value = crate::web::HTTP
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .timeout(Duration::from_secs(2))
        .send()
        .await?
        .json()
        .await?;
    v["webSocketDebuggerUrl"].as_str().map(String::from).ok_or_else(|| anyhow!("no webSocketDebuggerUrl"))
}

async fn connect(cfg: &Browser) -> Result<Arc<Session>> {
    let port = cfg.debug_port;
    let ws = match ws_url(port).await {
        Ok(u) => u,
        Err(e) => {
            if matches!(cfg.mode.as_str(), "attach_only" | "connect") {
                bail!(
                    "no browser debug endpoint on 127.0.0.1:{port} ({e}). Start the browser with --remote-debugging-port={port} and a non-default --user-data-dir"
                );
            }
            launch(cfg)?;
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                tokio::time::sleep(Duration::from_millis(300)).await;
                match ws_url(port).await {
                    Ok(u) => break u,
                    Err(e) if Instant::now() > deadline => {
                        bail!("browser launched but port {port} not reachable ({e}); close other windows using the same profile dir")
                    }
                    Err(_) => {}
                }
            }
        }
    };
    let cdp = Cdp::connect(&ws).await.context("CDP connect")?;
    let v = cdp.call("Browser.getVersion", json!({}), None).await?;
    let version = v["product"].as_str().unwrap_or("unknown").to_string();
    Ok(Arc::new(Session { cdp, port, version, current: Mutex::new(None), attached: Mutex::new(HashMap::new()) }))
}

pub fn profile_dir(cfg: &Browser) -> PathBuf {
    if cfg.user_data_dir.trim().is_empty() {
        data_dir().join("browser-profile")
    } else {
        PathBuf::from(cfg.user_data_dir.trim())
    }
}

fn launch(cfg: &Browser) -> Result<()> {
    let exe = find_browser(cfg).ok_or_else(|| anyhow!("browser '{}' not found; set browser.executable", cfg.kind))?;
    let dir = profile_dir(cfg);
    let _ = std::fs::create_dir_all(&dir);
    let mut args = vec![
        format!("--remote-debugging-port={}", cfg.debug_port),
        format!("--user-data-dir={}", dir.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
    ];
    if !cfg.profile.trim().is_empty() {
        args.push(format!("--profile-directory={}", cfg.profile.trim()));
    }
    if cfg.headless {
        args.push("--headless=new".into());
    }
    args.extend(cfg.extra_args.iter().cloned());
    tracing::info!("launching browser {} {:?}", exe.display(), args);
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    let mut child = cmd.spawn().with_context(|| format!("spawn {}", exe.display()))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Locate the browser executable for `cfg.kind` (or `cfg.executable`).
pub fn find_browser(cfg: &Browser) -> Option<PathBuf> {
    if !cfg.executable.trim().is_empty() {
        return Some(PathBuf::from(cfg.executable.trim()));
    }
    let kinds: Vec<&str> = match cfg.kind.as_str() {
        "" | "auto" => vec!["edge", "chrome", "brave", "chromium"],
        k => vec![k],
    };
    for k in kinds {
        for p in candidates(k) {
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(windows)]
fn candidates(kind: &str) -> Vec<PathBuf> {
    let rel: &[&str] = match kind {
        "edge" => &[r"Microsoft\Edge\Application\msedge.exe"],
        "chrome" => &[r"Google\Chrome\Application\chrome.exe"],
        "brave" => &[r"BraveSoftware\Brave-Browser\Application\brave.exe"],
        "chromium" => &[r"Chromium\Application\chrome.exe"],
        _ => &[],
    };
    let mut out = vec![];
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA", "ProgramW6432"] {
        if let Ok(base) = std::env::var(var) {
            for r in rel {
                out.push(PathBuf::from(&base).join(r));
            }
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn candidates(kind: &str) -> Vec<PathBuf> {
    let app = match kind {
        "edge" => "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "chrome" => "Google Chrome.app/Contents/MacOS/Google Chrome",
        "brave" => "Brave Browser.app/Contents/MacOS/Brave Browser",
        "chromium" => "Chromium.app/Contents/MacOS/Chromium",
        _ => return vec![],
    };
    let mut v = vec![PathBuf::from("/Applications").join(app)];
    if let Some(h) = std::env::var_os("HOME") {
        v.push(PathBuf::from(h).join("Applications").join(app));
    }
    v
}

#[cfg(all(unix, not(target_os = "macos")))]
fn candidates(kind: &str) -> Vec<PathBuf> {
    let names: &[&str] = match kind {
        "edge" => &["microsoft-edge", "microsoft-edge-stable"],
        "chrome" => &["google-chrome", "google-chrome-stable", "chrome"],
        "brave" => &["brave-browser", "brave"],
        "chromium" => &["chromium", "chromium-browser"],
        _ => &[],
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut out = vec![];
    for n in names {
        for d in std::env::split_paths(&path) {
            out.push(d.join(n));
        }
        out.push(PathBuf::from("/snap/bin").join(n));
    }
    out
}

#[derive(Debug, Clone)]
struct TabInfo {
    id: String,
    title: String,
    url: String,
}

impl Session {
    async fn tabs(&self) -> Result<Vec<TabInfo>> {
        let v = self.cdp.call("Target.getTargets", json!({}), None).await?;
        Ok(v["targetInfos"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|t| {
                        let u = t["url"].as_str().unwrap_or("");
                        // Edge turns edge://settings into a "browser_ui" target; skip Chrome's own UI popups.
                        (t["type"] == "page" || (t["type"] == "browser_ui" && !u.contains(".top-chrome")))
                            && !u.starts_with("devtools://")
                    })
                    .map(|t| TabInfo {
                        id: t["targetId"].as_str().unwrap_or("").into(),
                        title: t["title"].as_str().unwrap_or("").into(),
                        url: t["url"].as_str().unwrap_or("").into(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Resolve the tab to act on (by id prefix, else current, else first, else new).
    async fn target(&self, tab: Option<&str>) -> Result<String> {
        let tabs = self.tabs().await?;
        if let Some(t) = tab.map(str::trim).filter(|t| !t.is_empty()) {
            let t = t.to_ascii_lowercase();
            let m: Vec<_> = tabs.iter().filter(|x| x.id.to_ascii_lowercase().starts_with(&t)).collect();
            let id = match m.len() {
                1 => m[0].id.clone(),
                0 => bail!("no tab '{t}'"),
                _ => bail!("tab id '{t}' is ambiguous"),
            };
            *self.current.lock() = Some(id.clone());
            return Ok(id);
        }
        let cur = self.current.lock().clone();
        if let Some(c) = cur {
            if tabs.iter().any(|x| x.id == c) {
                return Ok(c);
            }
        }
        let id = match tabs.first() {
            Some(t) => t.id.clone(),
            None => self.new_tab("about:blank", false).await?,
        };
        *self.current.lock() = Some(id.clone());
        Ok(id)
    }

    async fn new_tab(&self, url: &str, background: bool) -> Result<String> {
        let v = self.cdp.call("Target.createTarget", json!({"url": url, "background": background}), None).await?;
        v["targetId"].as_str().map(String::from).ok_or_else(|| anyhow!("createTarget failed"))
    }

    async fn attach(&self, target: &str) -> Result<String> {
        if let Some(s) = self.attached.lock().get(target) {
            return Ok(s.clone());
        }
        let v = self.cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None).await?;
        let sid = v["sessionId"].as_str().ok_or_else(|| anyhow!("attach failed"))?.to_string();
        for m in ["Page.enable", "Runtime.enable", "Network.enable", "Log.enable"] {
            if let Err(e) = self.cdp.call(m, json!({}), Some(&sid)).await {
                tracing::debug!("{m}: {e}");
            }
        }
        self.attached.lock().insert(target.to_string(), sid.clone());
        Ok(sid)
    }

    /// Page-level call with one re-attach on a stale session.
    async fn pcall(&self, target: &str, method: &str, params: Value) -> Result<Value> {
        let sid = self.attach(target).await?;
        match self.cdp.call(method, params.clone(), Some(&sid)).await {
            Err(e) if e.to_string().contains("ession") && e.to_string().contains("not found") => {
                self.attached.lock().remove(target);
                self.cdp.forget(&sid);
                let sid = self.attach(target).await?;
                self.cdp.call(method, params, Some(&sid)).await
            }
            r => r,
        }
    }

    async fn eval(&self, target: &str, expr: &str) -> Result<Value> {
        let r = self
            .pcall(
                target,
                "Runtime.evaluate",
                json!({"expression": expr, "returnByValue": true, "awaitPromise": true, "userGesture": true}),
            )
            .await?;
        if let Some(d) = r.get("exceptionDetails") {
            let msg = d["exception"]["description"].as_str().or(d["text"].as_str()).unwrap_or("exception");
            bail!("{}", clip(msg, 400));
        }
        Ok(r["result"].get("value").cloned().unwrap_or_else(|| {
            let o = &r["result"];
            if o["type"] == "undefined" {
                Value::Null
            } else {
                Value::String(remote_obj(o))
            }
        }))
    }

    async fn wait_load(&self, target: &str, max: Duration) -> bool {
        let deadline = Instant::now() + max;
        tokio::time::sleep(Duration::from_millis(150)).await;
        loop {
            if let Ok(v) = self.eval(target, "document.readyState").await {
                if v == "complete" {
                    return true;
                }
            }
            if Instant::now() > deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn page_line(&self, target: &str) -> String {
        match self.eval(target, "[document.title, location.href]").await {
            Ok(v) => format!("{} — {}", clip(v[0].as_str().unwrap_or(""), 100), clip(v[1].as_str().unwrap_or(""), 200)),
            Err(e) => format!("({e})"),
        }
    }

    async fn url(&self, target: &str) -> String {
        self.eval(target, "location.href").await.ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
    }
}

// ---------------------------------------------------------------- google via browser

const GOOGLE_JS: &str = r#"(() => { const out = []; const seen = new Set();
for (const h of document.querySelectorAll('#search a h3, #rso a h3, a h3')) {
  const a = h.closest('a'); if (!a || !a.href.startsWith('http')) continue;
  let url = a.href, u; try { u = new URL(url); } catch (e) { continue; }
  if (/(^|\.)google\.[a-z.]+$/.test(u.hostname) && u.pathname === '/url') { url = u.searchParams.get('q') || u.searchParams.get('url') || ''; try { u = new URL(url); } catch (e) { continue; } }
  const box = a.closest('div.g, div.MjjYud, div[data-hveid], div[data-sokoban-container]');
  if (/(^|\.)google\.[a-z.]+$/.test(u.hostname) && u.pathname === '/goto') {
    // Opaque redirect: rebuild from the displayed "https://host › a › b" cite when not truncated.
    const c = box && box.querySelector('cite'); const ct = c ? c.innerText.trim() : '';
    if (/^https?:\/\//.test(ct) && !ct.includes('...') && !ct.includes('…')) { url = ct.split(/\s*›\s*/).join('/'); u = new URL(url); }
    else { out.push({ title: h.innerText || h.textContent, url, snippet: '' }); continue; }
  }
  if (/(^|\.)google\.[a-z.]+$/.test(u.hostname) && !/^(support|developers|cloud|docs)\./.test(u.hostname)) continue;
  if (seen.has(url)) continue; seen.add(url);
  let sn = ''; if (box) { const s = box.querySelector('.VwiC3b, [data-sncf], .IsZvec, .lEBKkf'); if (s) sn = s.innerText; }
  out.push({ title: h.innerText || h.textContent, url, snippet: sn });
} return out; })()"#;

/// Search Google in a background tab of the controlled browser.
pub async fn google_search(cfg: &Browser, q: &str, n: usize) -> Result<Vec<Hit>> {
    let s = session(cfg).await?;
    let url = reqwest::Url::parse_with_params("https://www.google.com/search", &[("q", q), ("hl", "en"), ("num", "10")])?;
    let t = s.new_tab(url.as_str(), true).await?;
    let r = async {
        // Google often reloads itself (adds &sei=), so retry a few times.
        let mut v = Value::Null;
        for attempt in 0..4 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
            s.wait_load(&t, Duration::from_secs(15)).await;
            let u = s.url(&t).await;
            if u.contains("/sorry/") {
                bail!("google captcha");
            }
            if u.contains("consent.google") {
                bail!("google consent page; accept it once in the browser");
            }
            v = s.eval(&t, GOOGLE_JS).await.unwrap_or(Value::Null);
            if v.as_array().is_some_and(|a| !a.is_empty()) {
                break;
            }
        }
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .map(|h| Hit {
                        title: h["title"].as_str().unwrap_or("").trim().into(),
                        url: h["url"].as_str().unwrap_or("").into(),
                        snippet: h["snippet"].as_str().unwrap_or("").into(),
                    })
                    .take(n)
                    .collect()
            })
            .unwrap_or_default())
    }
    .await;
    let _ = s.cdp.call("Target.closeTarget", json!({"targetId": t}), None).await;
    s.attached.lock().remove(&t);
    r
}

// ---------------------------------------------------------------- tool

struct BrowserTool;

pub fn browser_tool() -> ToolRef {
    Arc::new(BrowserTool)
}

const ACTIONS: &[&str] = &[
    "tabs", "open", "goto", "close", "snapshot", "click", "type", "key", "eval", "console", "network", "screenshot", "back",
    "reload", "wait", "cookies",
];

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "browser".into(),
            description: "Control the user's browser. snapshot gives refs (e12) for click/type. type: text into ref/selector, key=Enter submits. wait: selector or text. tab: id from tabs.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ACTIONS},
                    "url": {"type": "string"},
                    "tab": {"type": "string"},
                    "ref": {"type": "string"},
                    "selector": {"type": "string"},
                    "text": {"type": "string"},
                    "key": {"type": "string", "description": "e.g. Enter, Control+a"},
                    "js": {"type": "string"}
                },
                "required": ["action"]
            }),
        }
    }
    fn read_only(&self, args: &Value) -> bool {
        matches!(
            arg_str(args, "action").unwrap_or(""),
            "tabs" | "snapshot" | "console" | "network" | "screenshot" | "cookies"
        )
    }
    fn summary(&self, args: &Value) -> String {
        let a = arg_str(args, "action").unwrap_or("?");
        let d = ["url", "ref", "selector", "text", "key", "js"]
            .iter()
            .find_map(|k| arg_str(args, k))
            .map(|s| clip(s, 120))
            .unwrap_or_default();
        format!("browser {a} {d}").trim().to_string()
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        tokio::select! {
            r = act(&args, ctx) => match r {
                Ok(s) => ToolOutput::ok(s),
                Err(e) => ToolOutput::err(format!("{e:#}")),
            },
            _ = ctx.cancel.cancelled() => ToolOutput::err("cancelled"),
        }
    }
}

fn norm_url(u: &str) -> String {
    let u = u.trim();
    if u.contains("://") || u.starts_with("about:") || u.starts_with("data:") || u.starts_with("javascript:") {
        u.to_string()
    } else if u.starts_with("localhost") || u.starts_with("127.0.0.1") {
        format!("http://{u}")
    } else {
        format!("https://{u}")
    }
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// JS that resolves the target element (by ref index, CSS selector, or text) into `el`.
fn find_js(args: &Value) -> Result<String> {
    if let Some(r) = arg_str(args, "ref").map(str::trim).filter(|r| !r.is_empty()) {
        let n: usize = r.trim_start_matches(['e', 'E', '[']).trim_end_matches(']').parse().map_err(|_| anyhow!("bad ref '{r}'"))?;
        return Ok(format!(
            "const el = (window.__xodeRefs||[])[{n}]; if (!el) throw new Error('ref e{n} not found; take a new snapshot'); if (!el.isConnected) throw new Error('ref e{n} is stale; take a new snapshot');"
        ));
    }
    if let Some(s) = arg_str(args, "selector").map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(format!(
            "const el = document.querySelector({0}); if (!el) throw new Error('no element matches ' + {0});",
            js_str(s)
        ));
    }
    bail!("ref or selector required")
}

async fn act(args: &Value, ctx: &ToolCtx) -> Result<String> {
    let cfg = &ctx.config.browser;
    let action = arg_str(args, "action").unwrap_or("").trim();
    if !ACTIONS.contains(&action) {
        bail!("action must be one of: {}", ACTIONS.join(" "));
    }
    let s = session(cfg).await?;
    let tab = arg_str(args, "tab");
    let url = arg_str(args, "url").map(str::trim).filter(|u| !u.is_empty());
    let max_out = ctx.config.token_saving.max_tool_output_tokens.max(500);
    match action {
        "tabs" => {
            let cur = s.current.lock().clone();
            let tabs = s.tabs().await?;
            if tabs.is_empty() {
                return Ok("no tabs".into());
            }
            let lines: Vec<String> = tabs
                .iter()
                .map(|t| {
                    let mark = if Some(&t.id) == cur.as_ref() { "*" } else { " " };
                    format!("{mark}{} {} — {}", &t.id[..t.id.len().min(6)], clip(&t.title, 60), clip(&t.url, 120))
                })
                .collect();
            Ok(lines.join("\n"))
        }
        "open" => {
            let u = url.map(norm_url).unwrap_or_else(|| "about:blank".into());
            let t = s.new_tab(&u, false).await?;
            *s.current.lock() = Some(t.clone());
            let _ = s.cdp.call("Target.activateTarget", json!({"targetId": t}), None).await;
            s.wait_load(&t, Duration::from_secs(20)).await;
            Ok(format!("tab {} {}", &t[..t.len().min(6)], s.page_line(&t).await))
        }
        "goto" => {
            let u = norm_url(url.ok_or_else(|| anyhow!("url required"))?);
            let t = s.target(tab).await?;
            let r = s.pcall(&t, "Page.navigate", json!({"url": u})).await?;
            if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
                bail!("navigate failed: {e}");
            }
            let done = s.wait_load(&t, Duration::from_secs(20)).await;
            Ok(format!("{}{}", s.page_line(&t).await, if done { "" } else { " (still loading)" }))
        }
        "close" => {
            let t = s.target(tab).await?;
            s.cdp.call("Target.closeTarget", json!({"targetId": t}), None).await?;
            if let Some(sid) = s.attached.lock().remove(&t) {
                s.cdp.forget(&sid);
            }
            *s.current.lock() = None;
            Ok("closed".into())
        }
        "back" => {
            let t = s.target(tab).await?;
            let h = s.pcall(&t, "Page.getNavigationHistory", json!({})).await?;
            let i = h["currentIndex"].as_i64().unwrap_or(0);
            if i <= 0 {
                bail!("no history");
            }
            let id = h["entries"][(i - 1) as usize]["id"].clone();
            s.pcall(&t, "Page.navigateToHistoryEntry", json!({"entryId": id})).await?;
            s.wait_load(&t, Duration::from_secs(15)).await;
            Ok(s.page_line(&t).await)
        }
        "reload" => {
            let t = s.target(tab).await?;
            s.pcall(&t, "Page.reload", json!({})).await?;
            s.wait_load(&t, Duration::from_secs(20)).await;
            Ok(s.page_line(&t).await)
        }
        "snapshot" => {
            let t = s.target(tab).await?;
            let v = s.eval(&t, SNAPSHOT_JS).await?;
            let text = v.as_str().unwrap_or("").to_string();
            Ok(cap_tokens(&text, cfg.snapshot_max_tokens.max(300)))
        }
        "click" => {
            let t = s.target(tab).await?;
            let find = find_js(args)?;
            let js = format!(
                "(() => {{ {find} el.scrollIntoView({{block:'center',inline:'center',behavior:'instant'}}); const r = el.getBoundingClientRect(); \
                 return {{x: r.left + r.width/2, y: r.top + r.height/2, w: r.width, h: r.height, \
                 d: el.tagName.toLowerCase() + ' ' + (el.innerText || el.value || el.getAttribute('aria-label') || '').replace(/\\s+/g,' ').trim().slice(0,60)}}; }})()"
            );
            let before = s.url(&t).await;
            let r = s.eval(&t, &js).await?;
            let (x, y) = (r["x"].as_f64().unwrap_or(0.0), r["y"].as_f64().unwrap_or(0.0));
            if r["w"].as_f64().unwrap_or(0.0) < 1.0 || r["h"].as_f64().unwrap_or(0.0) < 1.0 {
                s.eval(&t, &format!("(() => {{ {find} el.click(); }})()")).await?;
            } else {
                for (ty, btn) in [("mouseMoved", "none"), ("mousePressed", "left"), ("mouseReleased", "left")] {
                    s.pcall(
                        &t,
                        "Input.dispatchMouseEvent",
                        json!({"type": ty, "x": x, "y": y, "button": btn, "clickCount": if ty == "mouseMoved" {0} else {1}}),
                    )
                    .await?;
                }
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
            let mut out = format!("clicked {}", r["d"].as_str().unwrap_or(""));
            let after = s.url(&t).await;
            if after != before {
                s.wait_load(&t, Duration::from_secs(10)).await;
                out.push_str(&format!("\n{}", s.page_line(&t).await));
            }
            Ok(out)
        }
        "type" => {
            let t = s.target(tab).await?;
            let text = arg_str(args, "text").unwrap_or("");
            let has_target = arg_str(args, "ref").is_some() || arg_str(args, "selector").is_some();
            let mut desc = String::from("focused element");
            if has_target {
                let find = find_js(args)?;
                let js = format!(
                    "(() => {{ {find} el.scrollIntoView({{block:'center'}}); el.focus(); \
                     if ((el.tagName==='INPUT'||el.tagName==='TEXTAREA') && el.select) el.select(); \
                     else if (el.isContentEditable) document.execCommand('selectAll'); \
                     return el.tagName.toLowerCase() + (el.name ? ' ' + el.name : ''); }})()"
                );
                desc = s.eval(&t, &js).await?.as_str().unwrap_or("").to_string();
            }
            if !text.is_empty() {
                s.pcall(&t, "Input.insertText", json!({"text": text})).await?;
            }
            let mut out = format!("typed {} chars into {desc}", text.chars().count());
            if let Some(k) = arg_str(args, "key").filter(|k| !k.trim().is_empty()) {
                let before = s.url(&t).await;
                press(&s, &t, k).await?;
                out.push_str(&format!(", pressed {k}"));
                tokio::time::sleep(Duration::from_millis(400)).await;
                if s.url(&t).await != before {
                    s.wait_load(&t, Duration::from_secs(15)).await;
                    out.push_str(&format!("\n{}", s.page_line(&t).await));
                }
            }
            Ok(out)
        }
        "key" => {
            let t = s.target(tab).await?;
            let k = arg_str(args, "key").or(arg_str(args, "text")).ok_or_else(|| anyhow!("key required"))?;
            press(&s, &t, k).await?;
            Ok(format!("pressed {k}"))
        }
        "eval" => {
            let t = s.target(tab).await?;
            let js = arg_str(args, "js").or(arg_str(args, "text")).ok_or_else(|| anyhow!("js required"))?;
            let v = s.eval(&t, js).await?;
            let out = match v {
                Value::Null => "undefined".to_string(),
                Value::String(s) => s,
                v => serde_json::to_string_pretty(&v).unwrap_or_default(),
            };
            Ok(cap_tokens(&out, max_out))
        }
        "console" => {
            let t = s.target(tab).await?;
            let sid = s.attach(&t).await?;
            let lines = s.cdp.take_console(&sid);
            if lines.is_empty() {
                return Ok("no console messages since last check".into());
            }
            Ok(cap_tokens(&lines.join("\n"), max_out))
        }
        "network" => {
            let t = s.target(tab).await?;
            let sid = s.attach(&t).await?;
            let mut n = s.cdp.take_network(&sid);
            if n.is_empty() {
                return Ok("no requests since last check".into());
            }
            let skipped = n.len().saturating_sub(80);
            n.drain(..skipped);
            let mut lines: Vec<String> = n
                .iter()
                .map(|e| {
                    let st = match (&e.error, e.status) {
                        (Some(err), _) => clip(err, 40),
                        (None, Some(s)) => s.to_string(),
                        (None, None) => "…".into(),
                    };
                    format!("{} {st} {} {}", e.method, e.kind, clip(&e.url, 150))
                })
                .collect();
            if skipped > 0 {
                lines.insert(0, format!("[{skipped} earlier requests omitted]"));
            }
            Ok(cap_tokens(&lines.join("\n"), max_out))
        }
        "screenshot" => {
            let t = s.target(tab).await?;
            let v = s.pcall(&t, "Page.captureScreenshot", json!({"format": "png"})).await?;
            let b64 = v["data"].as_str().ok_or_else(|| anyhow!("no screenshot data"))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
            let (w, h) = png_size(&bytes).unwrap_or((0, 0));
            let path = ctx.out_dir().join(format!("shot-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S-%3f")));
            std::fs::write(&path, &bytes)?;
            Ok(format!("saved {} ({w}x{h}, {} KB)", ctx.display(&path), bytes.len() / 1024))
        }
        "cookies" => {
            let t = s.target(tab).await?;
            let u = match url {
                Some(u) => norm_url(u),
                None => s.url(&t).await,
            };
            let v = s.pcall(&t, "Network.getCookies", json!({"urls": [u]})).await?;
            let lines: Vec<String> = v["cookies"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| {
                            format!(
                                "{}={} {}{}",
                                c["name"].as_str().unwrap_or(""),
                                clip(c["value"].as_str().unwrap_or(""), 40),
                                c["domain"].as_str().unwrap_or(""),
                                c["path"].as_str().unwrap_or("")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            if lines.is_empty() {
                return Ok("no cookies".into());
            }
            Ok(cap_tokens(&lines.join("\n"), max_out))
        }
        "wait" => {
            let t = s.target(tab).await?;
            let cond = if let Some(sel) = arg_str(args, "selector").filter(|x| !x.is_empty()) {
                format!("!!document.querySelector({})", js_str(sel))
            } else if let Some(txt) = arg_str(args, "text").filter(|x| !x.is_empty()) {
                format!("!!(document.body && document.body.innerText.includes({}))", js_str(txt))
            } else {
                s.wait_load(&t, Duration::from_secs(15)).await;
                tokio::time::sleep(Duration::from_millis(800)).await;
                return Ok(s.page_line(&t).await);
            };
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if s.eval(&t, &cond).await.ok() == Some(Value::Bool(true)) {
                    return Ok("found".into());
                }
                if Instant::now() > deadline {
                    bail!("timeout after 15s");
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        _ => unreachable!(),
    }
}

fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[1..4] != b"PNG" {
        return None;
    }
    Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)))
}

/// (key, code, windowsVirtualKeyCode, text)
fn key_info(k: &str) -> (String, String, i64, Option<String>) {
    let named: &[(&str, &str, i64, Option<&str>)] = &[
        ("Enter", "Enter", 13, Some("\r")),
        ("Tab", "Tab", 9, None),
        ("Escape", "Escape", 27, None),
        ("Backspace", "Backspace", 8, None),
        ("Delete", "Delete", 46, None),
        ("ArrowUp", "ArrowUp", 38, None),
        ("ArrowDown", "ArrowDown", 40, None),
        ("ArrowLeft", "ArrowLeft", 37, None),
        ("ArrowRight", "ArrowRight", 39, None),
        ("Home", "Home", 36, None),
        ("End", "End", 35, None),
        ("PageUp", "PageUp", 33, None),
        ("PageDown", "PageDown", 34, None),
        (" ", "Space", 32, Some(" ")),
    ];
    let alias = match k.to_ascii_lowercase().as_str() {
        "esc" => "Escape",
        "return" => "Enter",
        "space" => " ",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "del" => "Delete",
        _ => k,
    };
    for (key, code, vk, text) in named {
        if key.eq_ignore_ascii_case(alias) {
            return (key.to_string(), code.to_string(), *vk, text.map(String::from));
        }
    }
    let mut chars = alias.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let up = c.to_ascii_uppercase();
        let (code, vk) = if up.is_ascii_alphabetic() {
            (format!("Key{up}"), up as i64)
        } else if c.is_ascii_digit() {
            (format!("Digit{c}"), c as i64)
        } else {
            (String::new(), 0)
        };
        return (c.to_string(), code, vk, Some(c.to_string()));
    }
    if let Some(n) = alias.strip_prefix(['F', 'f']).and_then(|n| n.parse::<i64>().ok()).filter(|n| (1..=12).contains(n)) {
        return (format!("F{n}"), format!("F{n}"), 111 + n, None);
    }
    (alias.to_string(), alias.to_string(), 0, None)
}

/// Press a key or combo like `Control+a`, `Shift+Tab`, `Enter`.
async fn press(s: &Session, t: &str, combo: &str) -> Result<()> {
    let parts: Vec<&str> = combo.split('+').filter(|p| !p.is_empty()).collect();
    let (mods, key) = match parts.split_last() {
        Some((k, m)) => (m, *k),
        None if combo.contains('+') => (&[][..], "+"),
        None => bail!("key required"),
    };
    let mut bits = 0;
    for m in mods {
        bits |= match m.to_ascii_lowercase().as_str() {
            "alt" | "option" => 1,
            "ctrl" | "control" => 2,
            "meta" | "cmd" | "command" | "win" => 4,
            "shift" => 8,
            other => bail!("unknown modifier '{other}'"),
        };
    }
    let (key, code, vk, text) = key_info(key);
    let text = if bits & 7 != 0 { None } else { text };
    let mut down = json!({"type": if text.is_some() {"keyDown"} else {"rawKeyDown"}, "key": key, "code": code,
        "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": bits});
    if let Some(tx) = &text {
        down["text"] = json!(tx);
        down["unmodifiedText"] = json!(tx);
    }
    s.pcall(t, "Input.dispatchKeyEvent", down).await?;
    s.pcall(
        t,
        "Input.dispatchKeyEvent",
        json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": bits}),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_js_static() {
        assert!(SNAPSHOT_JS.len() > 1000);
        assert!(SNAPSHOT_JS.contains("__xodeRefs"));
        assert!(SNAPSHOT_JS.trim_start().starts_with("(() =>"));
    }

    #[test]
    fn keys() {
        assert_eq!(key_info("Enter").2, 13);
        assert_eq!(key_info("a"), ("a".into(), "KeyA".into(), 65, Some("a".into())));
        assert_eq!(key_info("esc").0, "Escape");
        assert_eq!(key_info("F5").2, 116);
    }

    #[test]
    fn refs_and_urls() {
        assert!(find_js(&json!({"ref": "e12"})).unwrap().contains("[12]"));
        assert!(find_js(&json!({"ref": "[e3]"})).unwrap().contains("[3]"));
        assert!(find_js(&json!({"selector": "a[href='x']"})).unwrap().contains("\"a[href='x']\""));
        assert!(find_js(&json!({})).is_err());
        assert_eq!(norm_url("example.com"), "https://example.com");
        assert_eq!(norm_url("chrome://settings"), "chrome://settings");
        assert_eq!(norm_url("localhost:3000"), "http://localhost:3000");
    }

    #[test]
    fn schema_is_small() {
        let s = serde_json::to_string(&BrowserTool.spec().parameters).unwrap();
        assert!(xode_core::tokens::count(&s) < 200, "{}", xode_core::tokens::count(&s));
    }

    /// Live: launches headless Chrome with a temp profile (ignored by default).
    #[tokio::test]
    #[ignore]
    async fn live_browser() {
        let dir = std::env::temp_dir().join("xode-test-profile");
        let cfg = Browser {
            debug_port: 9333,
            mode: "launch".into(),
            headless: true,
            user_data_dir: dir.to_string_lossy().into(),
            ..Default::default()
        };
        let v = browser_test(&cfg).await.unwrap();
        println!("version: {v}");
        let s = session(&cfg).await.unwrap();
        let t = s.target(None).await.unwrap();
        let html = "data:text/html,<title>T</title><h1>Hello</h1><nav><a href='/x'>Link X</a></nav><label>Name <input id=n></label><button onclick=\"console.log('clicked!')\">Go</button>";
        s.pcall(&t, "Page.navigate", json!({"url": html})).await.unwrap();
        s.wait_load(&t, Duration::from_secs(5)).await;
        let snap = s.eval(&t, SNAPSHOT_JS).await.unwrap();
        println!("{}", snap.as_str().unwrap());
        s.cdp.call("Browser.close", json!({}), None).await.ok();
    }

    /// Live: drives the tool end to end in headless Chrome (ignored by default).
    #[tokio::test]
    #[ignore]
    async fn live_tool() {
        let mut cfg = xode_core::Config::default();
        cfg.browser = Browser {
            debug_port: 9334,
            headless: true,
            user_data_dir: std::env::temp_dir().join("xode-test-profile2").to_string_lossy().into(),
            ..Default::default()
        };
        let root = std::env::temp_dir().join("xode-net-test");
        let ctx = ToolCtx {
            session_id: "t".into(),
            project_root: root.clone(),
            extra_roots: vec![],
            config: Arc::new(cfg.clone()),
            state: Default::default(),
            cancel: Default::default(),
            outliner: None,
        };
        let t = browser_tool();
        let html = "data:text/html,<title>Form</title><h1>Test</h1><form onsubmit=\"event.preventDefault();document.getElementById('o').innerText='got '+this.q.value;console.warn('submitted')\"><input name=q placeholder=Search value=old><button>Go</button></form><p id=o>none</p><a href='https://example.com/'>Example</a>";
        for a in [
            json!({"action": "open", "url": html}),
            json!({"action": "snapshot"}),
            json!({"action": "type", "ref": "e0", "text": "hello", "key": "Enter"}),
            json!({"action": "eval", "js": "document.getElementById('o').innerText"}),
            json!({"action": "console"}),
            json!({"action": "click", "ref": "e2"}),
            json!({"action": "wait", "text": "Example Domain"}),
            json!({"action": "network"}),
            json!({"action": "cookies"}),
            json!({"action": "key", "key": "Control+a"}),
            json!({"action": "back"}),
            json!({"action": "screenshot"}),
            json!({"action": "goto", "url": "chrome://settings"}),
            json!({"action": "snapshot"}),
            json!({"action": "tabs"}),
            json!({"action": "close"}),
        ] {
            let r = t.run(a.clone(), &ctx).await;
            let c: String = r.content.chars().take(1500).collect();
            println!(">>> {a}\n{}{c}\n", if r.is_error { "ERR " } else { "" });
        }
        match google_search(&cfg.browser, "tokio rust", 3).await {
            Ok(h) => println!("google:\n{}", crate::web::format_hits(&h)),
            Err(e) => println!("google err: {e}"),
        }
        let s = session(&cfg.browser).await.unwrap();
        s.cdp.call("Browser.close", json!({}), None).await.ok();
    }
}
