//! Earcons one after the other (#238). Two earcons fired close together
//! (a turn ends, and a session switch chimes 1 ms later) used to be mixed
//! on top of each other, which sounds like an error. `Schedule` gives each
//! earcon a start time: at once when nothing is playing, else `GAP` after
//! the end of the earcon before it, so they play in the order they were
//! triggered and never overlap. A burst never plays a long chain: an
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

/// Why an earcon was not scheduled.
pub const DUPLICATE: &str = "the same earcon is queued right before it";
pub const FULL: &str = "too many earcons queued";
/// Why a waiting earcon was not played.
pub const MUTED: &str = "mute level 2";

/// The earcons waiting or playing, with when each starts and ends.
#[derive(Debug, Default)]
pub struct Schedule {
    slots: VecDeque<(Earcon, Instant, Instant)>,
}

impl Schedule {
    /// Schedule `e`, lasting `len`, at `now`: its start time, or why it is
    /// dropped.
    pub fn admit(
        &mut self,
        e: Earcon,
        len: Duration,
        now: Instant,
    ) -> Result<Instant, &'static str> {
        self.slots.retain(|(_, _, end)| *end > now);
        if let Some((last, _, end)) = self.slots.back() {
            if *last == e {
                return Err(DUPLICATE);
            }
            if self.slots.len() >= MAX {
                return Err(FULL);
            }
            let start = (*end + GAP).max(now);
            self.slots.push_back((e, start, start + len));
            return Ok(start);
        }
        self.slots.push_back((e, now, now + len));
        Ok(now)
    }

    /// Forget the earcons that have not started at `now` (they will not
    /// be played: mute level 2), so the next ones do not wait for them.
    pub fn drop_waiting(&mut self, now: Instant) {
        self.slots.retain(|(_, start, _)| *start <= now);
    }

    /// When the last earcon scheduled has ended and its gap passed (`now`
    /// when none is): what follows the earcons starts then.
    pub fn quiet_at(&self, now: Instant) -> Instant {
        self.slots
            .back()
            .map_or(now, |(_, _, end)| (*end + GAP).max(now))
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
