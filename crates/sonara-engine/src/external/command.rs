//! Kind `command` (spec 5.4, 13.1, 13.2): a program of the user's own on
//! this PC (for example a Piper install) speaks the chunk. Not an `Adapter`:
//! no HTTP, the backend runs the program.
//!
//! - **No shell**: `argv[0]` is the full path of an `.exe` (validated),
//!   started directly with its arguments, so a `&` or `|` in the text
//!   reaches the program as it is. No console window (`CREATE_NO_WINDOW`).
//! - **Placeholders** inside any argument: `{text}`, `{voice}`, `{rate}`
//!   (words per minute), `{speed}` (`rate / 200`, two decimals), `{out}`
//!   (a temporary `.wav` path, deleted afterwards).
//! - **Input**: the text on stdin (UTF-8, then closed) or in `{text}`.
//! - **Output**: a WAV on stdout, raw 16-bit mono PCM on stdout at
//!   `sample_rate`, or a WAV file at `{out}`.
//! - **Key**: when one resolves, in the environment as `SONARA_ENGINE_KEY`
//!   (never an argument); otherwise that variable is removed.
//! - **Ends**: over `timeout_ms` or on a cancel the process is killed.
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
    Arg,
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
}

/// A temporary output file, removed on drop (also after a kill).
struct TempOut(PathBuf);

impl TempOut {
    fn new() -> TempOut {
        static N: AtomicU64 = AtomicU64::new(0);
        TempOut(std::env::temp_dir().join(format!(
            "sonara-tts-{}-{}.wav",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        )))
    }
}

impl Drop for TempOut {
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

fn kill(child: &mut Child) {
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
                Some("arg") => Input::Arg,
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
    pub fn args(&self, text: &str, voice: &str, wpm: u32, out: Option<&Path>) -> Vec<String> {
        let speed = format!("{:.2}", rate::speed(Kind::Command, wpm).unwrap_or(1.0));
        let out = out.map(|p| p.display().to_string()).unwrap_or_default();
        self.argv
            .iter()
            .skip(1)
            .map(|a| {
                // `{text}` last, so text that holds a placeholder is not
                // filled in again.
                a.replace("{voice}", voice)
                    .replace("{rate}", &wpm.to_string())
                    .replace("{speed}", &speed)
                    .replace("{out}", &out)
                    .replace("{text}", text)
            })
            .collect()
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
        let out = (self.output == Output::File).then(TempOut::new);
        let mut cmd = std::process::Command::new(&self.argv[0]);
        cmd.args(self.args(text, voice, wpm, out.as_ref().map(|o| o.0.as_path())))
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
                    kill(&mut child);
                    return Err(self.err(Reason::BadConfig, format_args!("failed: {e}")));
                }
            }
            if cancelled() {
                kill(&mut child);
                return Err(self.err(Reason::Timeout, "was stopped"));
            }
            if start.elapsed() >= self.timeout {
                kill(&mut child);
                return Err(self.err(
                    Reason::Timeout,
                    format_args!("did not finish in {} s", self.timeout.as_secs()),
                ));
            }
            std::thread::sleep(POLL);
        };
        let stderr = stderr
            .and_then(|r| r.recv_timeout(DRAIN).ok())
            .map(|(b, _)| b)
            .unwrap_or_default();
        if !status.success() {
            let last = String::from_utf8_lossy(&stderr)
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
            "-s", "{speed}", "-o", "{out}", "--say", "{text}"], "input": "arg", "output": "file"}),
        );
        assert_eq!(
            c.args(
                "Fish & chips {voice}",
                "amy",
                250,
                Some(Path::new("C:\\T\\o.wav"))
            ),
            vec![
                "--voice=amy",
                "-r",
                "250",
                "-s",
                "1.25",
                "-o",
                "C:\\T\\o.wav",
                "--say",
                "Fish & chips {voice}"
            ]
        );
        assert_eq!(c.args("x", "", 200, None)[4], "1.00");
        assert_eq!(c.input, Input::Arg);
        assert_eq!(c.output, Output::File);
        assert_eq!(c.program, "tts.exe");
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
        assert!(
            e.message
                .starts_with("The speech program (sonara-missing-tts.exe) cannot start"),
            "{}",
            e.message
        );
        assert!(!e.message.contains("C:\\Nope"), "{}", e.message);
    }
}
