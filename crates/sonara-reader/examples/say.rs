//! Speak text end to end through the facade: `ReaderHandle` with an engine
//! and the default audio device.
//!
//!   cargo run -p sonara-reader --example say -- --engine onecore "Hello there."
//!   cargo run -p sonara-reader --example say -- --engine fake "Hello there."
//!
//! Options: `--engine onecore|fake` (default onecore), `--voice <id or name>`,
//! `--rate <words per minute>` (default 200). Exits 1 when the text could not
//! be spoken, printing why (for OneCore on a PC with missing voice data, the
//! repair).
use sonara_reader::{Config, Event, ItemPhase, QueueMode, ReaderHandle};
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

fn start(args: &Args) -> Result<ReaderHandle, String> {
    let registry = sonara_reader::default_registry();
    registry
        .register(Arc::new(sonara_engine::fake::FakeEngine::new()))
        .map_err(|e| e.to_string())?;
    let known: Vec<_> = registry.ids().iter().map(|id| id.as_str()).collect();
    let known = known.join(", ");
    let mut config = Config::new(registry);
    config.engine = Some(args.engine.clone());
    config.voice = args.voice.clone();
    config.rate = args.rate;
    ReaderHandle::new(config).map_err(|e| format!("{e}; engines: {known}"))
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let reader = match start(&args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("say: {e}");
            return ExitCode::from(2);
        }
    };
    let events = reader.subscribe().expect("reader is running");
    if let Ok(engine) = reader.get(sonara_reader::Key::Engine) {
        println!("engine: {engine:?}");
    }
    let id = reader
        .speak(&args.text, QueueMode::Append, false, None)
        .expect("reader is running");

    let mut chunk = None;
    let ok = loop {
        match events.recv_timeout(Duration::from_secs(30)) {
            Ok(Event::Item { item_id, phase }) if item_id == id => {
                println!("item {}: {phase:?}", item_id.0);
                match phase {
                    ItemPhase::Started => {}
                    ItemPhase::Finished => break true,
                    ItemPhase::Failed | ItemPhase::Skipped => break false,
                }
            }
            Ok(Event::State(s)) => {
                let now = s.now_playing.map(|n| n.chunk);
                if now.is_some() && now != chunk {
                    println!("playing chunk {}", now.unwrap_or_default());
                }
                chunk = now;
            }
            Ok(Event::Log { message }) => eprintln!("say: {message}"),
            Ok(_) => {}
            Err(_) => {
                eprintln!("say: no event for 30 s, giving up");
                break false;
            }
        }
    };
    reader.shutdown();
    if !ok {
        eprintln!("say: the text was not spoken");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
