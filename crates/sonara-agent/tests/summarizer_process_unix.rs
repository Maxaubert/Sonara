//! `ProcessSummarizer` against fake `claude` and `codex` `#!/bin/sh`
//! scripts in a temp folder (never the real CLI), the unix twin of
//! `summarizer_process.rs`: stdin, arguments, environment, the working
//! folder, failure, and the timeout that ends the child's whole group.
#![cfg(unix)]
use sonara_agent::summarizer::ProcessSummarizer;
use sonara_agent::{Summarizer, SummaryCommand, SummarySettings};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

struct Bin {
    dir: PathBuf,
}

impl Bin {
    fn new(name: &str, script: &str) -> Bin {
        Bin::command(name, "claude", script)
    }

    fn command(name: &str, file: &str, script: &str) -> Bin {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sonara-agent-unix-{name}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(file);
        std::fs::write(&f, format!("#!/bin/sh\n{script}")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
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

fn line<'a>(out: &'a str, prefix: &str) -> &'a str {
    out.lines()
        .find_map(|l| l.trim().strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no {prefix} in {out}"))
}

#[test]
fn the_answer_comes_from_stdout_with_the_prompt_on_stdin() {
    // grep echoes the stdin lines that match, so the reply proves the
    // message reached the child inside its tags.
    let bin = Bin::new(
        "echo",
        "grep '^Turn text'\necho \"flag=$SONARA_SUMMARIZER args=$*\"\n",
    );
    let out = bin
        .summarizer()
        .summarize("Turn text here.", &settings())
        .unwrap();
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert_eq!(lines[0], "Turn text here.");
    assert_eq!(
        lines[1], "flag=1 args=-p --model haiku --tools  --setting-sources",
        "the empty arguments arrive as they are"
    );
}

/// `claude` starts in the user's home folder (`HOME` on unix), never in
/// the runtime's current folder, where project instructions could be.
#[test]
fn claude_runs_in_the_home_folder() {
    let bin = Bin::new("home", "echo \"cwd=$(pwd -P)\"\n");
    let s = ProcessSummarizer {
        path: Some(bin.dir.clone().into_os_string()),
        cwd: None,
    };
    let out = s.summarize("Text.", &settings()).unwrap();
    let home = std::fs::canonicalize(std::env::var_os("HOME").unwrap()).unwrap();
    assert_eq!(PathBuf::from(line(&out, "cwd=")), home, "{out}");
}

// #59: codex gets no tools and an empty working folder, never the home
// folder.
#[test]
fn codex_runs_without_file_tools_in_an_empty_folder() {
    let bin = Bin::command(
        "codex",
        "codex",
        "echo \"cwd=$(pwd -P)\"\necho \"files=$(ls -A | wc -l | tr -d ' ')\"\necho \"args=$*\"\n",
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
    let cwd = PathBuf::from(line(&out, "cwd="));
    let home = std::fs::canonicalize(std::env::var_os("HOME").unwrap()).unwrap();
    assert_ne!(cwd, home, "{out}");
    let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    assert!(cwd.starts_with(&temp), "{out}");
    assert_eq!(line(&out, "files="), "0", "{out}");
    assert!(!cwd.exists(), "removed after the run: {out}");
    let args = line(&out, "args=");
    for feature in ["shell_tool", "unified_exec", "view_image"] {
        assert!(
            args.contains(&format!("--disable {feature}")),
            "{feature}: {args}"
        );
    }
}

#[test]
fn a_failing_command_and_the_skip_sentinel_are_errors() {
    let bin = Bin::new("fail", "echo oops 1>&2\nexit 3\n");
    let err = bin
        .summarizer()
        .summarize("Text.", &settings())
        .unwrap_err();
    assert!(err.contains("exit 3") && err.contains("oops"), "{err}");
    let bin = Bin::new("skip", "echo SKIP\n");
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

fn exists_after(marker: &Path, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    marker.exists()
}

/// At the timeout the child's whole process group is killed: a background
/// grandchild (as an npm launcher's `node`) neither keeps running nor
/// holds the pipes.
#[test]
fn a_hung_command_is_killed_with_its_children_at_the_timeout() {
    let bin = Bin::new("hang", "");
    let marker = bin.dir.join("marker");
    std::fs::write(
        bin.dir.join("claude"),
        format!(
            "#!/bin/sh\n(sleep 3; echo late > '{}') &\nsleep 60\necho late\n",
            marker.display()
        ),
    )
    .unwrap();
    let s = SummarySettings {
        timeout_s: 1,
        ..settings()
    };
    let start = Instant::now();
    let err = bin.summarizer().summarize("Text.", &s).unwrap_err();
    assert!(err.contains("timed out"), "{err}");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "took {:?}",
        start.elapsed()
    );
    assert!(
        !exists_after(&marker, start + Duration::from_millis(4500)),
        "the grandchild was killed too"
    );
}
