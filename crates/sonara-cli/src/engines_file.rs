//! `sonara engines add <id> --kind command`: a `command` engine runs a
//! program on this PC, so it is never sent over the protocol (sonarad
//! refuses it with `E_FORBIDDEN`). The CLI, run by the user, writes the
//! profile into `<home>\engines.json` itself, then asks the runtime to read
//! the file again (`engine_reload`, which takes no profile). A runtime that
//! finds the entry unusable gets the previous file back.
//!
//! The checks here are the ones a typo needs at once (the program is the
//! full path of an `.exe` that exists, the text is never an argument);
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

/// The program of a `command` profile: an absolute path to an `.exe` that
/// exists; `{text}` nowhere in argv.
pub fn check_command(profile: &Value) -> Result<(), String> {
    let argv = profile["options"]["argv"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or(
            "a command engine needs --option 'argv=[\"C:/path/to/program.exe\", ...]' \
             (a JSON list: the program's full path, then its arguments)",
        )?;
    let mut args = Vec::with_capacity(argv.len());
    for a in argv {
        args.push(a.as_str().ok_or("argv must be a list of texts")?);
    }
    let program = Path::new(args[0]);
    let exe = program
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if !program.is_absolute() || !exe {
        return Err(format!(
            "argv[0] '{}' must be the full path of an .exe, such as C:\\Tools\\tts.exe",
            args[0]
        ));
    }
    if !program.is_file() {
        return Err(format!("the program '{}' does not exist", args[0]));
    }
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
    doc.insert("format".into(), json!(1));
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
        for (argv, msg) in [
            (json!(["tts.exe"]), "full path of an .exe"),
            (json!(["C:\\Tools\\say.bat"]), "full path of an .exe"),
            (json!([missing]), "does not exist"),
            (json!([exe(), "--say", "{text}"]), "{text} is not allowed"),
            (json!([]), "needs --option"),
        ] {
            let e = write(&h, &piper("x", argv.clone()), false).unwrap_err();
            assert!(e.contains(msg), "{argv}: {e}");
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
