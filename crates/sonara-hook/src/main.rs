//! `sonara-hook.exe <Event>`: the Claude Code hook adapter (see the library
//! docs). A GUI-subsystem program, so no console window flashes up; it
//! never fails the Claude session (exit code 0, errors swallowed). It
//! starts `sonarad.exe` from its own folder when no runtime answers.
#![windows_subsystem = "windows"]

use serde_json::Value;
use sonara_client::{deliver, home, runtime_args, runtime_exe, spawn_detached, Delivery};
use sonara_hook::{
    debug_log, follow_job, follow_message, log, log_line, map_event_full, outcome, stamp,
    transcript, FOLLOW_ARG, FOLLOW_FOR, HELLO, START_BUDGET,
};
use std::io::Read;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long one hook may spend reaching the runtime and hearing back.
const TIMEOUT: Duration = Duration::from_secs(2);

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn run(t0: f64, started: Instant) {
    // Hooks fired inside the summarizer's own headless session (it sets
    // SONARA_SUMMARIZER) are not a user session: send nothing, or the
    // runtime would summarize its own summarizer.
    if env("SONARA_SUMMARIZER").is_some_and(|v| !v.is_empty()) {
        return;
    }
    let event = std::env::args().nth(1).unwrap_or_default();
    if event == FOLLOW_ARG {
        follow(&std::env::args().nth(2).unwrap_or_default(), started);
        return;
    }
    let mut raw = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut raw);
    // Optional capture of the raw payload (golden fixtures), before any
    // other work.
    if let Some(dir) = env("SONARA_CAPTURE").filter(|d| !d.is_empty()) {
        let dir = std::path::PathBuf::from(dir);
        let name = if event.is_empty() { "unknown" } else { &event };
        let _ = std::fs::create_dir_all(&dir).and_then(|_| {
            std::fs::write(
                dir.join(format!("{name}-{}.json", std::process::id())),
                &raw,
            )
        });
    }
    let payload: Value = std::str::from_utf8(&raw)
        .ok()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .and_then(|t| serde_json::from_str(t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()));
    // A question's lead-in (#283): one look; a question not on disk yet
    // gets its lead-in from the follow-up (#360).
    let lead_in = |path: &std::path::Path, id: &str| transcript::look(path, id);
    let before_reply = |path: &std::path::Path| transcript::before_reply(path);
    let mut msgs = map_event_full(&event, &payload, &env, &lead_in, &before_reply);
    stamp(&mut msgs, t0);
    let Some(home) = home(&env) else {
        return;
    };
    let delivery = if msgs.is_empty() {
        None
    } else {
        let exe = if env("SONARA_NO_START").is_some_and(|v| !v.is_empty()) {
            None
        } else {
            std::env::current_exe().ok().and_then(|me| runtime_exe(&me))
        };
        Some(deliver(
            &home,
            &HELLO,
            &msgs,
            exe.as_deref(),
            &runtime_args(&env),
            started + START_BUDGET,
            TIMEOUT,
        ))
    };
    let delivered = matches!(delivery, Some(Delivery::Sent | Delivery::Started));
    if let Some(job) = follow_job(&payload, &msgs).filter(|_| delivered) {
        if let Ok(me) = std::env::current_exe() {
            let _ = spawn_detached(&me, &[FOLLOW_ARG.to_string(), job.to_string()]);
        }
    }
    // The troubleshooting log (#219), last, so it never delays delivery.
    let line = log_line(
        &event,
        &raw,
        &msgs,
        outcome(delivery),
        started.elapsed(),
        debug_log(&home),
    );
    log(&home, &line);
}

/// The lead-in follow-up of a question (#360): wait for the question to
/// reach the transcript, then send its lead-in (empty when there is none,
/// so the runtime stops holding the question). Never starts the runtime.
fn follow(job: &str, started: Instant) {
    let Ok(job) = serde_json::from_str::<Value>(job) else {
        return;
    };
    let Some(home) = home(&env) else {
        return;
    };
    let path = job["transcript_path"].as_str().unwrap_or("");
    let id = job["tool_use_id"].as_str().unwrap_or("");
    let lead = transcript::lead_in(std::path::Path::new(path), id, started + FOLLOW_FOR);
    let msgs = [follow_message(&job, lead.as_deref())];
    let delivery = deliver(
        &home,
        &HELLO,
        &msgs,
        None,
        &[],
        Instant::now() + START_BUDGET,
        TIMEOUT,
    );
    let line = log_line(
        "FollowLeadIn",
        b"{}",
        &msgs,
        outcome(Some(delivery)),
        started.elapsed(),
        debug_log(&home),
    );
    log(&home, &line);
}

fn main() {
    // The process start time (#174): the runtime drops text stamped before
    // the channel's last new turn.
    let t0 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let started = Instant::now();
    let _ = std::panic::catch_unwind(|| run(t0, started));
    std::process::exit(0);
}
