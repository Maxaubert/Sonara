//! Sonara L2: channels. Several named sources (terminal tabs, chats) share
//! one L1 reader; each channel keeps its own batch and policy, and one
//! channel reads at a time.
//!
//! `Channels` wraps a `sonara_reader::ReaderHandle` and uses only its public
//! API. The pure rules live in `router` (see its module docs); this driver
//! carries them out:
//!
//! - Channel entries are held here and fed to the reader **one item at a
//!   time**, and only when the reader is idle: text spoken to the reader
//!   directly (protocol core `speak`) is read first. When the fed item ends
//!   (finished, skipped or failed), the next one is fed. So the item a
//!   channel fed is always the reader's current item.
//! - A switch between channels is announced by a short item (`"<label>."`,
//!   or `"<label>, reading again."` for a replay) fed before the new
//!   channel's first entry. `Config::announce` turns it off;
//!   `set_announce_texts` changes the texts (L3 says "Session changed:
//!   <label>."). `on_announce` runs a hook right before an announcement
//!   is handed to the reader (L3 plays its session-change earcon there,
//!   so the chime comes first); it runs under the driver's lock and must
//!   not call back into `Channels`.
//! - `next_channel` and `speak` with `interrupt` cut the current item (the
//!   reader's `interrupt`); another channel's message cut that way is read
//!   again later. The cut only replaces the current item: text already
//!   queued in the reader (core `speak`) still plays before the new
//!   channel's message, right after the announcement. An item ended from
//!   outside (a core `speak` with `interrupt`, `skip`) counts as read: L2
//!   cannot tell it from a user skip. `Stop` without a channel flushes every channel,
//!   with a channel only that one. `Restart` while idle replays the engaged
//!   channel's batch (the Python plugin's Up key).
//! - `prioritize` puts a channel ahead of the others (and of the batch
//!   reading now) from the next item on, until it has nothing unread: L3
//!   uses it so a decision preempts (the Python router's decision rule).
//! - `set_muted` mutes one channel (its entries wait; muting the channel
//!   being read cuts its item) and `set_focus_only` reads only the focused
//!   channel automatically: L3 uses them for per-channel mute and the
//!   background speech policy (router docs). Text a host speaks into a
//!   channel (`speak`) is always read (it authorizes the channel); L3's
//!   own text goes through `add`, which the gates apply to.
//! - A thread drains the reader's events; it holds the driver weakly and
//!   ends with the reader.
//! - `on_drop` (#219) reports every entry dropped unread (and an item cut
//!   while it was read) with the reason: `replaced` (policy `latest` or
//!   mode `replace`), the reason given to `control_because` (L3 passes
//!   `turn_start`, `answered`, `mute`, `stop`), `closed`, `muted`. It runs
//!   under the driver's lock and must not call back into `Channels`.
pub mod router;

pub use router::{Channel, Entry, Feed, Policy, Router};
pub use sonara_reader::{Control, ItemId, QueueMode, ReaderHandle};

use sonara_reader::{Event, ItemPhase};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// Item tags remembered for `tag` (state events are rendered after the
/// fact, so a few recent items are kept).
const TAGS: usize = 256;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Reader(#[from] sonara_reader::Error),
    #[error("no channel '{0}' is open")]
    UnknownChannel(String),
    #[error("a channel id must be a non-empty string")]
    EmptyChannel,
}

/// How switches are announced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Speak a short announcement on a channel switch.
    pub announce: bool,
    /// The announcement; `{label}` is the channel's label.
    pub announce_text: String,
    /// The announcement when the batch is read again from the top.
    pub replay_text: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            announce: true,
            announce_text: "{label}.".into(),
            replay_text: "{label}, reading again.".into(),
        }
    }
}

/// A switch announcement about to be read (`Channels::on_announce`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announced {
    pub channel: String,
    pub label: String,
    /// The batch is read again from the top.
    pub replay: bool,
    /// A manual switch (`next_channel`, `restart` with a channel).
    pub manual: bool,
}

/// The hook of `Channels::on_announce`.
pub type AnnounceHook = Arc<dyn Fn(&Announced) + Send + Sync>;

/// Text dropped before it was heard (`Channels::on_drop`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub channel: String,
    /// The channel entry.
    pub entry: u64,
    pub text: String,
    /// Why (module docs).
    pub reason: String,
    /// The reader item cut while it was being read, if the entry was.
    pub item: Option<ItemId>,
}

/// The hook of `Channels::on_drop`.
pub type DropHook = Arc<dyn Fn(&Dropped) + Send + Sync>;

/// Which channel an item came from (`state.now_playing.channel`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub channel: String,
    pub host_tab: Option<String>,
    /// The item is a switch announcement.
    pub announcement: bool,
    /// The channel entry it reads (`None` for an announcement).
    pub entry: Option<u64>,
}

/// The item this driver fed and is waiting for.
#[derive(Debug, Clone)]
struct InFlight {
    id: ItemId,
    channel: String,
    /// The entry it reads; `None` for an announcement.
    entry: Option<u64>,
    /// Its text (for `on_drop`: a replace can drop the entry from the
    /// batch while it is read).
    text: String,
}

struct State {
    router: Router,
    config: Config,
    on_announce: Option<AnnounceHook>,
    on_drop: Option<DropHook>,
    in_flight: Option<InFlight>,
    tags: VecDeque<(ItemId, Tag)>,
}

struct Inner {
    reader: ReaderHandle,
    state: Mutex<State>,
}

/// L2 over one reader. Clones share it.
#[derive(Clone)]
pub struct Channels {
    inner: Arc<Inner>,
}

/// What `speak` did with the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spoken {
    /// The reader item reading it, when it was fed at once; `None` while
    /// it waits in its channel (behind other entries or an announcement).
    pub item_id: Option<ItemId>,
    /// Unread entries of the channel that were dropped for it.
    pub dropped: usize,
    /// The channel entry holding the text.
    pub entry: u64,
}

fn check(channel: &str) -> Result<()> {
    if channel.is_empty() {
        Err(Error::EmptyChannel)
    } else {
        Ok(())
    }
}

impl Channels {
    /// Start L2 on `reader`.
    pub fn new(reader: ReaderHandle, config: Config) -> Result<Channels> {
        let events = reader.subscribe()?;
        let inner = Arc::new(Inner {
            reader,
            state: Mutex::new(State {
                router: Router::new(),
                config,
                on_announce: None,
                on_drop: None,
                in_flight: None,
                tags: VecDeque::new(),
            }),
        });
        let weak: Weak<Inner> = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("sonara-channels".into())
            .spawn(move || {
                while let Ok(e) = events.recv() {
                    let Some(inner) = weak.upgrade() else {
                        return;
                    };
                    if let Event::Item { item_id, phase } = e {
                        if phase != ItemPhase::Started {
                            inner.item_ended(item_id);
                        }
                    }
                }
            })
            .map_err(|e| sonara_reader::Error::Start(e.to_string()))?;
        Ok(Channels { inner })
    }

    /// The reader underneath (for core requests).
    pub fn reader(&self) -> &ReaderHandle {
        &self.inner.reader
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock()
    }

    /// Open a channel, or update an open one (its entries stay). `policy`
    /// `None` keeps the current one (`latest` for a new channel). Returns
    /// true when it was created.
    pub fn open(
        &self,
        channel: &str,
        label: Option<String>,
        host_tab: Option<String>,
        policy: Option<Policy>,
    ) -> Result<bool> {
        check(channel)?;
        Ok(self.lock().router.open(channel, label, host_tab, policy))
    }

    /// Close a channel. If it is being read, its item is cut and the next
    /// channel's text follows.
    pub fn close(&self, channel: &str) -> Result<()> {
        let mut st = self.lock();
        let unread = unread(&st, channel);
        let cut = cut_entry(&st, channel);
        if !st.router.close(channel) {
            return Err(Error::UnknownChannel(channel.to_string()));
        }
        report(&st, channel, unread, "closed", None);
        if let Some((item, e)) =
            cut.filter(|_| st.in_flight.as_ref().is_some_and(|f| f.channel == channel))
        {
            report(&st, channel, vec![e], "closed", Some(item));
        }
        self.inner.cut_if(&mut st, channel, None)?;
        self.inner.pump(&mut st)
    }

    /// Put a channel in front: it is read next once the channel reading now
    /// has drained its batch.
    pub fn focus(&self, channel: &str) -> Result<()> {
        let mut st = self.lock();
        if !st.router.focus(channel) {
            return Err(Error::UnknownChannel(channel.to_string()));
        }
        self.inner.pump(&mut st)
    }

    /// Read `channel` before the other channels once the item playing ends
    /// (it does not cut), until its unread entries are read: the batch
    /// reading now waits. L3 uses it so a decision preempts.
    pub fn prioritize(&self, channel: &str) -> Result<()> {
        let mut st = self.lock();
        if !st.router.prioritize(channel) {
            return Err(Error::UnknownChannel(channel.to_string()));
        }
        self.inner.pump(&mut st)
    }

    /// Add `text` to `channel` (opened with the defaults if needed).
    /// `mode` overrides the channel's policy for this text (`Replace` drops
    /// its unread entries). `interrupt` reads it now: it cuts the current
    /// item and takes the floor for this channel. A host's text is read
    /// whatever the focus-only gate (the channel is authorized); a muted
    /// channel still holds it.
    pub fn speak(
        &self,
        channel: &str,
        text: &str,
        mode: Option<QueueMode>,
        interrupt: bool,
        label: Option<String>,
    ) -> Result<Spoken> {
        self.push(channel, text, mode, interrupt, label, true)
    }

    /// Append `text` to `channel` (opened with the defaults if needed)
    /// under the gates: L3's prose and decisions. A channel the focus-only
    /// gate holds back keeps it until it is focused or authorized.
    pub fn add(&self, channel: &str, text: &str) -> Result<Spoken> {
        self.push(channel, text, Some(QueueMode::Append), false, None, false)
    }

    fn push(
        &self,
        channel: &str,
        text: &str,
        mode: Option<QueueMode>,
        interrupt: bool,
        label: Option<String>,
        authorize: bool,
    ) -> Result<Spoken> {
        check(channel)?;
        let mut st = self.lock();
        if st.router.channel(channel).is_none() {
            st.router.open(channel, None, None, None);
        }
        let replace = match mode {
            Some(QueueMode::Replace) => true,
            Some(QueueMode::Append) => false,
            None => st.router.channel(channel).map(|c| c.policy) == Some(Policy::Latest),
        };
        let before = st.router.channel(channel).map_or(0, Channel::pending);
        if replace {
            let why = if mode == Some(QueueMode::Replace) {
                "replaced by newer text (mode replace)"
            } else {
                "replaced by newer text (policy latest)"
            };
            report(&st, channel, unread(&st, channel), why, None);
        }
        let entry = st
            .router
            .push_with(channel, text, label, replace, interrupt)
            .ok_or_else(|| Error::UnknownChannel(channel.to_string()))?;
        if authorize {
            st.router.authorize(channel);
        }
        let dropped = if replace { before } else { 0 };
        let fed = if interrupt {
            // Another channel's message cut here was not heard: it is read
            // again once this channel's batch drains (as for next_channel).
            if let Some(cut) = st.in_flight.take() {
                if let (Some(e), true) = (cut.entry, cut.channel != channel) {
                    st.router.rewind(&cut.channel, e);
                }
            }
            st.router.take_floor(channel);
            self.inner.feed(&mut st, true)?
        } else {
            self.inner.pump_fed(&mut st)?
        };
        let item_id = fed.filter(|f| f.entry == Some(entry)).map(|f| f.id);
        Ok(Spoken {
            item_id,
            dropped,
            entry,
        })
    }

    /// Mute or unmute a channel (open or not: a channel opened later starts
    /// muted). Its entries wait while it is muted; muting the channel being
    /// read cuts its item.
    pub fn set_muted(&self, channel: &str, muted: bool) -> Result<()> {
        check(channel)?;
        let mut st = self.lock();
        st.router.set_muted(channel, muted);
        if muted {
            self.inner.cut_if(&mut st, channel, Some("muted"))?;
        }
        self.inner.pump(&mut st)
    }

    pub fn is_muted(&self, channel: &str) -> bool {
        self.lock().router.is_muted(channel)
    }

    /// Read only the focused channel automatically (router docs).
    pub fn set_focus_only(&self, on: bool) -> Result<()> {
        let mut st = self.lock();
        st.router.set_focus_only(on);
        self.inner.pump(&mut st)
    }

    pub fn focus_only(&self) -> bool {
        self.lock().router.focus_only()
    }

    /// Let `channel` past the focus-only gate until it has nothing unread
    /// (a summary delivery). False if it is not open.
    pub fn authorize(&self, channel: &str) -> Result<bool> {
        let mut st = self.lock();
        let ok = st.router.authorize(channel);
        self.inner.pump(&mut st)?;
        Ok(ok)
    }

    /// A playback control. Without a channel it acts on the reader, except
    /// `Stop` (also flushes every channel) and `Restart` while idle (replays
    /// the engaged channel's batch). With a channel: `Stop` flushes that
    /// channel (cutting its item if it is being read), `Restart` replays it
    /// (switching to it), and other actions apply only while it is being
    /// read.
    pub fn control(&self, c: Control, channel: Option<&str>) -> Result<()> {
        self.control_because(c, channel, "stop")
    }

    /// `control`, with the reason a `Stop` drops text reported to
    /// `on_drop` (module docs).
    pub fn control_because(&self, c: Control, channel: Option<&str>, reason: &str) -> Result<()> {
        let mut st = self.lock();
        let reader = &self.inner.reader;
        let Some(ch) = channel else {
            return match c {
                Control::Stop => {
                    let ids: Vec<String> =
                        st.router.channels().iter().map(|c| c.id.clone()).collect();
                    for id in ids {
                        report(&st, &id, unread(&st, &id), reason, None);
                    }
                    if let Some(f) = st.in_flight.clone() {
                        if let Some((item, e)) = cut_entry(&st, &f.channel) {
                            report(&st, &f.channel, vec![e], reason, Some(item));
                        }
                    }
                    st.router.flush(None);
                    st.router.done();
                    st.in_flight = None;
                    Ok(reader.control(Control::Stop)?)
                }
                Control::Restart if reader.state()?.now_playing.is_none() => {
                    let engaged = st.router.engaged().map(str::to_string);
                    match engaged {
                        Some(e) if st.router.replay(&e) => self.inner.pump(&mut st),
                        _ => Ok(reader.control(Control::Restart)?),
                    }
                }
                other => Ok(reader.control(other)?),
            };
        };
        if st.router.channel(ch).is_none() {
            return Err(Error::UnknownChannel(ch.to_string()));
        }
        let reading = st.in_flight.as_ref().is_some_and(|f| f.channel == ch);
        match c {
            Control::Stop => {
                report(&st, ch, unread(&st, ch), reason, None);
                st.router.flush(Some(ch));
                self.inner.cut_if(&mut st, ch, Some(reason))?;
                self.inner.pump(&mut st)
            }
            Control::Restart if reading => Ok(reader.control(Control::Restart)?),
            Control::Restart => {
                if !st.router.replay(ch) {
                    return Ok(());
                }
                let cut = st.in_flight.take();
                if let Some(cut) = cut {
                    if let Some(entry) = cut.entry {
                        st.router.rewind(&cut.channel, entry);
                    }
                }
                let fed = self.inner.feed(&mut st, true)?;
                if fed.is_none() && reader.state()?.now_playing.is_some() {
                    reader.control(Control::Skip)?;
                }
                Ok(())
            }
            other if reading => Ok(reader.control(other)?),
            _ => Ok(()),
        }
    }

    /// Switch to the next channel now (the Python plugin's next-session
    /// key): cut the current item, announce the target and read it.
    /// Returns the target, or `None` when no channel is open.
    pub fn next_channel(&self) -> Result<Option<String>> {
        let mut st = self.lock();
        let Some((target, _replay)) = st.router.next_channel() else {
            return Ok(None);
        };
        let cut = st.in_flight.take();
        if let Some(cut) = &cut {
            if let (Some(entry), true) = (cut.entry, cut.channel != target) {
                st.router.rewind(&cut.channel, entry);
            }
        }
        let fed = self.inner.feed(&mut st, true)?;
        if fed.is_none() && cut.is_some() {
            self.inner.reader.control(Control::Skip)?;
        }
        Ok(Some(target))
    }

    /// Turn switch announcements on or off.
    pub fn set_announce(&self, on: bool) {
        self.lock().config.announce = on;
    }

    pub fn announce(&self) -> bool {
        self.lock().config.announce
    }

    /// Replace the announcement texts (`{label}` is the channel's label):
    /// `text` for a switch, `replay` for a batch read again from the top.
    pub fn set_announce_texts(&self, text: &str, replay: &str) {
        let mut st = self.lock();
        st.config.announce_text = text.to_string();
        st.config.replay_text = replay.to_string();
    }

    /// Run `hook` right before each spoken switch announcement is handed
    /// to the reader (`None` removes it). It runs under the driver's lock:
    /// it must not call back into `Channels`.
    pub fn on_announce(&self, hook: Option<AnnounceHook>) {
        self.lock().on_announce = hook;
    }

    /// Report every entry dropped unread from now on to `hook` (`None`
    /// removes it): module docs. It runs under the driver's lock.
    pub fn on_drop(&self, hook: Option<DropHook>) {
        self.lock().on_drop = hook;
    }

    /// The channel an item came from, if a channel fed it (recent items).
    pub fn tag(&self, item: ItemId) -> Option<Tag> {
        let st = self.lock();
        st.tags
            .iter()
            .rev()
            .find(|(id, _)| *id == item)
            .map(|(_, t)| t.clone())
    }

    /// Unread entries over all channels.
    pub fn pending(&self) -> usize {
        self.lock().router.pending()
    }

    /// Nothing fed and waiting, and nothing a channel would feed now.
    pub fn is_idle(&self) -> bool {
        let mut st = self.lock();
        st.in_flight.is_none() && !st.router.has_work()
    }

    /// The channel reading now, else the one that read last.
    pub fn engaged(&self) -> Option<String> {
        self.lock().router.engaged().map(str::to_string)
    }

    /// The focused channel.
    pub fn focused(&self) -> Option<String> {
        self.lock().router.focused().map(str::to_string)
    }

    /// A copy of a channel (label, policy, batch and cursor).
    pub fn channel(&self, channel: &str) -> Option<Channel> {
        self.lock().router.channel(channel).cloned()
    }

    /// Open channel ids, in opening order.
    pub fn channel_ids(&self) -> Vec<String> {
        self.lock()
            .router
            .channels()
            .iter()
            .map(|c| c.id.clone())
            .collect()
    }
}

/// The unread entries of `channel`.
fn unread(st: &State, channel: &str) -> Vec<Entry> {
    st.router
        .channel(channel)
        .map(|c| c.entries()[c.cursor()..].to_vec())
        .unwrap_or_default()
}

/// The item in flight for `channel` and the entry it reads.
fn cut_entry(st: &State, channel: &str) -> Option<(ItemId, Entry)> {
    let f = st.in_flight.as_ref().filter(|f| f.channel == channel)?;
    let e = Entry {
        id: f.entry?,
        text: f.text.clone(),
        label: None,
    };
    Some((f.id, e))
}

/// Tell `on_drop` about `entries` of `channel`.
fn report(st: &State, channel: &str, entries: Vec<Entry>, reason: &str, item: Option<ItemId>) {
    let Some(hook) = &st.on_drop else {
        return;
    };
    for e in entries {
        hook(&Dropped {
            channel: channel.to_string(),
            entry: e.id,
            text: e.text,
            reason: reason.to_string(),
            item,
        });
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic under the lock already failed a test; keep serving.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The reader ended an item: if it was ours, feed the next one.
    fn item_ended(&self, id: ItemId) {
        let mut st = self.lock();
        if st.in_flight.as_ref().is_some_and(|f| f.id == id) {
            st.in_flight = None;
            st.router.done();
        }
        let _ = self.pump(&mut st);
    }

    fn pump(&self, st: &mut State) -> Result<()> {
        self.pump_fed(st).map(|_| ())
    }

    /// Feed the next item if nothing is in flight and the reader is idle.
    fn pump_fed(&self, st: &mut State) -> Result<Option<InFlight>> {
        if st.in_flight.is_some() {
            return Ok(None);
        }
        let s = self.reader.state()?;
        if s.now_playing.is_some() || s.queued > 0 {
            return Ok(None);
        }
        self.feed(st, false)
    }

    /// Feed the router's next item; with `interrupt` it cuts the reader's
    /// current item.
    fn feed(&self, st: &mut State, interrupt: bool) -> Result<Option<InFlight>> {
        loop {
            let Some(feed) = st.router.next_feed() else {
                return Ok(None);
            };
            let (channel, entry, text, label) = match feed {
                Feed::Announce {
                    channel,
                    label,
                    replay,
                    manual,
                } => {
                    if !st.config.announce {
                        continue;
                    }
                    let template = if replay {
                        &st.config.replay_text
                    } else {
                        &st.config.announce_text
                    };
                    let text = template.replace("{label}", &label);
                    if let Some(hook) = &st.on_announce {
                        hook(&Announced {
                            channel: channel.clone(),
                            label: label.clone(),
                            replay,
                            manual,
                        });
                    }
                    (channel, None, text, Some(label))
                }
                Feed::Entry { channel, entry } => {
                    let label = entry
                        .label
                        .clone()
                        .or_else(|| st.router.channel(&channel).and_then(|c| c.label.clone()));
                    (channel, Some(entry.id), entry.text, label)
                }
            };
            let host_tab = st.router.channel(&channel).and_then(|c| c.host_tab.clone());
            let id = self
                .reader
                .speak(&text, QueueMode::Append, interrupt, label)?;
            if st.tags.len() >= TAGS {
                st.tags.pop_front();
            }
            st.tags.push_back((
                id,
                Tag {
                    channel: channel.clone(),
                    host_tab,
                    announcement: entry.is_none(),
                    entry,
                },
            ));
            let f = InFlight {
                id,
                channel,
                entry,
                text,
            };
            st.in_flight = Some(f.clone());
            return Ok(Some(f));
        }
    }

    /// Cut the item in flight if it belongs to `channel`, reporting its
    /// entry dropped for `reason` when one is given.
    fn cut_if(&self, st: &mut State, channel: &str, reason: Option<&str>) -> Result<()> {
        if let (Some(reason), Some((item, e))) = (reason, cut_entry(st, channel)) {
            report(st, channel, vec![e], reason, Some(item));
        }
        if st.in_flight.as_ref().is_some_and(|f| f.channel == channel) {
            st.in_flight = None;
            st.router.done();
            self.reader.control(Control::Skip)?;
        }
        Ok(())
    }
}
