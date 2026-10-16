//! `ProcessSummarizer` against fake `claude.cmd` scripts in a temp folder
//! (never the real CLI): stdin, arguments, environment, failure, timeout.
#![cfg(windows)]
use sonara_agent::summarizer::ProcessSummarizer;
use sonara_agent::{Summarizer, SummaryCommand, SummarySettings};
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Bin {
    dir: PathBuf,
}

impl Bin {
    fn new(name: &str, script: &str) -> Bin {
        Bin::command(name, "claude.cmd", script)
    }

    fn command(name: &str, file: &str, script: &str) -> Bin {
        let dir = std::env::temp_dir().join(format!("sonara-agent-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(file), script.replace('\n', "\r\n")).unwrap();
        Bin { dir }
    }

    fn summarizer(&self) -> ProcessSummarizer {
        ProcessSummarizer {
            path: Some(self.dir.clone().into_os_string()),
            cwd: Some(self.dir.clone()),
        }
    }
}

impl Drop for Bin {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn settings() -> SummarySettings {
    SummarySettings {
        enabled: true,
        ..SummarySettings::default()
    }
}

#[test]
fn the_answer_comes_from_stdout_with_the_prompt_on_stdin() {
    // findstr echoes the stdin lines that match, so the reply proves the
    // message reached the child inside its tags.
    let bin = Bin::new(
        "echo",
        "@echo off\nfindstr /b /c:\"Turn text\"\necho flag=%SONARA_SUMMARIZER% args=%*\n",
    );
    let out = bin
        .summarizer()
        .summarize("Turn text here.", &settings())
        .unwrap();
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert_eq!(lines[0], "Turn text here.");
    assert_eq!(
        lines[1],
        r#"flag=1 args=-p --model haiku --tools "" --setting-sources """#
    );
}

// #59: codex under a read-only sandbox could still read any file (its shell
// reads anywhere), so text in the turn could make it speak a secret. It
// gets no tools and an empty working folder, never the home folder.
#[test]
fn codex_runs_without_file_tools_in_an_empty_folder() {
    let bin = Bin::command(
        "codex",
        "codex.cmd",
        "@echo off\necho cwd=%CD%\necho args=%*\n",
    );
    let s = ProcessSummarizer {
        path: Some(bin.dir.clone().into_os_string()),
        cwd: None,
    };
    let settings = SummarySettings {
        command: SummaryCommand::Codex,
        model: "m".into(),
        ..settings()
    };
    let out = s.summarize("Text.", &settings).unwrap();
    let cwd = out
        .lines()
        .find_map(|l| l.trim().strip_prefix("cwd="))
        .map(PathBuf::from)
        .unwrap();
    let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
    assert_ne!(cwd, home, "{out}");
    assert!(cwd.starts_with(std::env::temp_dir()), "{out}");
    // The folder was empty and is gone after the run.
    assert!(!cwd.exists(), "{out}");
    let args = out.lines().find(|l| l.starts_with("args=")).unwrap();
    for feature in ["shell_tool", "unified_exec", "view_image"] {
        assert!(
            args.contains(&format!("--disable {feature}")),
            "{feature}: {args}"
        );
    }
}

#[test]
fn a_failing_command_and_the_skip_sentinel_are_errors() {
    let bin = Bin::new("fail", "@echo off\necho oops 1>&2\nexit /b 3\n");
    let err = bin
        .summarizer()
        .summarize("Text.", &settings())
        .unwrap_err();
    assert!(err.contains("exit 3") && err.contains("oops"), "{err}");
    let bin = Bin::new("skip", "@echo off\necho SKIP\n");
    assert!(bin.summarizer().summarize("Text.", &settings()).is_err());
    assert!(bin.summarizer().summarize("  ", &settings()).is_err());
}

#[test]
fn a_missing_command_is_an_error() {
    let s = ProcessSummarizer {
        path: Some(
            std::env::temp_dir()
                .join("sonara-no-such-dir")
                .into_os_string(),
        ),
        cwd: None,
    };
    let err = s.summarize("Text.", &settings()).unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

#[test]
fn a_hung_command_is_killed_with_its_children_at_the_timeout() {
    // ping runs as a grandchild (cmd.exe -> ping.exe) holding the pipes:
    // without the tree kill the reads would wait for it.
    let bin = Bin::new("hang", "@echo off\nping -n 60 127.0.0.1 >nul\necho late\n");
    let s = SummarySettings {
        timeout_s: 1,
        ..settings()
    };
    let start = Instant::now();
    let err = bin.summarizer().summarize("Text.", &s).unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "took {:?}",
        start.elapsed()
    );
}
