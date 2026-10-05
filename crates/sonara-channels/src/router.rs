//! The pure L2 router: channels, their read cursors, which channel reads,
//! and the switch announcements. No threads, no reader: `Channels` asks it
//! what to feed next and carries that out. Ported from the Python daemon's
//! `router.py` and `channel.py` without the agent parts (turns, decisions,
//! digests, minimum batches), which are L3. L3 sets the gates below that
//! carry out its background speech policy and per-channel mute.
//!
//! Rules (binding for `Channels` and the `channels` protocol extension):
//! - A channel keeps its current **batch**: the entries pushed since it was
//!   last caught up, with a cursor over them. Read entries stay, so a manual
//!   return can replay the batch; a push into a caught-up channel whose
//!   last entry is no longer being read starts a new batch. Policy `latest` (or a push with `replace`) drops the
//!   channel's unread entries first, so the newest entry is never dropped
//!   and the batch is just that entry ("one message, always the last").
//! - Auto pick: a **prioritized** channel first (`prioritize`, oldest
//!   first, until its batch drains: L3 decisions preempt); then the channel
//!   that is reading keeps the floor until its batch drains; then the
//!   focused channel; then the first channel (in opening order) with
//!   something unread. A channel the user switched away from with
//!   `next_channel` is skipped until it gets new content.
//! - **Muted** channels (`set_muted`, kept by id whether open or not) are
//!   never picked: their entries wait, unread, until they are unmuted (the
//!   Python `session_prefs` `muted` rule). A muted channel never takes the
//!   floor on `next_channel` unless every channel is muted.
//! - **Focus only** (`set_focus_only`, L3's background policy
//!   `earcon_only`): the auto pick reads only the focused channel, the
//!   channel reading now and **authorized** channels (`authorize`: a
//!   replay, a summary delivery, host text, the previous focus finishing
//!   what it had when the focus moved); the others wait. With no channel
//!   focused nothing is held back. An authorization lasts until the
//!   channel has nothing unread.
//! - Switching from one channel to another is announced before the new
//!   channel's first entry: automatically when the channel that read last
//!   differs (never for the first reader), always on `next_channel`. The
//!   channel that read last is remembered also after it closed (#241: a
//!   session that ends right after its reply, then a question in another
//!   one), so every path to the floor (the auto pick with or without
//!   `prioritize`, `take_floor`, `replay`) announces a switch, except for
//!   a channel in the closed last reader's host tab (a `/clear` or a
//!   relaunch in the same tab replaces the session). A channel
//!   without a label is announced without one (`Feed::Announce` with
//!   `label: None`; the driver words it or skips it). `label_if_missing`
//!   names a channel that has no label yet.
//! - **Agent batches** (#243, `append`, L3's text): a batch L3 writes is
//!   the whole message. It **grows**: a push into it while it is caught up
//!   appends instead of starting a new batch, until the channel is flushed
//!   by its id (`flush(Some)`: L3's new turn, an answer, the flush hotkey).
//!   Text L3 **stores** (`append` with `heard`: written while Sonara is
//!   muted) joins the batch already read: it is never fed automatically,
//!   also not on unmute, and a manual return (`next_channel`, `replay`)
//!   reads it, from the top of the batch. When the channel still has
//!   unread entries or is being read (a replay in progress, also on its
//!   last entry) stored text is appended unread. Entries marked as
//!   **decisions** leave the batch with `drop_decisions` (L3: the decision
//!   was answered; `Resolved` says which ones), so a replay never reads an
//!   answered question or a decided permission. `end_batch` stops the
//!   growth without a flush (L3: a new turn without `turn_start`).
//! - `next_channel` is a round robin over the channels in opening order,
//!   skipping channels with nothing to hear (unless all are empty), from the
//!   channel reading or, after an idle gap, the one that read last. A fully
//!   heard target, landing on yourself, or re-landing on a replay in
//!   progress replays the batch from the top; unread content resumes.
use std::collections::{HashMap, HashSet};

/// How a channel treats a new entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// The new entry replaces the channel's unread entries.
    Latest,
    /// The new entry is read after the channel's unread entries.
    Queue,
}

impl Policy {
    /// The protocol names `latest` and `queue`.
    pub fn parse(name: &str) -> Option<Policy> {
        match name {
            "latest" => Some(Policy::Latest),
            "queue" => Some(Policy::Queue),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Policy::Latest => "latest",
            Policy::Queue => "queue",
        }
    }
}

/// One text pushed into a channel. `id` is unique within one router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub text: String,
    pub label: Option<String>,
}

/// Which decision entries `drop_decisions` takes out (module docs, #243).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// Every one: the session's decisions were answered (`answered`, a new
    /// turn).
    All,
    /// The ones read aloud, not stored ones and never unread ones: a tool
    /// ran, which decides the permission heard before it, but a parallel
    /// tool or a subagent's tool may come while another decision waits.
    Heard,
    /// The ones read or stored, never unread ones: the turn ended, so
    /// everything it asked was decided.
    Settled,
}

/// One named source and its current batch.
#[derive(Debug, Clone)]
pub struct Channel {
    pub id: String,
    pub label: Option<String>,
    pub host_tab: Option<String>,
    pub policy: Policy,
    entries: Vec<Entry>,
    cursor: usize,
    /// Content generation: bumped by every push, so "changed since" checks
    /// survive a batch that lands back on the same length (#115).
    gen: u64,
    /// A manual replay is in progress: re-landing on it restarts from the
    /// top (#118). New content ends it.
    replaying: bool,
    /// Never picked (`Router::set_muted`).
    muted: bool,
    /// An agent batch (module docs, #243): a push while caught up appends
    /// to it. Cleared by a flush of this channel.
    growing: bool,
    /// Entry ids of the batch that are decisions (`drop_decisions`), each
    /// with whether it was stored (written while muted) rather than fed.
    decisions: HashMap<u64, bool>,
    /// L3 wrote into this channel (`append`): an empty batch is its latest
    /// message, so `Restart` reads nothing rather than the core's last item.
    agent: bool,
}

impl Channel {
    fn new(id: &str, label: Option<String>, host_tab: Option<String>, policy: Policy) -> Self {
        Channel {
            id: id.to_string(),
            label,
            host_tab,
            policy,
            entries: Vec::new(),
            cursor: 0,
            gen: 0,
            replaying: false,
            muted: false,
            growing: false,
            decisions: HashMap::new(),
            agent: false,
        }
    }

    /// The channel is muted: its entries wait, unread.
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// The batch, read and unread.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Index of the next entry to read.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Unread entries.
    pub fn pending(&self) -> usize {
        self.entries.len() - self.cursor
    }

    pub fn caught_up(&self) -> bool {
        self.cursor >= self.entries.len()
    }

    /// Add `entry`. A caught-up channel starts a new batch unless its last
    /// entry is still being read (`reading`); `replace` drops the unread
    /// entries first; `front` puts the entry before the unread ones (it is
    /// read next).
    fn push(&mut self, entry: Entry, replace: bool, front: bool, reading: bool) {
        if replace || (self.caught_up() && !reading && !self.growing) {
            self.entries.clear();
            self.decisions.clear();
            self.cursor = 0;
        }
        let at = if front {
            self.cursor
        } else {
            self.entries.len()
        };
        self.entries.insert(at, entry);
        self.gen += 1;
        self.replaying = false;
    }

    fn take(&mut self) -> Option<Entry> {
        let e = self.entries.get(self.cursor)?.clone();
        self.cursor += 1;
        Some(e)
    }

    /// Mark every unread entry read (they stay replayable). Returns how many.
    fn skip_to_end(&mut self) -> usize {
        let n = self.pending();
        self.cursor = self.entries.len();
        n
    }

    /// Remove the decision entries `which` names from the batch (module
    /// docs). Returns the ones that were unread.
    fn drop_decisions(&mut self, which: Resolved) -> Vec<Entry> {
        let mut unread = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            let id = self.entries[i].id;
            let before = i < self.cursor;
            let goes = self.decisions.get(&id).is_some_and(|stored| match which {
                Resolved::All => true,
                Resolved::Heard => before && !stored,
                Resolved::Settled => before,
            });
            if goes {
                self.decisions.remove(&id);
                let e = self.entries.remove(i);
                if before {
                    self.cursor -= 1;
                } else {
                    unread.push(e);
                }
            } else {
                i += 1;
            }
        }
        unread
    }

    /// Step back over `entry` if it is the one just taken (it was cut off).
    fn rewind(&mut self, entry: u64) -> bool {
        if self.cursor > 0 && self.entries[self.cursor - 1].id == entry {
            self.cursor -= 1;
            return true;
        }
        false
    }
}

/// What to feed the reader next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feed {
    /// Announce a switch to `channel` (`label` is `None` when the channel
    /// has none).
    Announce {
        channel: String,
        label: Option<String>,
        /// The batch is read again from the top.
        replay: bool,
        /// Armed by `next_channel` (a key press), not by an auto hand-off.
        manual: bool,
    },
    /// Read this entry of `channel`.
    Entry { channel: String, entry: Entry },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Switch {
    channel: String,
    replay: bool,
    manual: bool,
}

/// See the module docs.
#[derive(Debug, Default)]
pub struct Router {
    /// In opening order.
    channels: Vec<Channel>,
    next_entry: u64,
    /// The channel reading now (cleared when nothing is left to pick).
    active: Option<String>,
    /// The channel that read last; survives idle gaps.
    last_active: Option<String>,
    /// The channel that read last, kept after it closed: whether a channel
    /// taking the floor is a switch (#241). `last_active` is cleared on
    /// close because `next_channel` and `engaged` mean an open channel.
    last_reader: Option<String>,
    /// The host tab of `last_reader` once it closed: a new session in that
    /// tab (a `/clear`, a relaunch) replaces it rather than switching.
    closed_reader_tab: Option<String>,
    focus: Option<String>,
    /// An armed switch announcement, fed before the next entry.
    announce: Option<Switch>,
    /// The entry fed last, until the next feed or `done`: (channel, entry).
    reading: Option<(String, u64)>,
    /// Channels switched away from by `next_channel`: channel -> its `gen`
    /// at the time. A different `gen` (new content) lifts it.
    suppressed: HashMap<String, u64>,
    /// Channels to read before anything else, oldest first; one leaves the
    /// list once it has nothing unread.
    priority: Vec<String>,
    /// Muted channel ids (open or not, so a channel opened later starts
    /// muted).
    muted: HashSet<String>,
    /// Only the focused channel is read automatically (module docs).
    focus_only: bool,
    /// Channels read past the focus-only gate until they have nothing
    /// unread.
    authorized: HashSet<String>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.channels.iter().position(|c| c.id == id)
    }

    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    fn channel_mut(&mut self, id: &str) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| c.id == id)
    }

    /// Every channel, in opening order.
    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn last_active(&self) -> Option<&str> {
        self.last_active.as_deref()
    }

    pub fn focused(&self) -> Option<&str> {
        self.focus.as_deref()
    }

    /// The channel the user is engaged with: the one reading, else the one
    /// that read last.
    pub fn engaged(&self) -> Option<&str> {
        self.active().or(self.last_active())
    }

    /// The channel whose batch got the newest entry (`Restart` before any
    /// channel was read, #243).
    pub fn written_last(&self) -> Option<String> {
        self.channels
            .iter()
            .filter_map(|c| c.entries.last().map(|e| (e.id, &c.id)))
            .max_by_key(|(e, _)| *e)
            .map(|(_, id)| id.clone())
    }

    /// Open `id`, or update an open channel's label, host tab and (when
    /// given) policy; its entries stay. Returns true when it was created.
    pub fn open(
        &mut self,
        id: &str,
        label: Option<String>,
        host_tab: Option<String>,
        policy: Option<Policy>,
    ) -> bool {
        if let Some(c) = self.channel_mut(id) {
            c.label = label;
            c.host_tab = host_tab;
            if let Some(p) = policy {
                c.policy = p;
            }
            return false;
        }
        let policy = policy.unwrap_or(Policy::Latest);
        let mut c = Channel::new(id, label, host_tab, policy);
        c.muted = self.muted.contains(id);
        self.channels.push(c);
        true
    }

    /// Give `id` the label `label` if it has none (a channel a message
    /// opened before the client named it). Returns true when it was set;
    /// false if it is not open, already has a label or `label` is empty.
    pub fn label_if_missing(&mut self, id: &str, label: &str) -> bool {
        if label.is_empty() {
            return false;
        }
        match self.channel_mut(id) {
            Some(c) if c.label.as_deref().is_none_or(str::is_empty) => {
                c.label = Some(label.to_string());
                true
            }
            _ => false,
        }
    }

    /// Whether `id` taking the floor is a switch from the channel that read
    /// last (also when that one has closed since).
    /// A channel in the host tab of a closed last reader replaces it: no
    /// switch.
    fn is_handoff(&self, id: &str) -> bool {
        if !self.last_reader.as_deref().is_some_and(|last| last != id) {
            return false;
        }
        let tab = self.channel(id).and_then(|c| c.host_tab.as_deref());
        !(tab.is_some() && tab == self.closed_reader_tab.as_deref())
    }

    /// `id` reads now.
    fn set_reader(&mut self, id: &str) {
        self.active = Some(id.to_string());
        self.last_active = Some(id.to_string());
        self.last_reader = Some(id.to_string());
        self.closed_reader_tab = None;
    }

    /// Forget `id` and everything about it. Returns false if it was not open.
    pub fn close(&mut self, id: &str) -> bool {
        let Some(i) = self.index(id) else {
            return false;
        };
        let closed = self.channels.remove(i);
        if self.last_reader.as_deref() == Some(id) {
            self.closed_reader_tab = closed.host_tab.filter(|t| !t.is_empty());
        }
        let is = |o: &Option<String>| o.as_deref() == Some(id);
        if is(&self.active) {
            self.active = None;
        }
        if is(&self.last_active) {
            self.last_active = None;
        }
        if is(&self.focus) {
            self.focus = None;
        }
        if self.announce.as_ref().map(|s| s.channel.as_str()) == Some(id) {
            self.announce = None;
        }
        if self.reading.as_ref().is_some_and(|(c, _)| c == id) {
            self.reading = None;
        }
        self.suppressed.remove(id);
        self.priority.retain(|p| p != id);
        self.authorized.remove(id);
        true
    }

    /// Put `id` in front for the auto pick. False if it is not open. The
    /// channel focused before keeps the right to finish what it has unread
    /// (the Python cooperative hand-off): it is authorized.
    pub fn focus(&mut self, id: &str) -> bool {
        if self.index(id).is_none() {
            return false;
        }
        if let Some(old) = self.focus.clone().filter(|o| o != id) {
            if self.ready(&old) {
                self.authorized.insert(old);
            }
        }
        self.focus = Some(id.to_string());
        true
    }

    /// Mute or unmute `id` (open or not; remembered by id).
    pub fn set_muted(&mut self, id: &str, muted: bool) {
        if muted {
            self.muted.insert(id.to_string());
        } else {
            self.muted.remove(id);
        }
        if let Some(c) = self.channel_mut(id) {
            c.muted = muted;
        }
    }

    pub fn is_muted(&self, id: &str) -> bool {
        self.muted.contains(id)
    }

    /// Read only the focused channel automatically (module docs).
    pub fn set_focus_only(&mut self, on: bool) {
        self.focus_only = on;
    }

    pub fn focus_only(&self) -> bool {
        self.focus_only
    }

    /// Let `id` past the focus-only gate until it has nothing unread. False
    /// if it is not open.
    pub fn authorize(&mut self, id: &str) -> bool {
        if self.index(id).is_none() {
            return false;
        }
        self.authorized.insert(id.to_string());
        true
    }

    /// `id` is authorized now (diagnostics, tests).
    pub fn is_authorized(&self, id: &str) -> bool {
        self.authorized.contains(id)
    }

    /// The focus-only gate holds `id` back.
    fn gated(&self, id: &str) -> bool {
        self.focus_only
            && self.focus.as_deref().is_some_and(|f| f != id)
            && !self.authorized.contains(id)
    }

    /// Push `text` into an open channel by its policy: `latest` drops the
    /// unread entries, `queue` appends. Returns the entry id, or `None` if
    /// the channel is not open.
    pub fn push(&mut self, id: &str, text: &str, label: Option<String>) -> Option<u64> {
        let replace = self.channel(id)?.policy == Policy::Latest;
        self.push_with(id, text, label, replace, false)
    }

    /// Push `text` into an open channel, ignoring its policy: `replace`
    /// drops its unread entries; `front` makes it the next entry read.
    pub fn push_with(
        &mut self,
        id: &str,
        text: &str,
        label: Option<String>,
        replace: bool,
        front: bool,
    ) -> Option<u64> {
        let next = self.next_entry + 1;
        let reading = self.reading.as_ref().is_some_and(|(c, _)| c == id);
        let c = self.channel_mut(id)?;
        c.push(
            Entry {
                id: next,
                text: text.to_string(),
                label,
            },
            replace,
            front,
            reading,
        );
        self.next_entry = next;
        Some(next)
    }

    /// Push L3's `text` into an open channel (module docs, #243): appended
    /// to its agent batch, which grows. `decision` marks it for
    /// `drop_decisions`; `heard` stores it read (never fed automatically)
    /// unless the channel has unread entries. Returns the entry id, or
    /// `None` if the channel is not open.
    pub fn append(&mut self, id: &str, text: &str, decision: bool, heard: bool) -> Option<u64> {
        // Being read (a replay on its last entry) counts as unread: the
        // replay goes on into the new text.
        let unread =
            self.channel(id)?.pending() > 0 || self.reading.as_ref().is_some_and(|(c, _)| c == id);
        let entry = self.push_with(id, text, None, false, false)?;
        let c = self.channel_mut(id)?;
        c.growing = true;
        c.agent = true;
        if decision {
            c.decisions.insert(entry, heard);
        }
        if heard && !unread {
            c.cursor = c.entries.len();
        }
        Some(entry)
    }

    /// Remove `id`'s decision entries `which` names from its batch (module
    /// docs): they were answered. Returns the ones that were unread.
    pub fn drop_decisions(&mut self, id: &str, which: Resolved) -> Vec<Entry> {
        self.channel_mut(id)
            .map(|c| c.drop_decisions(which))
            .unwrap_or_default()
    }

    /// End `id`'s agent batch: its entries stay (replayable), and L3's next
    /// text starts a new batch once the channel is caught up (a new turn
    /// without a `turn_start`, #243).
    pub fn end_batch(&mut self, id: &str) {
        if let Some(c) = self.channel_mut(id) {
            c.growing = false;
        }
    }

    /// True if L3 wrote into `id` (`append`).
    pub fn is_agent(&self, id: &str) -> bool {
        self.channel(id).is_some_and(|c| c.agent)
    }

    /// True if `id` was switched away from and has not changed since. New
    /// content (or a closed channel) lifts it.
    pub fn is_suppressed(&mut self, id: &str) -> bool {
        let Some(&gen) = self.suppressed.get(id) else {
            return false;
        };
        match self.channel(id) {
            Some(c) if c.gen == gen => true,
            _ => {
                self.suppressed.remove(id);
                false
            }
        }
    }

    fn ready(&self, id: &str) -> bool {
        self.channel(id).is_some_and(|c| c.pending() > 0)
    }

    /// Something unread and not muted.
    fn audible(&self, id: &str) -> bool {
        self.channel(id)
            .is_some_and(|c| c.pending() > 0 && !c.muted)
    }

    /// May the auto pick choose `id` (other than the channel reading now)?
    fn pickable(&mut self, id: &str) -> bool {
        self.audible(id) && !self.is_suppressed(id) && !self.gated(id)
    }

    /// Read `id` before every other channel (after the item being read)
    /// until its unread entries are read: an L3 decision preempts the batch
    /// reading now. False if it is not open.
    pub fn prioritize(&mut self, id: &str) -> bool {
        if self.index(id).is_none() {
            return false;
        }
        if !self.priority.iter().any(|p| p == id) {
            self.priority.push(id.to_string());
        }
        true
    }

    /// Channels prioritized and not yet drained, oldest first.
    pub fn prioritized(&self) -> &[String] {
        &self.priority
    }

    /// The channel to read next, by the auto rules (module docs).
    pub fn pick(&mut self) -> Option<String> {
        let drained: Vec<String> = self
            .priority
            .iter()
            .filter(|p| !self.ready(p))
            .cloned()
            .collect();
        self.priority.retain(|p| !drained.contains(p));
        // An authorization ends once its channel has nothing unread.
        let spent: Vec<String> = self
            .authorized
            .iter()
            .filter(|a| !self.ready(a))
            .cloned()
            .collect();
        for a in spent {
            self.authorized.remove(&a);
        }
        let first = self.priority.clone().into_iter().find(|p| self.pickable(p));
        if first.is_some() {
            return first;
        }
        if let Some(a) = self.active.clone() {
            if self.audible(&a) {
                return Some(a);
            }
        }
        if let Some(f) = self.focus.clone() {
            if self.audible(&f) && !self.is_suppressed(&f) {
                return Some(f);
            }
        }
        let ids: Vec<String> = self.channels.iter().map(|c| c.id.clone()).collect();
        ids.into_iter().find(|id| self.pickable(id))
    }

    fn arm(&mut self, id: &str, replay: bool, manual: bool) {
        self.announce = Some(Switch {
            channel: id.to_string(),
            replay,
            manual,
        });
    }

    /// Drop an armed announcement that was not fed yet.
    pub fn clear_announce(&mut self) {
        self.announce = None;
    }

    /// Whether an announcement is armed.
    pub fn announce_armed(&self) -> bool {
        self.announce.is_some()
    }

    /// The next thing to read: an armed announcement, else the next entry of
    /// the picked channel (arming an auto hand-off announcement first when
    /// the channel differs from the one that read last). `None` when
    /// nothing is left; the reading channel is then cleared.
    pub fn next_feed(&mut self) -> Option<Feed> {
        self.reading = None;
        if let Some(sw) = self.announce.take() {
            if let Some(c) = self.channel(&sw.channel) {
                let label = c.label.clone().filter(|l| !l.is_empty());
                return Some(Feed::Announce {
                    channel: sw.channel,
                    label,
                    replay: sw.replay,
                    manual: sw.manual,
                });
            }
        }
        let Some(target) = self.pick() else {
            self.active = None;
            return None;
        };
        if self.active.as_deref() != Some(target.as_str()) {
            let handoff = self.is_handoff(&target);
            self.set_reader(&target);
            if handoff {
                self.arm(&target, false, false);
                return self.next_feed();
            }
        }
        let c = self.channel_mut(&target)?;
        let entry = c.take()?;
        if c.pending() == 0 {
            // Its last unread entry: the authorization is spent.
            self.authorized.remove(&target);
        }
        self.reading = Some((target.clone(), entry.id));
        Some(Feed::Entry {
            channel: target,
            entry,
        })
    }

    /// The entry fed last has ended (read, skipped or cut). Also implied
    /// by the next `next_feed`.
    pub fn done(&mut self) {
        self.reading = None;
    }

    /// Manual switch: move the reading channel one step around the ring
    /// (module docs) and arm its announcement. Returns the target and
    /// whether its batch is replayed, or `None` when no channel is open.
    pub fn next_channel(&mut self) -> Option<(String, bool)> {
        let all: Vec<String> = self.channels.iter().map(|c| c.id.clone()).collect();
        if all.is_empty() {
            return None;
        }
        // A muted channel never takes the floor, unless every channel is
        // muted (never dead-end).
        let audible: Vec<String> = all
            .iter()
            .filter(|id| self.channel(id).is_some_and(|c| !c.muted))
            .cloned()
            .collect();
        let ring = if audible.is_empty() { &all } else { &audible };
        // A channel with nothing to hear is skipped, unless all are empty
        // (never dead-end; #117).
        let nonempty: Vec<String> = ring
            .iter()
            .filter(|id| self.channel(id).is_some_and(|c| !c.entries.is_empty()))
            .cloned()
            .collect();
        let ring = if nonempty.is_empty() { ring } else { &nonempty };
        let old = self.active.clone();
        // The ring position survives idle gaps (#111).
        let cur = self.active.clone().or_else(|| self.last_active.clone());
        let pos = |list: &[String], id: &Option<String>| {
            id.as_ref().and_then(|id| list.iter().position(|x| x == id))
        };
        let target = if let Some(i) = pos(ring, &cur) {
            ring[(i + 1) % ring.len()].clone()
        } else if let Some(i) = pos(&all, &cur) {
            // The position is filtered out of the ring: advance from its
            // slot in the full order to the next ring member.
            (1..=all.len())
                .map(|j| &all[(i + j) % all.len()])
                .find(|c| ring.contains(c))
                .cloned()
                .unwrap_or_else(|| ring[0].clone())
        } else {
            ring[0].clone()
        };
        if let Some(old) = old.filter(|o| *o != target) {
            if let Some(c) = self.channel(&old) {
                self.suppressed.insert(old, c.gen);
            }
        }
        self.suppressed.remove(&target);
        let landing_on_self = cur.as_deref() == Some(target.as_str());
        let c = self.channel_mut(&target)?;
        let replay = c.caught_up() || landing_on_self || c.replaying;
        if replay {
            c.cursor = 0;
            c.replaying = true;
        }
        self.set_reader(&target);
        self.arm(&target, replay, true);
        Some((target, replay))
    }

    /// Make `id` the reading channel now (a `speak` with `interrupt`):
    /// announced when another channel read last. False if not open.
    pub fn take_floor(&mut self, id: &str) -> bool {
        if self.index(id).is_none() {
            return false;
        }
        let handoff = self.is_handoff(id);
        self.set_reader(id);
        self.suppressed.remove(id);
        if handoff {
            self.arm(id, false, false);
        } else if self.announce.as_ref().is_some_and(|s| s.channel != id) {
            self.announce = None;
        }
        true
    }

    /// Read `id`'s batch again from the top and make it the reading channel
    /// (restart when idle). Announced when another channel read last.
    /// False if it is not open or has nothing to replay.
    pub fn replay(&mut self, id: &str) -> bool {
        let Some(c) = self.channel_mut(id) else {
            return false;
        };
        if c.entries.is_empty() {
            return false;
        }
        c.cursor = 0;
        c.replaying = true;
        let handoff = self.is_handoff(id);
        self.set_reader(id);
        self.suppressed.remove(id);
        // A replay is read whatever the focus (Python authorize_replay).
        self.authorized.insert(id.to_string());
        if handoff {
            self.arm(id, true, true);
        }
        true
    }

    /// Skip to the end of one channel, or of all (also dropping an armed
    /// announcement). Entries stay replayable. Returns how many were
    /// unread.
    pub fn flush(&mut self, id: Option<&str>) -> usize {
        match id {
            Some(id) => {
                if self.announce.as_ref().map(|s| s.channel.as_str()) == Some(id) {
                    self.announce = None;
                }
                self.channel_mut(id).map_or(0, |c| {
                    // The next agent text starts a new batch (#243).
                    c.growing = false;
                    c.skip_to_end()
                })
            }
            None => {
                self.announce = None;
                self.channels.iter_mut().map(Channel::skip_to_end).sum()
            }
        }
    }

    /// Step `id` back over `entry` if it was the entry just taken (it was
    /// cut off before its end).
    pub fn rewind(&mut self, id: &str, entry: u64) -> bool {
        self.channel_mut(id).is_some_and(|c| c.rewind(entry))
    }

    /// Unread entries over all channels.
    pub fn pending(&self) -> usize {
        self.channels.iter().map(Channel::pending).sum()
    }

    /// Something would be fed now (an armed announcement or a pick).
    pub fn has_work(&mut self) -> bool {
        self.announce.is_some() || self.pick().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_names() {
        assert_eq!(Policy::parse("latest"), Some(Policy::Latest));
        assert_eq!(Policy::parse("queue"), Some(Policy::Queue));
        assert_eq!(Policy::parse("LATEST"), None);
        assert_eq!(Policy::Queue.as_str(), "queue");
    }

    #[test]
    fn rewind_only_steps_over_the_entry_just_taken() {
        let mut r = Router::new();
        r.open("a", None, None, Some(Policy::Queue));
        let one = r.push("a", "One.", None).unwrap();
        let two = r.push("a", "Two.", None).unwrap();
        assert!(matches!(r.next_feed(), Some(Feed::Entry { .. })));
        assert!(!r.rewind("a", two));
        assert!(r.rewind("a", one));
        assert_eq!(r.channel("a").unwrap().cursor(), 0);
    }
}
