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
use sonara_reader::{Event, ReaderHandle, State};
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

/// The `state` event, with the `channels` extension's fields once it is
/// enabled.
fn state_json(s: &State, engine: &EngineName, channels: &Slot) -> serde_json::Value {
    let mut v = wire::state_event(s, &engine_name(engine));
    if let Some(ch) = channels.get() {
        channels_ext::annotate_state(ch, &mut v);
    }
    v
}

fn render(e: &Event, set: EventSet, engine: &EngineName, channels: &Slot) -> Option<WireEvent> {
    let (name, value) = match e {
        Event::State(s) if set.state => ("state", state_json(s, engine, channels)),
        Event::Item { item_id, phase } if set.items => {
            ("item", wire::item_event(item_id.0, *phase))
        }
        Event::Log { message } if set.log => ("log", wire::log_event(message)),
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
    if set.state {
        let s = reader.state()?;
        let _ = tx.try_send(WireEvent {
            name: "state",
            json: state_json(&s, &engine, &channels).to_string(),
        });
    }
    std::thread::Builder::new()
        .name("sonarad-events".into())
        .spawn(move || {
            while let Ok(e) = events.recv() {
                let Some(w) = render(&e, set, &engine, &channels) else {
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
