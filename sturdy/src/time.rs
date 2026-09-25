//! `sturdy::time`: `sleep`/`timeout`, matching `tokio::time`'s common-case shape.
//!
//! ## This is a timer *driver*, not a thread pool
//!
//! There is exactly **one** dedicated OS thread here (lazily started on first use), and it is
//! architecturally nothing like the engine's `SFT::Async::Scheduler` or a work-stealing pool: it
//! never polls a future, never runs task code, and holds no state beyond a min-heap of
//! `(deadline, id, Waker)` triples behind a `Mutex` + `Condvar`. Its entire job is to sleep until
//! the next deadline (or until a nearer one is registered, via the condvar) and then call
//! `.wake()` on whichever wakers have expired. Exactly like every other `Waker` in this crate,
//! calling `.wake()` here does not run any code belonging to the sleeping future itself — it just
//! resubmits that future's next poll to the engine's scheduler (see `task.rs`'s `Wake` impl),
//! which is what actually runs it, on the engine's own worker pool. Do not mistake this for a
//! second scheduler: it does no load balancing and cannot run arbitrary work, only wake things up
//! on time.

use std::cmp::Ordering as CmpOrdering;
use std::collections::BinaryHeap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

// Re-exported for API-surface parity with `tokio::time::{Duration, Instant}`.
pub use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// Timer driver thread
// ---------------------------------------------------------------------------------------------

struct TimerEntry {
    deadline: Instant,
    id: u64,
    waker: Waker,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}
impl Eq for TimerEntry {}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // `BinaryHeap` is a max-heap; reverse so the *earliest* deadline sorts to the top.
        other.deadline.cmp(&self.deadline).then_with(|| other.id.cmp(&self.id))
    }
}

struct Driver {
    heap: Mutex<BinaryHeap<TimerEntry>>,
    condvar: Condvar,
    next_id: AtomicU64,
}

impl Driver {
    fn register(&'static self, deadline: Instant, waker: Waker) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut heap = self.heap.lock().unwrap();
        // Only need to wake the driver thread if this entry is nearer than whatever it's
        // currently waiting on (or it wasn't waiting on anything at all).
        let wake_driver = heap.peek().is_none_or(|top| deadline < top.deadline);
        heap.push(TimerEntry { deadline, id, waker });
        drop(heap);
        if wake_driver {
            self.condvar.notify_one();
        }
    }
}

fn driver() -> &'static Driver {
    static DRIVER: OnceLock<&'static Driver> = OnceLock::new();
    DRIVER.get_or_init(|| {
        let driver: &'static Driver = Box::leak(Box::new(Driver {
            heap: Mutex::new(BinaryHeap::new()),
            condvar: Condvar::new(),
            next_id: AtomicU64::new(0),
        }));
        std::thread::Builder::new()
            .name("sturdy-timer".into())
            .spawn(move || run_driver_thread(driver))
            .expect("failed to spawn sturdy timer driver thread");
        driver
    })
}

fn run_driver_thread(driver: &'static Driver) {
    let mut heap = driver.heap.lock().unwrap();
    loop {
        // Pop and collect every entry whose deadline has already passed, then drop the lock
        // before calling `.wake()` (a waker can synchronously re-enter `register` on the same
        // thread depending on the executor, e.g. via `block_on`'s immediate poll-on-wake — never
        // call arbitrary code while holding the heap's lock).
        let now = Instant::now();
        let mut expired = Vec::new();
        while heap.peek().is_some_and(|top| top.deadline <= now) {
            expired.push(heap.pop().unwrap());
        }
        if !expired.is_empty() {
            drop(heap);
            for entry in expired {
                entry.waker.wake();
            }
            heap = driver.heap.lock().unwrap();
            continue;
        }
        heap = match heap.peek() {
            None => driver.condvar.wait(heap).unwrap(),
            Some(top) => {
                let wait_for = top.deadline.saturating_duration_since(Instant::now());
                driver.condvar.wait_timeout(heap, wait_for).unwrap().0
            }
        };
    }
}

// ---------------------------------------------------------------------------------------------
// Sleep / sleep()
// ---------------------------------------------------------------------------------------------

/// The future returned by [`sleep`]; a named type in case a caller wants to construct one
/// directly (e.g. to `reset` it) rather than going through the free function.
pub struct Sleep {
    deadline: Instant,
}

impl Sleep {
    pub fn new(duration: Duration) -> Self {
        Self { deadline: Instant::now() + duration }
    }

    /// Completes at an explicit instant rather than after a relative duration. Used by
    /// [`Interval::tick`] to sleep until its next scheduled tick without drifting by however long
    /// the previous tick's body took to run.
    pub fn until(deadline: Instant) -> Self {
        Self { deadline }
    }

    /// The instant this sleep will (or did) complete.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Reschedules this sleep to fire `duration` from now. Any wakeup already registered against
    /// the old deadline is left in the driver's heap rather than removed — harmless: it just
    /// fires early against the stale deadline, wakes the task, which re-polls `Sleep`, sees the
    /// new (later) deadline hasn't passed yet, and registers a fresh entry for it. At worst one
    /// extra harmless early wakeup, never a missed one.
    pub fn reset(&mut self, duration: Duration) {
        self.deadline = Instant::now() + duration;
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if Instant::now() >= self.deadline {
            return Poll::Ready(());
        }
        // Always register a fresh waker on every `Pending`, not just the first — the same
        // missed-wakeup discipline as `sync.rs`'s futures (see its module doc): the driver thread
        // fires a `Waker` at most once (it's popped off the heap when woken), so a `Sleep` that's
        // re-polled while still pending (e.g. after `reset`, or a spurious wake) must push a new
        // entry or the real deadline would never fire.
        driver().register(self.deadline, cx.waker().clone());
        Poll::Pending
    }
}

/// Completes after `duration` has elapsed. Backed by the dedicated timer driver thread described
/// in this module's docs — not the engine scheduler, which has no notion of time.
pub fn sleep(duration: Duration) -> Sleep {
    Sleep::new(duration)
}

// ---------------------------------------------------------------------------------------------
// timeout()
// ---------------------------------------------------------------------------------------------

/// Error returned by [`timeout`] when the timer fires before `future` completes, matching
/// `tokio::time::error::Elapsed`'s shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed(());

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("deadline has elapsed")
    }
}

impl std::error::Error for Elapsed {}

/// Races `future` against a [`sleep`] timer; resolves to `Err(Elapsed)` if the timer fires first,
/// otherwise to `future`'s output. Built directly on [`crate::select::select2`] — a `timeout` is
/// just a two-way `select!` between the future and a `Sleep`.
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, Elapsed> {
    match crate::select::select2(future, sleep(duration)).await {
        crate::select::Either::Left(value) => Ok(value),
        crate::select::Either::Right(()) => Err(Elapsed(())),
    }
}

// ---------------------------------------------------------------------------------------------
// interval()
// ---------------------------------------------------------------------------------------------

/// A repeating timer, matching `tokio::time::interval`'s common-case shape (`MissedTickBehavior::
/// Burst`, the tokio default): each call to [`Interval::tick`] resolves at the next multiple of
/// `period` since the interval was created, with the first tick resolving immediately.
///
/// Built directly on [`Sleep::until`] (via the same driver thread `sleep`/`timeout` use) rather
/// than repeatedly calling [`sleep`] with a relative duration, so ticks land on a fixed schedule
/// instead of drifting by however long each tick's body takes to run.
pub struct Interval {
    period: Duration,
    next: Instant,
}

impl Interval {
    /// Resolves when the next tick is due (immediately, for the very first call). If the caller
    /// fell behind by more than one period (the previous tick's body took too long), this catches
    /// up to `now + period` rather than firing a burst of already-elapsed ticks back-to-back —
    /// matching Tokio's `MissedTickBehavior::Burst` default, whose "burst" is bounded to at most
    /// one immediate catch-up tick, not one per missed period.
    pub async fn tick(&mut self) -> Instant {
        Sleep::until(self.next).await;
        let fired_at = Instant::now();
        self.next += self.period;
        if self.next < fired_at {
            // Fell behind by more than one period: catch up to one period from now instead of
            // firing a burst of already-elapsed ticks back-to-back.
            self.next = fired_at + self.period;
        }
        fired_at
    }

    /// The configured period between ticks.
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Reschedules the next tick to `period` from now, discarding the original fixed schedule.
    pub fn reset(&mut self) {
        self.next = Instant::now() + self.period;
    }
}

/// Creates an [`Interval`] that first fires immediately and then every `period` thereafter.
pub fn interval(period: Duration) -> Interval {
    Interval { period, next: Instant::now() }
}
