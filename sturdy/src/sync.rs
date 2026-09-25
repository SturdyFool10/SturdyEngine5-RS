//! Async-aware synchronization primitives shaped after `tokio::sync`: an async [`Mutex`] and a
//! bounded [`mpsc`] channel.
//!
//! Unlike `sturdy::task`, none of this touches the C++ engine at all — it's pure coordination
//! between tasks spawned with [`crate::task::spawn`] (or any other executor, including
//! [`crate::task::block_on`]), built on `std::sync::Mutex` for internal bookkeeping plus a stored
//! waker list, the standard pattern for hand-rolled async primitives.

use std::collections::VecDeque;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::task::{Context, Poll, Waker};

// ---------------------------------------------------------------------------------------------
// Mutex
// ---------------------------------------------------------------------------------------------

/// An async mutex, matching `tokio::sync::Mutex`'s shape: `lock().await` yields a guard instead
/// of blocking the thread while contended.
pub struct Mutex<T> {
    state: StdMutex<MutexState<T>>,
}

struct MutexState<T> {
    locked: bool,
    value: T,
    waiters: VecDeque<Waker>,
}

/// RAII guard returned by [`Mutex::lock`]/[`Mutex::try_lock`]; unlocks on drop.
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
    // Only `None` transiently during `Drop`; always `Some` otherwise. Using `ManuallyDrop`-free
    // `Option` keeps `Deref`/`DerefMut` simple.
    value: Option<*mut T>,
}

// SAFETY: a `MutexGuard` behaves like `std::sync::MutexGuard` — it gives out `&`/`&mut` to data
// protected by the mutex's internal exclusion, so it's Send/Sync exactly when `T` is.
unsafe impl<T: Send> Send for MutexGuard<'_, T> {}
unsafe impl<T: Sync> Sync for MutexGuard<'_, T> {}

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self { state: StdMutex::new(MutexState { locked: false, value, waiters: VecDeque::new() }) }
    }

    /// Acquires the lock, waiting (as a task, not a thread block) if it's currently held.
    pub async fn lock(&self) -> MutexGuard<'_, T> {
        LockFuture { mutex: self }.await
    }

    /// Acquires the lock if it is not currently held, without waiting.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let mut state = self.state.lock().unwrap();
        if state.locked {
            None
        } else {
            state.locked = true;
            Some(MutexGuard { mutex: self, value: Some(std::ptr::addr_of_mut!(state.value)) })
        }
    }

    fn unlock(&self) {
        let mut state = self.state.lock().unwrap();
        state.locked = false;
        // Wake exactly one waiter (if any); it will re-attempt the lock and, seeing it free,
        // take it. Waking only one avoids a thundering herd for a resource only one task can
        // hold anyway.
        if let Some(waker) = state.waiters.pop_front() {
            drop(state);
            waker.wake();
        }
    }
}

struct LockFuture<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<'a, T> Future for LockFuture<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.mutex.state.lock().unwrap();
        if !state.locked {
            state.locked = true;
            return Poll::Ready(MutexGuard {
                mutex: self.mutex,
                value: Some(std::ptr::addr_of_mut!(state.value)),
            });
        }
        // Always (re-)register on every `Pending` return, not just the first: `unlock()`
        // consumes a waiter's entry via `pop_front` when it wakes it, so a future that gets
        // woken, re-polled, and finds the lock already retaken by someone else must push a
        // *fresh* waker or its wakeup is lost for good (a "registered once" guard here would be
        // a missed-wakeup bug — this was caught by a contention stress test that hung forever).
        state.waiters.push_back(cx.waker().clone());
        Poll::Pending
    }
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: `value` is a valid pointer into the mutex's state for the guard's whole
        // lifetime; we hold exclusive access because `locked` was true for the duration.
        unsafe { &*self.value.unwrap() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: see `Deref`.
        unsafe { &mut *self.value.unwrap() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

// ---------------------------------------------------------------------------------------------
// RwLock
// ---------------------------------------------------------------------------------------------

/// An async reader-writer lock, matching `tokio::sync::RwLock`'s shape.
///
/// Fairness: write-preferring, like Tokio's. New `read()` calls only take the fast (immediate)
/// path when there is no active writer *and* the waiter queue is already empty; as soon as a
/// writer is waiting, the queue is non-empty, so subsequent readers queue behind it too instead
/// of jumping ahead — this bounds how long a writer can be starved by a continuous stream of
/// readers to "however long the currently-active readers take to finish", not "forever". It is
/// not a strict FIFO in the other direction: once a writer's turn comes and it finishes, every
/// reader queued directly behind it (a contiguous run, up to the next writer) is woken together,
/// same as Tokio.
pub struct RwLock<T> {
    state: StdMutex<RwLockState>,
    value: std::cell::UnsafeCell<T>,
}

// SAFETY: `RwLock<T>` hands out `&T` (possibly to many threads at once, guarded by `readers`) and
// `&mut T` (exclusively, guarded by `writer`) through the same exclusion discipline
// `std::sync::RwLock` uses; the bounds match it exactly.
unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

struct RwLockState {
    readers: usize,
    writer: bool,
    waiters: VecDeque<RwWaiter>,
}

/// A queued waiter. `granted` is shared with the waiting future (see [`RwLockReadFuture`] /
/// [`RwLockWriteFuture`]): [`RwLock::wake_next`] sets it to `true` *and* applies the
/// corresponding state mutation (`readers += 1` / `writer = true`) atomically, under the same
/// `state` lock, at the moment it pops this entry — so by the time the future observes
/// `granted == true`, the resource is already reserved for it and it can go straight to `Ready`
/// without re-deriving eligibility from `readers`/`writer` (which, for the future's own request,
/// would now incorrectly look contended — see the note on `RwLockReadFuture::poll`).
enum RwWaiter {
    Read(Arc<AtomicBool>, Waker),
    Write(Arc<AtomicBool>, Waker),
}

impl<T> RwLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            state: StdMutex::new(RwLockState { readers: 0, writer: false, waiters: VecDeque::new() }),
            value: std::cell::UnsafeCell::new(value),
        }
    }

    /// Acquires a shared read lock, waiting (as a task) if a writer currently holds it or is
    /// waiting for its turn (see the fairness note on the type).
    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        RwLockReadFuture { lock: self, registration: None }.await
    }

    /// Acquires the exclusive write lock, waiting (as a task) while any readers or another writer
    /// hold it.
    pub async fn write(&self) -> RwLockWriteGuard<'_, T> {
        RwLockWriteFuture { lock: self, registration: None }.await
    }

    /// Acquires a read lock immediately if possible (same fast-path condition as `read()`),
    /// without waiting.
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        let mut state = self.state.lock().unwrap();
        if !state.writer && state.waiters.is_empty() {
            state.readers += 1;
            Some(RwLockReadGuard { lock: self })
        } else {
            None
        }
    }

    /// Acquires the write lock immediately if it is completely uncontended, without waiting.
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        let mut state = self.state.lock().unwrap();
        if !state.writer && state.readers == 0 && state.waiters.is_empty() {
            state.writer = true;
            Some(RwLockWriteGuard { lock: self })
        } else {
            None
        }
    }

    /// Pops and grants every waiter at the front of the queue that can now proceed: either a
    /// single writer, or a contiguous run of readers (stopping at the next writer). Granting
    /// means *both* applying the state mutation (`readers += 1` / `writer = true`) and flipping
    /// the waiter's shared `granted` flag to `true`, atomically, right here under `state`'s lock
    /// — by the time the corresponding future is polled again (in response to the `Waker` this
    /// returns), the reservation already exists; the future just has to notice the flag and
    /// return `Ready`. Must be called with `state`'s lock held; the caller drops it and wakes the
    /// returned wakers afterward, outside the lock.
    fn wake_next(state: &mut RwLockState) -> Vec<Waker> {
        let mut woken = Vec::new();
        loop {
            match state.waiters.front() {
                Some(RwWaiter::Write(_, _)) if state.readers == 0 && !state.writer => {
                    if let Some(RwWaiter::Write(granted, waker)) = state.waiters.pop_front() {
                        state.writer = true;
                        granted.store(true, Ordering::Release);
                        woken.push(waker);
                    }
                    break;
                }
                Some(RwWaiter::Read(_, _)) if !state.writer => {
                    if let Some(RwWaiter::Read(granted, waker)) = state.waiters.pop_front() {
                        state.readers += 1;
                        granted.store(true, Ordering::Release);
                        woken.push(waker);
                    }
                }
                _ => break,
            }
        }
        woken
    }

    fn unlock_read(&self) {
        let mut state = self.state.lock().unwrap();
        state.readers -= 1;
        let woken = if state.readers == 0 { Self::wake_next(&mut state) } else { Vec::new() };
        drop(state);
        for waker in woken {
            waker.wake();
        }
    }

    fn unlock_write(&self) {
        let mut state = self.state.lock().unwrap();
        state.writer = false;
        let woken = Self::wake_next(&mut state);
        drop(state);
        for waker in woken {
            waker.wake();
        }
    }
}

struct RwLockReadFuture<'a, T> {
    lock: &'a RwLock<T>,
    /// `Some` once this future has pushed itself onto the waiter queue (from its first `Pending`
    /// onward); the flag inside is [`RwLock::wake_next`]'s signal that our reservation is ready.
    registration: Option<Arc<AtomicBool>>,
}

impl<'a, T> Future for RwLockReadFuture<'a, T> {
    type Output = RwLockReadGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.lock.state.lock().unwrap();

        if let Some(granted) = &this.registration {
            // Already queued from a previous poll. Unlike `LockFuture`/`SendFuture`/`RecvFuture`
            // above, we deliberately do *not* re-derive eligibility from `readers`/`writer` here:
            // `wake_next` already applied `readers += 1` for us the instant it set this flag, so
            // re-checking the generic "is the lock free" condition would see our own pending
            // reservation as contention and wrongly requeue forever (that was a real deadlock
            // caught by the concurrent-readers-plus-writer stress test in the `hello` example —
            // it hung indefinitely on the writer's turn). There is also no missed-wakeup risk
            // from not re-registering: only `wake_next` can ever set this flag, and it always
            // wakes us in the same critical section it sets it in, so if we observe `false` here
            // we are simply still waiting for that one guaranteed future wakeup, not one we could
            // have missed.
            if granted.load(Ordering::Acquire) {
                this.registration = None;
                return Poll::Ready(RwLockReadGuard { lock: this.lock });
            }
            return Poll::Pending;
        }

        // First poll: try the immediate fast path.
        if !state.writer && state.waiters.is_empty() {
            state.readers += 1;
            return Poll::Ready(RwLockReadGuard { lock: this.lock });
        }
        let granted = Arc::new(AtomicBool::new(false));
        state.waiters.push_back(RwWaiter::Read(granted.clone(), cx.waker().clone()));
        this.registration = Some(granted);
        Poll::Pending
    }
}

impl<T> Drop for RwLockReadFuture<'_, T> {
    fn drop(&mut self) {
        // Cancellation safety: if we're dropped (e.g. as the losing branch of a `select!`) while
        // still tracked by the lock, undo whatever `RwLock` currently believes about us instead
        // of leaking a phantom reservation.
        let Some(granted) = self.registration.take() else { return };
        if granted.load(Ordering::Acquire) {
            // Already granted (readers was incremented for us) but we never turned that into a
            // guard: release it exactly as if a guard had been created and immediately dropped.
            self.lock.unlock_read();
        } else {
            // Still queued, never granted: remove our specific entry so it doesn't sit in the
            // queue forever blocking waiters behind it for a request nobody is waiting on anymore.
            let mut state = self.lock.state.lock().unwrap();
            state.waiters.retain(|w| !matches!(w, RwWaiter::Read(g, _) if Arc::ptr_eq(g, &granted)));
        }
    }
}

struct RwLockWriteFuture<'a, T> {
    lock: &'a RwLock<T>,
    /// See [`RwLockReadFuture::registration`] — same shape, same reasoning.
    registration: Option<Arc<AtomicBool>>,
}

impl<'a, T> Future for RwLockWriteFuture<'a, T> {
    type Output = RwLockWriteGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.lock.state.lock().unwrap();

        if let Some(granted) = &this.registration {
            // See the note in `RwLockReadFuture::poll`: trust the flag, don't re-derive.
            if granted.load(Ordering::Acquire) {
                this.registration = None;
                return Poll::Ready(RwLockWriteGuard { lock: this.lock });
            }
            return Poll::Pending;
        }

        if !state.writer && state.readers == 0 && state.waiters.is_empty() {
            state.writer = true;
            return Poll::Ready(RwLockWriteGuard { lock: this.lock });
        }
        let granted = Arc::new(AtomicBool::new(false));
        state.waiters.push_back(RwWaiter::Write(granted.clone(), cx.waker().clone()));
        this.registration = Some(granted);
        Poll::Pending
    }
}

impl<T> Drop for RwLockWriteFuture<'_, T> {
    fn drop(&mut self) {
        // See `RwLockReadFuture::drop` for why this is needed at all.
        let Some(granted) = self.registration.take() else { return };
        if granted.load(Ordering::Acquire) {
            self.lock.unlock_write();
        } else {
            let mut state = self.lock.state.lock().unwrap();
            state.waiters.retain(|w| !matches!(w, RwWaiter::Write(g, _) if Arc::ptr_eq(g, &granted)));
        }
    }
}

/// RAII guard returned by [`RwLock::read`]/[`RwLock::try_read`]; releases the read lock on drop.
pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: while any `RwLockReadGuard` exists, `state.writer` is false and no
        // `RwLockWriteGuard` can be created (both futures check `readers`/`writer` under the same
        // `state` lock before handing one out), so shared access is sound for as long as this
        // guard lives.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.unlock_read();
    }
}

// SAFETY: mirrors `std::sync::RwLockReadGuard`'s bounds.
unsafe impl<T: Sync> Sync for RwLockReadGuard<'_, T> {}

/// RAII guard returned by [`RwLock::write`]/[`RwLock::try_write`]; releases the write lock on
/// drop.
pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: see `DerefMut`.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: while a `RwLockWriteGuard` exists, `state.writer` is true, which blocks every
        // other reader and writer future from being granted (checked under the same `state` lock
        // before any guard is handed out) — this is the only live reference to `value`.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.unlock_write();
    }
}

// SAFETY: mirrors `std::sync::RwLockWriteGuard`'s bounds.
unsafe impl<T: Sync> Sync for RwLockWriteGuard<'_, T> {}

// ---------------------------------------------------------------------------------------------
// mpsc
// ---------------------------------------------------------------------------------------------

/// A bounded async multi-producer, single-consumer channel, matching `tokio::sync::mpsc`'s shape
/// (`sturdy::sync::mpsc::channel`, `Sender`, `Receiver`), plus an unbounded variant
/// (`sturdy::sync::mpsc::unbounded_channel`, `UnboundedSender`, `UnboundedReceiver`) whose `send`
/// is synchronous (there is no capacity to wait for).
pub mod mpsc {

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll, Waker};

/// Error returned by [`Sender::send`] when every [`Receiver`] has been dropped; carries the value
/// back to the caller, matching `tokio::sync::mpsc::error::SendError`.
pub struct SendError<T>(pub T);

impl<T> std::fmt::Debug for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SendError(..)")
    }
}

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("channel closed")
    }
}

impl<T> std::error::Error for SendError<T> {}

struct ChannelState<T> {
    queue: VecDeque<T>,
    capacity: usize,
    senders_alive: usize,
    receiver_alive: bool,
    /// Woken when the queue transitions from empty to non-empty, or the channel closes.
    recv_wakers: Vec<Waker>,
    /// Woken when the queue transitions from full to non-full, or the channel closes.
    send_wakers: Vec<Waker>,
}

struct Channel<T> {
    state: StdMutex<ChannelState<T>>,
}

/// The sending half of a bounded channel created by [`channel`]. Cloneable; the channel closes
/// (and pending/future `recv()`s return `None`) once every clone is dropped.
pub struct Sender<T> {
    channel: Arc<Channel<T>>,
}

/// The receiving half of a bounded channel created by [`channel`].
pub struct Receiver<T> {
    channel: Arc<Channel<T>>,
}

/// Creates a bounded async channel with room for `capacity` buffered values, matching
/// `tokio::sync::mpsc::channel`'s shape.
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    assert!(capacity > 0, "sturdy::sync::mpsc::channel capacity must be > 0");
    let channel = Arc::new(Channel {
        state: StdMutex::new(ChannelState {
            queue: VecDeque::with_capacity(capacity),
            capacity,
            senders_alive: 1,
            receiver_alive: true,
            recv_wakers: Vec::new(),
            send_wakers: Vec::new(),
        }),
    });
    (Sender { channel: channel.clone() }, Receiver { channel })
}

impl<T> Sender<T> {
    /// Sends `value`, waiting (as a task) if the channel is currently full. Resolves to
    /// `Err(SendError(value))` if every `Receiver` has already been dropped.
    pub fn send(&self, value: T) -> impl Future<Output = Result<(), SendError<T>>> + '_ {
        SendFuture { sender: self, value: Some(value) }
    }

    /// Sends `value` immediately if there is buffer space, without waiting.
    pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        let mut state = self.channel.state.lock().unwrap();
        if !state.receiver_alive {
            return Err(TrySendError::Closed(value));
        }
        if state.queue.len() >= state.capacity {
            return Err(TrySendError::Full(value));
        }
        state.queue.push_back(value);
        let wakers = std::mem::take(&mut state.recv_wakers);
        drop(state);
        for waker in wakers {
            waker.wake();
        }
        Ok(())
    }
}

/// Error returned by [`Sender::try_send`].
pub enum TrySendError<T> {
    Full(T),
    Closed(T),
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.channel.state.lock().unwrap().senders_alive += 1;
        Sender { channel: self.channel.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut state = self.channel.state.lock().unwrap();
        state.senders_alive -= 1;
        if state.senders_alive == 0 {
            // Last sender gone: wake every pending receiver so `recv()` can observe closure.
            let wakers = std::mem::take(&mut state.recv_wakers);
            drop(state);
            for waker in wakers {
                waker.wake();
            }
        }
    }
}

struct SendFuture<'a, T> {
    sender: &'a Sender<T>,
    value: Option<T>,
}

impl<T> Future for SendFuture<'_, T> {
    type Output = Result<(), SendError<T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `SendFuture` never relies on pinning `T` in place — `value` is only ever moved
        // out whole (never split, never referenced by address), which is exactly the same
        // operation `Option::take` performs on any owned field regardless of pinning.
        let this = unsafe { self.get_unchecked_mut() };
        let mut state = this.sender.channel.state.lock().unwrap();
        if !state.receiver_alive {
            let value = this.value.take().expect("SendFuture polled after completion");
            return Poll::Ready(Err(SendError(value)));
        }
        if state.queue.len() < state.capacity {
            let value = this.value.take().expect("SendFuture polled after completion");
            state.queue.push_back(value);
            let wakers = std::mem::take(&mut state.recv_wakers);
            drop(state);
            for waker in wakers {
                waker.wake();
            }
            return Poll::Ready(Ok(()));
        }
        // Always (re-)register on every `Pending`: a wake drains *all* `send_wakers` via
        // `mem::take`, so a future that's woken, re-polled, and finds the channel already full
        // again (raced by another sender) must push a fresh waker or its wakeup is permanently
        // lost — the same missed-wakeup class of bug as `LockFuture` (see its comment).
        state.send_wakers.push(cx.waker().clone());
        Poll::Pending
    }
}

impl<T> Receiver<T> {
    /// Receives the next value, waiting (as a task) if the channel is currently empty. Resolves
    /// to `None` once the channel is empty *and* every `Sender` has been dropped.
    pub fn recv(&mut self) -> impl Future<Output = Option<T>> + '_ {
        RecvFuture { receiver: self }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut state = self.channel.state.lock().unwrap();
        state.receiver_alive = false;
        let wakers = std::mem::take(&mut state.send_wakers);
        drop(state);
        for waker in wakers {
            waker.wake();
        }
    }
}

struct RecvFuture<'a, T> {
    receiver: &'a mut Receiver<T>,
}

impl<T> Future for RecvFuture<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // `RecvFuture` has no self-referential/pinned fields, so it's `Unpin`; project by value.
        let this = self.get_mut();
        let mut state = this.receiver.channel.state.lock().unwrap();
        if let Some(value) = state.queue.pop_front() {
            let wakers = std::mem::take(&mut state.send_wakers);
            drop(state);
            for waker in wakers {
                waker.wake();
            }
            return Poll::Ready(Some(value));
        }
        if state.senders_alive == 0 {
            return Poll::Ready(None);
        }
        // See the note in `SendFuture::poll`/`LockFuture::poll`: always re-register.
        state.recv_wakers.push(cx.waker().clone());
        Poll::Pending
    }
}

// ---------------------------------------------------------------------------------------------
// unbounded channel
// ---------------------------------------------------------------------------------------------

struct UnboundedState<T> {
    queue: VecDeque<T>,
    senders_alive: usize,
    receiver_alive: bool,
    /// Woken when the queue transitions from empty to non-empty, or the channel closes. There is
    /// no `send_wakers` counterpart here — sends never wait, there's no capacity to become
    /// available.
    recv_wakers: Vec<Waker>,
}

struct UnboundedChannel<T> {
    state: StdMutex<UnboundedState<T>>,
}

/// The sending half of an unbounded channel created by [`unbounded_channel`]. Cloneable;
/// `send` never blocks or waits — there is no capacity to be full.
pub struct UnboundedSender<T> {
    channel: Arc<UnboundedChannel<T>>,
}

/// The receiving half of an unbounded channel created by [`unbounded_channel`].
pub struct UnboundedReceiver<T> {
    channel: Arc<UnboundedChannel<T>>,
}

/// Creates an unbounded async channel, matching `tokio::sync::mpsc::unbounded_channel`'s shape:
/// [`UnboundedSender::send`] is synchronous (it always succeeds immediately unless every receiver
/// has been dropped), while [`UnboundedReceiver::recv`] is async exactly like the bounded
/// [`Receiver::recv`].
pub fn unbounded_channel<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
    let channel = Arc::new(UnboundedChannel {
        state: StdMutex::new(UnboundedState {
            queue: VecDeque::new(),
            senders_alive: 1,
            receiver_alive: true,
            recv_wakers: Vec::new(),
        }),
    });
    (UnboundedSender { channel: channel.clone() }, UnboundedReceiver { channel })
}

impl<T> UnboundedSender<T> {
    /// Sends `value` immediately. Fails only once every [`UnboundedReceiver`] has been dropped,
    /// handing the value back via [`SendError`].
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let mut state = self.channel.state.lock().unwrap();
        if !state.receiver_alive {
            return Err(SendError(value));
        }
        state.queue.push_back(value);
        let wakers = std::mem::take(&mut state.recv_wakers);
        drop(state);
        for waker in wakers {
            waker.wake();
        }
        Ok(())
    }
}

impl<T> Clone for UnboundedSender<T> {
    fn clone(&self) -> Self {
        self.channel.state.lock().unwrap().senders_alive += 1;
        UnboundedSender { channel: self.channel.clone() }
    }
}

impl<T> Drop for UnboundedSender<T> {
    fn drop(&mut self) {
        let mut state = self.channel.state.lock().unwrap();
        state.senders_alive -= 1;
        if state.senders_alive == 0 {
            let wakers = std::mem::take(&mut state.recv_wakers);
            drop(state);
            for waker in wakers {
                waker.wake();
            }
        }
    }
}

impl<T> UnboundedReceiver<T> {
    /// Receives the next value, waiting (as a task) if the channel is currently empty. Resolves
    /// to `None` once the channel is empty *and* every [`UnboundedSender`] has been dropped.
    pub fn recv(&mut self) -> impl Future<Output = Option<T>> + '_ {
        UnboundedRecvFuture { receiver: self }
    }
}

impl<T> Drop for UnboundedReceiver<T> {
    fn drop(&mut self) {
        self.channel.state.lock().unwrap().receiver_alive = false;
    }
}

struct UnboundedRecvFuture<'a, T> {
    receiver: &'a mut UnboundedReceiver<T>,
}

impl<T> Future for UnboundedRecvFuture<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this.receiver.channel.state.lock().unwrap();
        if let Some(value) = state.queue.pop_front() {
            return Poll::Ready(Some(value));
        }
        if state.senders_alive == 0 {
            return Poll::Ready(None);
        }
        // See the note on `RecvFuture::poll` above: always re-register on every `Pending`.
        state.recv_wakers.push(cx.waker().clone());
        Poll::Pending
    }
}

} // mod mpsc
