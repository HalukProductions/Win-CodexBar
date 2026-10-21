//! Ordered disk writes for in-memory caches.
//!
//! A cache copy is taken under the state lock together with a revision that
//! rises with every change. The disk write runs after the lock is released, so
//! two writes can finish out of order. Each cache keeps its own gate, and a
//! write lands only when its revision is newer than every revision accepted
//! before it.

use std::io;
use std::sync::Mutex;

/// Runs `write` only when `revision` is newer than every revision accepted so
/// far. The revision is recorded before the write, so a failed write still
/// blocks older snapshots. The gate is held for the whole write, so writes are
/// serialized. Returns whether `write` ran.
pub(crate) fn write_in_order(
    gate: &Mutex<u64>,
    revision: u64,
    write: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    let mut accepted = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if revision <= *accepted {
        return Ok(false);
    }
    *accepted = revision;
    write().map(|()| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_apply_in_revision_order_and_skip_older_snapshots() {
        let gate = Mutex::new(0);
        let mut writes = Vec::new();
        assert!(
            write_in_order(&gate, 2, || {
                writes.push("r2");
                Ok(())
            })
            .unwrap()
        );
        // An older revision that arrives late must not land after the newer one.
        assert!(
            !write_in_order(&gate, 1, || {
                writes.push("r1");
                Ok(())
            })
            .unwrap()
        );
        // Equal revision is a repeat, not a new change.
        assert!(
            !write_in_order(&gate, 2, || {
                writes.push("r2-again");
                Ok(())
            })
            .unwrap()
        );
        assert!(
            write_in_order(&gate, 3, || {
                writes.push("r3");
                Ok(())
            })
            .unwrap()
        );
        assert_eq!(writes, vec!["r2", "r3"]);
    }

    #[test]
    fn failed_write_still_blocks_older_revisions() {
        let gate = Mutex::new(0);
        let failed = write_in_order(&gate, 5, || Err(io::Error::other("disk full")));
        assert!(failed.is_err());
        let mut wrote_older = false;
        assert!(
            !write_in_order(&gate, 4, || {
                wrote_older = true;
                Ok(())
            })
            .unwrap()
        );
        assert!(!wrote_older);
    }
}
