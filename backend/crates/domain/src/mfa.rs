//! Rules of two-step verification (specification §14): TOTP codes from an
//! authenticator app (RFC 6238: HMAC-SHA1, 6 digits, 30-second steps) and
//! single-use backup codes. The HMAC itself is computed in `infra::mfa`.

use std::time::Duration;

/// Length of a TOTP time step, in seconds.
pub const TOTP_STEP_SECS: u64 = 30;

/// Digits of a TOTP code.
pub const TOTP_DIGITS: u8 = 6;

/// Steps accepted before and after the current one, for clock drift: a code
/// stays valid about 30 seconds on each side.
pub const TOTP_WINDOW_STEPS: u64 = 1;

/// Backup codes given when two-step verification is enabled.
pub const BACKUP_CODE_COUNT: usize = 8;

/// Characters of a backup code, without look-alikes (0/o, 1/i/l).
pub const BACKUP_CODE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Characters of a backup code (about 50 bits of randomness).
pub const BACKUP_CODE_CHARS: usize = 10;

/// Failed codes on one account before verification is refused for
/// [`crate::account::SIGN_IN_WINDOW`].
pub const MAX_FAILED_CODES: u32 = 5;

/// Time to enter the code after the password.
pub const CHALLENGE_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// Time to enroll when the tenant requires it, after the password.
pub const ENROLLMENT_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// How far a session has gone (stored with it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStage {
    /// Fully signed in.
    Complete,
    /// Password checked; the second factor is expected.
    MfaChallenge,
    /// Password checked; the tenant requires two-step verification, which
    /// the member must enable before anything else.
    MfaEnrollment,
}

impl SessionStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::MfaChallenge => "mfa_challenge",
            Self::MfaEnrollment => "mfa_enrollment",
        }
    }
}

/// The stage of a session opened by a correct password.
pub fn stage_after_password(mfa_enabled: bool, policy_requires_mfa: bool) -> SessionStage {
    if mfa_enabled {
        SessionStage::MfaChallenge
    } else if policy_requires_mfa {
        SessionStage::MfaEnrollment
    } else {
        SessionStage::Complete
    }
}

/// Time step of `unix_secs`.
pub fn time_step(unix_secs: u64) -> u64 {
    unix_secs / TOTP_STEP_SECS
}

/// Steps whose code is accepted at `now_step`: the current one and its
/// neighbours, but none at or before the last accepted one (no replay).
pub fn candidate_steps(now_step: u64, last_used_step: Option<u64>) -> Vec<u64> {
    let first = now_step.saturating_sub(TOTP_WINDOW_STEPS);
    let last = now_step.saturating_add(TOTP_WINDOW_STEPS);
    (first..=last)
        .filter(|step| last_used_step.is_none_or(|used| *step > used))
        .collect()
}

/// What a member typed as second factor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Code {
    /// 6 digits from the authenticator app.
    Totp(String),
    /// A backup code, normalized (lower case, no separator).
    Backup(String),
}

/// Reads a code: 6 digits (spaces allowed) are a TOTP code; 10 characters of
/// the backup alphabet (any case, spaces and dashes allowed) a backup code.
pub fn parse_code(raw: &str) -> Option<Code> {
    let compact: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    if compact.len() == usize::from(TOTP_DIGITS) && compact.bytes().all(|b| b.is_ascii_digit()) {
        return Some(Code::Totp(compact));
    }
    if compact.len() == BACKUP_CODE_CHARS && compact.bytes().all(|b| BACKUP_CODE_ALPHABET.contains(&b)) {
        return Some(Code::Backup(compact));
    }
    None
}

/// A backup code as shown to the member: `abcde-fghij`.
pub fn display_backup_code(code: &str) -> String {
    let (head, tail) = code.split_at(code.len() / 2);
    format!("{head}-{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_stage_follows_the_account_and_the_policy() {
        assert_eq!(stage_after_password(false, false), SessionStage::Complete);
        assert_eq!(stage_after_password(true, false), SessionStage::MfaChallenge);
        assert_eq!(stage_after_password(true, true), SessionStage::MfaChallenge);
        assert_eq!(stage_after_password(false, true), SessionStage::MfaEnrollment);
    }

    #[test]
    fn codes_are_accepted_one_step_before_and_after_the_current_one() {
        let now = time_step(1_700_000_015);
        assert_eq!(now, 56_666_667);
        assert_eq!(candidate_steps(now, None), [now - 1, now, now + 1]);
        // 29 seconds later the same window still holds the previous code.
        assert!(candidate_steps(time_step(1_700_000_044), None).contains(&now));
        // Two steps away, no more.
        assert!(!candidate_steps(now + 2, None).contains(&now));
        assert_eq!(candidate_steps(0, None), [0, 1]);
    }

    #[test]
    fn a_code_is_never_accepted_twice() {
        let now = 1000;
        assert_eq!(candidate_steps(now, Some(999)), [1000, 1001]);
        assert_eq!(candidate_steps(now, Some(1000)), [1001]);
        assert!(candidate_steps(now, Some(1001)).is_empty());
    }

    #[test]
    fn totp_and_backup_codes_are_told_apart() {
        assert_eq!(parse_code(" 482 913 "), Some(Code::Totp("482913".to_owned())));
        assert_eq!(
            parse_code("ABCDE-FGHJK"),
            Some(Code::Backup("abcdefghjk".to_owned()))
        );
        assert_eq!(
            parse_code("abcde fghjk"),
            Some(Code::Backup("abcdefghjk".to_owned()))
        );
        for bad in ["", "12345", "1234567", "abcde-fghij", "abcdefghjk1", "48291a"] {
            assert_eq!(parse_code(bad), None, "{bad}");
        }
    }

    #[test]
    fn backup_codes_are_shown_in_two_halves() {
        assert_eq!(display_backup_code("abcdefghjk"), "abcde-fghjk");
        let shown = display_backup_code("abcdefghjk");
        assert_eq!(parse_code(&shown), Some(Code::Backup("abcdefghjk".to_owned())));
    }
}
