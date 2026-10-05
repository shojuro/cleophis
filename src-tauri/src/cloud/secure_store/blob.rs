//! The parts of Android secure storage that are **not** Android: which file a
//! secret lives in, what AAD binds it there, and the on-disk envelope around
//! the ciphertext.
//!
//! Compiled on every platform and tested on the desktop suite — **decision
//! D-3**, because every failure mode in this file is silent. A path that
//! escapes its directory throws nothing. Two slots that derive the same
//! filename overwrite each other and throw nothing. A `save` and a `load` that
//! derive *different* AAD for the same slot produce a decrypt failure whose
//! symptom is "you keep getting signed out", pointing at the keystore rather
//! than at this arithmetic. None of that is checkable by a cross-compile, and
//! nothing on Android runs until a founder device checkpoint.
//!
//! What stays behind the `cfg` is the JVM call itself, which D-3 exempts.

use std::fmt;

/// Filenames carry this prefix and the reader rejects anything else.
///
/// **The version has exactly one home, and it is here rather than inside the
/// ciphertext.** The obvious alternative — a version byte inside the encrypted
/// envelope, next to the IV — puts the format's identity where only Kotlin can
/// read it, so Rust would have to trust a blob before it could check it, and a
/// future format change would surface as an indistinguishable decrypt failure.
/// Kotlin owns the crypto payload (`iv ‖ ciphertext‖tag`, base64); this file
/// owns the envelope. Putting a version in both would be the D-4 coupling trap
/// with a fresher number.
const MAGIC: &str = "cleophis-secure-v1";

/// A logical secret. Not a filename and not a keyring key — the two derived
/// forms come from [`SlotKey`], together, from one validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot<'a> {
    RefreshToken,
    Verifier { user_id: &'a str },
}

/// The two derived facts a slot needs, produced together so it is impossible
/// to have a filename without the matching AAD label.
///
/// That pairing is the point. If they were separate functions, a caller could
/// validate one and not the other, or `save` could derive the label one way
/// and `load` another — and the resulting decrypt failure looks exactly like a
/// tampered blob, which is the alarm we least want to fire spuriously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlotKey {
    /// File name within the secure directory. Never a path — no separators,
    /// no traversal, guaranteed by construction.
    pub(crate) file_name: String,
    /// GCM additional authenticated data, binding a ciphertext to this slot.
    pub(crate) aad: String,
}

/// Rejected because the `user_id` is not a clean path segment.
///
/// **This exists because the port creates an attack surface the desktop
/// version never had.** On desktop, `verifier_keyring_key` builds an OS
/// keyring *entry name*, where `../` is an ordinary character. On Android the
/// same `user_id` becomes a **filename**. Nothing about the value changed; the
/// thing it is interpolated into did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MalformedUserId;

impl<'a> Slot<'a> {
    /// `None` when a `Verifier`'s `user_id` is not already a clean segment.
    ///
    /// **Reject, don't strip** — the rule the rest of this codebase uses for
    /// exactly this (`kpack.rs`, `convstore.rs`, `store.rs` all carry it).
    /// Reusing `store::account_dir_segment` rather than writing a fourth copy
    /// is deliberate: a sanitizer that disagrees with its siblings is worse
    /// than one that is merely strict, and the alternative considered
    /// (hashing the id into a filename, which cannot traverse and needs no
    /// rejection path) would have given "user_id → path segment" a *second*
    /// rule in a codebase that already has one.
    pub(crate) fn key(&self) -> Result<SlotKey, MalformedUserId> {
        match self {
            Slot::RefreshToken => Ok(SlotKey {
                file_name: "refresh-token.blob".to_string(),
                aad: format!("{MAGIC}/refresh-token"),
            }),
            Slot::Verifier { user_id } => {
                let segment =
                    crate::cloud::store::account_dir_segment(user_id).ok_or(MalformedUserId)?;
                Ok(SlotKey {
                    file_name: format!("verifier-{segment}.blob"),
                    aad: format!("{MAGIC}/verifier/{segment}"),
                })
            }
        }
    }
}

/// Why a stored blob could not be read back.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EnvelopeError {
    /// Not our format at all, or a version this build does not know.
    UnknownFormat,
    /// Right prefix, nothing after it.
    Empty,
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnvelopeError::UnknownFormat => write!(f, "not a {MAGIC} blob"),
            EnvelopeError::Empty => write!(f, "{MAGIC} blob has no payload"),
        }
    }
}

/// Wrap Kotlin's base64 payload in the on-disk envelope.
pub(crate) fn wrap(payload_b64: &str) -> String {
    format!("{MAGIC}\n{payload_b64}")
}

/// Unwrap a stored file back to the payload Kotlin can decrypt.
///
/// A future `v2` reaches `UnknownFormat` here and is reported as "no
/// credential", rather than being handed to a decrypt that would fail its tag
/// check — which would raise a **tamper** alarm for what is actually a format
/// migration. Distinguishing those two is the whole reason the envelope exists.
pub(crate) fn unwrap(contents: &str) -> Result<&str, EnvelopeError> {
    let rest = contents
        .strip_prefix(MAGIC)
        .and_then(|r| r.strip_prefix('\n'))
        .ok_or(EnvelopeError::UnknownFormat)?;
    let payload = rest.trim_end_matches('\n');
    if payload.is_empty() {
        return Err(EnvelopeError::Empty);
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_token_slot_is_a_bare_filename_and_a_distinct_label() {
        let k = Slot::RefreshToken.key().unwrap();
        assert_eq!(k.file_name, "refresh-token.blob");
        assert_eq!(k.aad, "cleophis-secure-v1/refresh-token");
        assert!(!k.file_name.contains('/') && !k.file_name.contains('\\'));
    }

    #[test]
    fn verifier_slot_derives_from_the_user_id() {
        let k = Slot::Verifier { user_id: "uid-A_1" }.key().unwrap();
        assert_eq!(k.file_name, "verifier-uid-A_1.blob");
        assert_eq!(k.aad, "cleophis-secure-v1/verifier/uid-A_1");
    }

    /// THE traversal assertion. The desktop implementation never needed one,
    /// because a keyring entry name is not a path; this is the surface the
    /// Android port creates.
    #[test]
    fn verifier_slot_rejects_traversal_and_separator_ids() {
        for bad in ["../evil", "a/b", "a\\b", "", "..", "a b", "uid:1", "a\0b"] {
            assert_eq!(
                Slot::Verifier { user_id: bad }.key(),
                Err(MalformedUserId),
                "a malformed user_id must never become a filename: {bad:?}"
            );
        }
    }

    /// The property that makes save/load agree. If these ever diverge, every
    /// decrypt fails its tag check and the user is silently signed out on
    /// every launch — with the symptom pointing at the keystore.
    #[test]
    fn slot_keys_are_deterministic() {
        for slot in [Slot::RefreshToken, Slot::Verifier { user_id: "uid-B" }] {
            assert_eq!(slot.key().unwrap(), slot.key().unwrap());
        }
    }

    /// Two accounts must not share a file OR a label. Sharing the file would
    /// overwrite; sharing only the label is the interchangeable-blob case AAD
    /// exists to prevent — B's offline sign-in accepting A's password.
    #[test]
    fn different_accounts_get_different_files_and_different_labels() {
        let a = Slot::Verifier { user_id: "uid-A" }.key().unwrap();
        let b = Slot::Verifier { user_id: "uid-B" }.key().unwrap();
        assert_ne!(a.file_name, b.file_name);
        assert_ne!(a.aad, b.aad);
    }

    /// A verifier slot must never collide with the refresh-token slot, on
    /// either axis, whatever the account is called.
    #[test]
    fn verifier_slots_never_collide_with_the_refresh_token_slot() {
        let rt = Slot::RefreshToken.key().unwrap();
        for uid in ["refresh-token", "refresh", "uid-C"] {
            let v = Slot::Verifier { user_id: uid }.key().unwrap();
            assert_ne!(v.file_name, rt.file_name);
            assert_ne!(v.aad, rt.aad);
        }
    }

    #[test]
    fn envelope_round_trips() {
        let wrapped = wrap("QUJDRA==");
        assert_eq!(unwrap(&wrapped), Ok("QUJDRA=="));
    }

    #[test]
    fn envelope_tolerates_a_trailing_newline() {
        assert_eq!(unwrap("cleophis-secure-v1\nQUJDRA==\n"), Ok("QUJDRA=="));
    }

    /// A future v2 must be reported as an unreadable format, NOT handed to a
    /// decrypt whose tag failure would read as tampering.
    #[test]
    fn envelope_rejects_an_unknown_version() {
        assert_eq!(
            unwrap("cleophis-secure-v2\nQUJDRA=="),
            Err(EnvelopeError::UnknownFormat)
        );
    }

    #[test]
    fn envelope_rejects_garbage_and_emptiness() {
        assert_eq!(unwrap(""), Err(EnvelopeError::UnknownFormat));
        assert_eq!(unwrap("QUJDRA=="), Err(EnvelopeError::UnknownFormat));
        assert_eq!(
            unwrap("cleophis-secure-v1QUJDRA=="),
            Err(EnvelopeError::UnknownFormat)
        );
        assert_eq!(unwrap("cleophis-secure-v1\n"), Err(EnvelopeError::Empty));
        assert_eq!(unwrap("cleophis-secure-v1\n\n"), Err(EnvelopeError::Empty));
    }
}
