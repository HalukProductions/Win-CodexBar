use super::{MAX_GRANT_RECORDS, MAX_RESETS, inventory_from_block, redact_usage_body};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{Value, json};

const DAY: i64 = 86_400;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 27, 4, 35, 27).unwrap()
}

fn iso(offset_secs: i64) -> String {
    (now() + Duration::seconds(offset_secs)).to_rfc3339()
}

/// Grant in the observed wire shape, including fields CodexBar must ignore
/// (`id`, `label`, `clears`, `usable_now`).
fn grant(resets_left: i64, ends_in: Option<i64>) -> Value {
    json!({
        "id": "grant_secret",
        "label": "Fixture reset",
        "resets_total": resets_left.max(1),
        "resets_left": resets_left,
        "starts_at": iso(-DAY),
        "ends_at": ends_in.map(iso),
        "clears": ["five_hour", "seven_day"],
        "paused": false,
        "usable_now": true,
    })
}

fn with(mut grant: Value, key: &str, value: Value) -> Value {
    grant[key] = value;
    grant
}

fn eligible(grants: Vec<Value>) -> Value {
    json!({ "eligible": true, "grants": grants })
}

fn count_and_expiry(block: &Value) -> Option<(u32, Option<DateTime<Utc>>)> {
    inventory_from_block(block, now()).map(|item| (item.available_count, item.next_expires_at))
}

#[test]
fn sums_remaining_resets_of_started_unpaused_unexpired_grants() {
    let block = eligible(vec![
        grant(2, Some(5 * DAY)),
        grant(1, Some(DAY)),
        grant(1, None),
        // `usable_now` is not consulted.
        with(grant(1, Some(2 * DAY)), "usable_now", json!(false)),
        with(grant(1, Some(DAY)), "paused", json!(true)),
        grant(0, Some(DAY)),
        grant(1, Some(-60)),
        with(grant(1, Some(9 * DAY)), "starts_at", json!(iso(DAY))),
    ]);

    let item = inventory_from_block(&block, now()).unwrap();

    assert_eq!(item.id, "reset-credits");
    assert_eq!(item.title, "Limit Reset Credits");
    assert_eq!(item.available_count, 5);
    assert_eq!(item.next_expires_at, Some(now() + Duration::seconds(DAY)));
}

#[test]
fn no_expiry_grants_sort_last_and_leave_no_next_expiry() {
    let mixed = eligible(vec![grant(1, None), grant(1, Some(3 * DAY))]);
    assert_eq!(
        count_and_expiry(&mixed),
        Some((2, Some(now() + Duration::seconds(3 * DAY))))
    );

    let open_ended = eligible(vec![grant(2, None)]);
    assert_eq!(count_and_expiry(&open_ended), Some((2, None)));
}

#[test]
fn grant_ids_never_reach_the_inventory() {
    let item = inventory_from_block(&eligible(vec![grant(1, Some(DAY))]), now()).unwrap();
    assert!(!format!("{item:?}").contains("grant_secret"));
}

#[test]
fn malformed_grants_are_dropped_without_hiding_valid_grants() {
    let block = eligible(vec![
        json!({"resets_left": "many", "paused": false}),
        json!({"resets_left": 2, "resets_total": 1, "paused": false}),
        json!({"resets_left": -1, "paused": false}),
        json!({"resets_left": 1, "resets_total": 1}),
        json!({"resets_left": 1, "resets_total": 1, "paused": null}),
        json!({"resets_left": 1, "paused": false, "ends_at": "next tuesday"}),
        json!({"resets_left": 1, "paused": false, "starts_at": "soon"}),
        json!({"resets_left": 1, "resets_total": "one", "paused": false}),
        json!("not an object"),
        json!({"resets_left": 1, "resets_total": 1, "paused": false,
               "starts_at": null, "ends_at": null}),
    ]);

    assert_eq!(count_and_expiry(&block), Some((1, None)));
}

#[test]
fn fractional_second_bounds_are_readable() {
    let block = eligible(vec![json!({
        "resets_left": 1,
        "paused": false,
        "starts_at": "2026-09-26T04:35:27.123456+00:00",
        "ends_at": "2026-10-23T00:00:00.500Z",
    })]);

    let (count, expiry) = count_and_expiry(&block).unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        expiry,
        Some(Utc.with_ymd_and_hms(2026, 10, 23, 0, 0, 0).unwrap() + Duration::milliseconds(500))
    );
}

#[test]
fn implausibly_large_inventory_shows_nothing() {
    let limit = i64::try_from(MAX_RESETS).unwrap();
    assert_eq!(
        count_and_expiry(&eligible(vec![grant(limit, Some(DAY))])).map(|(count, _)| count),
        Some(u32::try_from(MAX_RESETS).unwrap())
    );

    for grants in [
        vec![grant(limit + 1, Some(DAY))],
        vec![grant(i64::MAX, Some(DAY))],
        vec![
            grant(limit / 2 + 1, Some(DAY)),
            grant(limit / 2 + 1, Some(DAY)),
        ],
        // More grant records than the cap, even though all but one are used up.
        std::iter::repeat_n(grant(0, Some(DAY)), MAX_GRANT_RECORDS)
            .chain([grant(1, Some(DAY))])
            .collect(),
    ] {
        assert_eq!(inventory_from_block(&eligible(grants), now()), None);
    }

    // Grants that have not started neither count nor trip the cap.
    let with_future = eligible(vec![
        with(
            grant(limit + 1, Some(9 * DAY)),
            "starts_at",
            json!(iso(DAY)),
        ),
        grant(1, Some(DAY)),
    ]);
    assert_eq!(
        count_and_expiry(&with_future),
        Some((1, Some(now() + Duration::seconds(DAY))))
    );
}

#[test]
fn grant_record_cap_is_inclusive() {
    let at_cap = eligible(
        std::iter::repeat_n(grant(0, Some(DAY)), MAX_GRANT_RECORDS - 1)
            .chain([grant(1, Some(DAY))])
            .collect(),
    );
    assert_eq!(
        count_and_expiry(&at_cap),
        Some((1, Some(now() + Duration::seconds(DAY))))
    );
}

#[test]
fn ineligible_absent_or_unreadable_block_shows_nothing() {
    let grants = json!([grant(1, Some(DAY))]);
    for block in [
        json!({"eligible": false, "ineligible_reason": "surface", "grants": grants}),
        json!({"grants": grants}),
        json!({"eligible": null, "grants": grants}),
        json!({"eligible": "yes", "grants": grants}),
        eligible(vec![]),
        json!({"eligible": true}),
        json!({"eligible": true, "grants": "none"}),
        Value::Null,
        json!([]),
    ] {
        assert_eq!(inventory_from_block(&block, now()), None, "{block}");
    }
}

/// Inventory read back from a stored (redacted) usage body.
fn stored_count(block: &Value) -> Option<(u32, Option<DateTime<Utc>>)> {
    let body = json!({ "five_hour": { "utilization": 12 }, "cedar_ember": block }).to_string();
    let stored: Value = serde_json::from_str(&redact_usage_body(&body)).unwrap();
    count_and_expiry(&stored["cedar_ember"])
}

#[test]
fn redacted_body_drops_grant_handles_and_keeps_the_inventory() {
    let block = eligible(vec![
        grant(2, Some(DAY)),
        with(grant(1, None), "paused", json!(true)),
        json!("not a grant"),
    ]);
    let body = json!({ "five_hour": { "utilization": 12 }, "cedar_ember": block }).to_string();

    let stored = redact_usage_body(&body);

    for hidden in ["grant_secret", "Fixture reset", "clears", "usable_now"] {
        assert!(!stored.contains(hidden), "{hidden} reached the stored body");
    }
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(stored["five_hour"]["utilization"], json!(12));
    assert_eq!(stored_count(&block), count_and_expiry(&block));
    assert_eq!(
        count_and_expiry(&stored["cedar_ember"]).map(|(n, _)| n),
        Some(2)
    );
}

#[test]
fn redaction_keeps_the_grant_record_cap_and_unreadable_blocks() {
    for records in [MAX_GRANT_RECORDS, MAX_GRANT_RECORDS + 1] {
        let block = eligible(
            std::iter::once(grant(1, Some(DAY)))
                .chain(std::iter::repeat_n(json!({}), records - 1))
                .collect(),
        );
        assert_eq!(stored_count(&block), count_and_expiry(&block), "{records}");
    }
    for block in [
        json!(null),
        json!("eligible"),
        json!({ "eligible": true, "grants": "invalid" }),
        json!({ "eligible": false, "grants": [grant(1, Some(DAY))] }),
    ] {
        assert_eq!(stored_count(&block), None, "{block}");
    }
}

#[test]
fn bodies_without_the_block_are_stored_unchanged() {
    for body in [r#"{"five_hour":{"utilization":12}}"#, "not json", "[1,2]"] {
        assert_eq!(redact_usage_body(body), body);
    }
}
