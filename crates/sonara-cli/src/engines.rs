//! `sonara engines ...` (spec 11.1): external engine profiles through the
//! protocol (`engine_*` messages). Parsing turns the arguments into one
//! request; a key is never an argument (stdin or a prompt without echo).
//! `add --kind command` is the exception: a program is never added over the
//! protocol, so it is written into `engines.json` locally (`engines_file`)
//! and the runtime reloads the file.
use serde_json::{json, Map, Value};

pub const USAGE: &str = "usage: sonara engines <command>

  list                       the engines you added, with their key and status
  add <id> [--kind K] [--preset P] [--url U] [--model M]
          [--voice V] [--label L] [--key-env NAME | --no-key]
          [--option KEY=VALUE ...] [--replace]
                             add (or with --replace, change) an engine; it is
                             not used until `sonara engines use <id>`
  key <id>                   store its key: read from stdin, or asked for
                             without echo (never an argument)
  key <id> --clear           delete its stored key
  use <id>                   read with it from now on (`use kokoro` goes back)
  test <id> [TEXT]           speak one sentence with it, with no fallback
  remove <id> [--keep-key]   remove it (and its stored key)

Kinds: openai-compatible (the default), elevenlabs, azure, google, cartesia,
deepgram, command (a program of your own on this PC; written into
engines.json by this command, never sent over the protocol; the text goes
on its stdin, or with --option input=file in a file at {in}).
Presets of openai-compatible: openai, kokoro-fastapi, localai, speaches,
openedai-speech, chatterbox-api, chatterbox-server, generic.
Examples: add el --kind elevenlabs --voice <voice_id>
          add az --kind azure --voice en-US-AvaMultilingualNeural --option region=westeurope
          add gg --kind google --voice en-US-Chirp3-HD-Kore
          add ca --kind cartesia --voice <voice_id>
          add dg --kind deepgram --voice aura-2-thalia-en
          add piper --kind command --option output=file --option
              'argv=[\"C:/piper/piper.exe\", \"-m\", \"C:/piper/en_US-amy.onnx\", \"-f\", \"{out}\"]'";

/// What a command asks of the runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    List,
    /// A request to send as it is.
    Request(Value),
    /// `add --kind command`: write the profile into `engines.json`, then
    /// `engine_reload`.
    AddLocal {
        profile: Value,
        replace: bool,
    },
    /// `engine_key`; the key is read after parsing (stdin or a prompt).
    SetKey {
        id: String,
    },
}

fn id_arg(rest: &[String], cmd: &str) -> Result<String, String> {
    rest.first()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .ok_or_else(|| format!("'engines {cmd}' needs an engine id"))
}

/// An option value: JSON when it parses (numbers, true, objects), else text.
fn option_value(v: &str) -> Value {
    serde_json::from_str(v).unwrap_or_else(|_| json!(v))
}

/// Parse `sonara engines <args>`.
pub fn parse(args: &[String]) -> Result<Action, String> {
    let Some(cmd) = args.first().map(String::as_str) else {
        return Err("missing command".into());
    };
    let rest = &args[1..];
    match cmd {
        "list" => Ok(Action::List),
        "add" => parse_add(rest),
        "key" => {
            let id = id_arg(rest, "key")?;
            match rest.get(1).map(String::as_str) {
                None => Ok(Action::SetKey { id }),
                Some("--clear") => Ok(Action::Request(
                    json!({"type": "engine_key", "engine": id, "secret": null}),
                )),
                Some(other) => Err(format!(
                    "unknown argument '{other}' (a key is never an argument: pipe it in, or \
                     type it when asked)"
                )),
            }
        }
        "use" => Ok(Action::Request(
            json!({"type": "set", "key": "engine", "value": id_arg(rest, "use")?}),
        )),
        "test" => {
            let id = id_arg(rest, "test")?;
            let mut m = json!({"type": "engine_test", "engine": id});
            if rest.len() > 1 {
                m["text"] = json!(rest[1..].join(" "));
            }
            Ok(Action::Request(m))
        }
        "remove" => {
            let id = id_arg(rest, "remove")?;
            let keep = match rest.get(1).map(String::as_str) {
                None => false,
                Some("--keep-key") => true,
                Some(other) => return Err(format!("unknown argument '{other}'")),
            };
            Ok(Action::Request(
                json!({"type": "engine_remove", "engine": id, "forget_key": !keep}),
            ))
        }
        other => Err(format!("unknown engines command '{other}'")),
    }
}

fn parse_add(rest: &[String]) -> Result<Action, String> {
    let id = id_arg(rest, "add")?;
    let mut profile = Map::new();
    profile.insert("id".into(), json!(id));
    let mut options = Map::new();
    let mut replace = false;
    let mut it = rest[1..].iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--kind" => {
                profile.insert("kind".into(), json!(value("--kind")?));
            }
            "--preset" => {
                options.insert("preset".into(), json!(value("--preset")?));
            }
            "--url" | "--model" | "--voice" | "--label" => {
                let v = value(a)?;
                profile.insert(a[2..].into(), json!(v));
            }
            "--key-env" => {
                profile.insert(
                    "key_ref".into(),
                    json!(format!("env:{}", value("--key-env")?)),
                );
            }
            "--no-key" => {
                profile.insert("key_ref".into(), json!("none"));
            }
            "--option" => {
                let kv = value("--option")?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or(format!("--option '{kv}': use KEY=VALUE"))?;
                options.insert(k.trim().to_string(), option_value(v.trim()));
            }
            "--replace" => replace = true,
            "--key" | "--secret" | "--api-key" => {
                return Err("a key is never an argument: add the engine, then run \
                     `sonara engines key <id>` and paste it when asked"
                    .into())
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if !profile.contains_key("kind") {
        profile.insert("kind".into(), json!("openai-compatible"));
    }
    if !options.is_empty() {
        profile.insert("options".into(), Value::Object(options));
    }
    if profile["kind"] == "command" {
        return Ok(Action::AddLocal {
            profile: Value::Object(profile),
            replace,
        });
    }
    Ok(Action::Request(json!({
        "type": "engine_add",
        "engine": Value::Object(profile),
        "replace": replace,
    })))
}

/// One line per engine for `list`.
pub fn list_lines(reply: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let empty = Vec::new();
    let engines = reply["engines"].as_array().unwrap_or(&empty);
    if engines.is_empty() {
        out.push("No engines added. Built in: Kokoro and the Windows voices.".to_string());
    }
    for e in engines {
        let key = match (
            e["key_ref"].as_str().unwrap_or("none"),
            e["key_present"] == true,
        ) {
            ("none", _) => "no key needed".to_string(),
            (r, true) if r.starts_with("env:") => format!("key from {}", &r[4..]),
            (r, false) if r.starts_with("env:") => format!("no key: {} is not set", &r[4..]),
            (_, true) => "key saved".to_string(),
            (_, false) => "no key".to_string(),
        };
        let place = if e["local"] == true {
            "runs on this PC".to_string()
        } else {
            format!(
                "sends text to {}",
                e["sends_text_to"].as_str().unwrap_or("?")
            )
        };
        let status = if e["supported"] == false {
            "not supported by this version".to_string()
        } else if let Some(err) = e["error"].as_str() {
            format!("not usable: {err}")
        } else {
            match e["status"]["reason"].as_str() {
                Some(r) => format!(
                    "{} ({r}), reading with {}",
                    e["status"]["status"].as_str().unwrap_or("?"),
                    e["status"]["fallback"]
                        .as_str()
                        .unwrap_or("the built-in voice")
                ),
                None => e["status"]["status"]
                    .as_str()
                    .unwrap_or("ready")
                    .to_string(),
            }
        };
        out.push(format!(
            "{} {} ({}, {}): {place}; {key}; {status}",
            if e["current"] == true { "*" } else { " " },
            e["id"].as_str().unwrap_or("?"),
            e["label"].as_str().unwrap_or(""),
            e["kind"].as_str().unwrap_or("?"),
        ));
    }
    out
}

/// What a successful reply means to the user.
pub fn done_line(request: &Value, reply: &Value) -> String {
    match request["type"].as_str().unwrap_or("") {
        "engine_add" | "engine_reload" => format!(
            "Added {}: {}. Use it with `sonara engines use {}`{}.",
            reply["engine"]["id"].as_str().unwrap_or("?"),
            if reply["engine"]["local"] == true {
                "it runs on this PC".to_string()
            } else {
                format!(
                    "it sends the text Sonara reads to {}",
                    reply["engine"]["sends_text_to"].as_str().unwrap_or("?")
                )
            },
            reply["engine"]["id"].as_str().unwrap_or("?"),
            if reply["engine"]["key_present"] == false && reply["engine"]["key_ref"] == "credman" {
                format!(
                    "; store its key first with `sonara engines key {}`",
                    reply["engine"]["id"].as_str().unwrap_or("?")
                )
            } else {
                String::new()
            }
        ),
        "engine_key" => {
            if reply["key_present"] == true {
                "Key saved.".into()
            } else {
                "Key deleted.".into()
            }
        }
        "engine_test" => format!(
            "Spoke {} ms of audio with voice {} in {} ms.",
            reply["duration_ms"],
            reply["voice"].as_str().unwrap_or("?"),
            reply["ms"]
        ),
        "engine_remove" => format!(
            "Removed {}; reading with {}.",
            reply["removed"].as_str().unwrap_or("?"),
            reply["engine"].as_str().unwrap_or("?")
        ),
        "set" => format!(
            "Reading with {} from now on.",
            reply["value"].as_str().unwrap_or("?")
        ),
        _ => "Done.".into(),
    }
}

/// The error of a reply, with its reason when there is one.
pub fn error_line(reply: &Value) -> String {
    let e = &reply["error"];
    match e["reason"].as_str() {
        Some(r) => format!("{} ({r})", e["message"].as_str().unwrap_or("failed")),
        None => e["message"].as_str().unwrap_or("failed").to_string(),
    }
}

/// Read a key: stdin when it is piped, else a prompt without echo.
pub fn read_key() -> Result<String, String> {
    use std::io::{IsTerminal, Read};
    let stdin = std::io::stdin();
    let key = if stdin.is_terminal() {
        prompt_hidden("Paste the key (it is not shown) and press Enter: ")?
    } else {
        let mut s = String::new();
        stdin
            .lock()
            .read_to_string(&mut s)
            .map_err(|e| format!("cannot read the key from stdin: {e}"))?;
        s
    };
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("no key given".into());
    }
    Ok(key)
}

#[cfg(windows)]
fn prompt_hidden(prompt: &str) -> Result<String, String> {
    use std::io::{BufRead, Write};
    use windows::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE, ENABLE_ECHO_INPUT,
        STD_INPUT_HANDLE,
    };
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    // SAFETY: the process's own stdin handle; the mode is restored below.
    let restore = unsafe {
        let h = GetStdHandle(STD_INPUT_HANDLE).map_err(|e| e.to_string())?;
        let mut mode = CONSOLE_MODE(0);
        GetConsoleMode(h, &mut mode).map_err(|e| e.to_string())?;
        SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT).map_err(|e| e.to_string())?;
        (h, mode)
    };
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    // SAFETY: as above.
    unsafe {
        let _ = SetConsoleMode(restore.0, restore.1);
    }
    eprintln!();
    read.map_err(|e| e.to_string())?;
    Ok(line)
}

#[cfg(not(windows))]
fn prompt_hidden(_prompt: &str) -> Result<String, String> {
    Err("pipe the key in: echo KEY | sonara engines key <id>".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Action, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn add_builds_the_profile() {
        let a = p(&[
            "add",
            "kgpu",
            "--preset",
            "kokoro-fastapi",
            "--url",
            "http://127.0.0.1:8880/v1",
            "--voice",
            "af_heart",
            "--no-key",
            "--option",
            "prefetch=2",
            "--option",
            "extra={\"stream\": false}",
            "--replace",
        ])
        .unwrap();
        assert_eq!(
            a,
            Action::Request(json!({"type": "engine_add", "replace": true, "engine": {
                "id": "kgpu", "kind": "openai-compatible", "url": "http://127.0.0.1:8880/v1",
                "voice": "af_heart", "key_ref": "none",
                "options": {"preset": "kokoro-fastapi", "prefetch": 2, "extra": {"stream": false}}}}))
        );
        let a = p(&[
            "add",
            "oa",
            "--kind",
            "openai-compatible",
            "--key-env",
            "OPENAI_API_KEY",
        ])
        .unwrap();
        let Action::Request(r) = a else { panic!() };
        assert_eq!(r["engine"]["key_ref"], "env:OPENAI_API_KEY");
        assert!(r["engine"].get("options").is_none());
    }

    #[test]
    fn add_a_cloud_kind_with_its_options() {
        let a = p(&[
            "add",
            "az",
            "--kind",
            "azure",
            "--voice",
            "en-US-AvaMultilingualNeural",
            "--option",
            "region=westeurope",
            "--key-env",
            "AZURE_SPEECH_KEY",
        ])
        .unwrap();
        assert_eq!(
            a,
            Action::Request(json!({"type": "engine_add", "replace": false, "engine": {
                "id": "az", "kind": "azure", "voice": "en-US-AvaMultilingualNeural",
                "key_ref": "env:AZURE_SPEECH_KEY", "options": {"region": "westeurope"}}}))
        );
        assert!(USAGE.contains(
            "elevenlabs, azure, google, cartesia,
deepgram, command"
        ));
        // A command's argv is a JSON list in one --option; it is written
        // into engines.json locally, never sent as engine_add.
        let a = p(&[
            "add",
            "piper",
            "--kind",
            "command",
            "--option",
            r#"argv=["C:/piper/piper.exe", "-f", "{out}"]"#,
            "--option",
            "output=file",
        ])
        .unwrap();
        assert_eq!(
            a,
            Action::AddLocal {
                profile: json!({"id": "piper", "kind": "command", "options": {
                    "argv": ["C:/piper/piper.exe", "-f", "{out}"], "output": "file"}}),
                replace: false
            }
        );
        let Action::AddLocal { replace, .. } =
            p(&["add", "piper", "--kind", "command", "--replace"]).unwrap()
        else {
            panic!("a command is never a protocol request")
        };
        assert!(replace);
    }

    #[test]
    fn a_key_is_never_an_argument() {
        assert!(p(&["add", "oa", "--key", "sk-x"])
            .unwrap_err()
            .contains("never an argument"));
        assert!(p(&["key", "oa", "sk-x"])
            .unwrap_err()
            .contains("never an argument"));
        assert_eq!(
            p(&["key", "oa"]).unwrap(),
            Action::SetKey { id: "oa".into() }
        );
        assert_eq!(
            p(&["key", "oa", "--clear"]).unwrap(),
            Action::Request(json!({"type": "engine_key", "engine": "oa", "secret": null}))
        );
    }

    #[test]
    fn the_other_commands() {
        assert_eq!(p(&["list"]).unwrap(), Action::List);
        assert_eq!(
            p(&["use", "oa"]).unwrap(),
            Action::Request(json!({"type": "set", "key": "engine", "value": "oa"}))
        );
        assert_eq!(
            p(&["test", "oa", "Hello", "there."]).unwrap(),
            Action::Request(json!({"type": "engine_test", "engine": "oa", "text": "Hello there."}))
        );
        assert_eq!(
            p(&["remove", "oa", "--keep-key"]).unwrap(),
            Action::Request(json!({"type": "engine_remove", "engine": "oa", "forget_key": false}))
        );
        assert!(p(&["remove"]).is_err());
        assert!(p(&["fly"]).is_err());
        assert!(p(&[]).is_err());
    }

    #[test]
    fn list_lines_say_where_the_text_goes() {
        let reply = json!({"engines": [
            {"id": "oa", "label": "OpenAI", "kind": "openai-compatible", "key_ref": "credman",
             "key_present": false, "local": false, "sends_text_to": "api.openai.com",
             "supported": true, "current": true,
             "status": {"ready": false, "status": "unavailable", "reason": "no_key", "fallback": "kokoro"}},
            {"id": "k", "label": "Kokoro FastAPI", "kind": "openai-compatible", "key_ref": "none",
             "key_present": false, "local": true, "sends_text_to": "127.0.0.1", "supported": true,
             "current": false, "status": {"ready": true, "status": "ready"}}]});
        assert_eq!(
            list_lines(&reply),
            vec![
                "* oa (OpenAI, openai-compatible): sends text to api.openai.com; no key; \
                 unavailable (no_key), reading with kokoro",
                "  k (Kokoro FastAPI, openai-compatible): runs on this PC; no key needed; ready",
            ]
        );
        assert_eq!(
            error_line(&json!({"error": {"code": "E_ENGINE", "message": "m", "reason": "auth"}})),
            "m (auth)"
        );
    }
}
