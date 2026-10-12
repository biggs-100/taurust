//! Retry policy for provider requests.
//!
//! Ported from tau's `tau_ai/retry.py`. The delay doubles from a quarter of a
//! second and is capped by provider configuration; a non-positive cap disables
//! backoff entirely.

use std::time::Duration;

use tau_protocol::provider::CancellationToken;

use crate::provider_events::ProviderEvent;

/// How often a waiting retry checks for cancellation.
pub const RETRY_POLL_SECONDS: f64 = 0.05;

/// The delay before the first retry.
pub const RETRY_BASE_DELAY_SECONDS: f64 = 0.25;

/// The delay before the retry after `attempt`, capped by `max_delay_seconds`.
///
/// `attempt` is zero-based: attempt 0 is the first retry, so it waits the base
/// delay. A maximum of zero or less disables waiting.
pub fn retry_delay_seconds(attempt: u32, max_delay_seconds: f64) -> f64 {
    if max_delay_seconds <= 0.0 {
        return 0.0;
    }
    let base_delay = RETRY_BASE_DELAY_SECONDS.min(max_delay_seconds);
    max_delay_seconds.min(base_delay * 2f64.powi(attempt as i32))
}

/// Build the retry progress event for the retry that follows `attempt`.
///
/// `attempt` is zero-based, so the retry it announces is attempt + 2 as a
/// one-based count.
pub fn provider_retry_event(
    attempt: u32,
    max_retries: u32,
    delay_seconds: f64,
    reason: &str,
) -> ProviderEvent {
    let next_attempt = attempt + 2;
    let max_attempts = max_retries + 1;
    let delay_suffix = if delay_seconds == 0.0 {
        String::new()
    } else {
        format!(" in {}s", format_seconds(delay_seconds))
    };

    ProviderEvent::Retry {
        attempt: next_attempt,
        max_attempts,
        delay_seconds,
        message: format!(
            "Retrying provider request {next_attempt}/{max_attempts} after {reason}{delay_suffix}."
        ),
        data: None,
    }
}

/// Wait before a retry, returning early when the caller cancels.
///
/// The wait is polled rather than slept whole, so a cancellation is noticed
/// within [`RETRY_POLL_SECONDS`] instead of after the full delay. Returns
/// whether the wait ran to completion.
pub async fn wait_for_retry(delay_seconds: f64, signal: Option<&dyn CancellationToken>) -> bool {
    if delay_seconds <= 0.0 {
        return !is_cancelled(signal);
    }

    let mut remaining = delay_seconds;
    while remaining > 0.0 {
        if is_cancelled(signal) {
            return false;
        }
        let step = RETRY_POLL_SECONDS.min(remaining);
        tokio::time::sleep(Duration::from_secs_f64(step)).await;
        remaining -= step;
    }

    !is_cancelled(signal)
}

fn is_cancelled(signal: Option<&dyn CancellationToken>) -> bool {
    signal.is_some_and(CancellationToken::is_cancelled)
}

/// Render a delay the way Python's `%g` does for the domain delays live in.
///
/// Backoff delays are always between [`RETRY_POLL_SECONDS`] and the configured
/// cap, so they render in plain notation with trailing zeros stripped: `1.0`
/// becomes `1`, `0.25` stays `0.25`. Values outside that domain fall back to
/// exponent notation, which `%g` would also choose, though the exact digit count
/// is not reproduced because nothing reaches it.
fn format_seconds(value: f64) -> String {
    if value != 0.0 && !(1e-4..1e6).contains(&value.abs()) {
        return format!("{value:e}");
    }

    let text = format!("{value:.6}");
    match text.split_once('.') {
        Some((whole, fraction)) => {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{fraction}")
            }
        }
        None => text,
    }
}
