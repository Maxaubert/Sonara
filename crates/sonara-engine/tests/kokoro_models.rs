//! The Kokoro model manager against a local HTTP server: fresh download,
//! resume, hash mismatch, offline with backoff, pre-seeded folders.
#![cfg(feature = "kokoro")]
mod common;

use common::{sha256, Fault, FileServer, TempDir};
use sonara_engine::kokoro::download::{Backoff, Manager, ModelFile, Phase};
use std::path::Path;
use std::time::{Duration, Instant};

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

struct Fixture {
    model: Vec<u8>,
    voices: Vec<u8>,
    server: FileServer,
    dir: TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let model = bytes(300_000, 7);
        let voices = bytes(50_000, 3);
        let server = FileServer::start(&[
            ("model.onnx", model.clone()),
            ("voices.bin", voices.clone()),
        ]);
        Fixture {
            model,
            voices,
            server,
            dir: TempDir::new("models"),
        }
    }

    fn files(&self, base: &str) -> Vec<ModelFile> {
        vec![
            ModelFile {
                name: "model.onnx".into(),
                url: format!("{base}/model.onnx"),
                sha256: sha256(&self.model),
                size: self.model.len() as u64,
            },
            ModelFile {
                name: "voices.bin".into(),
                url: format!("{base}/voices.bin"),
                sha256: sha256(&self.voices),
                size: self.voices.len() as u64,
            },
        ]
    }

    fn manager(&self, backoff: Backoff) -> Manager {
        Manager::new(
            self.dir.path().join("kokoro/v1.0"),
            self.files(&self.server.base),
            backoff,
        )
    }

    fn file(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.path().join("kokoro/v1.0").join(name)).unwrap()
    }
}

fn quick() -> Backoff {
    Backoff {
        first: Duration::from_millis(200),
        max: Duration::from_secs(5),
    }
}

fn fetch(m: &Manager) -> Result<(), String> {
    assert!(m.begin(Instant::now()), "begin refused");
    m.fetch()
}

#[test]
fn model_download_fetches_verifies_and_moves_into_place() {
    let fx = Fixture::new();
    let m = fx.manager(quick());
    assert!(!m.present());
    assert_eq!(m.phase(), Phase::Idle);
    fetch(&m).unwrap();
    assert_eq!(m.phase(), Phase::Ready);
    assert!(m.present());
    assert_eq!(fx.file("model.onnx"), fx.model);
    assert_eq!(fx.file("voices.bin"), fx.voices);
    // No partial files are left, and a second run needs no request.
    let dir = fx.dir.path().join("kokoro/v1.0");
    assert!(!dir.join("model.onnx.part").exists());
    let before = fx.server.requests();
    fetch(&m).unwrap();
    assert_eq!(fx.server.requests(), before);
}

#[test]
fn model_download_resumes_after_a_broken_connection() {
    let fx = Fixture::new();
    fx.server.fault("model.onnx", Fault::CutAfter(120_000));
    let m = fx.manager(quick());
    let e = fetch(&m).unwrap_err();
    assert!(e.contains("model.onnx"), "{e}");
    let part = fx.dir.path().join("kokoro/v1.0/model.onnx.part");
    let kept = std::fs::metadata(&part).unwrap().len();
    assert!(kept > 0 && kept <= 120_000, "{kept}");
    assert!(!fx.dir.path().join("kokoro/v1.0/model.onnx").exists());
    // The retry (after the backoff) asks only for the rest.
    std::thread::sleep(Duration::from_millis(250));
    fetch(&m).unwrap();
    let seen = fx.server.seen();
    let model_requests: Vec<_> = seen
        .iter()
        .filter(|s| s.path.ends_with("model.onnx"))
        .collect();
    assert_eq!(model_requests.len(), 2);
    assert_eq!(model_requests[0].range, None);
    assert_eq!(model_requests[1].range, Some(format!("bytes={kept}-")));
    assert_eq!(fx.file("model.onnx"), fx.model);
}

#[test]
fn model_download_restarts_when_the_server_ignores_the_range() {
    let fx = Fixture::new();
    fx.server.fault("model.onnx", Fault::CutAfter(100_000));
    fx.server.fault("model.onnx", Fault::IgnoreRange);
    let m = fx.manager(quick());
    assert!(fetch(&m).is_err());
    std::thread::sleep(Duration::from_millis(250));
    fetch(&m).unwrap();
    assert_eq!(fx.file("model.onnx"), fx.model);
}

#[test]
fn model_download_hash_mismatch_keeps_nothing() {
    let fx = Fixture::new();
    fx.server.fault("voices.bin", Fault::Corrupt);
    let m = fx.manager(quick());
    let e = fetch(&m).unwrap_err();
    assert!(e.contains("hash mismatch"), "{e}");
    let dir = fx.dir.path().join("kokoro/v1.0");
    assert!(!dir.join("voices.bin").exists());
    assert!(!dir.join("voices.bin.part").exists());
    assert!(!m.present());
    assert!(matches!(m.phase(), Phase::Failed { .. }));
    // The model file that did verify is kept; the retry fetches the rest.
    std::thread::sleep(Duration::from_millis(250));
    let before = fx.server.requests();
    fetch(&m).unwrap();
    assert_eq!(fx.server.requests(), before + 1);
    assert!(m.present());
}

#[test]
fn model_download_offline_waits_out_the_backoff_without_a_retry_storm() {
    let fx = Fixture::new();
    for _ in 0..3 {
        fx.server.fault("model.onnx", Fault::Status(503));
    }
    let m = fx.manager(Backoff {
        first: Duration::from_millis(300),
        max: Duration::from_secs(10),
    });
    let e = fetch(&m).unwrap_err();
    assert!(e.contains("503"), "{e}");
    // Callers ask on every sentence; none of them may start a fetch yet.
    for _ in 0..100 {
        assert!(!m.begin(Instant::now()));
    }
    assert_eq!(fx.server.requests(), 1);
    let Phase::Failed { retry_at, reason } = m.phase() else {
        panic!("not failed: {:?}", m.phase())
    };
    assert!(reason.contains("503"));
    // After the wait one fetch starts; the second failure doubles the wait.
    std::thread::sleep(retry_at.saturating_duration_since(Instant::now()));
    assert!(fetch(&m).is_err());
    let Phase::Failed {
        retry_at: second, ..
    } = m.phase()
    else {
        panic!()
    };
    let wait = second.saturating_duration_since(Instant::now());
    assert!(wait > Duration::from_millis(400), "{wait:?}");
    assert_eq!(fx.server.requests(), 2);
}

#[test]
fn model_download_offline_with_no_server_fails_fast() {
    let fx = Fixture::new();
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/m", l.local_addr().unwrap())
    };
    let m = Manager::new(fx.dir.path().join("k"), fx.files(&closed), quick());
    let t = Instant::now();
    let e = fetch(&m).unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    assert!(e.contains("cannot download"), "{e}");
    assert!(!m.begin(Instant::now()));
}

fn seed(dir: &Path, name: &str, data: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(name), data).unwrap();
}

#[test]
fn a_pre_seeded_folder_is_used_without_downloading() {
    let fx = Fixture::new();
    let dir = fx.dir.path().join("kokoro/v1.0");
    seed(&dir, "model.onnx", &fx.model);
    seed(&dir, "voices.bin", &fx.voices);
    let m = fx.manager(quick());
    // Not verified yet: present() never hashes.
    assert!(!m.present());
    fetch(&m).unwrap();
    assert_eq!(fx.server.requests(), 0);
    assert!(m.present());
    // A new manager (the next run) trusts the recorded verification.
    assert!(fx.manager(quick()).present());
}

#[test]
fn a_wrong_pre_seeded_file_is_replaced() {
    let fx = Fixture::new();
    let dir = fx.dir.path().join("kokoro/v1.0");
    seed(&dir, "model.onnx", &bytes(300_000, 99));
    seed(&dir, "voices.bin", &fx.voices);
    let m = fx.manager(quick());
    fetch(&m).unwrap();
    assert_eq!(fx.file("model.onnx"), fx.model);
    assert_eq!(fx.server.requests(), 1);
    // Changing a verified file makes it unverified again.
    std::fs::write(dir.join("voices.bin"), b"tampered").unwrap();
    assert!(!fx.manager(quick()).present());
}

#[test]
fn progress_is_reported_while_downloading() {
    let fx = Fixture::new();
    let m = std::sync::Arc::new(fx.manager(quick()));
    assert!(m.begin(Instant::now()));
    let worker = {
        let m = m.clone();
        std::thread::spawn(move || m.fetch())
    };
    let mut seen_total = None;
    while !worker.is_finished() {
        if let Phase::Downloading { total, done } = m.phase() {
            assert!(done <= total);
            seen_total = Some(total);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    worker.join().unwrap().unwrap();
    if let Some(total) = seen_total {
        assert_eq!(total, 350_000);
    }
    assert_eq!(m.phase(), Phase::Ready);
}
