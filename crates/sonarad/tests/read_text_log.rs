//! The `read text` lines (#219) hold exactly what went to the voice: a
//! real reader (fake engine, silent real-time output) reads three items
//! through a channel, and the cleaned chunks the engine was given are the
//! chunks the log names, in order.
use serde_json::Value;
use sonara_channels::{Channels, Config as ChannelsConfig, QueueMode};
use sonara_engine::fake::FakeEngine;
use sonara_reader::{Config, ReaderHandle, Registry};
use sonarad::home::Home;
use sonarad::null_output::NullOutput;
use sonarad::support_log::watch_reading;
use sonarad::trace_log::Origins;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn the_text_lines_are_what_the_engine_was_given() {
    let dir = std::env::temp_dir().join(format!("sonarad-read-text-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let home = Home {
        dir: dir.clone(),
        is_default: false,
    };
    let engine = Arc::new(FakeEngine::new());
    let mut registry = Registry::default();
    registry.register(engine.clone()).unwrap();
    let (out, events) = NullOutput::new();
    let reader =
        ReaderHandle::new(Config::new(registry).with_output(Box::new(out), events)).unwrap();
    let channels = Channels::new(
        reader.clone(),
        ChannelsConfig {
            announce: false,
            ..Default::default()
        },
    )
    .unwrap();
    let tags = channels.clone();
    watch_reading(
        &home,
        &reader,
        Box::new(move |id| tags.tag(id)),
        Origins::default(),
    );
    for text in [
        "Hello there. This is **bold** text.",
        "Second item with `code` and a [link](https://example.com).",
        "# Third\n\n- one\n- two",
    ] {
        channels
            .speak("c1", text, Some(QueueMode::Append), false, None)
            .unwrap();
    }
    let end = Instant::now() + Duration::from_secs(30);
    let log = loop {
        let log = std::fs::read_to_string(home.log_path()).unwrap_or_default();
        if log.matches("read end ").count() >= 3 {
            break log;
        }
        assert!(Instant::now() < end, "not read in time: {log}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut logged = Vec::new();
    let mut chunks = 0;
    for line in log.lines().filter(|l| l.contains(" read text ")) {
        assert!(
            line.contains("finished") || line.contains("chunks="),
            "{line}"
        );
        let (head, text) = line.split_once(" text=").expect("a text field");
        let n: usize = head
            .rsplit_once("chunks=")
            .and_then(|(_, c)| c.split_once('/'))
            .map(|(read, _)| read.parse().unwrap())
            .unwrap();
        chunks += n;
        let text: Value = serde_json::from_str(text).unwrap();
        logged.push(text.as_str().unwrap().to_string());
    }
    assert_eq!(logged.len(), 3, "{log}");
    let given = engine.texts();
    assert_eq!(chunks, given.len(), "one chunk per synthesis: {given:?}");
    assert_eq!(logged.join(" "), given.join(" "), "{log}");
    assert!(
        !logged.join(" ").contains("**"),
        "the cleaned text: {logged:?}"
    );
    reader.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
