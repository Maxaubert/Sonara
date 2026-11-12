//! Earcons one after the other (#238). Two earcons fired close together
//! (a turn ends, and a session switch chimes 1 ms later) used to be mixed
//! on top of each other, which sounds like an error. `Schedule` gives each
//! earcon a start time: at once when nothing is playing, else `GAP` after
//! the end of the earcon before it, so they play in the order they were
//! triggered and never overlap. An earcon that starts late (a busy
//! machine wakes its thread late) moves the ones after it and the hold on
//! the announcement by as much (`started`). A burst never plays a long chain: an
//! earcon equal to the one scheduled right before it (still waiting or
//! playing) is dropped, and at most `MAX` earcons wait or play at a time.
//! Pure: the agent plays what it is told, when.
use crate::Earcon;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The pause between two earcons that follow each other.
pub const GAP: Duration = Duration::from_millis(60);
/// The most earcons waiting or playing at one time.
pub const MAX: usize = 3;
/// How long after a key's sound (the flush sound, which plays at once and is
/// not queued) the chimes and the speech that follow it wait.
pub const AFTER_KEY: Duration = Duration::from_millis(300);

/// Why an earcon was not scheduled.
pub const DUPLICATE: &str = "the same earcon is queued right before it";
pub const FULL: &str = "too many earcons queued";
/// Why a waiting earcon was not played.
pub const MUTED: &str = "mute level 2";

/// One earcon waiting or playing.
#[derive(Debug)]
struct Slot {
    earcon: Earcon,
    /// The start `admit` gave it, which names it (`start_of`, `started`).
    due: Instant,
    start: Instant,
    end: Instant,
}

/// The earcons waiting or playing, with when each starts and ends.
#[derive(Debug, Default)]
pub struct Schedule {
    slots: VecDeque<Slot>,
    /// When the last key's sound ends (it plays at once and is not queued).
    key_end: Option<Instant>,
    /// When the held announcement may start (`hold`).
    hold: Option<Instant>,
}

impl Schedule {
    /// A key's sound of `len` plays at `now`: what is scheduled after it
    /// starts `AFTER_KEY` after it ended.
    pub fn key_sound(&mut self, len: Duration, now: Instant) {
        let end = now + len;
        self.key_end = Some(self.key_end.map_or(end, |k| k.max(end)));
    }

    /// The earliest start for what is scheduled at `now`: `now`, or `AFTER_KEY`
    /// after the last key's sound when that is still to come.
    fn floor(&self, now: Instant) -> Instant {
        match self.key_end {
            Some(end) if end + AFTER_KEY > now => end + AFTER_KEY,
            _ => now,
        }
    }

    /// Schedule `e`, lasting `len`, at `now`: its start time, or why it is
    /// dropped.
    pub fn admit(
        &mut self,
        e: Earcon,
        len: Duration,
        now: Instant,
    ) -> Result<Instant, &'static str> {
        self.slots.retain(|s| s.end > now);
        let floor = self.floor(now);
        let start = match self.slots.back() {
            Some(last) if last.earcon == e => return Err(DUPLICATE),
            Some(_) if self.slots.len() >= MAX => return Err(FULL),
            Some(last) => (last.end + GAP).max(floor),
            None => floor,
        };
        self.slots.push_back(Slot {
            earcon: e,
            due: start,
            start,
            end: start + len,
        });
        Ok(start)
    }

    /// When the earcon `admit` gave `due` starts now (later when one before
    /// it started late); `due` when it is no longer scheduled.
    pub fn start_of(&self, due: Instant) -> Instant {
        self.slots
            .iter()
            .find(|s| s.due == due)
            .map_or(due, |s| s.start)
    }

    /// The announcement is held until `until` (the reader's `hold_start`).
    pub fn hold(&mut self, until: Instant) {
        self.hold = Some(until);
    }

    /// The earcon `admit` gave `due` started at `at`. When that is late, it
    /// and the earcons after it move by as much, and so does a hold still
    /// in force: its new end is returned, for the reader.
    pub fn started(&mut self, due: Instant, at: Instant) -> Option<Instant> {
        let i = self.slots.iter().position(|s| s.due == due)?;
        let late = at.saturating_duration_since(self.slots[i].start);
        if late.is_zero() {
            return None;
        }
        for s in self.slots.iter_mut().skip(i) {
            s.start += late;
            s.end += late;
        }
        let held = self.hold.filter(|h| *h > at)?;
        let until = held.max(self.quiet_at(at));
        self.hold = Some(until);
        (until > held).then_some(until)
    }

    /// Forget the earcons that have not started at `now` (they will not
    /// be played: mute level 2), so the next ones do not wait for them.
    pub fn drop_waiting(&mut self, now: Instant) {
        self.slots.retain(|s| s.start <= now);
    }

    /// When the last earcon scheduled has ended and its gap passed (`now`
    /// when none is): what follows the earcons starts then.
    pub fn quiet_at(&self, now: Instant) -> Instant {
        let floor = self.floor(now);
        self.slots
            .back()
            .map_or(floor, |s| (s.end + GAP).max(floor))
    }
}

/// How long a mono clip of `samples` at `rate` lasts.
pub fn length(samples: usize, rate: u32) -> Duration {
    Duration::from_secs_f64(samples as f64 / f64::from(rate.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn an_earcon_after_another_starts_once_it_ended_and_the_gap_passed() {
        let mut s = Schedule::default();
        let t = Instant::now();
        assert_eq!(s.admit(Earcon::TurnDone, ms(750), t), Ok(t));
        let next = s.admit(Earcon::SessionChange, ms(1000), t + ms(2));
        assert_eq!(next, Ok(t + ms(750) + GAP));
        assert_eq!(s.quiet_at(t), t + ms(750) + GAP + ms(1000) + GAP);
    }

    #[test]
    fn an_earcon_that_starts_late_moves_the_ones_after_it_and_the_hold() {
        let mut s = Schedule::default();
        let t = Instant::now();
        s.admit(Earcon::TurnDone, ms(750), t).unwrap();
        let due = s.admit(Earcon::SessionChange, ms(1000), t + ms(2)).unwrap();
        let quiet = s.quiet_at(t);
        s.hold(quiet);
        // turn_done starts 150 ms late: session_change and the hold follow.
        assert_eq!(s.started(t, t + ms(150)), Some(quiet + ms(150)));
        assert_eq!(s.start_of(due), due + ms(150));
        // session_change then starts on its new time: nothing moves.
        assert_eq!(s.started(due, due + ms(150)), None);
        assert_eq!(s.quiet_at(t), quiet + ms(150));
        // On time, or with no hold in force, there is nothing for the reader.
        let mut s = Schedule::default();
        s.admit(Earcon::TurnDone, ms(750), t).unwrap();
        assert_eq!(s.started(t, t), None);
        assert_eq!(s.started(t, t + ms(150)), None);
        assert_eq!(s.quiet_at(t), t + ms(150) + ms(750) + GAP);
    }

    #[test]
    fn an_earcon_after_a_key_sound_waits_for_it_and_a_third_of_a_second() {
        // The flush sound plays at once and is not queued, yet the chime of
        // the session switch that follows must not start under it.
        let mut s = Schedule::default();
        let t = Instant::now();
        s.key_sound(ms(333), t);
        assert_eq!(
            s.admit(Earcon::SessionChange, ms(1000), t + ms(2)),
            Ok(t + ms(333) + AFTER_KEY)
        );
        assert_eq!(s.quiet_at(t), t + ms(333) + AFTER_KEY + ms(1000) + GAP);
        // A key sound long over changes nothing.
        let mut s = Schedule::default();
        s.key_sound(ms(333), t);
        let later = t + ms(5000);
        assert_eq!(s.admit(Earcon::TurnDone, ms(750), later), Ok(later));
    }

    #[test]
    fn an_earcon_after_the_last_one_ended_starts_at_once() {
        let mut s = Schedule::default();
        let t = Instant::now();
        s.admit(Earcon::Nav, ms(14), t).unwrap();
        let later = t + ms(500);
        assert_eq!(s.admit(Earcon::Nav, ms(14), later), Ok(later));
        assert_eq!(s.quiet_at(later + ms(100)), later + ms(100));
    }

    #[test]
    fn a_duplicate_back_to_back_and_a_fourth_earcon_are_dropped() {
        let mut s = Schedule::default();
        let t = Instant::now();
        s.admit(Earcon::TurnDone, ms(750), t).unwrap();
        assert_eq!(s.admit(Earcon::TurnDone, ms(750), t), Err(DUPLICATE));
        s.admit(Earcon::Choice, ms(350), t).unwrap();
        s.admit(Earcon::TurnDone, ms(750), t).unwrap();
        assert_eq!(s.admit(Earcon::Error, ms(900), t), Err(FULL));
    }

    #[test]
    fn dropping_the_waiting_earcons_keeps_the_one_playing() {
        let mut s = Schedule::default();
        let t = Instant::now();
        s.admit(Earcon::TurnDone, ms(750), t).unwrap();
        s.admit(Earcon::Choice, ms(350), t).unwrap();
        s.drop_waiting(t + ms(10));
        assert_eq!(s.quiet_at(t), t + ms(750) + GAP);
    }
}
