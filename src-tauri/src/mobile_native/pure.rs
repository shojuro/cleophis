//! The pure half of the share path, compiled on **every** platform so its
//! tests run in the desktop suite (**decision D-3**).
//!
//! This module is the correction of a mistake made while writing this file:
//! `safe_file_stem` was first placed inside the `cfg(target_os = "android")`
//! implementation, with its tests beside it — where **they would never have
//! executed**, on this branch or any other, because nothing on Android runs
//! until a founder device checkpoint. It is a sanitiser whose failure mode is
//! a path separator surviving into a filename: silent, security-relevant, and
//! invisible to a cross-compile `check`. That is D-3's trigger exactly, and
//! the first draft put it on the wrong side of the line.
/// Extension and MIME per export format. `text/markdown` over
/// `text/plain` is deliberate: a share target that understands Markdown
/// should get the chance to, and every target that does not falls back to
/// plain text anyway.
pub(crate) fn ext_and_mime(format: &str) -> Option<(&'static str, &'static str)> {
    match format {
        "markdown" => Some(("md", "text/markdown")),
        "json" => Some(("json", "application/json")),
        "txt" => Some(("txt", "text/plain")),
        _ => None,
    }
}

/// A user-supplied chat title, reduced to one safe filename component.
///
/// **Deliberately sanitise-and-keep rather than the "reject, don't strip"
/// rule used for `user_id`.** The rules differ because the inputs differ:
/// a `user_id` is machine-issued, so anything malformed is a bug or an
/// attack and rejecting it costs a legitimate user nothing. A chat title
/// is *typed by a human*, and a chat honestly named "3/4 of what?" is not
/// an attack — refusing to share it would be a defect. So every character
/// outside `[A-Za-z0-9 _-]` becomes `_`, and a title that survives as
/// nothing falls back to a constant.
///
/// The value cannot escape its directory regardless: separators are
/// replaced here, the result is a single component joined onto a directory
/// we choose, and the length is capped so a pathological title cannot
/// produce a name the filesystem rejects.
///
/// **The fallback condition is "carries no information", not "is empty".**
/// The first draft tested `is_empty()` after a `.trim_matches('.')`, and
/// running the tests locally showed both halves were wrong: the trim was
/// **dead code** (dots have already become `_` by then, so it could never
/// fire), and a title of `"..."` sanitised to `"___"` — non-empty, perfectly
/// safe, and a useless filename. Requiring at least one alphanumeric covers
/// `""`, `"   "`, `"..."` and `"///"` with one rule and no unreachable
/// defence.
pub(crate) fn safe_file_stem(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let capped: String = cleaned.chars().take(60).collect();
    let trimmed = capped.trim();
    if !trimmed.chars().any(|c| c.is_ascii_alphanumeric()) {
        "chat".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_titles_survive_readably() {
        assert_eq!(safe_file_stem("Algebra help"), "Algebra help");
        assert_eq!(safe_file_stem("chapter-3_notes"), "chapter-3_notes");
    }

    /// THE assertion this module exists for: nothing that could act as a
    /// path separator or traversal survives into the filename.
    #[test]
    fn no_separator_or_traversal_survives() {
        for (input, why) in [
            ("../../etc/passwd", "traversal"),
            ("a/b", "forward slash"),
            ("a\\b", "backslash"),
            ("..", "bare dots"),
            ("./x", "leading dot-slash"),
            ("a\0b", "NUL"),
            ("a:b", "colon"),
        ] {
            let out = safe_file_stem(input);
            assert!(
                !out.contains('/') && !out.contains('\\') && !out.contains('\0'),
                "{why}: separator survived in {out:?}"
            );
            assert!(!out.contains(".."), "{why}: traversal survived in {out:?}");
            assert!(!out.is_empty(), "{why}: produced an empty name");
        }
    }

    /// A title that carries no information must still yield a usable name —
    /// otherwise the share writes to `.md`, or to `___.md`.
    ///
    /// This test failed on the first implementation and the CODE was wrong,
    /// not the expectation: `"..."` sanitised to `"___"`, which is non-empty
    /// and safe but tells the user nothing about what they just shared. It
    /// also exposed a `trim_matches('.')` that could never fire, since the
    /// character map has already replaced every dot by that point.
    #[test]
    fn titles_that_carry_no_information_fall_back() {
        assert_eq!(safe_file_stem(""), "chat");
        assert_eq!(safe_file_stem("   "), "chat");
        assert_eq!(safe_file_stem("..."), "chat");
        assert_eq!(safe_file_stem("///"), "chat");
        assert_eq!(safe_file_stem("!@#$%"), "chat");
        // ...but anything with real content is kept, junk and all.
        assert_eq!(safe_file_stem("__a__"), "__a__");
    }

    /// Capped before trimming, so a 500-character title cannot produce a
    /// name the filesystem rejects — and the cap cannot re-introduce a
    /// trailing space.
    #[test]
    fn long_titles_are_capped_and_still_trimmed() {
        let long = "x".repeat(500);
        assert_eq!(safe_file_stem(&long).chars().count(), 60);

        let padded = format!("{}{}", "y".repeat(59), "   tail");
        let out = safe_file_stem(&padded);
        assert!(out.chars().count() <= 60);
        assert_eq!(out, out.trim());
    }

    #[test]
    fn formats_map_to_their_extension_and_mime() {
        assert_eq!(ext_and_mime("markdown"), Some(("md", "text/markdown")));
        assert_eq!(ext_and_mime("json"), Some(("json", "application/json")));
        assert_eq!(ext_and_mime("txt"), Some(("txt", "text/plain")));
    }

    /// An unknown format must be refused, not defaulted — the desktop
    /// `export_chat_to_file` refuses too, and the two paths should not
    /// disagree about what a valid format is.
    #[test]
    fn unknown_formats_are_refused() {
        for bad in ["", "pdf", "MARKDOWN", "md", "text"] {
            assert_eq!(ext_and_mime(bad), None, "must refuse {bad:?}");
        }
    }
}
