//! Code chunks through the handle (#295): read at the code speed, a code
//! line followed by its pause, and the step hint naming the next-line key.
mod common;

use common::{len, play, Rig};
use sonara_core::code_speech::{CODE_LINE, CODE_START};
use sonara_engine::fake::SAMPLE_RATE;
use sonara_reader::{CodeReading, QueueMode};

fn code(step: bool) -> CodeReading {
    CodeReading {
        speed: 80,
        pause_ms: 500,
        step,
        next_key: "Ctrl+Alt+Right".into(),
    }
}

#[test]
fn a_code_line_is_read_at_the_code_speed_with_its_pause_after_it() {
    let (r, _) = Rig::new();
    r.h.set_code_reading(code(false)).unwrap();
    assert_eq!(r.h.code_reading().unwrap(), code(false));
    r.h.speak(
        &format!("{CODE_LINE}Line 1. a."),
        QueueMode::Append,
        false,
        None,
    )
    .unwrap();
    let pause = (500 * SAMPLE_RATE / 1000) as usize;
    // The rig reads at 200 words per minute: code at 80 % is 160.
    assert_eq!(r.calls(1), [play(1, 0, 1, len("Line 1. a.", 160) + pause)]);
}

#[test]
fn prose_keeps_the_speech_rate_and_no_pause() {
    let (r, _) = Rig::new();
    r.h.set_code_reading(code(false)).unwrap();
    r.h.speak("Plain words.", QueueMode::Append, false, None)
        .unwrap();
    assert_eq!(r.calls(1), [play(1, 0, 1, len("Plain words.", 200))]);
}

#[test]
fn in_step_mode_the_announcement_names_the_next_line_key() {
    let (r, _) = Rig::new();
    r.h.set_code_reading(code(true)).unwrap();
    r.h.speak(
        &format!("{CODE_START}Python code block, 1 line."),
        QueueMode::Append,
        false,
        None,
    )
    .unwrap();
    let said = "Python code block, 1 line. Use Ctrl+Alt+Right for the next line.";
    assert_eq!(r.calls(1), [play(1, 0, 1, len(said, 160))]);
}

#[test]
fn out_of_range_code_settings_are_refused() {
    let (r, _) = Rig::new();
    let mut c = code(false);
    c.speed = 40;
    assert!(r.h.set_code_reading(c.clone()).is_err());
    c.speed = 80;
    c.pause_ms = 2_001;
    assert!(r.h.set_code_reading(c).is_err());
}
