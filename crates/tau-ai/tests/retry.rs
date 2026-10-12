//! Retry delay policy, retry messaging and cancellation-aware backoff.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tau_ai::provider_events::ProviderEvent;
use tau_ai::retry::{provider_retry_event, retry_delay_seconds, wait_for_retry};
use tau_protocol::provider::ManualCancellation;

/// The fields of a retry event, failing loudly on any other variant.
fn retry_fields(event: ProviderEvent) -> (u32, u32, f64, String) {
    match event {
        ProviderEvent::Retry {
            attempt,
            max_attempts,
            delay_seconds,
            message,
            ..
        } => (attempt, max_attempts, delay_seconds, message),
        other => panic!("expected a retry event, got {other:?}"),
    }
}

#[test]
fn the_retry_delay_doubles_from_a_quarter_second() {
    assert_eq!(retry_delay_seconds(0, 1.0), 0.25);
    assert_eq!(retry_delay_seconds(1, 1.0), 0.5);
    assert_eq!(retry_delay_seconds(2, 1.0), 1.0);
}

#[test]
fn the_delay_never_exceeds_the_configured_maximum() {
    assert_eq!(retry_delay_seconds(3, 1.0), 1.0);
    assert_eq!(retry_delay_seconds(9, 1.0), 1.0);
    assert_eq!(retry_delay_seconds(4, 30.0), 4.0);
}

#[test]
fn a_non_positive_maximum_disables_backoff() {
    assert_eq!(retry_delay_seconds(0, 0.0), 0.0);
    assert_eq!(retry_delay_seconds(2, -1.0), 0.0);
}

#[test]
fn the_base_delay_is_capped_by_the_maximum() {
    assert_eq!(retry_delay_seconds(0, 0.1), 0.1);
    assert_eq!(retry_delay_seconds(1, 0.1), 0.1);
}

#[test]
fn the_retry_event_matches_the_python_wording() {
    let event = provider_retry_event(0, 2, 0.5, "HTTP 429");

    assert_eq!(event.kind(), "retry");
    let (attempt, max_attempts, delay_seconds, message) = retry_fields(event);
    assert_eq!(attempt, 2);
    assert_eq!(max_attempts, 3);
    assert_eq!(delay_seconds, 0.5);
    assert_eq!(
        message,
        "Retrying provider request 2/3 after HTTP 429 in 0.5s."
    );
}

#[test]
fn the_retry_event_omits_the_delay_when_there_is_none() {
    let (_, _, _, message) = retry_fields(provider_retry_event(1, 3, 0.0, "connection reset"));

    assert_eq!(
        message,
        "Retrying provider request 3/4 after connection reset."
    );
}

#[test]
fn a_whole_second_delay_is_written_without_a_decimal_point() {
    let (_, _, _, message) = retry_fields(provider_retry_event(0, 2, 1.0, "HTTP 500"));

    assert_eq!(
        message,
        "Retrying provider request 2/3 after HTTP 500 in 1s."
    );
}

#[tokio::test]
async fn a_wait_returns_true_when_nothing_cancels_it() {
    assert!(wait_for_retry(0.0, None).await);
    assert!(wait_for_retry(0.06, None).await);
}

#[tokio::test]
async fn a_cancelled_wait_returns_false() {
    let signal = ManualCancellation::default();
    signal.cancel();

    assert!(!wait_for_retry(0.0, Some(&signal)).await);
    assert!(!wait_for_retry(0.2, Some(&signal)).await);
}

#[tokio::test]
async fn cancellation_interrupts_a_long_backoff() {
    let signal = Arc::new(ManualCancellation::default());
    let trigger = Arc::clone(&signal);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        trigger.cancel();
    });

    let started = Instant::now();
    let completed = wait_for_retry(10.0, Some(signal.as_ref())).await;

    assert!(
        !completed,
        "an interrupted backoff reports that it did not finish"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "backoff must stop when cancelled, not sleep the full delay"
    );
}
