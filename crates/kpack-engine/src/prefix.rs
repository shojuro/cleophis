//! How much of a prompt is already in the KV cache — the one arithmetic
//! decision behind prefix reuse (task 1.5).
//!
//! It lives here, outside the `real` feature gate, because the `llama` module
//! is only compiled by a build that links llama.cpp and is therefore only
//! *checked* by the Android cross-compile, where nothing runs. This function
//! decides how many tokens are skipped, and getting it wrong by one is not a
//! crash: reuse one token too many and the sampler reads the previous turn's
//! logits, which produces a plausible answer to a question nobody asked. That
//! failure has to be tested, and the desktop suite is where tests run.

/// How many leading tokens of `prompt` may be served from a cache currently
/// holding `cached`.
///
/// The shared prefix, capped at `prompt.len() - 1` so that **at least one token
/// is always decoded**. That cap is invariant 2 of the reuse scheme and it is
/// not an optimisation detail: a fully-reused prompt never calls `decode`, so
/// the logits the sampler reads are still the ones the *previous* turn left
/// behind, and the turn answers the wrong question in fluent prose.
///
/// Generic over `T` only so it can be tested without llama.cpp's token type;
/// the sole production caller passes `LlamaToken`.
pub(crate) fn reusable_prefix<T: PartialEq>(cached: &[T], prompt: &[T]) -> usize {
    let shared = cached
        .iter()
        .zip(prompt)
        .take_while(|(a, b)| a == b)
        .count();
    shared.min(prompt.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_cache_reuses_nothing() {
        assert_eq!(reusable_prefix::<u32>(&[], &[1, 2, 3]), 0);
    }

    #[test]
    fn a_repeated_prompt_still_decodes_its_last_token() {
        // The invariant-2 case, and the one that fails silently: everything
        // matches, yet one token must still be decoded so the sampler reads
        // logits belonging to THIS turn.
        assert_eq!(reusable_prefix(&[1, 2, 3], &[1, 2, 3]), 2);
    }

    #[test]
    fn a_continued_conversation_reuses_the_whole_shared_prefix() {
        // The ordinary case: last turn's prompt plus what it generated, and
        // this turn extends it.
        assert_eq!(reusable_prefix(&[1, 2, 3], &[1, 2, 3, 4, 5]), 3);
    }

    #[test]
    fn divergence_caps_the_reuse_at_the_first_difference() {
        assert_eq!(reusable_prefix(&[1, 2, 9, 9], &[1, 2, 3, 4]), 2);
        // Including at the very first token — a different conversation, or a
        // re-tokenisation that changed the opening.
        assert_eq!(reusable_prefix(&[9, 2, 3], &[1, 2, 3]), 0);
    }

    #[test]
    fn a_cache_longer_than_the_prompt_is_bounded_by_the_prompt() {
        // The trim case: the previous turn generated past where this prompt
        // ends. The excess is stale and the caller clears it; what matters here
        // is that the count never exceeds the prompt.
        assert_eq!(reusable_prefix(&[1, 2, 3, 4, 5], &[1, 2, 3]), 2);
    }

    #[test]
    fn a_single_token_prompt_reuses_nothing() {
        assert_eq!(reusable_prefix(&[1], &[1]), 0);
    }

    #[test]
    fn an_empty_prompt_does_not_underflow() {
        // `prompt.len() - 1` on an empty prompt is the obvious panic, and a
        // template that renders to nothing is a model-shaped input, not an
        // impossible one.
        assert_eq!(reusable_prefix(&[1, 2], &[]), 0);
        assert_eq!(reusable_prefix::<u32>(&[], &[]), 0);
    }
}
