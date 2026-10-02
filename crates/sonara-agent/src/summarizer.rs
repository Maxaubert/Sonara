//! The out-of-band summarizer: a throwaway headless `claude -p` or
//! `codex exec` whose only input is the turn's text and a fixed instruction
//! (port of the Python plugin's `summarizer.py`). The user's own agent
//! session is never touched. Any failure maps to `Err(reason)`, and the
//! rules then speak the raw text.
//!
//! - The command is found on `PATH` only (absolute entries, with `PATHEXT`
//!   for a bare name such as an npm `.cmd` shim), never in the current
//!   folder (#138, audit H1).
//! - The prompt goes on stdin, the child runs in the user's home folder
//!   (no project instructions are picked up) with `SONARA_SUMMARIZER=1` (the
//!   hook adapter sends nothing from inside it) and no console window.
//! - On timeout the whole process tree is killed (an npm shim runs
//!   `cmd.exe` then `node`, and killing `cmd.exe` alone leaves `node`
//!   holding the pipes, #138 audit F5) and the pipes are abandoned.
use crate::settings::{Style, SummaryCommand, SummarySettings};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Writes the summary of a turn (`None` from the rules' point of view on
/// any failure; the reason is for the log).
pub trait Summarizer: Send + Sync {
    fn summarize(&self, text: &str, settings: &SummarySettings) -> Result<String, String>;
}

/// The built-in instruction of a style (`prompts/<style>.txt`).
pub fn instruction(style: Style) -> String {
    let raw = match style {
        Style::Tidy => include_str!("../prompts/tidy.txt"),
        Style::Natural => include_str!("../prompts/natural.txt"),
        Style::Brief => include_str!("../prompts/brief.txt"),
    };
    // A Windows checkout may turn the files' line ends into CRLF.
    raw.replace("\r\n", "\n")
}

/// The full stdin prompt: the instruction (a custom one wins), a blank line
/// and the message in `<message>` tags, so the model treats it strictly as
/// content to restate.
pub fn prompt(text: &str, settings: &SummarySettings) -> String {
    let base = settings
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| instruction(settings.style));
    format!("{base}\n\n<message>\n{text}\n</message>")
}

/// The command's arguments (the prompt is never an argument).
///
/// claude: `--tools ""` makes it text in, text out; `--setting-sources ""`
/// loads no settings, so no plugin (Sonara's own hooks included) runs in
/// the child (an endless summary loop otherwise, verified live).
/// codex: read-only sandbox, no repository, MCP servers and memories off,
/// low reasoning effort, prompt on stdin (`-`).
pub fn args(command: SummaryCommand, model: &str) -> Vec<String> {
    let v: Vec<&str> = match command {
        SummaryCommand::Claude => vec![
            "-p",
            "--model",
            model,
            "--tools",
            "",
            "--setting-sources",
            "",
        ],
        SummaryCommand::Codex => vec![
            "exec",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--color",
            "never",
            "-c",
            "mcp_servers={}",
            "--disable",
            "memories",
            "-c",
            "model_reasoning_effort=\"low\"",
            "-m",
            model,
            "-",
        ],
    };
    v.into_iter().map(str::to_string).collect()
}

/// The file names a bare command may have (`PATHEXT`); a name that already
/// has one of the extensions is kept.
fn command_names(name: &str, pathext: &str) -> Vec<String> {
    let exts: Vec<&str> = pathext.split(';').filter(|e| !e.is_empty()).collect();
    let lower = name.to_lowercase();
    if exts.iter().any(|e| lower.ends_with(&e.to_lowercase())) {
        return vec![name.to_string()];
    }
    exts.iter().map(|e| format!("{name}{e}")).collect()
}

/// Find `name` on `path` (absolute entries only). A name with a folder part
/// is taken as given.
pub fn resolve(name: &str, path: &OsString, pathext: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.parent().is_some_and(|d| !d.as_os_str().is_empty()) {
        return p.is_file().then(|| p.to_path_buf());
    }
    for dir in std::env::split_paths(path) {
        let dir = PathBuf::from(dir.to_string_lossy().trim_matches('"'));
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        for n in command_names(name, pathext) {
            let f = dir.join(n);
            if f.is_file() {
                return Some(f);
            }
        }
    }
    None
}

/// Read the answer: the "SKIP" sentinel (the model's "nothing to say") and
/// empty output are failures.
pub fn parse_output(out: &str) -> Result<String, String> {
    let out = out.trim();
    if out
        .trim_matches(|c: char| c == '.' || c == '!' || c == ' ')
        .eq_ignore_ascii_case("skip")
    {
        return Err("summarizer returned the SKIP sentinel".into());
    }
    if out.is_empty() {
        return Err("summarizer returned empty output".into());
    }
    Ok(out.to_string())
}

/// Spawns the real command.
#[derive(Debug, Clone, Default)]
pub struct ProcessSummarizer {
    /// Search path instead of `PATH` (tests).
    pub path: Option<OsString>,
    /// Working folder instead of the user's home (tests).
    pub cwd: Option<PathBuf>,
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = (cmd, CREATE_NO_WINDOW);
}

/// Kill `child` and every process it started. Never fails.
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut tk = Command::new("taskkill");
        tk.args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut tk);
        if let Ok(mut p) = tk.spawn() {
            let end = Instant::now() + Duration::from_secs(10);
            while Instant::now() < end {
                if let Ok(Some(_)) = p.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    let _ = child.kill();
}

fn drain(mut r: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx
}

impl ProcessSummarizer {
    pub fn run(
        &self,
        settings: &SummarySettings,
        stdin_text: &str,
    ) -> Result<(i32, String, String), String> {
        let name = settings.command.as_str();
        let path = self
            .path
            .clone()
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let pathext =
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
        let exe = resolve(name, &path, &pathext)
            .ok_or_else(|| format!("summarizer command not found on PATH: {name}"))?;
        let cwd = self
            .cwd
            .clone()
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from));
        let mut cmd = Command::new(&exe);
        cmd.args(args(settings.command, &settings.model))
            .env("SONARA_SUMMARIZER", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd.filter(|d| d.is_dir()) {
            cmd.current_dir(dir);
        }
        hide_window(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("summarizer spawn failed: {e}"))?;
        let mut stdin = child.stdin.take().expect("piped");
        let input = stdin_text.as_bytes().to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
        let out = drain(child.stdout.take().expect("piped"));
        let err = drain(child.stderr.take().expect("piped"));
        let end = Instant::now() + settings.timeout();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if Instant::now() >= end => {
                    kill_tree(&mut child);
                    let _ = child.wait();
                    return Err(format!(
                        "summarizer timed out after {} s",
                        settings.timeout_s
                    ));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => {
                    kill_tree(&mut child);
                    return Err(format!("summarizer wait failed: {e}"));
                }
            }
        };
        // The child ended; a grandchild could still hold the pipes, so the
        // reads are bounded too.
        let grace = Duration::from_secs(5);
        let stdout = out.recv_timeout(grace).unwrap_or_default();
        let stderr = err.recv_timeout(grace).unwrap_or_default();
        Ok((
            status.code().unwrap_or(-1),
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        ))
    }
}

impl Summarizer for ProcessSummarizer {
    fn summarize(&self, text: &str, settings: &SummarySettings) -> Result<String, String> {
        if text.trim().is_empty() {
            return Err("nothing to summarize".into());
        }
        let (code, out, err) = self.run(settings, &prompt(text, settings))?;
        if code != 0 {
            let detail: String = err.chars().take(300).collect();
            return Err(format!("summarizer exit {code}: {detail}"));
        }
        parse_output(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_wraps_the_message_after_the_instruction() {
        let s = SummarySettings::default();
        let p = prompt("Hello.", &s);
        assert!(p.starts_with("You are a spoken-digest engine"));
        assert!(p.ends_with("reply with exactly: SKIP\n\n<message>\nHello.\n</message>"));
        assert!(!p.contains('\r'));
        let custom = SummarySettings {
            prompt: Some("  Say it short.  ".into()),
            ..SummarySettings::default()
        };
        assert_eq!(
            prompt("Hi.", &custom),
            "Say it short.\n\n<message>\nHi.\n</message>"
        );
        for style in [Style::Tidy, Style::Natural, Style::Brief] {
            assert!(instruction(style).contains("SKIP"), "{style:?}");
        }
    }

    #[test]
    fn arguments_per_command() {
        assert_eq!(
            args(SummaryCommand::Claude, "haiku"),
            [
                "-p",
                "--model",
                "haiku",
                "--tools",
                "",
                "--setting-sources",
                ""
            ]
        );
        let codex = args(SummaryCommand::Codex, "gpt-5");
        assert_eq!(codex[0], "exec");
        assert_eq!(codex[codex.len() - 2..], ["gpt-5", "-"]);
    }

    #[test]
    fn output_sentinel_and_empty_are_failures() {
        assert_eq!(parse_output("  A recap. \n").unwrap(), "A recap.");
        assert!(parse_output("SKIP").is_err());
        assert!(parse_output("skip.").is_err());
        assert!(parse_output(" \n").is_err());
    }

    #[test]
    fn pathext_names() {
        assert_eq!(
            command_names("claude", ".EXE;.CMD"),
            ["claude.EXE", "claude.CMD"]
        );
        assert_eq!(command_names("codex.cmd", ".EXE;.CMD"), ["codex.cmd"]);
    }

    #[test]
    fn resolve_searches_absolute_path_entries_only() {
        let dir = std::env::temp_dir().join(format!("sonara-agent-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("claude.cmd"), "@echo off\r\n").unwrap();
        let path = std::env::join_paths([PathBuf::from("."), dir.clone()]).unwrap();
        assert_eq!(
            resolve("claude", &path, ".EXE;.CMD"),
            Some(dir.join("claude.CMD"))
                .filter(|p| p.is_file())
                .or(Some(dir.join("claude.cmd")))
        );
        let relative = std::env::join_paths([PathBuf::from(".")]).unwrap();
        assert_eq!(resolve("claude", &relative, ".CMD"), None);
        assert_eq!(resolve("nothere", &path, ".EXE;.CMD"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
