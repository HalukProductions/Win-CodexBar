//! Bounded, order-preserving provider fetches for `codexbar usage` (upstream
//! 0.76.0 #4501 `collectUsageOutputs`).
//!
//! Up to [`MAX_CONCURRENT_PROVIDER_FETCHES`] providers run at once. Results
//! come back in selection order whatever order the fetches finish in, and a
//! provider that fails renders its own error row without stopping the others.
//! Diagnostics from each fetch are held until every fetch has finished. The
//! credential writeback audit is on [`fetch_in_selection_order`].

use std::future::Future;
use std::io::Write;

use futures::StreamExt;
use tracing::instrument::WithSubscriber;

use crate::logging::{self, DiagnosticBuffer};

/// Most provider fetches in flight for one `usage` command.
pub(super) const MAX_CONCURRENT_PROVIDER_FETCHES: usize = 3;

/// Run `fetch` for every item with at most three in flight, and return the
/// fetch values in item order.
///
/// Diagnostics: events emitted on a fetch's own future are captured and
/// written to stderr after all fetches finish, in item order. Nothing from
/// the fetches streams live while they run. Events from `tokio::spawn` and
/// `spawn_blocking` tasks that a provider starts run outside the fetch future,
/// so they go to the global subscriber, print live and are not held or
/// reordered.
///
/// Writeback audit (CLI usage path, checked against origin/main 1bfe3e930):
/// - No token-account writeback exists on this path. `FetchContext` has no
///   token-updater hook, and `TokenAccountStore::save`/`save_provider` run only
///   from the `codexbar account` commands (`cli/account.rs`). The usage path
///   only loads (`cli/usage/fetch_helpers.rs`).
/// - `Settings::load` writes one thing: the one-shot tray-pin migration, on
///   Windows without `CODEXBAR_CONFIG` (`settings/persistence.rs:28-31,127-141`).
///   `UsageCommand::from_args` loads settings and writes that marker before the
///   fan-out starts. `fetch_helpers.rs:59` loads settings again for each
///   provider, but by then the marker exists and those loads do not write. If
///   the marker cannot be written, per-provider loads repeat the same save.
///   Settings writes are atomic (`secure_file::write_string` ->
///   `atomic_file::write_atomic_private`), so the file stays whole and the
///   last identical write wins.
/// - Provider-owned writers. Claude credential refresh and account-store writes
///   run under `CREDENTIAL_OPERATION` (`providers/claude/accounts.rs`), which
///   `ClaudeOAuthFetcher::fetch` holds across the whole fetch
///   (`providers/claude/oauth/mod.rs`). That lock is the serialization for
///   Claude credential refresh. Grok takes its own `CREDENTIAL_OPERATION`
///   (`providers/grok/accounts/refresh.rs:22,27`) around renewal and its store
///   write (`persist_renewed`, `:135`). Other writers are keyed per provider,
///   such as the burndown history (`core/quota_burndown.rs:188-189`) and the
///   cookie cache (`browser/cookie_cache.rs`). A command fetches each provider
///   once, so no two concurrent fetches share a store file.
pub(super) async fn fetch_in_selection_order<I, T, F, Fut>(items: &[I], fetch: F) -> Vec<T>
where
    I: Copy,
    F: Fn(I) -> Fut,
    Fut: Future<Output = T>,
{
    fetch_in_selection_order_to(items, fetch, &mut std::io::stderr()).await
}

/// [`fetch_in_selection_order`] with the diagnostics replay written to `out`.
/// Tests pass a buffer; the command passes stderr.
pub(super) async fn fetch_in_selection_order_to<I, T, F, Fut, W>(
    items: &[I],
    fetch: F,
    out: &mut W,
) -> Vec<T>
where
    I: Copy,
    F: Fn(I) -> Fut,
    Fut: Future<Output = T>,
    W: Write,
{
    let captured = capture_in_selection_order(items, fetch).await;
    let mut values = Vec::with_capacity(captured.len());
    for (value, diagnostics) in captured {
        if !diagnostics.is_empty() {
            let _written = out.write_all(diagnostics.as_bytes());
        }
        values.push(value);
    }
    values
}

/// The bounded fetch without the final stderr replay: returns each value with
/// its captured diagnostics, in item order.
pub(super) async fn capture_in_selection_order<I, T, F, Fut>(
    items: &[I],
    fetch: F,
) -> Vec<(T, String)>
where
    I: Copy,
    F: Fn(I) -> Fut,
    Fut: Future<Output = T>,
{
    futures::stream::iter(items.iter().copied())
        .map(|item| {
            let buffer = DiagnosticBuffer::default();
            let fut = fetch(item);
            async move {
                let value = fut
                    .with_subscriber(logging::capture_dispatch(&buffer))
                    .await;
                (value, buffer.contents())
            }
        })
        // `buffered` keeps item order and bounds the in-flight futures.
        .buffered(MAX_CONCURRENT_PROVIDER_FETCHES)
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use tokio::sync::oneshot;

    use super::{
        MAX_CONCURRENT_PROVIDER_FETCHES, capture_in_selection_order, fetch_in_selection_order_to,
    };

    /// Items 0..5. Fetches are gated so the test controls completion order:
    /// the first three start together, then 2, 1, 0 finish, and 3 and 4 start
    /// only after slots free up.
    #[tokio::test]
    async fn results_and_diagnostics_keep_selection_order_when_later_fetches_finish_first() {
        const COUNT: usize = 5;
        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        for _ in 0..COUNT {
            let (tx, rx) = oneshot::channel::<()>();
            senders.push(Some(tx));
            receivers.push(RefCell::new(Some(rx)));
        }
        let receivers = Rc::new(receivers);
        let started = Rc::new(Cell::new(0usize));
        let finished_order = Rc::new(RefCell::new(Vec::new()));

        let items: Vec<usize> = (0..COUNT).collect();
        let fetch = {
            let receivers = Rc::clone(&receivers);
            let started = Rc::clone(&started);
            let finished_order = Rc::clone(&finished_order);
            move |index: usize| {
                let receivers = Rc::clone(&receivers);
                let started = Rc::clone(&started);
                let finished_order = Rc::clone(&finished_order);
                async move {
                    started.set(started.get() + 1);
                    tracing::warn!("diagnostic for provider {index}");
                    let rx = receivers[index]
                        .borrow_mut()
                        .take()
                        .expect("gate taken once");
                    let _released = rx.await;
                    finished_order.borrow_mut().push(index);
                    format!("row-{index}")
                }
            }
        };

        let driver = async {
            wait_for(|| started.get() >= 3).await;
            // Later providers finish first: release 2, then 1, then 0.
            for index in [2, 1, 0] {
                release(&mut senders, index);
                wait_for(|| finished_order.borrow().len() > (2 - index)).await;
            }
            wait_for(|| started.get() >= 4).await;
            release(&mut senders, 3);
            wait_for(|| started.get() >= 5).await;
            release(&mut senders, 4);
        };

        let (captured, ()) = tokio::join!(capture_in_selection_order(&items, fetch), driver);

        let values: Vec<String> = captured.iter().map(|(value, _)| value.clone()).collect();
        assert_eq!(values, ["row-0", "row-1", "row-2", "row-3", "row-4"]);
        assert_eq!(*finished_order.borrow(), vec![2, 1, 0, 3, 4]);
        // Diagnostics stay with their provider: each buffer holds only its own line.
        for (index, (_, diagnostics)) in captured.iter().enumerate() {
            assert!(
                diagnostics.contains(&format!("diagnostic for provider {index}")),
                "missing diagnostic for {index}: {diagnostics:?}"
            );
            for other in (0..COUNT).filter(|other| *other != index) {
                assert!(!diagnostics.contains(&format!("diagnostic for provider {other}")));
            }
        }
    }

    #[tokio::test]
    async fn at_most_three_fetches_are_in_flight() {
        const COUNT: usize = 7;
        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        for _ in 0..COUNT {
            let (tx, rx) = oneshot::channel::<()>();
            senders.push(Some(tx));
            receivers.push(RefCell::new(Some(rx)));
        }
        let receivers = Rc::new(receivers);
        let in_flight = Rc::new(Cell::new(0usize));
        let max_in_flight = Rc::new(Cell::new(0usize));
        let started = Rc::new(Cell::new(0usize));

        let items: Vec<usize> = (0..COUNT).collect();
        let fetch = {
            let receivers = Rc::clone(&receivers);
            let in_flight = Rc::clone(&in_flight);
            let max_in_flight = Rc::clone(&max_in_flight);
            let started = Rc::clone(&started);
            move |index: usize| {
                let receivers = Rc::clone(&receivers);
                let in_flight = Rc::clone(&in_flight);
                let max_in_flight = Rc::clone(&max_in_flight);
                let started = Rc::clone(&started);
                async move {
                    started.set(started.get() + 1);
                    in_flight.set(in_flight.get() + 1);
                    max_in_flight.set(max_in_flight.get().max(in_flight.get()));
                    let rx = receivers[index]
                        .borrow_mut()
                        .take()
                        .expect("gate taken once");
                    let _released = rx.await;
                    in_flight.set(in_flight.get() - 1);
                    index
                }
            }
        };

        let driver = async {
            wait_for(|| started.get() >= MAX_CONCURRENT_PROVIDER_FETCHES).await;
            // Give the stream every chance to start a fourth fetch; it must not.
            for _ in 0..50 {
                tokio::task::yield_now().await;
            }
            assert_eq!(started.get(), MAX_CONCURRENT_PROVIDER_FETCHES);
            assert_eq!(in_flight.get(), MAX_CONCURRENT_PROVIDER_FETCHES);
            for index in 0..COUNT {
                release(&mut senders, index);
                let next_expected = (index + MAX_CONCURRENT_PROVIDER_FETCHES + 1).min(COUNT);
                wait_for(|| started.get() >= next_expected).await;
                assert!(in_flight.get() <= MAX_CONCURRENT_PROVIDER_FETCHES);
            }
        };

        let (captured, ()) = tokio::join!(capture_in_selection_order(&items, fetch), driver);

        assert_eq!(max_in_flight.get(), MAX_CONCURRENT_PROVIDER_FETCHES);
        let order: Vec<usize> = captured.iter().map(|(value, _)| *value).collect();
        assert_eq!(order, items);
    }

    #[tokio::test]
    async fn one_failing_provider_does_not_stop_the_others() {
        let items = [0usize, 1, 2];
        let captured = capture_in_selection_order(&items, |index| async move {
            if index == 1 {
                Err(format!("provider {index} failed"))
            } else {
                Ok(format!("row-{index}"))
            }
        })
        .await;
        let values: Vec<Result<String, String>> =
            captured.into_iter().map(|(value, _)| value).collect();
        assert_eq!(
            values,
            vec![
                Ok("row-0".to_string()),
                Err("provider 1 failed".to_string()),
                Ok("row-2".to_string()),
            ]
        );
    }

    /// The stderr replay writes each fetch's diagnostics in item order, even
    /// though the later fetches finish first. Each fetch yields a different
    /// number of times, so completion order is fixed on the single-threaded
    /// test runtime.
    #[tokio::test]
    async fn stderr_replay_keeps_selection_order_when_later_fetches_finish_first() {
        const COUNT: usize = 3;
        let items: Vec<usize> = (0..COUNT).collect();
        let finished_order = Rc::new(RefCell::new(Vec::new()));
        let mut out: Vec<u8> = Vec::new();
        let values = fetch_in_selection_order_to(
            &items,
            |index| {
                let finished_order = Rc::clone(&finished_order);
                async move {
                    for _ in 0..(COUNT - index) * 10 {
                        tokio::task::yield_now().await;
                    }
                    tracing::warn!("replay diagnostic for provider {index}");
                    finished_order.borrow_mut().push(index);
                    index
                }
            },
            &mut out,
        )
        .await;

        assert_eq!(*finished_order.borrow(), vec![2, 1, 0]);
        assert_eq!(values, items);
        let text = String::from_utf8(out).expect("replay is UTF-8");
        assert_eq!(text.matches("replay diagnostic").count(), COUNT);
        let positions: Vec<usize> = (0..COUNT)
            .map(|index| {
                text.find(&format!("replay diagnostic for provider {index}"))
                    .unwrap_or_else(|| panic!("missing diagnostic {index}: {text:?}"))
            })
            .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "diagnostics out of order: {text:?}"
        );
    }

    /// Yield until `condition` holds. Each pass lets the buffered stream poll.
    async fn wait_for(condition: impl Fn() -> bool) {
        for _ in 0..10_000 {
            if condition() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("condition not reached");
    }

    fn release(senders: &mut [Option<oneshot::Sender<()>>], index: usize) {
        if let Some(tx) = senders[index].take() {
            let _sent = tx.send(());
        }
    }
}
