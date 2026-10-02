//! Relays reader events to one network client.
//!
//! Each subscription gets its own reader subscription, drained on a thread
//! into a bounded queue that the transport reads. The reader never waits on
//! a client: when a client does not read and its queue is full, events are
//! dropped for it (each `state` event is a full snapshot, so the next one
//! brings it up to date). The thread ends with the reader, or at the first
//! event after the client went away. The `agent` extension's `earcon`
//! events come from the agent's own subscription, drained by a second
//! thread into the same queue; that thread also ends within a second of
//! its client going away.
use crate::agent_ext;
use crate::channels_ext::{self, Slot};
use crate::wire;
use sonara_reader::{EngineStatus, Event, ReaderHandle, State};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

/// Events queued per client before dropping.
pub const QUEUE: usize = 256;

/// How often the earcon thread checks that its client is still there.
const CLOSED_POLL: Duration = Duration::from_secs(1);

/// Which events a client asked for (`subscribe.events`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventSet {
    pub state: bool,
    pub items: bool,
    pub log: bool,
    /// `earcon` events (extension `agent`).
    pub earcons: bool,
}

impl EventSet {
    /// The core streams (a subscription without `events`).
    pub const ALL: EventSet = EventSet {
        state: true,
        items: true,
        log: true,
        earcons: false,
    };

    /// Parse protocol names (`state`, `items`, `log`, and `earcons` of the
    /// agent extension); the unknown name on error.
    pub fn parse<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<EventSet, String> {
        let mut set = EventSet::default();
        for n in names {
            match n {
                "state" => set.state = true,
                "items" => set.items = true,
                "log" => set.log = true,
                "earcons" => set.earcons = true,
                other => return Err(other.to_string()),
            }
        }
        Ok(set)
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.state {
            v.push("state");
        }
        if self.items {
            v.push("items");
        }
        if self.log {
            v.push("log");
        }
        if self.earcons {
            v.push("earcons");
        }
        v
    }
}

/// One event as sent: its name (the SSE `event:` field) and its JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireEvent {
    pub name: &'static str,
    pub json: String,
}

/// The engine id shown in `state.engine_status`, kept by the protocol layer.
pub type EngineName = Arc<Mutex<String>>;

pub fn engine_name(e: &EngineName) -> String {
    e.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// What one stream knows to build its `state` events: the reader's last
/// state, the engine's status, and how many status changes it has seen
/// (added to the reader's `seq`, so a status-only change is a new `seq`).
struct StateView {
    state: State,
    status: EngineStatus,
    bumps: u64,
    /// The last `seq` sent: older reader states (raced with the first
    /// snapshot) are not sent again.
    sent: Option<u64>,
}

impl StateView {
    fn seq(&self) -> u64 {
        self.state.seq + self.bumps
    }

    /// The `state` event, with the `channels` extension's fields once it is
    /// enabled; `None` if it would not move `seq` forward.
    fn render(&mut self, engine: &EngineName, channels: &Slot) -> Option<WireEvent> {
        let seq = self.seq();
        if self.sent.is_some_and(|s| seq <= s) {
            return None;
        }
        self.sent = Some(seq);
        let mut v = wire::state_event_with(&self.state, seq, &engine_name(engine), &self.status);
        if let Some(ch) = channels.get() {
            channels_ext::annotate_state(ch, &mut v);
        }
        Some(WireEvent {
            name: "state",
            json: v.to_string(),
        })
    }
}

fn render(
    e: Event,
    set: EventSet,
    view: &mut StateView,
    engine: &EngineName,
    channels: &Slot,
) -> Option<WireEvent> {
    let (name, value) = match e {
        Event::State(s) => {
            // An older state raced with the first snapshot: keep the newer.
            if s.seq <= view.state.seq && view.sent.is_some() {
                return None;
            }
            view.state = s;
            return set.state.then(|| view.render(engine, channels)).flatten();
        }
        Event::EngineStatus { status, .. } => {
            view.status = status;
            view.bumps += 1;
            return set.state.then(|| view.render(engine, channels)).flatten();
        }
        Event::Item { item_id, phase } if set.items => ("item", wire::item_event(item_id.0, phase)),
        Event::Log { message } if set.log => ("log", wire::log_event(&message)),
        _ => return None,
    };
    Some(WireEvent {
        name,
        json: value.to_string(),
    })
}

/// Start relaying `set` from `reader`. When `state` is asked for, the first
/// event is the current state, so a client never waits for a change to
/// draw its player.
pub fn subscribe(
    reader: &ReaderHandle,
    engine: EngineName,
    channels: Slot,
    agent: agent_ext::Slot,
    set: EventSet,
) -> sonara_reader::Result<mpsc::Receiver<WireEvent>> {
    let events = reader.subscribe()?;
    let (tx, rx) = mpsc::channel(QUEUE);
    if let (true, Some(a)) = (set.earcons, agent.get()) {
        let earcons = a.subscribe();
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("sonarad-earcons".into())
            .spawn(move || {
                // Earcons can be rare: wake up now and then to notice a
                // client that went away, so its thread does not wait for
                // the next earcon to end.
                loop {
                    let e = match earcons.recv_timeout(CLOSED_POLL) {
                        Ok(e) => e,
                        Err(RecvTimeoutError::Timeout) if !tx.is_closed() => continue,
                        Err(_) => return,
                    };
                    let w = WireEvent {
                        name: "earcon",
                        json: agent_ext::earcon_event(e).to_string(),
                    };
                    if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(w) {
                        return;
                    }
                }
            })
            .map_err(|e| sonara_reader::Error::Start(e.to_string()))?;
    }
    let mut view = StateView {
        state: reader.state()?,
        status: reader.engine_status()?,
        bumps: 0,
        sent: None,
    };
    if set.state {
        if let Some(w) = view.render(&engine, &channels) {
            let _ = tx.try_send(w);
        }
    }
    std::thread::Builder::new()
        .name("sonarad-events".into())
        .spawn(move || {
            while let Ok(e) = events.recv() {
                let Some(w) = render(e, set, &mut view, &engine, &channels) else {
                    continue;
                };
                if let Err(mpsc::error::TrySendError::Closed(_)) = tx.try_send(w) {
                    return;
                }
            }
        })
        .map_err(|e| sonara_reader::Error::Start(e.to_string()))?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonara_reader::{EngineId, Readiness};
    use std::sync::{Arc, Mutex};

    fn view(seq: u64) -> StateView {
        StateView {
            state: State {
                seq,
                now_playing: None,
                queued: 0,
                paused: false,
                muted: false,
                volume: 100,
                rate: 200,
                voice: None,
            },
            status: EngineStatus::ready(),
            bumps: 0,
            sent: None,
        }
    }

    fn seq_of(w: Option<WireEvent>) -> Option<u64> {
        w.map(|w| {
            let v: serde_json::Value = serde_json::from_str(&w.json).unwrap();
            v["seq"].as_u64().unwrap()
        })
    }

    #[test]
    fn engine_status_changes_are_state_events_with_a_new_seq() {
        let engine: EngineName = Arc::new(Mutex::new("kokoro".into()));
        let channels = Slot::default();
        let mut v = view(5);
        let set = EventSet::ALL;
        assert_eq!(seq_of(v.render(&engine, &channels)), Some(5));
        // An older reader state (raced with the snapshot) is not sent.
        let mut old = v.state.clone();
        old.seq = 4;
        assert_eq!(
            seq_of(render(Event::State(old), set, &mut v, &engine, &channels)),
            None
        );
        // A status change alone moves seq on and carries the status.
        let downloading = EngineStatus {
            readiness: Readiness::Downloading,
            progress: Some((10, 100)),
            fallback: Some(EngineId("onecore")),
            message: None,
        };
        let w = render(
            Event::EngineStatus {
                engine: EngineId("kokoro"),
                status: downloading,
            },
            set,
            &mut v,
            &engine,
            &channels,
        )
        .unwrap();
        let j: serde_json::Value = serde_json::from_str(&w.json).unwrap();
        assert_eq!(j["seq"], 6);
        assert_eq!(
            j["engine_status"],
            serde_json::json!({"engine": "kokoro", "ready": false, "status": "downloading",
                               "progress": {"done": 10, "total": 100}, "fallback": "onecore"})
        );
        // The next reader state keeps counting from there.
        let mut next = v.state.clone();
        next.seq = 6;
        assert_eq!(
            seq_of(render(Event::State(next), set, &mut v, &engine, &channels)),
            Some(7)
        );
        // Without the state stream nothing is sent.
        let none = EventSet::parse(["log"]).unwrap();
        assert!(render(
            Event::EngineStatus {
                engine: EngineId("kokoro"),
                status: EngineStatus::ready(),
            },
            none,
            &mut v,
            &engine,
            &channels,
        )
        .is_none());
    }

    #[test]
    fn event_names_parse_and_reject_unknown_ones() {
        assert_eq!(
            EventSet::parse(["state", "log"]).unwrap().names(),
            ["state", "log"]
        );
        assert_eq!(EventSet::parse(["items", "bogus"]), Err("bogus".into()));
        assert_eq!(
            EventSet::parse(["earcons", "state"]).unwrap().names(),
            ["state", "earcons"]
        );
        assert_eq!(EventSet::ALL.names(), ["state", "items", "log"]);
        assert_eq!(EventSet::parse([]).unwrap(), EventSet::default());
    }
}
