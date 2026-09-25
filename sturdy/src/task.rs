//! A tiny Tokio-shaped async task API layered over the engine's own worker-pool scheduler
//! (`sturdy_sys::diagnostics::ffi::scheduler_spawn`).
//!
//! This is *not* a work-stealing executor like Tokio's real one: there is exactly one thread
//! pool (the engine's `SFT::Async::Scheduler`), and "scheduling" a task means submitting a
//! single "poll this future once" closure to it. The names and shapes ([`spawn`], [`JoinHandle`],
//! [`JoinError`], [`yield_now`]) deliberately mirror `tokio::task` so the API is familiar, even
//! though the machinery underneath is much simpler.
//!
//! ## Why panics don't abort here
//!
//! [`crate::diagnostics::spawn`] (and the `guard()` pattern in `sturdy/src/runtime.rs`) abort the
//! process on panic, because those closures can be invoked from C++ stack frames where unwinding
//! across the FFI boundary is undefined behavior. **This module is different.** A future spawned
//! with [`spawn`] is only ever polled from plain Rust code (the closure submitted to
//! `scheduler_spawn` does nothing but call `Future::poll`); the poll never crosses into C++, so
//! there is nothing for a panic to unwind through unsafely. That makes `catch_unwind` around each
//! poll both safe and the more useful behavior: it turns a panicking task into an `Err(JoinError)`
//! that the caller can observe, instead of taking down the whole process. Do not "fix" this to
//! match `guard()`'s abort-on-panic behavior — the two situations are not the same.

use std::error::Error as StdError;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

// ---------------------------------------------------------------------------------------------
// JoinError
// ---------------------------------------------------------------------------------------------

/// Why a [`JoinHandle`] resolved to an error instead of the task's output.
///
/// Mirrors the shape of `tokio::task::JoinError` (a panic payload or a cancellation), not its
/// internals.
pub struct JoinError {
    kind: JoinErrorKind,
}

enum JoinErrorKind {
    Panic(Box<dyn std::any::Any + Send + 'static>),
    Cancelled,
}

impl JoinError {
    fn panic(payload: Box<dyn std::any::Any + Send + 'static>) -> Self {
        Self { kind: JoinErrorKind::Panic(payload) }
    }

    pub fn cancelled() -> Self {
        Self { kind: JoinErrorKind::Cancelled }
    }

    /// Whether the task panicked (as opposed to being aborted).
    pub fn is_panic(&self) -> bool {
        matches!(self.kind, JoinErrorKind::Panic(_))
    }

    /// Whether the task was aborted via [`JoinHandle::abort`].
    pub fn is_cancelled(&self) -> bool {
        matches!(self.kind, JoinErrorKind::Cancelled)
    }

    /// Consumes the error, returning the panic payload if this was a panic.
    ///
    /// The payload is whatever was passed to `panic!`; downcast it (e.g. to `&str` or `String`)
    /// to recover a message, matching `std::panic::catch_unwind`'s own convention.
    pub fn into_panic(self) -> Box<dyn std::any::Any + Send + 'static> {
        match self.kind {
            JoinErrorKind::Panic(payload) => payload,
            JoinErrorKind::Cancelled => Box::new("JoinError::into_panic called on a cancelled task"),
        }
    }
}

impl fmt::Debug for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            JoinErrorKind::Panic(_) => f.write_str("JoinError::Panic(..)"),
            JoinErrorKind::Cancelled => f.write_str("JoinError::Cancelled"),
        }
    }
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            JoinErrorKind::Panic(_) => write!(f, "task panicked"),
            JoinErrorKind::Cancelled => write!(f, "task was aborted"),
        }
    }
}

impl StdError for JoinError {}

// ---------------------------------------------------------------------------------------------
// Task state machine
// ---------------------------------------------------------------------------------------------

// The classic atomic-state-machine pattern for a single re-entrant "poll on wake" task:
//   IDLE              - not currently being polled; a wake should schedule a poll.
//   POLLING           - currently being polled on some worker thread.
//   POLLING_NOTIFIED  - being polled, *and* woken again while that poll is in flight; when the
//                       in-flight poll finishes it must immediately reschedule instead of going
//                       back to IDLE, so the wakeup is never lost.
//   COMPLETE          - the future has resolved (or panicked/aborted); no more polls.
//
// This guarantees: (a) a wake arriving during a poll never causes a second concurrent poll of the
// same future (which would be unsound), and (b) it is never silently dropped either.
const IDLE: u8 = 0;
const POLLING: u8 = 1;
const POLLING_NOTIFIED: u8 = 2;
const COMPLETE: u8 = 3;

struct Shared<T> {
    state: AtomicU8,
    /// `None` until the future resolves/panics/is cancelled, `Some` after. Guarded by a plain
    /// mutex: contention is negligible (touched once by the poller, once by the joiner).
    output: Mutex<Option<Result<T, JoinError>>>,
    /// Waker for whoever is `.await`-ing the `JoinHandle` (not the task future's own waker).
    joiner: Mutex<Option<Waker>>,
    /// `Arc`-wrapped (rather than a plain `AtomicBool`) so [`JoinHandle::abort_handle`] can hand
    /// out a detached, non-generic [`AbortHandle`] that shares this exact flag without holding a
    /// reference to `Shared<T>` (and therefore without being generic over `T`) — used by
    /// [`JoinSet::abort_all`].
    aborted: Arc<std::sync::atomic::AtomicBool>,
}

/// The unit of work resubmitted to the scheduler: "poll the future once, then decide what to do
/// next based on the state machine".
struct TaskCell<F: Future> {
    shared: Arc<Shared<F::Output>>,
    // `None` once the future has resolved/panicked/been aborted and its slot has been vacated.
    future: Mutex<Option<Pin<Box<F>>>>,
}

impl<F> Wake for TaskCell<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn wake(self: Arc<Self>) {
        Self::wake_by_ref(&self)
    }

    fn wake_by_ref(self: &Arc<Self>) {
        loop {
            match self.shared.state.compare_exchange(
                IDLE,
                POLLING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    // We transitioned IDLE -> POLLING ourselves: schedule a poll.
                    schedule_poll(self.clone());
                    return;
                }
                Err(POLLING) => {
                    // Someone else is polling right now; tell them to poll again afterwards.
                    match self.shared.state.compare_exchange(
                        POLLING,
                        POLLING_NOTIFIED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => return,
                        Err(_) => continue, // state changed under us, retry
                    }
                }
                Err(POLLING_NOTIFIED) => return, // already going to be polled again
                Err(COMPLETE) => return,         // nothing to do
                Err(_) => continue,
            }
        }
    }
}

fn schedule_poll<F>(cell: Arc<TaskCell<F>>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    sturdy_sys::diagnostics::ffi::scheduler_spawn(
        Box::new(sturdy_sys::diagnostics::SchedulerTask(Some(Box::new(move || {
            poll_once(cell);
        })))),
        false,
    );
}

fn poll_once<F>(cell: Arc<TaskCell<F>>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    // Aborted before we ever got to poll: finish immediately without touching the future logic.
    if cell.shared.aborted.load(Ordering::Acquire) {
        finish(&cell, Err(JoinError::cancelled()));
        return;
    }

    let waker = Waker::from(cell.clone());
    let mut cx = Context::from_waker(&waker);

    // Take the future out so we can poll it without holding the mutex across `poll` (poll may
    // itself call back into code that touches `cell`, e.g. via a nested wake).
    let mut fut = match cell.future.lock().unwrap().take() {
        Some(fut) => fut,
        None => return, // already completed/aborted concurrently
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fut.as_mut().poll(&mut cx)));

    match result {
        Ok(Poll::Ready(value)) => {
            finish(&cell, Ok(value));
        }
        Ok(Poll::Pending) => {
            if cell.shared.aborted.load(Ordering::Acquire) {
                // Aborted while we were polling: drop the future now, don't put it back.
                finish(&cell, Err(JoinError::cancelled()));
                return;
            }
            // Put the future back for the next poll.
            *cell.future.lock().unwrap() = Some(fut);
            // Decide what happens next based on whether we were woken while polling.
            loop {
                match cell.shared.state.compare_exchange(
                    POLLING,
                    IDLE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return, // back to idle, next wake() will schedule a poll
                    Err(POLLING_NOTIFIED) => {
                        // Woken while polling: go straight back to POLLING and reschedule.
                        match cell.shared.state.compare_exchange(
                            POLLING_NOTIFIED,
                            POLLING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ) {
                            Ok(_) => {
                                schedule_poll(cell.clone());
                                return;
                            }
                            Err(_) => continue,
                        }
                    }
                    Err(_) => continue,
                }
            }
        }
        Err(panic) => {
            finish(&cell, Err(JoinError::panic(panic)));
        }
    }
}

fn finish<F>(cell: &Arc<TaskCell<F>>, result: Result<F::Output, JoinError>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    *cell.future.lock().unwrap() = None;
    *cell.shared.output.lock().unwrap() = Some(result);
    cell.shared.state.store(COMPLETE, Ordering::Release);
    if let Some(waker) = cell.shared.joiner.lock().unwrap().take() {
        waker.wake();
    }
}

// ---------------------------------------------------------------------------------------------
// spawn / JoinHandle
// ---------------------------------------------------------------------------------------------

/// Spawns `future` onto the engine's scheduler and returns a [`JoinHandle`] to `.await` its
/// output.
///
/// The future is polled by resubmitting a "poll once" closure to
/// `sturdy_sys::diagnostics::ffi::scheduler_spawn` each time it is woken, never concurrently (see
/// the state machine documented on [`TaskCell`]/[`Wake`] impl above). A panic inside `future` is
/// caught and reported through the returned handle as `Err(JoinError)` rather than aborting the
/// process — see the module-level docs for why that's correct here.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let shared = Arc::new(Shared {
        state: AtomicU8::new(POLLING),
        output: Mutex::new(None),
        joiner: Mutex::new(None),
        aborted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    let cell = Arc::new(TaskCell { shared: shared.clone(), future: Mutex::new(Some(Box::pin(future))) });
    schedule_poll(cell);
    JoinHandle { shared }
}

/// Handle to a task spawned with [`spawn`]. Implements `Future<Output = Result<T, JoinError>>`,
/// matching `tokio::task::JoinHandle`'s shape.
pub struct JoinHandle<T> {
    shared: Arc<Shared<T>>,
}

impl<T> JoinHandle<T> {
    /// Best-effort cancellation: marks the task aborted. Safe to call from any thread, at any
    /// time, including after completion (no-op) or concurrently with the task running — the next
    /// poll (or the current one, if in flight) after this call observes the abort and resolves to
    /// `Err(JoinError::cancelled())` instead of continuing to poll the inner future.
    pub fn abort(&self) {
        self.shared.aborted.store(true, Ordering::Release);
    }

    /// Whether the task has finished (successfully, panicked, or been aborted).
    pub fn is_finished(&self) -> bool {
        self.shared.state.load(Ordering::Acquire) == COMPLETE
    }

    /// Returns a detached [`AbortHandle`] that can abort this task without holding on to the
    /// `JoinHandle` itself (e.g. so a [`JoinSet`] can track it after moving the handle into an
    /// adapter task — see [`JoinSet::spawn`]).
    pub fn abort_handle(&self) -> AbortHandle {
        AbortHandle { aborted: self.shared.aborted.clone() }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.shared.state.load(Ordering::Acquire) == COMPLETE {
            if let Some(result) = self.shared.output.lock().unwrap().take() {
                return Poll::Ready(result);
            }
            // Output already taken by a previous poll (handle polled again after Ready); tokio's
            // JoinHandle also only yields the value once.
            return Poll::Ready(Err(JoinError::cancelled()));
        }
        *self.shared.joiner.lock().unwrap() = Some(cx.waker().clone());
        // Re-check after registering the waker to avoid a lost wakeup if the task completed
        // between the load above and registering.
        if self.shared.state.load(Ordering::Acquire) == COMPLETE
            && let Some(result) = self.shared.output.lock().unwrap().take() {
                return Poll::Ready(result);
            }
        Poll::Pending
    }
}

// ---------------------------------------------------------------------------------------------
// yield_now
// ---------------------------------------------------------------------------------------------

/// Cooperatively yields once: returns `Pending` the first time it is polled (immediately
/// rewaking itself so the executor polls it again), then `Ready` the second time. Matches
/// `tokio::task::yield_now`'s contract.
pub fn yield_now() -> impl Future<Output = ()> {
    struct YieldNow {
        yielded: bool,
    }

    impl Future for YieldNow {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.yielded {
                return Poll::Ready(());
            }
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    YieldNow { yielded: false }
}

// ---------------------------------------------------------------------------------------------
// block_on
// ---------------------------------------------------------------------------------------------

/// Drives `future` to completion on the calling thread using a `Condvar`-based waker, for use
/// *outside* a running engine loop (tests, tools, `main` before/after `sturdy::run`). Not backed
/// by the scheduler at all — this just parks the current thread between wakeups.
pub fn block_on<F: Future>(future: F) -> F::Output {
    use std::sync::Condvar;

    struct ParkWaker {
        mutex: Mutex<bool>,
        condvar: Condvar,
    }

    impl Wake for ParkWaker {
        fn wake(self: Arc<Self>) {
            Self::wake_by_ref(&self)
        }
        fn wake_by_ref(self: &Arc<Self>) {
            *self.mutex.lock().unwrap() = true;
            self.condvar.notify_one();
        }
    }

    let park = Arc::new(ParkWaker { mutex: Mutex::new(false), condvar: Condvar::new() });
    let waker = Waker::from(park.clone());
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);

    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                let mut ready = park.mutex.lock().unwrap();
                while !*ready {
                    ready = park.condvar.wait(ready).unwrap();
                }
                *ready = false;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AbortHandle
// ---------------------------------------------------------------------------------------------

/// A detached handle that can abort a task spawned via [`spawn`] without holding on to its
/// [`JoinHandle`] (obtained via [`JoinHandle::abort_handle`]). Mirrors `tokio::task::AbortHandle`'s
/// shape: it shares only the abort flag, not the task's output type, so (unlike `JoinHandle<T>`)
/// it isn't generic over `T` and many of them can be stored together regardless of what their
/// tasks return — exactly what [`JoinSet::abort_all`] needs.
pub struct AbortHandle {
    aborted: Arc<std::sync::atomic::AtomicBool>,
}

impl AbortHandle {
    /// Best-effort cancellation; see [`JoinHandle::abort`] for the exact semantics (same flag).
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------------------------
// JoinSet
// ---------------------------------------------------------------------------------------------

/// A growable collection of spawned tasks, matching `tokio::task::JoinSet`'s common-case shape.
///
/// ## Design
///
/// Each `spawn`ed future is submitted exactly the way top-level [`spawn`] submits it — same
/// `TaskCell`/`Wake` state machine, same resubmission to the engine's scheduler, nothing
/// duplicated. `JoinSet` itself never polls the outstanding tasks: instead each one gets a tiny
/// *adapter* task (also just an ordinary [`spawn`]) whose whole body is `handle.await` followed by
/// pushing the `Result<T, JoinError>` into a shared `Mutex<VecDeque<_>>` and waking whoever is
/// parked in [`JoinSet::join_next`] — the same shared-queue-plus-waker shape `sync::mpsc` uses
/// internally. `join_next` therefore only ever waits on that queue; it does not round-robin-poll
/// the set's tasks.
pub struct JoinSet<T> {
    queue: Arc<Mutex<std::collections::VecDeque<Result<T, JoinError>>>>,
    waiter: Arc<Mutex<Option<Waker>>>,
    abort_handles: Vec<AbortHandle>,
    /// Number of tasks spawned but not yet retrieved through `join_next` (mirrors `tokio`'s
    /// `JoinSet::len`: a finished-but-unretrieved task still counts).
    remaining: usize,
}

impl<T: Send + 'static> JoinSet<T> {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            waiter: Arc::new(Mutex::new(None)),
            abort_handles: Vec::new(),
            remaining: 0,
        }
    }

    /// Spawns `future` onto the set, same as [`spawn`] but tracked so [`JoinSet::join_next`] can
    /// retrieve its result (in completion order, not spawn order).
    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = T> + Send + 'static,
    {
        let handle = spawn(future);
        self.abort_handles.push(handle.abort_handle());
        self.remaining += 1;

        let queue = self.queue.clone();
        let waiter = self.waiter.clone();
        // Fire-and-forget adapter task: its own `JoinHandle` is dropped immediately. It cannot
        // itself panic (it only awaits `handle` and pushes the already-caught `Result`), so
        // there is nothing meaningful to observe about *it* completing.
        spawn(async move {
            let result = handle.await;
            queue.lock().unwrap().push_back(result);
            if let Some(waker) = waiter.lock().unwrap().take() {
                waker.wake();
            }
        });
    }

    /// Waits for the next task to complete (in whatever order they actually finish) and returns
    /// its result, or `None` once every spawned task has already been retrieved.
    pub async fn join_next(&mut self) -> Option<Result<T, JoinError>> {
        if self.remaining == 0 {
            return None;
        }
        let result = JoinNextFuture { set: self }.await;
        self.remaining -= 1;
        Some(result)
    }

    /// Number of tasks spawned but not yet retrieved via [`JoinSet::join_next`].
    pub fn len(&self) -> usize {
        self.remaining
    }

    pub fn is_empty(&self) -> bool {
        self.remaining == 0
    }

    /// Marks every currently-tracked task aborted (best-effort, same semantics as
    /// [`JoinHandle::abort`]): each one resolves to `Err(JoinError::cancelled())` on its next
    /// poll, forwarded into the queue exactly like a normal completion. Still requires calling
    /// [`JoinSet::join_next`] to drain them (matches `tokio::task::JoinSet::abort_all`, which does
    /// not implicitly join).
    pub fn abort_all(&mut self) {
        for handle in &self.abort_handles {
            handle.abort();
        }
    }

    /// Forgets every currently-tracked task without aborting them: they keep running to
    /// completion on the engine's scheduler exactly as if [`spawn`] had been called directly, but
    /// this set stops tracking them entirely (`len()` becomes 0, and their results — even ones
    /// already queued — are discarded rather than being returned from a future [`join_next`]
    /// call). Matches `tokio::task::JoinSet::detach_all`.
    pub fn detach_all(&mut self) {
        self.abort_handles.clear();
        self.queue.lock().unwrap().clear();
        self.remaining = 0;
    }
}

impl<T: Send + 'static> Default for JoinSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

struct JoinNextFuture<'a, T> {
    set: &'a JoinSet<T>,
}

impl<T> Future for JoinNextFuture<'_, T> {
    type Output = Result<T, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(result) = self.set.queue.lock().unwrap().pop_front() {
            return Poll::Ready(result);
        }
        // Always (re-)register on every `Pending`, not just the first — same missed-wakeup
        // discipline as everywhere else in this crate (see `sync.rs`'s module doc): the adapter
        // task drains the waker slot with `take()` when it wakes it, so a `join_next` that gets
        // woken, re-polled, and finds the queue already emptied by a concurrent `join_next` call
        // must push a fresh waker or a later completion's wakeup would have nothing to fire.
        *self.set.waiter.lock().unwrap() = Some(cx.waker().clone());
        // Re-check after registering to avoid a lost wakeup if an adapter task pushed a result
        // between the check above and registering here.
        if let Some(result) = self.set.queue.lock().unwrap().pop_front() {
            return Poll::Ready(result);
        }
        Poll::Pending
    }
}
