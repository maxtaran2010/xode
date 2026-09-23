use crate::rtk;
use crate::util::spec;
use async_trait::async_trait;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use xode_core::tool::{arg_bool, arg_str, arg_u64, Tool, ToolCtx, ToolOutput};
use xode_core::types::ToolSpec;

const MARK: &str = "<<XODE_CWD>>";
const MAX_CAPTURE: usize = 32 * 1024 * 1024;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }
    fn spec(&self) -> ToolSpec {
        let desc = if cfg!(windows) {
            "Run a PowerShell command. cd persists between calls."
        } else {
            "Run a bash command. cd persists between calls."
        };
        spec(
            "shell",
            desc,
            &[
                ("command", "string", ""),
                ("timeout", "integer", "seconds"),
                ("cwd", "string", ""),
                ("background", "boolean", "run detached, output to a log file"),
            ],
            &["command"],
        )
    }
    fn summary(&self, a: &Value) -> String {
        arg_str(a, "command").unwrap_or("").chars().take(160).collect()
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> ToolOutput {
        let Some(command) = arg_str(&args, "command").filter(|c| !c.trim().is_empty()) else {
            return ToolOutput::err("missing command");
        };
        let cwd = match arg_str(&args, "cwd").filter(|c| !c.trim().is_empty()) {
            Some(c) => ctx.resolve(c),
            None => ctx.cwd(),
        };
        if !cwd.is_dir() {
            return ToolOutput::err(format!("cwd not found: {}", ctx.display(&cwd)));
        }
        let sh = pick_shell(&ctx.config.tools.shell);
        if arg_bool(&args, "background").unwrap_or(false) {
            return run_background(ctx, &sh, command, &cwd);
        }
        let secs = arg_u64(&args, "timeout").unwrap_or(ctx.config.tools.shell_timeout_s).clamp(1, 7200);
        run_foreground(ctx, &sh, command, &cwd, Duration::from_secs(secs)).await
    }
}

// ---------------------------------------------------------------- shell selection

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Shell {
    /// pwsh / powershell executable.
    Ps(String),
    #[cfg_attr(not(windows), allow(dead_code))]
    Cmd,
    /// bash / zsh / sh executable.
    Posix(String),
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
    for d in std::env::split_paths(&path) {
        for e in exts {
            let p = d.join(format!("{name}{e}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

pub(crate) fn pick_shell(cfg: &str) -> Shell {
    static AUTO: OnceCell<Shell> = OnceCell::new();
    match cfg.trim().to_lowercase().as_str() {
        "pwsh" => Shell::Ps("pwsh".into()),
        "powershell" => Shell::Ps("powershell".into()),
        #[cfg(windows)]
        "cmd" => Shell::Cmd,
        s @ ("bash" | "zsh" | "sh") => Shell::Posix(s.into()),
        _ => AUTO
            .get_or_init(|| {
                if cfg!(windows) {
                    if which("pwsh").is_some() {
                        Shell::Ps("pwsh".into())
                    } else {
                        Shell::Ps("powershell".into())
                    }
                } else if which("bash").is_some() {
                    Shell::Posix("bash".into())
                } else {
                    Shell::Posix("sh".into())
                }
            })
            .clone(),
    }
}

const PS_PREFIX: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8;$OutputEncoding=[Text.Encoding]::UTF8;$ProgressPreference='SilentlyContinue'";

/// Script text for the shell. With `track_cwd`, it prints `MARK<cwd>` at the end and keeps the exit code.
fn script(sh: &Shell, command: &str, track_cwd: bool) -> String {
    match sh {
        // The command starts on line 1 so PowerShell error line numbers match the model's command.
        Shell::Ps(_) if track_cwd => format!(
            "{PS_PREFIX};$__ok=$true;try {{ {command}\n$__ok=$?\n}} finally {{ [Console]::Out.Write(\"`n{MARK}\"+(Get-Location).ProviderPath+\"`n\") }}\nif ($LASTEXITCODE) {{ exit $LASTEXITCODE }} elseif (-not $__ok) {{ exit 1 }}"
        ),
        Shell::Ps(_) => format!("{PS_PREFIX}; {command}"),
        Shell::Cmd if track_cwd => {
            let m = MARK.replace('<', "^<").replace('>', "^>");
            format!("{command} & (set __XC=!ERRORLEVEL!) & echo({m}!CD! & exit !__XC!")
        }
        Shell::Cmd => command.to_string(),
        Shell::Posix(_) if track_cwd => {
            let pwd = if cfg!(windows) { "$(pwd -W 2>/dev/null || pwd)" } else { "$PWD" };
            format!("{command}\n__xc=$?; printf '\\n{MARK}%s\\n' \"{pwd}\"; exit $__xc")
        }
        Shell::Posix(_) => command.to_string(),
    }
}

/// Temp script file, deleted on drop.
pub(crate) struct TempScript(pub PathBuf);

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Build the process. PowerShell runs a temp `.ps1` via `-File`: `-EncodedCommand`/`-Command`
/// make Windows PowerShell 5.1 emit CLIXML on redirected stderr and have quoting/length limits.
fn build(ctx: &ToolCtx, sh: &Shell, script: &str, cwd: &Path) -> std::io::Result<(std::process::Command, Option<TempScript>)> {
    let mut tmp = None;
    let mut c = match sh {
        Shell::Ps(exe) => {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("xode-{}-{n}.ps1", std::process::id()));
            // BOM so Windows PowerShell 5.1 reads the file as UTF-8.
            let mut body = vec![0xEF, 0xBB, 0xBF];
            body.extend_from_slice(script.as_bytes());
            std::fs::write(&p, body)?;
            let mut c = std::process::Command::new(exe);
            c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&p);
            tmp = Some(TempScript(p));
            c
        }
        #[cfg(windows)]
        Shell::Cmd => {
            use std::os::windows::process::CommandExt;
            let mut c = std::process::Command::new("cmd.exe");
            c.raw_arg(format!("/D /V:ON /S /C \"{script}\""));
            c
        }
        #[cfg(not(windows))]
        Shell::Cmd => {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(script);
            c
        }
        Shell::Posix(exe) => {
            let mut c = std::process::Command::new(exe);
            if exe == "bash" || exe == "zsh" {
                c.arg("-l");
            }
            c.arg("-c").arg(script);
            c
        }
    };
    c.current_dir(cwd).stdin(Stdio::null());
    for (k, v) in [
        ("NO_COLOR", "1"),
        ("GIT_PAGER", "cat"),
        ("PAGER", "cat"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("CARGO_TERM_PROGRESS_WHEN", "never"),
        ("PYTHONIOENCODING", "utf-8"),
        ("PYTHONUNBUFFERED", "1"),
    ] {
        c.env(k, v);
    }
    c.envs(ctx.state.lock().env.iter());
    Ok((c, tmp))
}

// ---------------------------------------------------------------- foreground

enum End {
    Exit(Option<i32>),
    Timeout,
    Cancelled,
}

fn pump<R: AsyncRead + Unpin + Send + 'static>(r: R, buf: Arc<Mutex<Vec<u8>>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut r = BufReader::new(r);
        let mut line = Vec::new();
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let mut b = buf.lock();
                    if b.len() < MAX_CAPTURE {
                        b.extend_from_slice(&line);
                    }
                }
            }
        }
    })
}

async fn kill_tree(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        #[cfg(windows)]
        {
            let mut k = std::process::Command::new("taskkill");
            k.args(["/T", "/F", "/PID", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null());
            use std::os::windows::process::CommandExt;
            k.creation_flags(CREATE_NO_WINDOW);
            let _ = tokio::process::Command::from(k).status().await;
        }
        #[cfg(unix)]
        unsafe {
            // The child leads its own process group (see `process_group(0)`).
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
}

async fn run_foreground(ctx: &ToolCtx, sh: &Shell, command: &str, cwd: &Path, timeout: Duration) -> ToolOutput {
    let (mut c, _tmp) = match build(ctx, sh, &script(sh, command, true), cwd) {
        Ok(x) => x,
        Err(e) => return ToolOutput::err(format!("failed to prepare shell: {e}")),
    };
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    let mut cmd = tokio::process::Command::from(c);
    cmd.kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(ch) => ch,
        Err(e) => return ToolOutput::err(format!("failed to start shell {sh:?}: {e}")),
    };
    let buf = Arc::new(Mutex::new(Vec::new()));
    let r1 = pump(child.stdout.take().unwrap(), buf.clone());
    let r2 = pump(child.stderr.take().unwrap(), buf.clone());

    let end = tokio::select! {
        s = child.wait() => End::Exit(s.ok().and_then(|s| s.code())),
        _ = tokio::time::sleep(timeout) => { kill_tree(&mut child).await; End::Timeout }
        _ = ctx.cancel.cancelled() => { kill_tree(&mut child).await; End::Cancelled }
    };
    // Grandchildren may hold the pipes open; don't wait on them forever.
    let (a1, a2) = (r1.abort_handle(), r2.abort_handle());
    if tokio::time::timeout(Duration::from_secs(2), async {
        let _ = r1.await;
        let _ = r2.await;
    })
    .await
    .is_err()
    {
        a1.abort();
        a2.abort();
    }
    let raw = std::mem::take(&mut *buf.lock());
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    let new_cwd = take_cwd(&mut text);

    let mut footer = vec![];
    if let Some(d) = new_cwd {
        let p = normalize_dir(ctx, PathBuf::from(&d));
        if p.is_dir() && !same_dir(&p, &ctx.cwd()) {
            ctx.state.lock().cwd = Some(p.clone());
            footer.push(format!("[cwd: {}]", ctx.display(&p)));
        }
    }
    let ts = &ctx.config.token_saving;
    let f = rtk::filter_output(command, &text, ts);
    ctx.add_rtk_saved(f.saved_tokens);
    let capped = rtk::cap_output(&f.text, ts, Some(&rtk::out_path(ctx, "shell")));
    let mut out = capped.text;
    let mut is_error = false;
    match end {
        End::Exit(Some(0)) => {}
        End::Exit(Some(code)) => {
            is_error = true;
            footer.insert(0, format!("[exit {code}]"));
        }
        End::Exit(None) => {
            is_error = true;
            footer.insert(0, "[killed by signal]".into());
        }
        End::Timeout => {
            is_error = true;
            footer.insert(0, format!("[timed out after {}s; process killed]", timeout.as_secs()));
        }
        End::Cancelled => {
            is_error = true;
            footer.insert(0, "[cancelled]".into());
        }
    }
    if out.is_empty() && footer.is_empty() {
        out = "(no output)".into();
    }
    for l in footer {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&l);
    }
    ToolOutput { content: out, is_error }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Re-express a reported directory under `project_root` when it is inside it (symlinks, `\\?\`).
fn normalize_dir(ctx: &ToolCtx, p: PathBuf) -> PathBuf {
    if p.starts_with(&ctx.project_root) {
        return p;
    }
    if let (Ok(cp), Ok(cr)) = (p.canonicalize(), ctx.project_root.canonicalize()) {
        if let Ok(rel) = cp.strip_prefix(&cr) {
            return ctx.project_root.join(rel);
        }
    }
    p
}

/// Remove the cwd marker line from the output and return the directory it carried.
pub(crate) fn take_cwd(out: &mut String) -> Option<String> {
    let i = out.rfind(MARK)?;
    let rest = &out[i + MARK.len()..];
    let eol = rest.find('\n').unwrap_or(rest.len());
    let dir = rest[..eol].trim().to_string();
    let tail = rest[eol..].strip_prefix('\n').unwrap_or(&rest[eol..]).to_string();
    let mut head = out[..i].to_string();
    if head.ends_with('\n') {
        head.pop();
        if head.ends_with('\r') {
            head.pop();
        }
    }
    *out = head + &tail;
    while out.ends_with(['\n', '\r']) {
        out.pop();
    }
    (!dir.is_empty()).then_some(dir)
}

// ---------------------------------------------------------------- background

fn run_background(ctx: &ToolCtx, sh: &Shell, command: &str, cwd: &Path) -> ToolOutput {
    let log = rtk::out_path(ctx, "bg").with_extension("log");
    let file = match std::fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => return ToolOutput::err(format!("log file: {e}")),
    };
    let Ok(file2) = file.try_clone() else { return ToolOutput::err("log file clone failed") };
    let (mut c, tmp) = match build(ctx, sh, &script(sh, command, false), cwd) {
        Ok(x) => x,
        Err(e) => return ToolOutput::err(format!("failed to prepare shell: {e}")),
    };
    c.stdout(Stdio::from(file)).stderr(Stdio::from(file2));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    let mut cmd = tokio::process::Command::from(c);
    cmd.kill_on_drop(false);
    let mut child = match cmd.spawn() {
        Ok(ch) => ch,
        Err(e) => return ToolOutput::err(format!("failed to start: {e}")),
    };
    let pid = child.id().unwrap_or(0);
    let log2 = log.clone();
    tokio::spawn(async move {
        let _tmp = tmp;
        if let Ok(s) = child.wait().await {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log2) {
                let _ = writeln!(f, "\n[exited {}]", s.code().map(|c| c.to_string()).unwrap_or_else(|| "by signal".into()));
            }
        }
    });
    ToolOutput::ok(format!("started pid {pid}; log: {}", crate::util::fwd(&log)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(unix, windows))]
    use crate::util::test::ctx;

    #[test]
    fn marker_parse() {
        let mut s = "hello\n\n<<XODE_CWD>>/tmp/x\n".to_string();
        assert_eq!(take_cwd(&mut s).as_deref(), Some("/tmp/x"));
        assert_eq!(s, "hello");
        let mut s = "a\r\n\r\n<<XODE_CWD>>C:\\p\r\nlate err\n".to_string();
        assert_eq!(take_cwd(&mut s).as_deref(), Some("C:\\p"));
        assert_eq!(s, "a\r\nlate err");
        let mut s = "no marker".to_string();
        assert_eq!(take_cwd(&mut s), None);
    }


    #[cfg(unix)]
    #[tokio::test]
    async fn runs_and_tracks_cwd() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let c = ctx(d.path());
        let r = ShellTool.run(serde_json::json!({"command": "echo hi; echo err >&2; cd sub"}), &c).await;
        assert!(!r.is_error, "{}", r.content);
        assert!(r.content.starts_with("hi\nerr") || r.content.starts_with("err\nhi"), "{}", r.content);
        assert!(r.content.ends_with("[cwd: sub]"), "{}", r.content);
        assert_eq!(c.cwd().canonicalize().unwrap(), d.path().join("sub").canonicalize().unwrap());
        let r = ShellTool.run(serde_json::json!({"command": "pwd; exit 3"}), &c).await;
        assert!(r.is_error);
        assert!(r.content.contains("sub\n[exit 3]"), "{}", r.content);
        let r = ShellTool.run(serde_json::json!({"command": "true"}), &c).await;
        assert_eq!(r.content, "(no output)");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_tree() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let t = std::time::Instant::now();
        let r = ShellTool.run(serde_json::json!({"command": "sleep 30 & sleep 30; echo never", "timeout": 1}), &c).await;
        assert!(t.elapsed() < Duration::from_secs(10));
        assert!(r.content.contains("timed out after 1s"), "{}", r.content);
        assert!(!r.content.contains("never"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn background_logs() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let r = ShellTool.run(serde_json::json!({"command": "echo bg-out", "background": true}), &c).await;
        assert!(r.content.starts_with("started pid "), "{}", r.content);
        let log = r.content.split("log: ").nth(1).unwrap().to_string();
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let s = std::fs::read_to_string(&log).unwrap_or_default();
            if s.contains("[exited 0]") {
                assert!(s.contains("bg-out"));
                return;
            }
        }
        panic!("background job did not finish");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_runs_and_tracks_cwd() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("sub")).unwrap();
        let c = ctx(d.path());
        let r = ShellTool.run(serde_json::json!({"command": "Write-Output 'hi ü'; cd sub"}), &c).await;
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(r.content, "hi ü\n[cwd: sub]");
        let r = ShellTool.run(serde_json::json!({"command": "(Get-Location).Path; cmd /c exit 3"}), &c).await;
        assert!(r.is_error);
        assert!(r.content.ends_with("sub\n[exit 3]"), "{}", r.content);
        let r = ShellTool.run(serde_json::json!({"command": "Get-Item C:\\definitely-not-here"}), &c).await;
        assert!(r.is_error, "{}", r.content);
        assert!(!r.content.contains("FullyQualifiedErrorId"), "{}", r.content);
        let t = std::time::Instant::now();
        let r = ShellTool.run(serde_json::json!({"command": "Start-Sleep 30", "timeout": 2}), &c).await;
        assert!(t.elapsed() < Duration::from_secs(15));
        assert!(r.content.contains("timed out"), "{}", r.content);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rtk_applies() {
        let d = tempfile::tempdir().unwrap();
        let c = ctx(d.path());
        let r = ShellTool.run(serde_json::json!({"command": "for i in 1 2 3 4 5 6; do echo same; done"}), &c).await;
        assert_eq!(r.content, "same (x6)");
    }
}
