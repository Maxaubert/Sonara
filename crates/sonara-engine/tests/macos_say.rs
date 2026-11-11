//! The macOS `say` engine against a stand-in `say` (a shell script): the
//! command line, the text on stdin, the WAV it reads, cancel and failures.
//! No real speech and no audio device.
#![cfg(all(target_os = "macos", feature = "macos-say"))]
mod common;

use common::TempDir;
use sonara_engine::macos_say::{MacSay, ID};
use sonara_engine::{wav, Engine, Error, LicenseClass, PcmChunk};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LISTING: &str = "\
Albert              en_US    # Hello! My name is Albert.
Ava (Premium)       en_US    # Hello! My name is Ava.
Daniel              en_GB    # Hello! My name is Daniel.
";

/// A stand-in `say` in `dir`: lists `LISTING`, records its arguments and
/// stdin, then does what `dir/mode` says: `ok` copies `dir/fixture.wav` to
/// the `-o` file, `sleep` waits for good, `fail` prints an error.
fn fake_say(dir: &Path, mode: &str) -> PathBuf {
    std::fs::write(dir.join("listing"), LISTING).unwrap();
    std::fs::write(dir.join("mode"), mode).unwrap();
    let pcm = PcmChunk {
        samples: vec![1, -2, 3, -4],
        sample_rate: 22_050,
        channels: 1,
    };
    std::fs::write(dir.join("fixture.wav"), wav::encode(&pcm)).unwrap();
    let d = dir.display();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "-v" ] && [ "$2" = "?" ]; then cat "{d}/listing"; exit 0; fi
printf '%s\n' "$@" > "{d}/args"
cat > "{d}/stdin"
out=""
while [ $# -gt 0 ]; do
  if [ "$1" = "-o" ]; then out="$2"; fi
  shift
done
case "$(cat "{d}/mode")" in
  ok) cp "{d}/fixture.wav" "$out" ;;
  sleep) echo "$$" > "{d}/pid"; exec sleep 30 ;;
  fail) echo "Speaking failed: -241" >&2; exit 1 ;;
esac
"#
    );
    let path = dir.join("say");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn collect(e: &MacSay, text: &str, voice: &str, rate: u32) -> Result<Vec<PcmChunk>, Error> {
    e.synthesize(text, voice, rate)?.collect()
}

#[test]
fn say_gets_the_chosen_voice_the_mapped_rate_and_the_text_on_stdin() {
    let dir = TempDir::new("say-args");
    let e = MacSay::with_program(fake_say(dir.path(), "ok"));
    let pcm = collect(&e, "Build [[rate 1]] done.", "", 250).unwrap();
    assert_eq!(pcm.len(), 1);
    assert_eq!(pcm[0].samples, vec![1, -2, 3, -4]);
    assert_eq!(pcm[0].sample_rate, 22_050);
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(args[..4], ["-v", "Ava (Premium)", "-r", "219"]);
    assert_eq!(args[4], "-o");
    let out = PathBuf::from(args[5]);
    assert_eq!(args[6..], ["--data-format=LEI16@22050", "-f", "-"]);
    // The text arrives on stdin with its embedded command defused, and the
    // temp WAV is gone.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("stdin")).unwrap(),
        "Build [ [rate 1]] done."
    );
    assert!(!out.exists(), "{} was left behind", out.display());
}

#[test]
fn a_named_voice_is_passed_as_listed_and_an_unknown_one_is_refused() {
    let dir = TempDir::new("say-voice");
    let e = MacSay::with_program(fake_say(dir.path(), "ok"));
    collect(&e, "Hi.", "daniel", 200).unwrap();
    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    assert!(args.starts_with("-v\nDaniel\n-r\n175\n"), "{args}");
    assert_eq!(
        collect(&e, "Hi.", "Nobody", 200),
        Err(Error::UnknownVoice("Nobody".into()))
    );
}

#[test]
fn voices_are_the_listed_ones_as_os_voices() {
    let dir = TempDir::new("say-voices");
    let e = MacSay::with_program(fake_say(dir.path(), "ok"));
    assert_eq!(e.id(), ID);
    assert_eq!(e.id().as_str(), "macos");
    assert_eq!(e.license_class(), LicenseClass::Os);
    let voices = e.voices();
    let names: Vec<&str> = voices.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["Albert", "Ava (Premium)", "Daniel"]);
    assert!(voices
        .iter()
        .all(|v| v.engine == ID && v.installed && v.id == v.name));
    assert_eq!(voices[2].language, "en-GB");
    e.warm().unwrap();
}

#[test]
fn cancel_kills_the_running_say_and_ends_its_synthesis() {
    let dir = TempDir::new("say-cancel");
    let e = std::sync::Arc::new(MacSay::with_program(fake_say(dir.path(), "sleep")));
    let worker = {
        let e = e.clone();
        std::thread::spawn(move || collect(&e, "A long reply.", "", 200))
    };
    let pid_file = dir.path().join("pid");
    let start = Instant::now();
    while !pid_file.exists() {
        assert!(start.elapsed() < Duration::from_secs(10), "say never ran");
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .to_string();
    let t = Instant::now();
    e.cancel();
    assert_eq!(worker.join().unwrap(), Err(Error::Cancelled));
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    // The process is gone (killed and reaped), not left sleeping.
    let alive = std::process::Command::new("/bin/kill")
        .args(["-0", &pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive, "say (pid {pid}) still runs after cancel");
}

#[test]
fn a_failing_say_reports_its_message() {
    let dir = TempDir::new("say-fail");
    let e = MacSay::with_program(fake_say(dir.path(), "fail"));
    match collect(&e, "Hi.", "", 200) {
        Err(Error::Engine(m)) => {
            assert!(m.starts_with("macOS speech failed ("), "{m}");
            assert!(m.ends_with("Speaking failed: -241"), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_say_lists_no_voice_and_says_why_it_cannot_speak() {
    let dir = TempDir::new("say-missing");
    let e = MacSay::with_program(dir.path().join("no-say"));
    assert!(e.voices().is_empty());
    match collect(&e, "Hi.", "", 200) {
        Err(Error::Engine(m)) => {
            assert!(
                m.starts_with("macOS speech is unavailable: cannot run"),
                "{m}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(e.warm().is_err());
}
