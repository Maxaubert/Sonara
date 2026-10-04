//! Command line of `sonarad`.
use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "usage: sonarad [--home DIR] [--engine kokoro|onecore|fake|ID] \
[--output device|null] [--system windows|fake] [--idle-exit SECONDS] [--standalone]
       [--keys windows|fake] [--no-external-engines] [--migrate-from DIR] [--version]

  --home DIR          home folder (default: SONARA_HOME, else %LOCALAPPDATA%\\Sonara)
  --engine ID         engine to start with (default: the saved one, else kokoro
                      when onnxruntime.dll is next to sonarad.exe, else
                      onecore). Kokoro downloads its model on first use and
                      speaks with onecore until it is ready. 'fake' is a
                      deterministic tone engine for tests and conformance runs
                      (no Kokoro, no download); a saved external engine still
                      applies, with the fake engine as its fallback
  --output KIND       'device' (default; 'null' with --engine fake) or 'null',
                      a silent output that keeps real time
  --system KIND       platform of the 'system' extension: 'windows' (default)
                      or 'fake', a testing aid that keeps fake apps, media
                      and hotkeys in <home>\\fake-system.json
  --keys KIND         where external engine keys are kept: 'windows'
                      (default, Windows Credential Manager) or 'fake', a
                      testing aid that keeps them in <home>\\fake-keys.json
  --no-external-engines
                      refuse external engines (speech servers and cloud
                      services the user adds): no 'engines' capability
  --idle-exit SECONDS exit this long after the last client left and nothing is
                      playing (default 30)
  --standalone        never exit for idleness
  --migrate-from DIR  import the Python plugin's settings from DIR when the
                      home has no config.json yet (default: %USERPROFILE%\\.sonara,
                      and only for the default home)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Device,
    Null,
}

/// Where external engine keys are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeysKind {
    /// Windows Credential Manager.
    Windows,
    /// `<home>\fake-keys.json` (tests).
    Fake,
}

/// The platform behind the `system` extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemKind {
    Windows,
    /// Fake apps, media and hotkeys in a JSON file in the home (tests).
    Fake,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub home: Option<PathBuf>,
    /// `--engine`: wins over the saved engine. `None`: the saved one, else
    /// kokoro if ONNX Runtime is installed, else onecore.
    pub engine: Option<String>,
    /// `--migrate-from DIR`.
    pub migrate_from: Option<PathBuf>,
    pub output: OutputKind,
    pub system: SystemKind,
    pub idle_exit: Duration,
    pub standalone: bool,
    pub keys: KeysKind,
    /// False with `--no-external-engines`.
    pub external_engines: bool,
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(Args),
    Help,
    Version,
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut home = None;
    let mut engine = None;
    let mut migrate_from = None;
    let mut output = None;
    let mut system = SystemKind::Windows;
    let mut idle_exit = crate::lifetime::DEFAULT_IDLE_EXIT;
    let mut standalone = false;
    let mut keys = KeysKind::Windows;
    let mut external_engines = true;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--home" => home = Some(PathBuf::from(value("--home")?)),
            "--engine" => engine = Some(value("--engine")?),
            "--migrate-from" => migrate_from = Some(PathBuf::from(value("--migrate-from")?)),
            "--output" => {
                output = Some(match value("--output")?.as_str() {
                    "device" => OutputKind::Device,
                    "null" => OutputKind::Null,
                    other => return Err(format!("unknown --output '{other}'")),
                })
            }
            "--system" => {
                system = match value("--system")?.as_str() {
                    "windows" => SystemKind::Windows,
                    "fake" => SystemKind::Fake,
                    other => return Err(format!("unknown --system '{other}'")),
                }
            }
            "--idle-exit" => {
                let v = value("--idle-exit")?;
                let secs: f64 = v
                    .parse()
                    .ok()
                    .filter(|s: &f64| s.is_finite() && *s >= 0.0)
                    .ok_or(format!("bad --idle-exit '{v}'"))?;
                idle_exit = Duration::from_secs_f64(secs);
            }
            "--standalone" => standalone = true,
            "--keys" => {
                keys = match value("--keys")?.as_str() {
                    "windows" => KeysKind::Windows,
                    "fake" => KeysKind::Fake,
                    other => return Err(format!("unknown --keys '{other}'")),
                }
            }
            "--no-external-engines" => external_engines = false,
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    let output = output.unwrap_or(if engine.as_deref() == Some("fake") {
        OutputKind::Null
    } else {
        OutputKind::Device
    });
    Ok(Command::Run(Args {
        home,
        engine,
        migrate_from,
        output,
        system,
        idle_exit,
        standalone,
        keys,
        external_engines,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Args {
        match parse(args.iter().map(|s| s.to_string())).unwrap() {
            Command::Run(a) => a,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn defaults() {
        let a = run(&[]);
        assert_eq!(a.engine, None);
        assert_eq!(a.output, OutputKind::Device);
        assert_eq!(a.idle_exit, Duration::from_secs(30));
        assert!(!a.standalone);
        assert_eq!(a.system, SystemKind::Windows);
        assert_eq!(a.migrate_from, None);
        assert_eq!(a.keys, KeysKind::Windows);
        assert!(a.external_engines);
    }

    #[test]
    fn keys_and_external_engines_flags() {
        let a = run(&["--keys", "fake", "--no-external-engines"]);
        assert_eq!(a.keys, KeysKind::Fake);
        assert!(!a.external_engines);
        assert!(parse(["--keys".to_string(), "vault".to_string()]).is_err());
    }

    #[test]
    fn engine_and_migration_flags() {
        let a = run(&["--engine", "fake", "--migrate-from", "old"]);
        assert_eq!(a.engine.as_deref(), Some("fake"));
        assert_eq!(a.migrate_from, Some(PathBuf::from("old")));
        assert!(parse(["--migrate-from".to_string()]).is_err());
    }

    #[test]
    fn the_engine_can_be_named() {
        assert_eq!(
            run(&["--engine", "kokoro"]).engine.as_deref(),
            Some("kokoro")
        );
        assert_eq!(run(&["--engine", "onecore"]).output, OutputKind::Device);
    }

    #[test]
    fn the_system_platform_can_be_the_fake() {
        assert_eq!(run(&["--system", "fake"]).system, SystemKind::Fake);
        assert!(parse(["--system".to_string(), "linux".to_string()]).is_err());
    }

    #[test]
    fn the_fake_engine_defaults_to_the_null_output() {
        let a = run(&["--engine", "fake", "--idle-exit", "0.5", "--home", "x"]);
        assert_eq!(a.output, OutputKind::Null);
        assert_eq!(a.idle_exit, Duration::from_millis(500));
        assert_eq!(a.home, Some(PathBuf::from("x")));
        let a = run(&["--engine", "fake", "--output", "device"]);
        assert_eq!(a.output, OutputKind::Device);
    }

    #[test]
    fn bad_arguments_are_errors() {
        let p = |v: &[&str]| parse(v.iter().map(|s| s.to_string()));
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--home"]).is_err());
        assert!(p(&["--idle-exit", "-1"]).is_err());
        assert!(p(&["--output", "speaker"]).is_err());
        assert_eq!(p(&["--version"]).unwrap(), Command::Version);
    }
}
