//! Budget-aware poll scheduling.
//!
//! GitHub gives an authenticated token 5000 REST requests an hour, in a window
//! that resets at a wall-clock instant it tells us about. Two facts shape
//! everything here:
//!
//! 1. **A 304 is free.** Conditional requests that match an `ETag` are not
//!    charged against the primary limit. Since a repository's workflow runs
//!    change rarely, almost every poll of an idle project costs nothing. This
//!    is what lets the idle cadence be minutes rather than tens of minutes.
//!    See [`crate::cache`].
//! 2. **The quota is not ours alone.** The same token is used by `gh`, by the
//!    user's editor, by scripts. Spending it to the last request would break
//!    those instead of us, which is the worst possible failure to cause. So a
//!    reserve is carved out and never touched.
//!
//! The governor therefore never asks "may I send this request?" in isolation.
//! It computes an interval that fits the remaining budget into the remaining
//! window, and the engine sleeps for it.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::protocol::{BudgetView, Settings};

/// What GitHub told us in the `x-ratelimit-*` headers of the last response.
#[derive(Debug, Clone, Copy)]
struct Window {
    limit: u32,
    remaining: u32,
    /// Unix seconds at which `remaining` returns to `limit`.
    reset_at: i64,
}

impl Default for Window {
    fn default() -> Self {
        // Assume a full authenticated budget until GitHub corrects us. Assuming
        // an empty one would stall the first poll, which is the one the user is
        // watching.
        Self {
            limit: 5000,
            remaining: 5000,
            reset_at: 0,
        }
    }
}

#[derive(Debug)]
pub struct Governor {
    window: Window,
    /// Set by a 403/429 carrying `Retry-After`, or a secondary-limit body.
    /// Nothing goes out until this passes, whatever the primary budget says.
    throttled_until: Option<Instant>,
    /// Consecutive transport failures, for exponential backoff.
    failures: u32,
    requests: u64,
    conditional_hits: u64,
    /// Cheap deterministic jitter source. Two Omarchy machines sharing a token
    /// should not line their polls up, and pulling in `rand` for a handful of
    /// bits is not worth a dependency.
    seed: u64,
    last_interval: Duration,
}

impl Default for Governor {
    fn default() -> Self {
        Self::new()
    }
}

impl Governor {
    pub fn new() -> Self {
        Self {
            window: Window::default(),
            throttled_until: None,
            failures: 0,
            requests: 0,
            conditional_hits: 0,
            seed: seed_from_clock(),
            last_interval: Duration::from_secs(Settings::default().idle_interval),
        }
    }

    /// Record the rate-limit headers from any response. `status` distinguishes
    /// a free 304 from a charged 200 for the statistics the panel shows.
    pub fn observe(
        &mut self,
        status: u16,
        limit: Option<u32>,
        remaining: Option<u32>,
        reset: Option<i64>,
    ) {
        self.requests = self.requests.saturating_add(1);
        if status == 304 {
            self.conditional_hits = self.conditional_hits.saturating_add(1);
        }
        if let Some(limit) = limit {
            if limit > 0 {
                self.window.limit = limit;
            }
        }
        if let Some(remaining) = remaining {
            self.window.remaining = remaining;
        }
        if let Some(reset) = reset {
            self.window.reset_at = reset;
        }
        if (200..400).contains(&status) {
            self.failures = 0;
        }
    }

    /// Honour an explicit `Retry-After` (seconds) or a secondary rate limit.
    /// GitHub asks for at least a minute when it does not name a duration.
    pub fn throttle(&mut self, retry_after: Option<u64>) {
        let wait = retry_after.unwrap_or(60).clamp(1, 3600);
        let until = Instant::now() + Duration::from_secs(wait);
        // Never shorten an existing throttle: two 403s in a row must not let
        // the second one talk us out of the first one's backoff.
        self.throttled_until = Some(match self.throttled_until {
            Some(existing) if existing > until => existing,
            _ => until,
        });
    }

    /// A transport-level failure. Distinct from a throttle: the API did not
    /// ask us to slow down, we simply could not reach it.
    pub fn note_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }

    /// How long the engine must wait before the next cycle may start.
    pub fn cooldown(&self) -> Duration {
        self.throttled_until.map_or(Duration::ZERO, |until| {
            until.saturating_duration_since(Instant::now())
        })
    }

    /// True when a cycle would be pointless because we are still backing off.
    pub fn is_throttled(&self) -> bool {
        !self.cooldown().is_zero()
    }

    /// Requests we are willing to spend before the window resets.
    ///
    /// Truncating division is intended throughout this module: a reserve or a
    /// cycle count rounded *down* errs toward spending less quota, which is
    /// the safe direction. Hence the scoped allow rather than a float.
    #[allow(
        clippy::integer_division,
        reason = "budget maths rounds down on purpose"
    )]
    fn spendable(&self, settings: &Settings) -> u32 {
        let reserve = self
            .window
            .limit
            .saturating_mul(settings.reserve_percent.min(90))
            / 100;
        self.window.remaining.saturating_sub(reserve)
    }

    /// Seconds until the window resets, as an unsigned count.
    ///
    /// Falls back to a full hour before GitHub has told us anything, and is
    /// clamped to at least one so the division in [`Self::plan`] stays
    /// defined. Returning `u64` rather than `i64` means the clamp is the only
    /// place the sign is reasoned about, instead of at every call site.
    fn resets_in(&self) -> u64 {
        if self.window.reset_at == 0 {
            return 3600;
        }
        self.window
            .reset_at
            .saturating_sub(unix_now())
            .clamp(1, 3600)
            .unsigned_abs()
    }

    /// The core decision: the interval between poll cycles.
    ///
    /// `repos` is how many requests a worst-case cycle costs — one per
    /// repository, assuming every single one changed and so none of them came
    /// back 304. Budgeting for the worst case means the reserve holds even in
    /// the hour where every project is being actively pushed to.
    #[allow(
        clippy::integer_division,
        reason = "cycle count rounds down on purpose"
    )]
    pub fn plan(
        &mut self,
        repos: usize,
        focused: bool,
        any_running: bool,
        settings: &Settings,
    ) -> Duration {
        let desired = if focused {
            settings.focused_interval
        } else if any_running {
            settings.active_interval
        } else {
            settings.idle_interval
        };

        if repos == 0 {
            self.last_interval = Duration::from_secs(desired);
            return self.last_interval;
        }

        // Backoff first: a failing network is not a budget problem, and the
        // budget maths would happily suggest polling a dead endpoint every
        // 15 seconds forever.
        if self.failures > 0 {
            let exponent = self.failures.min(6);
            let backoff = desired.saturating_mul(1u64 << exponent).min(1800);
            self.last_interval = Duration::from_secs(self.jitter(backoff.max(desired)));
            return self.last_interval;
        }

        let cost = repos as u64;
        let spendable = u64::from(self.spendable(settings));

        // With nothing left to spend, wait out the window rather than issue
        // requests we know will be refused. A 403 costs the same as a 200 and
        // teaches us nothing.
        if spendable < cost {
            self.last_interval = Duration::from_secs(self.resets_in().min(3600));
            return self.last_interval;
        }

        let window = self.resets_in();
        let cycles = spendable / cost;
        // The interval at which the budget lasts exactly to the reset.
        let sustainable = window.div_ceil(cycles.max(1));

        let chosen = desired.max(sustainable).clamp(10, 21600);
        self.last_interval = Duration::from_secs(self.jitter(chosen));
        self.last_interval
    }

    /// ±6% so that several shells, or several machines on one token, drift
    /// apart instead of hammering the same second every cycle.
    #[allow(
        clippy::integer_division,
        reason = "a whole number of seconds is the unit here"
    )]
    fn jitter(&mut self, secs: u64) -> u64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        let spread = (secs / 16).max(1);
        let offset = self.seed % (spread * 2 + 1);
        secs.saturating_add(offset).saturating_sub(spread).max(5)
    }

    pub fn view(&self, settings: &Settings) -> BudgetView {
        BudgetView {
            limit: self.window.limit,
            remaining: self.window.remaining,
            resets_in: i64::try_from(self.resets_in()).unwrap_or(i64::MAX),
            spendable: self.spendable(settings),
            interval: self.last_interval.as_secs(),
            requests: self.requests,
            conditional_hits: self.conditional_hits,
            throttled_for: i64::try_from(self.cooldown().as_secs()).unwrap_or(i64::MAX),
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn seed_from_clock() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0x2545_F491_4F6C_DD1D, |d| d.subsec_nanos().into());
    // A zero seed is a fixed point of xorshift, which would make the jitter a
    // constant. Any non-zero constant will do as a fallback.
    if nanos == 0 {
        0x9E37_79B9_7F4A_7C15
    } else {
        nanos
    }
}

#[cfg(test)]
mod tests {
    // Tests assert; a failed assertion is the point, so the panic-free
    // lints that guard the poll loop are relaxed here and only here.
    #![allow(
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::integer_division
    )]

    use super::*;

    fn settings() -> Settings {
        Settings::default().sanitized()
    }

    /// Ignore jitter when asserting on an interval.
    fn about(actual: Duration, expected: u64) -> bool {
        let secs = actual.as_secs();
        let slack = (expected / 8).max(2);
        secs + slack >= expected && secs <= expected + slack
    }

    #[test]
    fn a_healthy_budget_polls_at_the_requested_cadence() {
        let mut governor = Governor::new();
        governor.observe(200, Some(5000), Some(4800), Some(unix_now() + 3000));
        let interval = governor.plan(5, true, false, &settings());
        assert!(
            about(interval, 15),
            "expected the focused cadence, got {interval:?}"
        );
    }

    #[test]
    fn a_thin_budget_stretches_the_interval() {
        let mut governor = Governor::new();
        // 300 left of 5000, reserve is 25% = 1250, so nothing is spendable...
        governor.observe(200, Some(5000), Some(300), Some(unix_now() + 1800));
        let interval = governor.plan(10, true, false, &settings());
        assert!(
            interval.as_secs() >= 1000,
            "a spent budget must wait for the reset, got {interval:?}"
        );
    }

    #[test]
    fn the_reserve_is_never_spent() {
        let governor = Governor::new();
        let mut governor = governor;
        governor.observe(200, Some(5000), Some(1250), Some(unix_now() + 60));
        // remaining == reserve exactly, so nothing is spendable.
        assert_eq!(governor.spendable(&settings()), 0);
    }

    #[test]
    fn many_repos_cost_proportionally_more() {
        let mut few = Governor::new();
        few.observe(200, Some(5000), Some(1500), Some(unix_now() + 3600));
        let mut many = Governor::new();
        many.observe(200, Some(5000), Some(1500), Some(unix_now() + 3600));

        let cheap = few.plan(2, false, false, &settings());
        let dear = many.plan(200, false, false, &settings());
        assert!(
            dear > cheap,
            "200 repos must poll slower than 2: {dear:?} vs {cheap:?}"
        );
    }

    #[test]
    fn failures_back_off_exponentially() {
        let mut governor = Governor::new();
        governor.observe(200, Some(5000), Some(5000), Some(unix_now() + 3600));
        let first = governor.plan(3, true, false, &settings());

        governor.note_failure();
        let after_one = governor.plan(3, true, false, &settings());
        governor.note_failure();
        governor.note_failure();
        let after_three = governor.plan(3, true, false, &settings());

        assert!(after_one > first, "one failure should slow us down");
        assert!(
            after_three > after_one,
            "further failures should slow us more"
        );
        assert!(after_three.as_secs() <= 1800, "backoff must stay bounded");
    }

    #[test]
    fn a_success_clears_the_backoff() {
        let mut governor = Governor::new();
        governor.note_failure();
        governor.note_failure();
        governor.observe(200, Some(5000), Some(4000), Some(unix_now() + 3600));
        assert_eq!(governor.failures, 0);
    }

    #[test]
    fn retry_after_is_honoured_and_never_shortened() {
        let mut governor = Governor::new();
        governor.throttle(Some(120));
        let long = governor.cooldown();
        governor.throttle(Some(5));
        let still_long = governor.cooldown();
        assert!(
            still_long.as_secs() + 2 >= long.as_secs(),
            "a shorter retry-after must not cancel a longer one"
        );
        assert!(governor.is_throttled());
    }

    #[test]
    fn conditional_hits_are_counted_but_not_charged() {
        let mut governor = Governor::new();
        governor.observe(304, Some(5000), Some(4999), Some(unix_now() + 3600));
        governor.observe(200, Some(5000), Some(4998), Some(unix_now() + 3600));
        let view = governor.view(&settings());
        assert_eq!(view.requests, 2);
        assert_eq!(view.conditional_hits, 1);
    }

    #[test]
    fn no_repos_means_no_budget_pressure() {
        let mut governor = Governor::new();
        governor.observe(200, Some(5000), Some(0), Some(unix_now() + 3600));
        let interval = governor.plan(0, false, false, &settings());
        assert!(about(interval, settings().idle_interval));
    }

    #[test]
    fn jitter_stays_within_bounds_and_moves() {
        let mut governor = Governor::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let value = governor.jitter(160);
            assert!(
                (140..=180).contains(&value),
                "jitter escaped its band: {value}"
            );
            seen.insert(value);
        }
        assert!(seen.len() > 1, "jitter must actually vary");
    }
}
