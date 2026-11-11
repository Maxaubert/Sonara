//! `sonara engines add <id> --kind command`: a `command` engine runs a
//! program on this PC, so it is never sent over the protocol (sonarad
//! refuses it with `E_FORBIDDEN`). The CLI, run by the user, writes the
//! profile into `<home>\engines.json` itself, then asks the runtime to read
//! the file again (`engine_reload`, which takes no profile). A runtime that
//! finds the entry unusable gets the previous file back.
//!
//! The checks here are the ones a typo needs at once (the program is the
//! full path of an `.exe` that exists, on unix of an executable file; the
//! text is never an argument);
//! sonarad validates the whole profile on reload.
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub const FILE: &str = "engines.json";
/// As sonarad (`MAX_PROFILES`).
const MAX_PROFILES: usize = 16;

/// What `write` changed: the file and its text before, to put it back.
#[derive(Debug)]
pub struct Written {
    pub file: PathBuf,
    pub before: Option<String>,
}

impl Written {
    /// Put the previous file back (or remove the new one).
    pub fn undo(&self) -> Result<(), String> {
        match &self.before {
            Some(text) => write_atomic(&self.file, text),
            None => std::fs::remove_file(&self.file)
                .map_err(|e| format!("cannot remove {}: {e}", self.file.display())),
        }
    }
}

/// A `command` program as the messages name it, and an example.
#[cfg(windows)]
const PROGRAM: &str = "an .exe";
#[cfg(windows)]
const EXAMPLE: &str = "C:\\Tools\\tts.exe";
#[cfg(windows)]
const ARGV_NEEDED: &str =
    "a command engine needs --option 'argv=[\"C:/path/to/program.exe\", ...]' \
     (a JSON list: the program's full path, then its arguments)";
#[cfg(not(windows))]
const PROGRAM: &str = "an executable file";
#[cfg(not(windows))]
const EXAMPLE: &str = "/usr/local/bin/piper";
#[cfg(not(windows))]
const ARGV_NEEDED: &str = "a command engine needs --option 'argv=[\"/path/to/program\", ...]' \
     (a JSON list: the program's full path, then its arguments)";

/// The program of a `command` profile: an absolute path to an `.exe` (on
/// unix an executable file, a `#!` script too) that exists; `{text}`
/// nowhere in argv.
pub fn check_command(profile: &Value) -> Result<(), String> {
    let argv = profile["options"]["argv"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or(ARGV_NEEDED)?;
    let mut args = Vec::with_capacity(argv.len());
    for a in argv {
        args.push(a.as_str().ok_or("argv must be a list of texts")?);
    }
    let program = Path::new(args[0]);
    // Windows: only an .exe (a .bat or .cmd runs through cmd.exe). Unix:
    // any executable file (execve reads no shell syntax in the arguments).
    let kind_ok = !cfg!(windows)
        || program
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if !program.is_absolute() || !kind_ok {
        return Err(format!(
            "argv[0] '{}' must be the full path of {PROGRAM}, such as {EXAMPLE}",
            args[0]
        ));
    }
    let meta = match std::fs::metadata(program) {
        Ok(m) if m.is_file() => m,
        _ => return Err(format!("the program '{}' does not exist", args[0])),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "the program '{}' is not executable (chmod +x it)",
                args[0]
            ));
        }
    }
    let _ = meta;
    if args.iter().any(|a| a.contains("{text}")) {
        return Err(
            "{text} is not allowed in argv: the program reads the text on stdin \
             (the default) or from the file at {in} (--option input=file)"
                .into(),
        );
    }
    Ok(())
}

/// Write a temp file next to `file`, then rename it over `file`.
fn write_atomic(file: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let tmp = file.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", file.display())
    })
}

/// Add (or with `replace`, change) `profile` in `<home>\engines.json`,
/// keeping every other entry as it is.
pub fn write(home: &Path, profile: &Value, replace: bool) -> Result<Written, String> {
    check_command(profile)?;
    let id = profile["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("the engine needs an id")?;
    let file = home.join(FILE);
    let before = std::fs::read_to_string(&file).ok();
    let mut doc: Map<String, Value> = match &before {
        None => Map::new(),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => m,
            _ => {
                return Err(format!(
                    "{} is not valid JSON: fix it or move it away first",
                    file.display()
                ))
            }
        },
    };
    let mut engines = doc
        .get("engines")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match engines.iter().position(|e| e["id"] == id) {
        Some(_) if !replace => return Err(format!("engine '{id}' exists; add --replace")),
        Some(i) => engines[i] = profile.clone(),
        None if engines.len() >= MAX_PROFILES => {
            return Err(format!("at most {MAX_PROFILES} engines"))
        }
        None => engines.push(profile.clone()),
    }
    // Never lower the format (spec 6.4): a format 2 file written back as
    // 1 would be migrated again at the next start. A new file is 2; a file
    // the runtime has not migrated yet keeps its 1.
    let format = match &before {
        None => 2,
        Some(_) => doc.get("format").and_then(Value::as_u64).unwrap_or(1),
    };
    doc.insert("format".into(), json!(format));
    doc.insert("engines".into(), Value::Array(engines));
    let text = serde_json::to_string_pretty(&Value::Object(doc)).map_err(|e| e.to_string())?;
    write_atomic(&file, &text)?;
    Ok(Written { file, before })
}

/// The view of `id` in an `engine_reload` reply, or why it cannot be used.
pub fn reloaded_view(reply: &Value, id: &str) -> Result<Value, String> {
    let view = reply["engines"]
        .as_array()
        .and_then(|a| a.iter().find(|e| e["id"] == id))
        .cloned()
        .ok_or_else(|| format!("the runtime did not take engine '{id}'"))?;
    match view["error"].as_str() {
        Some(err) => Err(format!("engine '{id}' cannot be used: {err}")),
        None => Ok(view),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sonara-cli-ef-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn exe() -> String {
        std::env::current_exe().unwrap().display().to_string()
    }

    fn piper(id: &str, argv: Value) -> Value {
        json!({"id": id, "kind": "command", "options": {"argv": argv}})
    }

    #[test]
    fn writes_the_profile_and_keeps_the_others() {
        let h = home("write");
        std::fs::write(
            h.join(FILE),
            r#"{"format": 1, "engines": [{"id": "oa", "kind": "openai-compatible"}]}"#,
        )
        .unwrap();
        let meta = "a & b | c > d %PATH% ^ \"q\"";
        let p = piper("say", json!([exe(), meta, "{voice}"]));
        let w = write(&h, &p, false).unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(h.join(FILE)).unwrap()).unwrap();
        assert_eq!(v["engines"][0]["id"], "oa");
        assert_eq!(v["engines"][1], p, "argv is stored as given");
        assert_eq!(v["engines"][1]["options"]["argv"][1], meta);
        assert_eq!(
            write(&h, &p, false).unwrap_err(),
            "engine 'say' exists; add --replace"
        );
        let p2 = piper("say", json!([exe()]));
        write(&h, &p2, true).unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(h.join(FILE)).unwrap()).unwrap();
        assert_eq!(v["engines"][1], p2);
        assert_eq!(v["engines"].as_array().unwrap().len(), 2);
        // Undo puts the file back as it was before that write.
        w.undo().unwrap();
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(h.join(FILE)).unwrap()).unwrap();
        assert_eq!(v["engines"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn the_file_format_is_never_lowered() {
        // A format 2 file written back as format 1 would be migrated again
        // at the next start, binding env keys to addresses a protocol
        // client set (spec 6.4). A new file is format 2; a format 1 file
        // stays 1 so the runtime still migrates it once.
        let format = |h: &Path| -> Value {
            serde_json::from_str::<Value>(&std::fs::read_to_string(h.join(FILE)).unwrap()).unwrap()
                ["format"]
                .clone()
        };
        let h = home("format");
        write(&h, &piper("a", json!([exe()])), false).unwrap();
        assert_eq!(format(&h), 2, "a new file");
        std::fs::write(
            h.join(FILE),
            r#"{"format": 2, "engines": [{"id": "oa", "kind": "openai-compatible"}]}"#,
        )
        .unwrap();
        write(&h, &piper("b", json!([exe()])), false).unwrap();
        assert_eq!(format(&h), 2, "a format 2 file");
        std::fs::write(h.join(FILE), r#"{"engines": []}"#).unwrap();
        write(&h, &piper("c", json!([exe()])), false).unwrap();
        assert_eq!(format(&h), 1, "an unmigrated file");
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn a_new_file_is_removed_by_undo() {
        let h = home("new");
        let w = write(&h, &piper("say", json!([exe()])), false).unwrap();
        assert!(h.join(FILE).exists());
        w.undo().unwrap();
        assert!(!h.join(FILE).exists());
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn the_checks_a_typo_needs() {
        let h = home("checks");
        let missing = h.join("no-such-tts.exe").display().to_string();
        let full_path = format!("full path of {PROGRAM}");
        for (argv, msg) in [
            (json!(["tts.exe"]), full_path.as_str()),
            (json!(["C:\\Tools\\say.bat"]), full_path.as_str()),
            (json!([missing]), "does not exist"),
            (json!([exe(), "--say", "{text}"]), "{text} is not allowed"),
            (json!([]), "needs --option"),
        ] {
            let e = write(&h, &piper("x", argv.clone()), false).unwrap_err();
            assert!(e.contains(msg), "{argv}: {e}");
        }
        // Unix: a file without its execute bit is no program; a `#!`
        // script with one is.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = h.join("say.sh");
            std::fs::write(&script, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
            let path = script.display().to_string();
            assert_eq!(
                check_command(&piper("x", json!([path]))).unwrap_err(),
                format!("the program '{path}' is not executable (chmod +x it)")
            );
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(check_command(&piper("x", json!([path]))).is_ok());
        }
        assert!(!h.join(FILE).exists(), "nothing written");
        std::fs::write(h.join(FILE), "{not json").unwrap();
        assert!(write(&h, &piper("x", json!([exe()])), false)
            .unwrap_err()
            .contains("is not valid JSON"));
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn the_reload_reply_says_whether_it_is_usable() {
        let reply = json!({"engines": [
            {"id": "ok", "kind": "command"},
            {"id": "bad", "kind": "command", "error": "argv[0] must be an .exe"}]});
        assert_eq!(reloaded_view(&reply, "ok").unwrap()["id"], "ok");
        assert_eq!(
            reloaded_view(&reply, "bad").unwrap_err(),
            "engine 'bad' cannot be used: argv[0] must be an .exe"
        );
        assert!(reloaded_view(&reply, "gone").is_err());
    }
}
