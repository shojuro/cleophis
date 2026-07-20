//! zxcvbn password-strength gate for offline-auth enrollment. This is the
//! authoritative guard: Task 4 calls `check_strength` before deriving an
//! Argon2id verifier (verifier.rs), and the same function backs the
//! `check_password_strength` command so the FE can render a live meter.
//! Offline sign-in has no server round trip to rate-limit guesses against,
//! so a weak password here becomes brute-forceable against the locally
//! stored verifier — `ok` is deliberately strict on both length and score.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StrengthResult {
    pub score: u8,
    pub ok: bool,
    pub feedback: Vec<String>,
}

/// `ok` requires BOTH a minimum length and a minimum zxcvbn score — length
/// alone doesn't stop low-entropy-but-long passwords, and score alone
/// doesn't stop a short password zxcvbn happens to misjudge. `user_inputs`
/// (email, nickname) lets zxcvbn penalize passwords built from the
/// account's own identifying details.
pub fn check_strength(password: &str, user_inputs: &[&str]) -> StrengthResult {
    let estimate = zxcvbn::zxcvbn(password, user_inputs);
    let score = u8::from(estimate.score());
    let ok = password.chars().count() >= 12 && score >= 3;

    let mut feedback = Vec::new();
    if let Some(fb) = estimate.feedback() {
        if let Some(warning) = fb.warning() {
            feedback.push(warning.to_string());
        }
        feedback.extend(fb.suggestions().iter().map(|s| s.to_string()));
    }

    StrengthResult { score, ok, feedback }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_passphrase_passes() {
        let r = check_strength("wintergreen-diesel-canyon-42", &[]);
        assert!(r.ok, "score {} len ok expected pass", r.score);
    }
    #[test]
    fn short_fails_even_if_complex() {
        let r = check_strength("aB3$xY", &[]); // < 12 chars
        assert!(!r.ok);
    }
    #[test]
    fn common_complex_password_fails_on_score() {
        let r = check_strength("Password123!", &[]); // 12 chars but weak
        assert!(!r.ok, "zxcvbn should score this < 3");
    }
    // Brief's original example ("alice-smith-1988" against
    // ["alice", "alice@x.com"]) scores 4/ok even with user_inputs supplied —
    // this crate's built-in surname/date dictionaries are smaller than the
    // reference JS zxcvbn's, so "smith"/"1988" aren't recognized and the
    // combined guesses estimate stays high. "alice@x.com123" verifiably
    // isolates the effect instead: without user_inputs it scores 4/ok,
    // with them (the literal email as a dictionary entry) it drops to 1 —
    // proving `user_inputs` is actually reaching zxcvbn::zxcvbn.
    #[test]
    fn user_inputs_penalized() {
        let r = check_strength("alice@x.com123", &["alice", "alice@x.com"]);
        assert!(!r.ok, "score {} feedback {:?}", r.score, r.feedback);
    }
}
