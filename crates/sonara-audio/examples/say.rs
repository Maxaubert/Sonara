//! Speak text end to end: reader state machine + engine + audio output.
//!
//!   cargo run -p sonara-audio --example say -- --engine onecore "Hello there."
//!   cargo run -p sonara-audio --example say -- --engine fake "Hello there."
//!
//! Options: `--engine onecore|fake` (default onecore), `--voice <id or name>`,
//! `--rate <words per minute>` (default 200). Exits 1 when the text could not
//! be spoken, printing why (for OneCore on a PC with missing voice data, the
//! repair).
mod driver;

use driver::{Driver, Step};
use sonara_audio::RodioOutput;
use sonara_core::reader::{Effect, Event, ItemPhase};
use sonara_engine::{Engine, Registry};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

struct Args {
    engine: String,
    voice: Option<String>,
    rate: u32,
    text: String,
}

const USAGE: &str = "usage: say [--engine onecore|fake] [--voice NAME] [--rate WPM] TEXT...";

fn parse() -> Result<Args, String> {
    let mut args = Args {
        engine: "onecore".into(),
        voice: None,
        rate: 200,
        text: String::new(),
    };
    let mut words = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--engine" => args.engine = it.next().ok_or("--engine needs a value")?,
            "--voice" => args.voice = Some(it.next().ok_or("--voice needs a value")?),
            "--rate" => {
                let v = it.next().ok_or("--rate needs a value")?;
                args.rate = v.parse().map_err(|_| format!("bad --rate '{v}'"))?;
            }
            "-h" | "--help" => return Err(USAGE.into()),
            _ => words.push(a),
        }
    }
    args.text = words.join(" ");
    if args.text.trim().is_empty() {
        return Err(USAGE.into());
    }
    Ok(args)
}

fn engine(name: &str) -> Result<Arc<dyn Engine>, String> {
    let mut registry = Registry::default();
    #[cfg(windows)]
    registry
        .register(Arc::new(sonara_engine::onecore::OneCore::new()))
        .map_err(|e| e.to_string())?;
    registry
        .register(Arc::new(sonara_engine::fake::FakeEngine::new()))
        .map_err(|e| e.to_string())?;
    registry.get(name).map_err(|e| {
        let known: Vec<_> = registry.ids().iter().map(|id| id.as_str()).collect();
        format!("{e}; available: {}", known.join(", "))
    })
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let engine = match engine(&args.engine) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("say: {e}");
            return ExitCode::from(2);
        }
    };
    println!("engine: {}", engine.id());
    if let Err(e) = engine.warm() {
        // Keep going: the reader reports the failed chunks itself.
        eprintln!("say: engine is not ready: {e}");
    }

    let (output, events) = RodioOutput::new();
    let mut d = Driver::new(engine, output, events);
    d.with_reader(|r| r.set_rate(args.rate));
    d.with_reader(|r| r.set_voice(args.voice.clone()));
    let id = d.speak(&args.text);

    let mut shown = 0;
    let mut failed = false;
    loop {
        for step in &d.log[shown..] {
            match step {
                Step::Fx(Effect::Emit(Event::Item { item_id, phase })) if *item_id == id => {
                    println!("item {}: {phase:?}", item_id.0);
                    failed |= matches!(phase, ItemPhase::Failed | ItemPhase::Skipped);
                }
                Step::Fx(Effect::PlayChunk { chunk, .. }) => println!("playing chunk {chunk}"),
                Step::Audio(sonara_audio::AudioEvent::Failed { reason, .. }) => {
                    eprintln!("say: chunk failed: {reason}")
                }
                _ => {}
            }
        }
        shown = d.log.len();
        if d.idle() {
            break;
        }
        if !d.wait(Duration::from_secs(30)) {
            eprintln!("say: no audio event for 30 s, giving up");
            return ExitCode::from(1);
        }
    }
    if failed {
        eprintln!("say: the text was not spoken");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
