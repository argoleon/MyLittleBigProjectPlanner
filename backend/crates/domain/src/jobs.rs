//! Rules of the background job queue (specification §17).

use std::time::Duration;

/// Attempts of a job before it fails for good.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 5;

/// Delay before the first retry; doubles on each failed attempt.
pub const FIRST_RETRY_DELAY: Duration = Duration::from_secs(10);

/// Longest delay between two attempts.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(60 * 60);

/// What to do after attempt number `attempt` (1-based) of a job failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterFailure {
    /// Try again after this delay.
    RetryIn(Duration),
    /// Out of attempts: the job fails for good.
    GiveUp,
}

/// Exponential backoff: 10 s, 20 s, 40 s… capped at one hour, until
/// `max_attempts` attempts were made.
pub fn after_failure(attempt: u32, max_attempts: u32) -> AfterFailure {
    if attempt >= max_attempts {
        return AfterFailure::GiveUp;
    }
    let exponent = attempt.saturating_sub(1).min(20);
    let delay = FIRST_RETRY_DELAY.saturating_mul(1 << exponent);
    AfterFailure::RetryIn(delay.min(MAX_RETRY_DELAY))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_double_from_ten_seconds() {
        let delays: Vec<_> = (1..=4).map(|a| after_failure(a, 10)).collect();
        assert_eq!(
            delays,
            [10, 20, 40, 80].map(|s| AfterFailure::RetryIn(Duration::from_secs(s)))
        );
    }

    #[test]
    fn delays_are_capped_at_one_hour() {
        assert_eq!(
            after_failure(9, 100),
            AfterFailure::RetryIn(Duration::from_secs(2560))
        );
        assert_eq!(after_failure(10, 100), AfterFailure::RetryIn(MAX_RETRY_DELAY));
        assert_eq!(after_failure(60, 100), AfterFailure::RetryIn(MAX_RETRY_DELAY));
    }

    #[test]
    fn the_last_attempt_gives_up() {
        assert_eq!(after_failure(5, 5), AfterFailure::GiveUp);
        assert_eq!(after_failure(6, 5), AfterFailure::GiveUp);
        assert_eq!(after_failure(1, 1), AfterFailure::GiveUp);
        assert!(matches!(after_failure(4, 5), AfterFailure::RetryIn(_)));
    }
}
