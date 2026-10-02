//! What `logs\sonarad.log` records for support besides the rare notes
//! (migration, settings that could not apply, custom earcons): one line at
//! every start (version, process, engine and its model status, home) and a
//! line whenever the engine's readiness changes (the Kokoro model finished
//! downloading or loading, or failed). Download progress is not logged.
use crate::home::Home;
use sonara_engine::{EngineStatus, Readiness};
use sonara_reader::{Event, ReaderHandle};

/// The line written once at startup.
pub fn startup_line(version: &str, pid: u32, engine: &str, status: &EngineStatus) -> String {
    format!("sonarad {version} started (pid {pid}): engine {engine} {status}")
}

/// Log the startup line now.
pub fn log_startup(home: &Home, version: &str, reader: &ReaderHandle, engine: &str) {
    let status = reader
        .engine_status()
        .unwrap_or_else(|_| EngineStatus::ready());
    home.log(&format!(
        "{}; home {}",
        startup_line(version, std::process::id(), engine, &status),
        home.dir.display()
    ));
}

/// Log the engine's readiness changes from now on, on a thread that ends
/// with the reader.
pub fn watch_engine(home: &Home, reader: &ReaderHandle) {
    let Ok(events) = reader.subscribe() else {
        return;
    };
    let mut last: Option<Readiness> = reader.engine_status().ok().map(|s| s.readiness);
    let home = home.clone();
    let _ = std::thread::Builder::new()
        .name("sonarad-support-log".into())
        .spawn(move || {
            while let Ok(e) = events.recv() {
                if let Event::EngineStatus { engine, status, .. } = e {
                    if last != Some(status.readiness) {
                        last = Some(status.readiness);
                        home.log(&format!("engine {engine}: {status}"));
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_audio::TestOutput;
    use sonara_engine::fake::FakeEngine;
    use sonara_reader::{Config, Registry};
    use std::sync::Arc;

    #[test]
    fn the_startup_line_names_version_engine_and_model_status() {
        let mut s = EngineStatus::ready();
        assert_eq!(
            startup_line("0.11.1", 42, "kokoro", &s),
            "sonarad 0.11.1 started (pid 42): engine kokoro ready"
        );
        s.readiness = Readiness::Downloading;
        s.progress = Some((40, 100));
        let line = startup_line("0.11.1", 42, "kokoro", &s);
        assert!(line.contains("downloading its model (40%)"), "{line}");
    }

    #[test]
    fn startup_creates_the_log_file() {
        let dir = std::env::temp_dir().join(format!("sonarad-support-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home {
            dir: dir.clone(),
            is_default: false,
        };
        let mut registry = Registry::default();
        registry.register(Arc::new(FakeEngine::new())).unwrap();
        let (out, rx) = TestOutput::new();
        let reader =
            ReaderHandle::new(Config::new(registry).with_output(Box::new(out), rx)).unwrap();
        log_startup(&home, "9.9.9", &reader, "fake");
        let text = std::fs::read_to_string(home.log_path()).unwrap();
        assert!(
            text.contains("sonarad 9.9.9 started") && text.contains("engine fake ready"),
            "{text}"
        );
        assert!(text.contains(&dir.display().to_string()), "{text}");
    }
}
