//! `sturdy::select!`, a `tokio::select!`-shaped macro for racing several futures and running the
//! arm of whichever completes first, dropping (and thereby cancelling) the rest.
//!
//! ## Design
//!
//! The macro itself does almost nothing: it peels branches off one at a time and recurses,
//! pairing the first branch's future against "everything else" via the small hand-written
//! [`select2`] combinator (a struct that polls two futures in turn and resolves with whichever
//! finishes first, wrapped in [`Either`]). N branches nest N-1 levels deep. Because `select2`
//! polls *both* of its fields every time it is polled, and the "everything else" side is itself
//! (recursively) another `select2` polling its own two fields, every original branch really does
//! get polled on every wakeup — this is not a fold that only looks at branches one at a time.
//! When one branch resolves, the `Either::Right`/`Left` value carrying "the other future" for
//! every other branch (including deeply nested ones) is dropped as part of unwinding out of the
//! `.await`, which drops those inner futures and cancels them — standard `select!` semantics,
//! matching Tokio.
//!
//! ## `biased`, `else`, and per-branch guards
//!
//! `biased;` is accepted as an optional leading marker but is otherwise a no-op: this macro
//! always polls left-to-right and favors the first-listed branch on a tie anyway, which is
//! exactly what `biased` asks for, so there's no separate "unbiased" mode to opt out of.
//!
//! A branch may carry a `, if <guard>` condition, and the whole macro may end with an
//! `else => { .. }` arm, matching Tokio's syntax:
//!
//! ```ignore
//! sturdy::select! {
//!     v = fut_a, if enabled => { .. }
//!     v = fut_b => { .. }
//!     else => { .. }
//! }
//! ```
//!
//! A disabled branch (`if` guard evaluated to `false`) is never polled — its future is still
//! constructed (so side effects of *calling* the expression that produces it still happen), but
//! [`guarded`] wraps it so it's simply never polled, matching "this branch cannot complete"
//! rather than skipping construction entirely the way Tokio's does. If every branch is disabled,
//! the `else` arm runs immediately without polling anything; with no `else` arm, that case
//! panics, matching Tokio. One deliberate deviation from Tokio: each guard expression is
//! evaluated up to twice (once to compute whether *any* branch is enabled, once more inside that
//! branch's own wrapping) — fine for the cheap, side-effect-free boolean checks guards are
//! expected to be, but worth knowing if a guard does something unusual.

/// Wraps `fut` so it is only ever polled when `enabled` is `true`; otherwise it's permanently
/// [`Poll::Pending`] (never registers a waker, so it never gets a stray wakeup either — it just
/// sits inert while sibling branches in a [`select2`] tree are polled instead). Used by
/// `sturdy::select!` to implement per-branch `if guard =>` conditions.
pub fn guarded<F: Future>(enabled: bool, fut: F) -> Guarded<F> {
    Guarded { inner: if enabled { Some(fut) } else { None } }
}

pub struct Guarded<F> {
    inner: Option<F>,
}

impl<F: Future> Future for Guarded<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: standard pin projection for a single structurally-pinned field; `Guarded` has
        // no `Drop` impl and never moves `inner` out (only polls it in place, or leaves it
        // `None`), matching `Pin::new_unchecked`'s contract.
        let this = unsafe { self.get_unchecked_mut() };
        match &mut this.inner {
            Some(f) => unsafe { Pin::new_unchecked(f) }.poll(cx),
            None => Poll::Pending,
        }
    }
}

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// The result of [`select2`]: which of the two futures finished first, and its output.
pub enum Either<A, B> {
    Left(A),
    Right(B),
}

/// Polls `f1` and `f2` each time it is polled, resolving with whichever produces a value first
/// (favoring `f1` on a tie, i.e. both ready on the same poll). The loser is simply dropped when
/// this future is dropped (e.g. because the caller matched on the result and moved on) — that's
/// where cancellation of the losing branch actually happens, not inside this function.
pub fn select2<F1, F2>(f1: F1, f2: F2) -> Select2<F1, F2>
where
    F1: Future,
    F2: Future,
{
    Select2 { f1, f2 }
}

pub struct Select2<F1, F2> {
    f1: F1,
    f2: F2,
}

impl<F1: Future, F2: Future> Future for Select2<F1, F2> {
    type Output = Either<F1::Output, F2::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: standard manual pin-projection for two disjoint, non-overlapping fields.
        // `Select2` has no `Drop` impl and never moves `f1`/`f2` out of `self` (only polls them
        // in place through the pinned references constructed here), which is exactly the
        // structural-pinning contract `Pin::new_unchecked` requires.
        let this = unsafe { self.get_unchecked_mut() };
        let f1 = unsafe { Pin::new_unchecked(&mut this.f1) };
        if let Poll::Ready(value) = f1.poll(cx) {
            return Poll::Ready(Either::Left(value));
        }
        let f2 = unsafe { Pin::new_unchecked(&mut this.f2) };
        if let Poll::Ready(value) = f2.poll(cx) {
            return Poll::Ready(Either::Right(value));
        }
        Poll::Pending
    }
}

/// Races several futures, running the arm of whichever completes first and dropping (cancelling)
/// the rest.
///
/// ```ignore
/// sturdy::select! {
///     result = fut_a => { /* use result */ }
///     result = fut_b, if some_condition => { /* use result */ }
///     else => { /* every branch was disabled */ }
/// }
/// ```
///
/// All arm bodies (including `else`, if present) must produce the same type (the type of the
/// whole `select!` expression), same as an ordinary `match`. See the module docs for `biased`,
/// `else`, and per-branch `if guard` semantics.
#[macro_export]
macro_rules! select {
    // Strip an optional leading `biased;` marker — see module docs for why this is a no-op.
    //
    // NOTE: every arm below whose matcher starts with a literal `@`-prefixed tag must stay above
    // the catch-all entry arm at the bottom of this macro. `macro_rules` tries arms top-to-bottom
    // and commits to the first one whose matcher fits, and `$($rest:tt)*` fits *any* input,
    // including this macro's own internal `@collect [...] ..`-tagged recursive calls — if the
    // catch-all were listed first it would re-wrap its own output forever (infinite recursion).
    (biased; $($rest:tt)*) => {
        $crate::select!($($rest)*)
    };

    // ---- Phase 1: tt-munch branches one at a time into a `{pat, fut, guard, body}` group each
    // (guard defaults to the literal `true`), splitting off an optional trailing `else`. This has
    // to be a chain of separate match arms rather than one arm combining `$(branch)+` with a
    // trailing `$(else ..)?` — `macro_rules` rejects that as locally ambiguous, since a `pat`
    // fragment is opaque to it and it can't see that `else` (a keyword) could never actually start
    // one.
    //
    // The two `else`/empty arms below must additionally stay *above* the two branch-consuming
    // arms, not just "in some order": matching a literal token like `else` against non-matching
    // input is a soft failure that falls through to the next arm, but asking the `$pat:pat`
    // fragment parser to parse starting at a keyword like `else` is a *hard* parse error that
    // aborts the whole macro invocation instead of falling through — `macro_rules` does not
    // backtrack out of a nonterminal parse failure the way it does out of a literal-token
    // mismatch. Trying the literal-`else`/empty arms first means input that actually starts with
    // `else` never reaches the `$pat:pat` arms at all. ----
    (@collect [$($acc:tt)*] else => $else_body:block) => {
        $crate::select!(@enabled_check [$($acc)*] $else_body)
    };
    (@collect [$($acc:tt)*]) => {
        $crate::select!(@enabled_check [$($acc)*] {
            panic!("sturdy::select!: every branch is disabled by its `if` guard and there is no `else` branch")
        })
    };
    (@collect [$($acc:tt)*] $pat:pat = $fut:expr, if $guard:expr => $body:block $($rest:tt)*) => {
        $crate::select!(@collect [$($acc)* {$pat, $fut, $guard, $body}] $($rest)*)
    };
    (@collect [$($acc:tt)*] $pat:pat = $fut:expr => $body:block $($rest:tt)*) => {
        $crate::select!(@collect [$($acc)* {$pat, $fut, true, $body}] $($rest)*)
    };

    // ---- Phase 2: OR every guard together up front to decide whether to run the disabled-path
    // body (`else`, or the panic above) immediately, or proceed to actually poll. See the module
    // docs: this means a branch's guard expression is evaluated here *and* again in Phase 3 when
    // that specific branch is wrapped, i.e. up to twice total. ----
    (@enabled_check [$({$pat:pat, $fut:expr, $guard:expr, $body:block})+] $disabled_body:block) => {{
        #[allow(unused_mut)]
        let __sturdy_select_any_enabled = false $(|| $guard)+;
        if __sturdy_select_any_enabled {
            $crate::select!(@rec $({$pat, $fut, $guard, $body})+)
        } else {
            $disabled_body
        }
    }};

    // ---- Phase 3: the actual `select2` recursion over the collected groups, guard-wrapping each
    // branch's future via `guarded`. ----
    // Base case: exactly one branch left — await it (through `guarded`, so a false guard on a
    // *single*-branch select correctly hangs forever rather than silently skipping the guard,
    // matching Tokio's own behavior for a lone disabled branch with no `else`).
    (@rec {$pat:pat, $fut:expr, $guard:expr, $body:block}) => {{
        let $pat = $crate::select::guarded($guard, $fut).await;
        $body
    }};

    // Recursive case: two or more branches. Race the first branch (guard-wrapped) against
    // "everything else" (itself expanded, recursively, as a lazy `async move` block so it's a
    // `Future` rather than an already-evaluated value) via `select2`, then match on which side
    // won.
    (@rec {$pat1:pat, $fut1:expr, $guard1:expr, $body1:block} $({$pat:pat, $fut:expr, $guard:expr, $body:block})+) => {{
        match $crate::select::select2(
            $crate::select::guarded($guard1, $fut1),
            async move { $crate::select!(@rec $({$pat, $fut, $guard, $body})+) },
        )
        .await
        {
            $crate::select::Either::Left($pat1) => $body1,
            $crate::select::Either::Right(__sturdy_select_rest) => __sturdy_select_rest,
        }
    }};

    // Catch-all entry point: anything that didn't match one of the `@`-tagged internal arms above
    // is a top-level user invocation — kick off branch collection. Must stay last; see the note
    // above `biased;`.
    ($($rest:tt)*) => {
        $crate::select!(@collect [] $($rest)*)
    };
}

#[cfg(test)]
mod tests {
    use crate::task::block_on;

    #[test]
    fn else_arm_fires_when_all_disabled() {
        let result = block_on(async {
            crate::select! {
                _v = std::future::pending::<()>(), if false => { "never" }
                else => { "else" }
            }
        });
        assert_eq!(result, "else");
    }

    #[test]
    fn multi_branch_else_fires_when_all_disabled() {
        let result = block_on(async {
            crate::select! {
                _ = std::future::pending::<()>(), if false => { "a" }
                _ = std::future::pending::<()>(), if false => { "b" }
                _ = std::future::pending::<()>(), if false => { "c" }
                else => { "else" }
            }
        });
        assert_eq!(result, "else");
    }

    #[test]
    fn guard_skips_disabled_branch() {
        let result = block_on(async {
            crate::select! {
                v = async { "enabled" }, if true => { v }
                v = async { "disabled" }, if false => { v }
            }
        });
        assert_eq!(result, "enabled");
    }

    #[test]
    fn biased_marker_is_accepted_and_first_listed_wins_ties() {
        let result = block_on(async {
            crate::select! {
                biased;
                v = async { 1 } => { v }
                v = async { 2 } => { v }
            }
        });
        assert_eq!(result, 1);
    }

    #[test]
    fn no_else_and_no_enabled_branch_panics() {
        let result = std::panic::catch_unwind(|| {
            block_on(async {
                crate::select! {
                    _ = std::future::pending::<()>(), if false => { () }
                }
            })
        });
        assert!(result.is_err());
    }
}
