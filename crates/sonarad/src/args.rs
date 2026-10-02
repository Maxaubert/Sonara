//! Command line of `sonarad`.
use std::path::PathBuf;
use std::time::Duration;

pub const USAGE: &str = "usage: sonarad [--home DIR] [--engine onecore|fake] \
[--output device|null] [--idle-exit SECONDS] [--standalone] [--version]

  --home DIR          home folder (default: SONARA_HOME, else %LOCALAPPDATA%\\Sonara)
  --engine ID         engine to start with (default: onecore). 'fake' is a
                      deterministic tone engine for tests and conformance runs
  --output KIND       'device' (default; 'null' with --engine fake) or 'null',
                      a silent output that keeps real time
  --idle-exit SECONDS exit this long after the last client left and nothing is
                      playing (default 30)
  --standalone        never exit for idleness";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Device,
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub home: Option<PathBuf>,
    pub engine: String,
    pub output: OutputKind,
    pub idle_exit: Duration,
    pub standalone: bool,
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
    let mut engine = "onecore".to_string();
    let mut output = None;
    let mut idle_exit = crate::lifetime::DEFAULT_IDLE_EXIT;
    let mut standalone = false;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--home" => home = Some(PathBuf::from(value("--home")?)),
            "--engine" => engine = value("--engine")?,
            "--output" => {
                output = Some(match value("--output")?.as_str() {
                    "device" => OutputKind::Device,
                    "null" => OutputKind::Null,
                    other => return Err(format!("unknown --output '{other}'")),
                })
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
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    let output = output.unwrap_or(if engine == "fake" {
        OutputKind::Null
    } else {
        OutputKind::Device
    });
    Ok(Command::Run(Args {
        home,
        engine,
        output,
        idle_exit,
        standalone,
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
        assert_eq!(a.engine, "onecore");
        assert_eq!(a.output, OutputKind::Device);
        assert_eq!(a.idle_exit, Duration::from_secs(30));
        assert!(!a.standalone);
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
