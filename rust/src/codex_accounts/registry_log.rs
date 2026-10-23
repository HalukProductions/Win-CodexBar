//! Redacted diagnostics for Codex account registry mutations.
//!
//! These lines help explain a saved account that disappears. They carry only a
//! fixed operation name, row counts and an outcome. They never carry account
//! ids, emails, managed-home paths, tokens or error text.

/// Fixed operation names for registry mutations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryOperation {
    /// A managed account was added through login, or re-authenticated in place.
    AddOrReauth,
    /// A saved account was removed.
    Remove,
    /// Any other registry write, such as metadata reconciliation or switch bookkeeping.
    RegistryWrite,
}

impl RegistryOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AddOrReauth => "add-or-reauth",
            Self::Remove => "remove",
            Self::RegistryWrite => "registry-write",
        }
    }
}

/// Field values for one registry write attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistryWriteLog {
    pub(crate) operation: &'static str,
    pub(crate) rows_before: String,
    pub(crate) rows_requested: String,
    pub(crate) rows_written: String,
    pub(crate) result: &'static str,
}

/// Builds the fields for one registry write attempt.
///
/// Takes only counts and the outcome, so no identifier, path or error text can
/// reach the log line.
pub(crate) fn registry_write_log(
    operation: RegistryOperation,
    rows_before: Option<usize>,
    rows_requested: usize,
    succeeded: bool,
) -> RegistryWriteLog {
    RegistryWriteLog {
        operation: operation.as_str(),
        rows_before: count_field(rows_before),
        rows_requested: rows_requested.to_string(),
        rows_written: if succeeded {
            rows_requested.to_string()
        } else {
            "unknown".to_string()
        },
        result: if succeeded { "success" } else { "failed" },
    }
}

fn count_field(count: Option<usize>) -> String {
    count.map_or_else(|| "unknown".to_string(), |count| count.to_string())
}

/// Emits one registry-write log line built by [`registry_write_log`].
pub(crate) fn log_registry_write(log: &RegistryWriteLog) {
    if log.result == "success" {
        tracing::info!(
            operation = log.operation,
            rows_before = %log.rows_before,
            rows_requested = %log.rows_requested,
            rows_written = %log.rows_written,
            result = log.result,
            "Managed Codex account registry write"
        );
    } else {
        tracing::error!(
            operation = log.operation,
            rows_before = %log.rows_before,
            rows_requested = %log.rows_requested,
            rows_written = %log.rows_written,
            result = log.result,
            "Managed Codex account registry write failed"
        );
    }
}

/// Emits one log line for a successful managed-home deletion.
pub(crate) fn log_home_deletion(operation: RegistryOperation) {
    tracing::info!(
        operation = operation.as_str(),
        "Managed Codex account home deleted"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORBIDDEN_FRAGMENTS: [&str; 7] = [
        "@",
        "/",
        "\\",
        "example.com",
        "managed-homes",
        "auth.json",
        "token",
    ];

    fn assert_only_fixed_vocabulary(log: &RegistryWriteLog) {
        let rendered = format!("{log:?}");
        for fragment in FORBIDDEN_FRAGMENTS {
            assert!(
                !rendered.contains(fragment),
                "registry log leaked `{fragment}`: {rendered}"
            );
        }
        for value in [&log.rows_before, &log.rows_requested, &log.rows_written] {
            assert!(
                value == "unknown" || value.chars().all(|c| c.is_ascii_digit()),
                "count field is not a bare number or `unknown`: {value}"
            );
        }
        assert!(matches!(log.result, "success" | "failed"));
        assert!(matches!(
            log.operation,
            "add-or-reauth" | "remove" | "registry-write"
        ));
    }

    #[test]
    fn success_record_carries_only_counts_and_operation() {
        let log = registry_write_log(RegistryOperation::Remove, Some(3), 2, true);

        assert_eq!(log.operation, "remove");
        assert_eq!(log.rows_before, "3");
        assert_eq!(log.rows_requested, "2");
        assert_eq!(log.rows_written, "2");
        assert_eq!(log.result, "success");
        assert_only_fixed_vocabulary(&log);
    }

    #[test]
    fn failed_write_reports_unknown_written_rows() {
        let log = registry_write_log(RegistryOperation::AddOrReauth, None, 4, false);

        assert_eq!(log.operation, "add-or-reauth");
        assert_eq!(log.rows_before, "unknown");
        assert_eq!(log.rows_requested, "4");
        assert_eq!(log.rows_written, "unknown");
        assert_eq!(log.result, "failed");
        assert_only_fixed_vocabulary(&log);
    }

    #[test]
    fn registry_write_default_operation_name_is_fixed() {
        let log = registry_write_log(RegistryOperation::RegistryWrite, Some(0), 0, true);

        assert_eq!(log.operation, "registry-write");
        assert_eq!(log.rows_before, "0");
        assert_only_fixed_vocabulary(&log);
    }

    #[test]
    fn every_operation_name_is_identity_free() {
        for operation in [
            RegistryOperation::AddOrReauth,
            RegistryOperation::Remove,
            RegistryOperation::RegistryWrite,
        ] {
            let name = operation.as_str();
            assert!(!name.contains('@'));
            assert!(!name.contains('/'));
            assert!(!name.contains('\\'));
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        }
    }
}
