//! The lead-in of a question whose message has no text (#283, Bug 2).
//!
//! Claude Code shows a message's thinking, so a message made of thinking
//! blocks and an `AskUserQuestion` looks like a normal reply; but only text
//! blocks reach the hook (`MessageDisplay`), so Sonara read the question
//! alone. For `PreToolUse AskUserQuestion` the hook reads the end of the
//! session's own transcript (`transcript_path`), finds the assistant
//! message that holds the tool use (`tool_use_id`), and when none of that
//! message's rows has a text block, takes its last thinking block with
//! text. Claude Code writes each content block as its own row; the rows
//! of one message share `message.id`.
//!
//! Only the last `TAIL` bytes are read, and a row is parsed only when it
//! holds the id it is looked for by. Whatever goes wrong (no file, an IO
//! or parse error, the tool use not in the tail, a subagent's call whose
//! tool use is in another transcript, the time budget spent) gives `None`:
//! the question is read as before. Nothing is stored.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

/// How much of the transcript's end is read (bytes).
pub const TAIL: u64 = 2 * 1024 * 1024;
/// The longest lead-in, in characters (clipped at a sentence end).
pub const THINKING_MAX: usize = 4000;
/// The wait before the one retry when the tool use is not on disk yet.
pub const RETRY_AFTER: Duration = Duration::from_millis(100);

/// The thinking to read before the question `tool_use_id`, from the
/// transcript at `path` (module docs). One retry after `RETRY_AFTER` when
/// the tool use is not found, if `deadline` allows it.
pub fn lead_in(path: &Path, tool_use_id: &str, deadline: Instant) -> Option<String> {
    if tool_use_id.is_empty() {
        return None;
    }
    match read_tail(path).map(|t| find(&t, tool_use_id)) {
        Some(Found::Message(lead)) => lead,
        Some(Found::NoToolUse) if Instant::now() + RETRY_AFTER < deadline => {
            std::thread::sleep(RETRY_AFTER);
            lead_in_from(&read_tail(path)?, tool_use_id)
        }
        _ => None,
    }
}

/// The end of the file, from its first whole line.
fn read_tail(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut raw = Vec::new();
    f.read_to_end(&mut raw).ok()?;
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    if start > 0 {
        // The first line is cut: drop it.
        let cut = text.find('\n').map_or(text.len(), |i| i + 1);
        text.drain(..cut);
    }
    Some(text)
}

enum Found {
    /// The tool use is not in the tail.
    NoToolUse,
    /// Its message: the lead-in, if it has no text.
    Message(Option<String>),
}

/// `lead_in` on the transcript's tail `tail` (pure).
pub fn lead_in_from(tail: &str, tool_use_id: &str) -> Option<String> {
    match find(tail, tool_use_id) {
        Found::Message(lead) => lead,
        Found::NoToolUse => None,
    }
}

/// The content blocks of an assistant row with `message.id`.
fn assistant(row: &Value) -> Option<(&str, &Vec<Value>)> {
    if row.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = row.get("message")?;
    let id = message.get("id").and_then(Value::as_str)?;
    let content = message.get("content").and_then(Value::as_array)?;
    Some((id, content))
}

fn kind(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

fn find(tail: &str, tool_use_id: &str) -> Found {
    let lines: Vec<&str> = tail.lines().collect();
    let message = lines
        .iter()
        .rev()
        .filter(|l| l.contains(tool_use_id))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|row| {
            let (id, content) = assistant(&row)?;
            content
                .iter()
                .any(|b| {
                    kind(b) == "tool_use" && b.get("id").and_then(Value::as_str) == Some(tool_use_id)
                })
                .then(|| id.to_string())
        });
    let Some(message) = message else {
        return Found::NoToolUse;
    };
    let mut thinking: Option<String> = None;
    for row in lines
        .iter()
        .filter(|l| l.contains(message.as_str()))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let Some((id, content)) = assistant(&row) else {
            continue;
        };
        if id != message {
            continue;
        }
        for b in content {
            let text = |key: &str| b.get(key).and_then(Value::as_str).unwrap_or("");
            match kind(b) {
                // The message has text: it was read as prose.
                "text" if !text("text").trim().is_empty() => return Found::Message(None),
                "thinking" if !text("thinking").trim().is_empty() => {
                    thinking = Some(text("thinking").trim().to_string());
                }
                _ => {}
            }
        }
    }
    Found::Message(thinking.and_then(|t| clip(&t)))
}

/// At most `THINKING_MAX` characters, cut after the last sentence end
/// within them (a hard cut when there is none). `None` when empty.
fn clip(text: &str) -> Option<String> {
    let text = text.trim();
    if text.chars().count() <= THINKING_MAX {
        return (!text.is_empty()).then(|| text.to_string());
    }
    let head: String = text.chars().take(THINKING_MAX).collect();
    let cut = head
        .rfind(['.', '!', '?'])
        .map_or(head.as_str(), |i| &head[..=i]);
    let cut = cut.trim();
    (!cut.is_empty()).then(|| cut.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TOOL: &str = "toolu_01EXJLaWWqDBnjEJ38tJ3meY";

    fn fixture() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/transcripts/thinking_only_question.jsonl")
    }

    fn row(message: &str, block: Value) -> String {
        json!({"type": "assistant", "message": {"id": message, "role": "assistant",
               "content": [block]}})
        .to_string()
    }

    fn tool_use(id: &str) -> Value {
        json!({"type": "tool_use", "id": id, "name": "AskUserQuestion", "input": {}})
    }

    fn thinking(t: &str) -> Value {
        json!({"type": "thinking", "thinking": t, "signature": "c2ln"})
    }

    fn lines(rows: &[String]) -> String {
        rows.join("\n") + "\n"
    }

    #[test]
    fn a_question_without_text_reads_the_last_thinking_block() {
        // The row shape of transcript rows 4834 to 4836 (2026-10-07):
        // [thinking "", thinking "answer", tool_use], one row each.
        let far = Instant::now() + Duration::from_secs(5);
        let lead = lead_in(&fixture(), TOOL, far).expect("the thinking");
        assert!(lead.starts_with("The card is next."), "{lead}");
        assert!(lead.ends_with("I will ask both now."), "{lead}");
        // Two thinking blocks with text: the last one.
        let tail = lines(&[
            row("m1", thinking("First thought.")),
            row("m1", thinking("Second thought.")),
            row("m1", tool_use("t1")),
        ]);
        assert_eq!(lead_in_from(&tail, "t1").as_deref(), Some("Second thought."));
    }

    #[test]
    fn a_question_with_a_text_block_reads_no_thinking() {
        let tail = lines(&[
            row("m1", thinking("Thinking.")),
            row("m1", json!({"type": "text", "text": "Here is my answer."})),
            row("m1", tool_use("t1")),
        ]);
        assert_eq!(lead_in_from(&tail, "t1"), None);
    }

    #[test]
    fn an_empty_or_signature_only_thinking_reads_nothing() {
        let tail = lines(&[
            row("m1", thinking("")),
            row("m1", thinking("   ")),
            row("m1", json!({"type": "redacted_thinking", "data": "xyz"})),
            row("m1", tool_use("t1")),
        ]);
        assert_eq!(lead_in_from(&tail, "t1"), None);
        // No thinking at all.
        assert_eq!(lead_in_from(&lines(&[row("m1", tool_use("t1"))]), "t1"), None);
    }

    #[test]
    fn a_tool_use_outside_the_tail_or_missing_file_falls_back_silently() {
        let soon = Instant::now();
        assert_eq!(lead_in(Path::new("Z:/no/such/transcript.jsonl"), TOOL, soon), None);
        // A subagent's call: its tool use is not in the main transcript.
        assert_eq!(lead_in(&fixture(), "toolu_of_a_subagent", soon), None);
        assert_eq!(lead_in(&fixture(), "", soon), None);
        // A broken row is skipped.
        let tail = "{not json toolu_x\n".to_string() + &lines(&[row("m1", tool_use("t2"))]);
        assert_eq!(lead_in_from(&tail, "toolu_x"), None);
    }

    #[test]
    fn long_thinking_is_clipped_at_a_sentence_end() {
        let sentence = "This sentence is exactly fifty characters long ok. ";
        let long = sentence.repeat(100);
        let tail = lines(&[row("m1", thinking(&long)), row("m1", tool_use("t1"))]);
        let lead = lead_in_from(&tail, "t1").unwrap();
        assert!(lead.chars().count() <= THINKING_MAX, "{}", lead.len());
        assert!(lead.ends_with("long ok."), "{lead}");
        // No sentence end at all: a hard cut.
        let words = "word ".repeat(1000);
        let tail = lines(&[row("m1", thinking(&words)), row("m1", tool_use("t1"))]);
        assert!(lead_in_from(&tail, "t1").unwrap().chars().count() <= THINKING_MAX);
    }

    #[test]
    fn rows_of_other_messages_are_ignored() {
        // Another message with text must not suppress the thinking, and
        // another message's thinking is not read.
        let tail = lines(&[
            row("m0", thinking("Old thought.")),
            row("m0", json!({"type": "text", "text": "An earlier reply."})),
            row("m1", thinking("This message's thought.")),
            row("m1", tool_use("t1")),
            row("m2", json!({"type": "text", "text": "A later reply."})),
        ]);
        assert_eq!(
            lead_in_from(&tail, "t1").as_deref(),
            Some("This message's thought.")
        );
    }

    #[test]
    fn only_the_tail_is_read_from_its_first_whole_line() {
        let dir = std::env::temp_dir().join(format!("sonara-transcript-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.jsonl");
        let filler = row("m0", json!({"type": "text", "text": "x".repeat(1000)})) + "\n";
        let mut body = filler.repeat((TAIL as usize / filler.len()) + 10);
        body.push_str(&lines(&[row("m1", thinking("At the end.")), row("m1", tool_use("t1"))]));
        std::fs::write(&path, body).unwrap();
        let soon = Instant::now();
        assert_eq!(lead_in(&path, "t1", soon).as_deref(), Some("At the end."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
