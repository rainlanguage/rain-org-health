//! Splits a unified-diff patch's changed lines into CODE, COMMENT and BLANK, so
//! drift since an audit can report comment churn separately and — the point —
//! not call a repo stale for a NatSpec edit. An audit covers the contracts that
//! are there; rewording a comment changes nothing it audited.
//!
//! ## The bias is deliberate
//! Misreading code as a comment would show a genuinely drifted repo as CURRENT —
//! it would hide real, unaudited source change. Misreading a comment as code only
//! reports stale where it already did. So every ambiguity resolves to CODE, and
//! only lines this module can positively prove are comments are counted as such.
//!
//! ## Why block state is tracked per side
//! A hunk interleaves `+` and `-` lines, which belong to two different texts (the
//! new file and the old one). One shared `/* … */` state machine fed by both
//! would corrupt itself the moment a block comment is added next to unrelated
//! deletions, so the added-side and removed-side states are separate; a context
//! line (present in both texts) advances both. State resets at every `@@` hunk
//! header because the lines between hunks are unseen — a hunk that *begins*
//! inside a block comment is unknowable from the patch alone, and those lines
//! fall to CODE by the bias above rather than being guessed.

/// What one changed line is. `Blank` is neither: whitespace churn is not source
/// change, but it is not comment churn either, so it is counted apart from both.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum LineKind {
    Code,
    Comment,
    Blank,
}

/// Added/removed line counts split by kind. `code_*` is the only pair that
/// answers "did the audited source change?".
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct LineDrift {
    pub code_added: u64,
    pub code_removed: u64,
    pub comment_added: u64,
    pub comment_removed: u64,
    pub blank_added: u64,
    pub blank_removed: u64,
}

impl LineDrift {
    /// Did real source change? Comment and whitespace churn do not count.
    pub fn code_changed(&self) -> bool {
        self.code_added > 0 || self.code_removed > 0
    }

    pub fn add(&mut self, other: &LineDrift) {
        self.code_added += other.code_added;
        self.code_removed += other.code_removed;
        self.comment_added += other.comment_added;
        self.comment_removed += other.comment_removed;
        self.blank_added += other.blank_added;
        self.blank_removed += other.blank_removed;
    }
}

/// Classify one source line given whether we are already inside a `/* … */`
/// block we OBSERVED opening, returning the kind and the block state after it.
///
/// A trailing comment (`foo(); // why`) leaves real code on the line, so the
/// line is CODE — only a line that is entirely comment counts as comment.
fn classify(line: &str, in_block: bool) -> (LineKind, bool) {
    let t = line.trim();
    if in_block {
        // Inside a block we saw open: everything is comment until it closes.
        return (LineKind::Comment, !t.contains("*/"));
    }
    if t.is_empty() {
        return (LineKind::Blank, false);
    }
    if t.starts_with("//") {
        return (LineKind::Comment, false);
    }
    if let Some(rest) = t.strip_prefix("/*") {
        // Opens a block. Closed on the same line ⇒ state does not carry.
        return (LineKind::Comment, !rest.contains("*/"));
    }
    // Anything else — including a bare `*`-prefixed line with no opener in this
    // hunk, which could be a wrapped expression — is code by the safe bias.
    (LineKind::Code, false)
}

/// Count a unified-diff patch's `+`/`-` lines by kind.
///
/// `+++`/`---` file headers are not content and are skipped; `@@` resets both
/// block states; context lines advance both sides' state without being counted.
pub fn patch_drift(patch: &str) -> LineDrift {
    let mut d = LineDrift::default();
    let (mut in_add, mut in_del) = (false, false);
    for line in patch.lines() {
        if line.starts_with("@@") {
            in_add = false;
            in_del = false;
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => {
                let (kind, next) = classify(&line[1..], in_add);
                in_add = next;
                match kind {
                    LineKind::Code => d.code_added += 1,
                    LineKind::Comment => d.comment_added += 1,
                    LineKind::Blank => d.blank_added += 1,
                }
            }
            Some(b'-') => {
                let (kind, next) = classify(&line[1..], in_del);
                in_del = next;
                match kind {
                    LineKind::Code => d.code_removed += 1,
                    LineKind::Comment => d.comment_removed += 1,
                    LineKind::Blank => d.blank_removed += 1,
                }
            }
            // Context line: present in BOTH texts, so it advances both states.
            _ => {
                let body = line.strip_prefix(' ').unwrap_or(line);
                let (_, na) = classify(body, in_add);
                let (_, nd) = classify(body, in_del);
                in_add = na;
                in_del = nd;
            }
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_natspec_only_edit_is_all_comment_and_changes_no_code() {
        let patch = "\
@@ -1,4 +1,5 @@
 contract C {
-    /// Old wording.
+    /// New wording.
+    /// @param x The value.
     function f(uint256 x) internal {}
 }";
        let d = patch_drift(patch);
        assert_eq!(d.comment_added, 2);
        assert_eq!(d.comment_removed, 1);
        assert_eq!(d.code_added, 0);
        assert_eq!(d.code_removed, 0);
        assert!(
            !d.code_changed(),
            "a NatSpec edit must not read as code change"
        );
    }

    #[test]
    fn a_trailing_comment_leaves_the_line_as_code() {
        // The line still carries a statement; it is not comment churn.
        let d = patch_drift("@@ -1 +1 @@\n-    a = 1;\n+    a = 2; // bumped\n");
        assert_eq!(d.code_added, 1);
        assert_eq!(d.code_removed, 1);
        assert_eq!(d.comment_added, 0);
        assert!(d.code_changed());
    }

    #[test]
    fn a_block_comment_body_is_comment_once_its_opener_is_seen() {
        let patch = "\
@@ -1,0 +1,4 @@
+    /**
+     * Explains the thing.
+     */
+    uint256 x;";
        let d = patch_drift(patch);
        assert_eq!(
            d.comment_added, 3,
            "opener, body and closer are all comment"
        );
        assert_eq!(d.code_added, 1, "the declaration is code");
        assert!(d.code_changed());
    }

    #[test]
    fn a_single_line_block_comment_does_not_swallow_the_next_line() {
        let d = patch_drift("@@ -0,0 +1,2 @@\n+    /* inline */\n+    selfdestruct(payable(a));\n");
        assert_eq!(d.comment_added, 1);
        assert_eq!(d.code_added, 1, "the closed block must not leak state");
        assert!(d.code_changed());
    }

    /// The safe bias: with no opener in the hunk, a `*`-prefixed line could be a
    /// wrapped expression, so it must NOT be assumed to be a comment body.
    #[test]
    fn a_star_line_with_no_observed_opener_is_code() {
        let d = patch_drift("@@ -1 +1 @@\n+    * 2;\n");
        assert_eq!(d.code_added, 1);
        assert_eq!(d.comment_added, 0);
        assert!(
            d.code_changed(),
            "ambiguity must resolve toward stale, not current"
        );
    }

    /// A `+` and a `-` line interleaved belong to two different texts; one shared
    /// block state would mark the deletion as comment.
    #[test]
    fn added_and_removed_sides_track_block_state_separately() {
        let patch = "\
@@ -1,2 +1,2 @@
+    /* opens on the added side
+    still comment */
-    uint256 realCode = 1;";
        let d = patch_drift(patch);
        assert_eq!(d.comment_added, 2);
        assert_eq!(
            d.code_removed, 1,
            "the deletion is code, not swallowed by the added block"
        );
        assert!(d.code_changed());
    }

    #[test]
    fn blank_lines_are_neither_code_nor_comment() {
        let d = patch_drift("@@ -1 +1,2 @@\n+\n+   \n-\n");
        assert_eq!(d.blank_added, 2);
        assert_eq!(d.blank_removed, 1);
        assert_eq!(d.code_added, 0);
        assert_eq!(d.comment_added, 0);
        assert!(!d.code_changed(), "whitespace churn is not source change");
    }

    #[test]
    fn file_headers_are_not_counted_as_content() {
        let patch = "--- a/src/A.sol\n+++ b/src/A.sol\n@@ -1 +1 @@\n-    a = 1;\n+    a = 2;\n";
        let d = patch_drift(patch);
        assert_eq!(d.code_added, 1);
        assert_eq!(d.code_removed, 1);
    }

    #[test]
    fn a_hunk_header_resets_block_state() {
        // An unterminated block in one hunk must not bleed into the next.
        let patch = "@@ -1 +1 @@\n+    /* unterminated\n@@ -9 +9 @@\n+    uint256 y;\n";
        let d = patch_drift(patch);
        assert_eq!(d.comment_added, 1);
        assert_eq!(d.code_added, 1, "state must not carry across hunks");
    }

    #[test]
    fn an_empty_patch_yields_no_drift() {
        let d = patch_drift("");
        assert_eq!(d, LineDrift::default());
        assert!(!d.code_changed());
    }
}
