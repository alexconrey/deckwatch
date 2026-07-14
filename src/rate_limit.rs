// Per-namespace rate limiting for AI-agent jobs (diagnostics + ai-fix).
//
// AI jobs are the most expensive operator action deckwatch can trigger: each
// one spins up a K8s Job that pulls a large agent image, holds an API key,
// and burns LLM tokens on the customer's account. A misbehaving user, a
// runaway script, or an accidental double-click loop can rack up real
// spending in minutes. This module caps how many AI jobs any given namespace
// can start per hour.
//
// Design notes:
//
//   * **Sliding window over a fixed hour**, not a leaky bucket. Operators
//     understand "X per hour" easily, and the LLM billing meters are also
//     hourly-ish. The window is precise: we retain timestamps for the last
//     `WINDOW`-worth of jobs and drop older ones on every check.
//
//   * **In-memory, per-replica**. A deckwatch pod normally runs single-replica,
//     and the limit is a *safety cap* not a hard SLA — an operator running
//     HA can lift the limit or accept that the effective cluster-wide cap is
//     `limit * replicas`. Persisting counters to a ConfigMap would require
//     tolerating write races and would still be lossy across restarts; the
//     complexity isn't worth it for a soft cap. docs/AI_SAFETY.md is
//     explicit about this trade-off.
//
//   * **Fail-open on lock poisoning**. A poisoned mutex here would refuse
//     every AI job cluster-wide; the safety cap is not worth the outage.
//     We log and skip the check in that case.
//
//   * **Best-effort accounting**. `record` is called *after* the Job is
//     successfully created; if the caller crashes between the check and
//     the record the request is uncounted. Over-counting would be worse
//     (billing surprise), under-counting is a small window of extra spend.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Default per-namespace hourly cap. Chosen conservatively; operators can
/// raise it via Deckwatch settings. 10 jobs/hr covers casual debugging
/// (a handful of pods per shift) without letting a single crashlooping
/// deployment burn hundreds of dollars overnight.
pub const DEFAULT_HOURLY_LIMIT: u32 = 10;

/// Rolling window length. Fixed at one hour to match the human-friendly
/// "N per hour" mental model surfaced in the UI. If this ever becomes
/// configurable, plumb a Duration through the settings — do not overload
/// the limit field.
pub const WINDOW: Duration = Duration::from_secs(3600);

/// Snapshot returned by `check` and `snapshot`. Kept plain-Copy so the
/// handler can convert it into a JSON response without cloning.
#[derive(Debug, Clone, Copy)]
pub struct QuotaSnapshot {
    pub limit: u32,
    pub used: u32,
    /// Seconds until the oldest recorded job "falls out" of the window,
    /// i.e. the earliest moment a full-quota user regains a slot. `None`
    /// when the namespace hasn't run any jobs yet.
    pub reset_in_secs: Option<u64>,
}

impl QuotaSnapshot {
    pub fn remaining(&self) -> u32 {
        self.limit.saturating_sub(self.used)
    }

    pub fn exceeded(&self) -> bool {
        self.used >= self.limit
    }
}

/// Thread-safe, cloneable handle. Clone freely — the inner state is behind
/// an Arc<Mutex<...>> so all clones share the same counters.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
    /// Timestamps of recent job creations, newest last. Bounded by the
    /// per-namespace limit: even at the cap we keep at most `limit`
    /// entries because older ones are pruned before we could exceed it.
    per_ns: HashMap<String, Vec<Instant>>,
    limit: u32,
}

impl RateLimiter {
    pub fn new(limit: u32) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                per_ns: HashMap::new(),
                limit: limit.max(1),
            })),
        }
    }

    /// Hot-swap the cap. Existing recorded timestamps are kept — a shrunk
    /// cap can immediately push a namespace over quota, which is the right
    /// behavior (the operator just changed the policy and expects it to
    /// apply now).
    pub fn set_limit(&self, limit: u32) {
        let Ok(mut g) = self.inner.lock() else {
            tracing::warn!("rate limiter mutex poisoned; ignoring set_limit");
            return;
        };
        g.limit = limit.max(1);
    }

    pub fn limit(&self) -> u32 {
        self.inner
            .lock()
            .map(|g| g.limit)
            .unwrap_or(DEFAULT_HOURLY_LIMIT)
    }

    /// Read-only view of the namespace's current usage. Prunes expired
    /// entries as a side effect so the reported `used` is always in-window.
    pub fn snapshot(&self, ns: &str) -> QuotaSnapshot {
        let now = Instant::now();
        let Ok(mut g) = self.inner.lock() else {
            // Fail-open: report an empty window so the UI doesn't misreport
            // the operator as over-quota during a lock hiccup.
            return QuotaSnapshot {
                limit: DEFAULT_HOURLY_LIMIT,
                used: 0,
                reset_in_secs: None,
            };
        };
        let limit = g.limit;
        match g.per_ns.get_mut(ns) {
            Some(v) => {
                Self::prune(v, now);
                let used = v.len() as u32;
                let reset_in_secs = v.first().map(|oldest| {
                    let elapsed = now.duration_since(*oldest);
                    WINDOW.saturating_sub(elapsed).as_secs()
                });
                QuotaSnapshot {
                    limit,
                    used,
                    reset_in_secs,
                }
            }
            None => QuotaSnapshot {
                limit,
                used: 0,
                reset_in_secs: None,
            },
        }
    }

    /// Check whether the namespace can start another job. Does *not* record
    /// consumption — callers must call `record` after the job actually
    /// succeeds so a failure to create the Job doesn't burn a slot.
    pub fn check(&self, ns: &str) -> QuotaSnapshot {
        self.snapshot(ns)
    }

    /// Record that a job was successfully created. Idempotency is not
    /// provided — call exactly once per accepted request.
    pub fn record(&self, ns: &str) {
        let now = Instant::now();
        let Ok(mut g) = self.inner.lock() else {
            tracing::warn!(
                namespace = ns,
                "rate limiter mutex poisoned; skipping record",
            );
            return;
        };
        let entries = g.per_ns.entry(ns.to_string()).or_default();
        Self::prune(entries, now);
        entries.push(now);
    }

    fn prune(entries: &mut Vec<Instant>, now: Instant) {
        // Timestamps are inserted in monotonic order, so we can drop from
        // the front in one linear scan without sorting.
        let cutoff = match now.checked_sub(WINDOW) {
            Some(t) => t,
            None => return, // clock just started; nothing to prune
        };
        let keep_from = entries
            .iter()
            .position(|t| *t > cutoff)
            .unwrap_or(entries.len());
        if keep_from > 0 {
            entries.drain(..keep_from);
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_HOURLY_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_namespace_has_full_quota() {
        let rl = RateLimiter::new(5);
        let s = rl.snapshot("ns");
        assert_eq!(s.used, 0);
        assert_eq!(s.remaining(), 5);
        assert!(!s.exceeded());
        assert!(s.reset_in_secs.is_none());
    }

    #[test]
    fn record_increments_usage() {
        let rl = RateLimiter::new(3);
        rl.record("ns");
        rl.record("ns");
        let s = rl.snapshot("ns");
        assert_eq!(s.used, 2);
        assert_eq!(s.remaining(), 1);
        assert!(!s.exceeded());
    }

    #[test]
    fn exceeded_at_limit() {
        let rl = RateLimiter::new(2);
        rl.record("ns");
        rl.record("ns");
        let s = rl.snapshot("ns");
        assert!(s.exceeded());
        assert_eq!(s.remaining(), 0);
    }

    #[test]
    fn namespaces_isolated() {
        let rl = RateLimiter::new(2);
        rl.record("a");
        rl.record("a");
        assert!(rl.snapshot("a").exceeded());
        assert!(!rl.snapshot("b").exceeded());
    }

    #[test]
    fn set_limit_shrinks_immediately() {
        let rl = RateLimiter::new(10);
        for _ in 0..5 {
            rl.record("ns");
        }
        rl.set_limit(3);
        assert!(rl.snapshot("ns").exceeded());
    }

    #[test]
    fn zero_limit_normalized_to_one() {
        let rl = RateLimiter::new(0);
        assert_eq!(rl.limit(), 1);
    }
}
