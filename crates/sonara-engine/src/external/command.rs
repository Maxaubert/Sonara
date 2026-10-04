//! Kind `command` (spec 5.4, 13.1, 13.2): a program of the user's own on
//! this PC (for example a Piper install) speaks the chunk. Not an `Adapter`:
//! no HTTP, the backend runs the program.
//!
//! - **Local only**: a `command` profile comes from `engines.json`, which
//!   the user edits or `sonara engines add --kind command` writes; the
//!   protocol never adds or changes one (security review of PR3).
//! - **No shell**: `argv[0]` is the full path of an `.exe` (validated, and
//!   checked to exist before each start), started directly with its
//!   arguments (`std::process::Command`, which quotes each one for Windows),
//!   so a `&` or `|` in an argument reaches the program as it is. No
//!   console window (`CREATE_NO_WINDOW`).
//! - **The text is never in argv**: it goes on stdin (UTF-8, then closed)
//!   or in a temporary UTF-8 file at `{in}` (deleted afterwards).
//! - **Placeholders** inside any argument, filled in one pass (a value that
//!   holds a placeholder is not filled in again): `{voice}`, `{rate}`
//!   (words per minute), `{speed}` (`rate / 200`, two decimals), `{out}`
//!   (a temporary `.wav` path, deleted afterwards), `{in}`. A voice that
//!   goes into argv must be one of `options.voices` when that list is set,
//!   and never starts with `-` (it cannot read as an option).
//! - **Output**: a WAV on stdout, raw 16-bit mono PCM on stdout at
//!   `sample_rate`, or a WAV file at `{out}`.
//! - **Key**: when one resolves, in the environment as `SONARA_ENGINE_KEY`
//!   (never an argument); otherwise that variable is removed. The key is
//!   cut out of any stderr text kept for a failure message.
//! - **Ends**: over `timeout_ms` or on a cancel the process is killed,
//!   with every process it started (a Job Object). When the program exits,
//!   any child it left running is ended too, so it cannot hold the output
//!   pipe open.
use super::adapter::{VoiceInfo, MAX_AUDIO_BODY};
use super::audio::decode_body;
use super::error::{clean, ExtError};
use super::keys::Secret;
use super::profile::{program_name, Kind, Profile};
use super::rate;
use super::split::Limit;
use crate::{PcmChunk, Reason};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// The environment variable that carries a resolved key to the program.
pub const KEY_ENV: &str = "SONARA_ENGINE_KEY";
/// How often the run looks at the process, the clock and the cancel.
const POLL: Duration = Duration::from_millis(10);
/// The tail of stderr kept for the message of a failure.
const STDERR_KEEP: usize = 16 * 1024;
/// How long the output readers may take after the process ended (a
/// grandchild that kept the pipe open must not hang the reader).
const DRAIN: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Stdin,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    StdoutWav,
    StdoutPcm,
    File,
}

pub struct Command {
    argv: Vec<String>,
    input: Input,
    output: Output,
    sample_rate: Option<u32>,
    voices: Vec<String>,
    timeout: Duration,
    label: String,
    program: String,
    /// `{voice}` appears in argv.
    voice_in_argv: bool,
}

/// The longest voice that goes into argv.
const VOICE_MAX: usize = 200;

/// A temporary file (the output WAV, or the input text), removed on drop
/// (also after a kill).
struct TempFile(PathBuf);

impl TempFile {
    fn new(ext: &str) -> TempFile {
        static N: AtomicU64 = AtomicU64::new(0);
        TempFile(std::env::temp_dir().join(format!(
            "sonara-tts-{}-{}.{ext}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        )))
    }
}

/// Fill in the placeholders of one argument in one pass: a filled-in value
/// is never scanned again; an unknown `{name}` stays as it is.
fn fill(arg: &str, value: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        match after
            .find('}')
            .and_then(|close| value(&after[1..close]).map(|v| (v, close)))
        {
            Some((v, close)) => {
                out.push_str(&v);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Read a pipe to its end (at most `max` bytes kept: the head, or with
/// `tail` the last ones) on its own thread.
fn reader<R: Read + Send + 'static>(
    mut pipe: R,
    max: usize,
    tail: bool,
) -> Receiver<(Vec<u8>, bool)> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 64 * 1024];
        let mut over = false;
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    if out.len() > max {
                        if tail {
                            out.drain(..out.len() - max);
                        } else {
                            over = true;
                            out.truncate(max);
                            break;
                        }
                    }
                }
            }
        }
        let _ = tx.send((out, over));
    });
    rx
}

/// The program and every process it starts: ended together (on Windows a
/// Job Object that kills its processes when it is closed).
#[cfg(windows)]
mod tree {
    use std::os::windows::io::AsRawHandle;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Tree(HANDLE);

    impl Tree {
        /// `None` when Windows refuses a job (the program alone is killed
        /// then).
        pub fn new(child: &std::process::Child) -> Option<Tree> {
            // SAFETY: plain Win32 calls on a job handle this value owns and
            // on the child's process handle, which outlives the calls.
            unsafe {
                let tree = Tree(CreateJobObjectW(None, PCWSTR::null()).ok()?);
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    tree.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of_val(&info) as u32,
                )
                .ok()?;
                AssignProcessToJobObject(tree.0, HANDLE(child.as_raw_handle())).ok()?;
                Some(tree)
            }
        }

        pub fn kill(&self) {
            // SAFETY: the handle is owned and open until drop.
            unsafe {
                let _ = TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            // SAFETY: closed once; KILL_ON_JOB_CLOSE ends what is left.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod tree {
    pub struct Tree;

    impl Tree {
        pub fn new(_child: &std::process::Child) -> Option<Tree> {
            None
        }

        pub fn kill(&self) {}
    }
}

fn kill(child: &mut Child, tree: Option<&tree::Tree>) {
    if let Some(t) = tree {
        t.kill();
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Command {
    /// From a validated profile of kind `command`.
    pub fn new(p: &Profile) -> Command {
        let argv = p.command_argv();
        let voices = p
            .options
            .get("voices")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Command {
            program: program_name(argv.first().map(String::as_str).unwrap_or_default()).to_string(),
            argv,
            input: match p.option_str("input") {
                Some("file") => Input::File,
                _ => Input::Stdin,
            },
            output: match p.option_str("output") {
                Some("stdout-pcm") => Output::StdoutPcm,
                Some("file") => Output::File,
                _ => Output::StdoutWav,
            },
            sample_rate: p.option_u64("sample_rate").map(|r| r as u32),
            voices,
            timeout: Duration::from_millis(p.timeout_ms()),
            label: p.display_label(),
            voice_in_argv: p
                .command_argv()
                .iter()
                .skip(1)
                .any(|a| a.contains("{voice}")),
        }
    }

    pub fn input_limit(&self) -> Limit {
        Limit::Chars(4096)
    }

    /// `options.voices` (the list is the user's; any other id is passed on).
    pub fn voices(&self) -> Vec<VoiceInfo> {
        self.voices.iter().map(|v| VoiceInfo::named(v)).collect()
    }

    /// The arguments after `argv[0]` with the placeholders filled in.
    /// The text is never one of them.
    pub fn args(
        &self,
        voice: &str,
        wpm: u32,
        out: Option<&Path>,
        input: Option<&Path>,
    ) -> Vec<String> {
        let speed = format!("{:.2}", rate::speed(Kind::Command, wpm).unwrap_or(1.0));
        let path = |p: Option<&Path>| p.map(|p| p.display().to_string()).unwrap_or_default();
        let (out, input) = (path(out), path(input));
        let value = |name: &str| match name {
            "voice" => Some(voice.to_string()),
            "rate" => Some(wpm.to_string()),
            "speed" => Some(speed.clone()),
            "out" => Some(out.clone()),
            "in" => Some(input.clone()),
            _ => None,
        };
        self.argv.iter().skip(1).map(|a| fill(a, &value)).collect()
    }

    /// A voice that goes into argv: one of `options.voices` when that list
    /// is set, never an option (`-`), plain text of at most 200 characters.
    pub fn check_voice(&self, voice: &str) -> Result<(), ExtError> {
        if !self.voice_in_argv || voice.is_empty() {
            return Ok(());
        }
        if !self.voices.is_empty() && !self.voices.iter().any(|v| v == voice) {
            return Err(self.err(
                Reason::BadConfig,
                format_args!("was not started: the voice '{voice}' is not one of options.voices"),
            ));
        }
        if voice.starts_with('-')
            || voice.chars().any(char::is_control)
            || voice.chars().count() > VOICE_MAX
        {
            return Err(self.err(
                Reason::BadConfig,
                "was not started: the voice cannot be passed to it (it starts with '-', \
                 has control characters or is too long)",
            ));
        }
        Ok(())
    }

    fn err(&self, reason: Reason, what: impl std::fmt::Display) -> ExtError {
        ExtError::new(reason, format!("{} ({}) {what}", self.label, self.program))
    }

    /// Run the program once for `text`. `cancelled` is polled: when it
    /// says so, the process is killed.
    pub fn run(
        &self,
        text: &str,
        voice: &str,
        wpm: u32,
        key: Option<&Secret>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<PcmChunk, ExtError> {
        self.check_voice(voice)?;
        // Only ever the absolute path that was validated: never a search of
        // PATH or the current folder for a program by that name.
        let program = Path::new(&self.argv[0]);
        if !program.is_absolute() || !program.is_file() {
            return Err(self.err(
                Reason::BadConfig,
                "cannot start: the program does not exist",
            ));
        }
        let out = (self.output == Output::File).then(|| TempFile::new("wav"));
        let input = match self.input {
            Input::File => {
                let f = TempFile::new("txt");
                std::fs::write(&f.0, text.as_bytes()).map_err(|e| {
                    self.err(
                        Reason::BadConfig,
                        format_args!("cannot start: the text file cannot be written: {e}"),
                    )
                })?;
                Some(f)
            }
            Input::Stdin => None,
        };
        let mut cmd = std::process::Command::new(program);
        cmd.args(self.args(
            voice,
            wpm,
            out.as_ref().map(|o| o.0.as_path()),
            input.as_ref().map(|i| i.0.as_path()),
        ))
        .stdin(if self.input == Input::Stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if self.output == Output::File {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stderr(Stdio::piped());
        match key {
            Some(k) => cmd.env(KEY_ENV, k.expose()),
            None => cmd.env_remove(KEY_ENV),
        };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| self.err(Reason::BadConfig, format_args!("cannot start: {e}")))?;
        let tree = tree::Tree::new(&child);
        if let Some(mut stdin) = child.stdin.take() {
            let bytes = text.as_bytes().to_vec();
            // A thread, so a program that writes before it reads all of its
            // input cannot block on a full pipe against us.
            std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }
        let stdout = child
            .stdout
            .take()
            .map(|p| reader(p, MAX_AUDIO_BODY as usize, false));
        let stderr = child.stderr.take().map(|p| reader(p, STDERR_KEEP, true));
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => {
                    kill(&mut child, tree.as_ref());
                    return Err(self.err(Reason::BadConfig, format_args!("failed: {e}")));
                }
            }
            if cancelled() {
                kill(&mut child, tree.as_ref());
                return Err(self.err(Reason::Timeout, "was stopped"));
            }
            if start.elapsed() >= self.timeout {
                kill(&mut child, tree.as_ref());
                return Err(self.err(
                    Reason::Timeout,
                    format_args!("did not finish in {} s", self.timeout.as_secs()),
                ));
            }
            std::thread::sleep(POLL);
        };
        // The program is done: a child it left running must not keep the
        // output pipes open or go on using the PC.
        if let Some(t) = &tree {
            t.kill();
        }
        let stderr = stderr
            .and_then(|r| r.recv_timeout(DRAIN).ok())
            .map(|(b, _)| b)
            .unwrap_or_default();
        if !status.success() {
            // The program holds the key, so it may print it; the masking of
            // `clean` only knows token shapes, the exact key goes first.
            let mut err_text = String::from_utf8_lossy(&stderr).into_owned();
            if let Some(k) = key.map(Secret::expose).filter(|k| !k.is_empty()) {
                err_text = err_text.replace(k, "[redacted]");
            }
            let last = err_text
                .lines()
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(clean)
                .unwrap_or_default();
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".into());
            let mut what = format!("failed (exit code {code})");
            if !last.is_empty() {
                what.push_str(&format!(": {last}"));
            }
            return Err(self.err(Reason::Server, what));
        }
        let body = match (&out, stdout) {
            (Some(file), _) => std::fs::read(&file.0)
                .map_err(|_| self.err(Reason::Format, "wrote no audio file"))?,
            (None, Some(r)) => match r.recv_timeout(DRAIN) {
                Ok((_, true)) => {
                    return Err(self.err(
                        Reason::Format,
                        format_args!("printed more than {MAX_AUDIO_BODY} bytes"),
                    ))
                }
                Ok((b, false)) => b,
                Err(_) => return Err(self.err(Reason::Format, "did not close its output")),
            },
            (None, None) => Vec::new(),
        };
        self.decode(&body)
    }

    fn decode(&self, body: &[u8]) -> Result<PcmChunk, ExtError> {
        if body.is_empty() {
            return Err(self.err(Reason::Format, "gave no audio"));
        }
        let wav = body.starts_with(b"RIFF") && body.get(8..12) == Some(b"WAVE");
        let label = format!("{} ({})", self.label, self.program);
        match self.output {
            Output::StdoutPcm => decode_body(body, None, self.sample_rate, true, &label),
            Output::StdoutWav | Output::File if wav => decode_body(body, None, None, false, &label),
            Output::StdoutWav | Output::File => Err(self.err(
                Reason::Format,
                "gave audio that is not WAV (set options.output to stdout-pcm for raw samples)",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(options: serde_json::Value) -> Command {
        Command::new(
            &Profile::from_json(&json!({"id": "cmd", "kind": "command", "options": options}))
                .unwrap(),
        )
    }

    #[test]
    fn placeholders_are_filled_in_every_argument() {
        let c = command(
            json!({"argv": ["C:\\Tools\\tts.exe", "--voice={voice}", "-r", "{rate}",
            "-s", "{speed}", "-o", "{out}", "-i", "{in}"], "input": "file", "output": "file"}),
        );
        assert_eq!(
            c.args(
                "amy",
                250,
                Some(Path::new("C:\\T\\o.wav")),
                Some(Path::new("C:\\T\\i.txt"))
            ),
            vec![
                "--voice=amy",
                "-r",
                "250",
                "-s",
                "1.25",
                "-o",
                "C:\\T\\o.wav",
                "-i",
                "C:\\T\\i.txt"
            ]
        );
        assert_eq!(c.args("", 200, None, None)[4], "1.00");
        assert_eq!(c.input, Input::File);
        assert_eq!(c.output, Output::File);
        assert_eq!(c.program, "tts.exe");
    }

    /// One pass: a voice that holds a placeholder is not filled in again.
    #[test]
    fn a_value_is_never_filled_in_twice() {
        let c = command(json!({"argv": ["C:\\Tools\\tts.exe", "{voice}|{rate}|{nope}"]}));
        assert_eq!(c.args("{rate}", 200, None, None), vec!["{rate}|200|{nope}"]);
    }

    /// A voice that reaches argv is a voice of the list (when there is
    /// one), and never reads as an option.
    #[test]
    fn a_voice_in_argv_is_checked() {
        let listed =
            command(json!({"argv": ["C:\\Tools\\tts.exe", "-v", "{voice}"], "voices": ["amy"]}));
        assert!(listed.check_voice("amy").is_ok());
        assert!(
            listed.check_voice("").is_ok(),
            "no voice: {{voice}} is empty"
        );
        let e = listed.check_voice("joe").unwrap_err();
        assert_eq!(e.reason, Reason::BadConfig);
        assert!(
            e.message.contains("not one of options.voices"),
            "{}",
            e.message
        );
        let open = command(json!({"argv": ["C:\\Tools\\tts.exe", "-v", "{voice}"]}));
        assert!(open.check_voice("en_US-amy-medium").is_ok());
        for bad in ["--output=C:\\x", "-v", "a\nb", "x".repeat(201).as_str()] {
            assert_eq!(
                open.check_voice(bad).unwrap_err().reason,
                Reason::BadConfig,
                "{bad}"
            );
        }
        let unused = command(json!({"argv": ["C:\\Tools\\tts.exe"]}));
        assert!(unused.check_voice("--anything").is_ok(), "{{voice}} unused");
    }

    #[test]
    fn defaults_and_voices() {
        let c = command(json!({"argv": ["C:\\Tools\\tts.exe"], "voices": ["amy", "joe"]}));
        assert_eq!((c.input, c.output), (Input::Stdin, Output::StdoutWav));
        assert_eq!(c.timeout, Duration::from_secs(30));
        assert_eq!(
            c.voices().into_iter().map(|v| v.id).collect::<Vec<_>>(),
            vec!["amy", "joe"]
        );
        assert_eq!(c.input_limit(), Limit::Chars(4096));
    }

    #[test]
    fn a_missing_program_is_bad_config() {
        let c = command(json!({"argv": ["C:\\Nope\\sonara-missing-tts.exe"]}));
        let e = c.run("x", "", 200, None, &|| false).unwrap_err();
        assert_eq!(e.reason, Reason::BadConfig);
        assert_eq!(
            e.message,
            "The speech program (sonara-missing-tts.exe) cannot start: the program does not exist"
        );
        assert!(!e.message.contains("C:\\Nope"), "{}", e.message);
    }
}
