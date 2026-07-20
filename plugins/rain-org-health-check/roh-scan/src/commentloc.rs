//! Splits drift since an audit into CODE and COMMENT churn, so rewording NatSpec
//! does not mark a repo stale. An audit covers the contracts that are there; a
//! comment edit changes none of them.
//!
//! ## Why this lexes whole files rather than diff hunks
//! The obvious implementation — classify the `+`/`-` lines of a unified diff —
//! cannot work, and not because of sloppy matching. A hunk carries three lines of
//! context, so editing the middle of a long `/** … */` block never shows the
//! opener; the information simply is not in the fragment. Every such edit would
//! fall back to "code" and keep the repo stale — precisely the case this exists to
//! fix. So both versions of each file are lexed in full, where the block state is
//! unambiguous.
//!
//! Lexing is [`solang_parser`], the same lexer `forge fmt` uses, rather than a
//! hand-rolled scanner: it already knows that `//` inside a string literal is not
//! a comment, that `/* */` nests with strings and hex literals, and so on.
//!
//! ## Degradation is one-directional
//! Where a file cannot be fetched or lexed, its churn counts as CODE. Misreading
//! code as comment would show a genuinely drifted repo as CURRENT and hide
//! unaudited source change; misreading a comment as code only reports stale where
//! it already did. Callers are told (`fully_classified`) so the split is never
//! presented as authoritative when it is not.

use similar::{ChangeTag, TextDiff};
use solang_parser::lexer::Lexer;
use solang_parser::pt::Comment;

/// Added/removed line counts split by kind. `code_*` is the only pair that
/// answers "did the audited source change?".
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct LineDrift {
    pub code_added: u64,
    pub code_removed: u64,
    pub comment_added: u64,
    pub comment_removed: u64,
}

impl LineDrift {
    /// Did real source change? Comment churn does not count.
    pub fn code_changed(&self) -> bool {
        self.code_added > 0 || self.code_removed > 0
    }

    pub fn add(&mut self, other: &LineDrift) {
        self.code_added += other.code_added;
        self.code_removed += other.code_removed;
        self.comment_added += other.comment_added;
        self.comment_removed += other.comment_removed;
    }
}

/// The byte range each comment occupies, via the Solidity lexer.
/// `None` if the source could not be lexed at all.
fn comment_spans(src: &str) -> Option<Vec<(usize, usize)>> {
    let mut comments: Vec<Comment> = Vec::new();
    let mut errors = Vec::new();
    // The lexer streams tokens and collects comments as it goes; token errors are
    // irrelevant here because comments are lexical, not syntactic — a file that
    // does not PARSE still yields correct comment spans.
    let mut lexer = Lexer::new(src, 0, &mut comments, &mut errors);
    for _ in lexer.by_ref() {}
    Some(
        comments
            .iter()
            .map(|c| match c {
                Comment::Line(loc, _)
                | Comment::Block(loc, _)
                | Comment::DocLine(loc, _)
                | Comment::DocBlock(loc, _) => (loc.start(), loc.end()),
            })
            .collect(),
    )
}

/// Split a source file into (code-only, comments-only) line vectors.
///
/// Comment bytes are blanked rather than deleted so the code side keeps its line
/// structure; a line left with only whitespace after blanking was comment-only
/// and is dropped from the code side. That way a pure comment edit produces no
/// code-side change at all, while `a = 1; // note` keeps its statement.
fn split_source(src: &str) -> Option<(Vec<String>, Vec<String>)> {
    let spans = comment_spans(src)?;
    let bytes = src.as_bytes();
    let mut code = vec![b' '; bytes.len()];
    let mut is_comment = vec![false; bytes.len()];
    code.copy_from_slice(bytes);
    for (s, e) in spans {
        for i in s..e.min(bytes.len()) {
            // Keep newlines so line numbering survives on both sides.
            if bytes[i] != b'\n' {
                code[i] = b' ';
            }
            is_comment[i] = true;
        }
    }
    let code_src = String::from_utf8_lossy(&code).into_owned();

    let mut code_lines = Vec::new();
    for line in code_src.lines() {
        if !line.trim().is_empty() {
            // Normalise whitespace so reflowing code without changing it does not
            // read as drift; the comparison is about substance, not layout.
            code_lines.push(line.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }

    // The comment side: each source line's comment bytes, in order.
    let mut comment_lines = Vec::new();
    let mut offset = 0usize;
    for line in src.split_inclusive('\n') {
        let end = offset + line.len();
        let taken: String = (offset..end)
            .filter(|&i| is_comment[i] && bytes[i] != b'\n')
            .map(|i| bytes[i] as char)
            .collect();
        if !taken.trim().is_empty() {
            comment_lines.push(taken.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        offset = end;
    }
    Some((code_lines, comment_lines))
}

/// Count added/removed lines between two line vectors.
fn diff_counts(before: &[String], after: &[String]) -> (u64, u64) {
    let b = before.join("\n");
    let a = after.join("\n");
    let diff = TextDiff::from_lines(&b, &a);
    let (mut added, mut removed) = (0u64, 0u64);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

/// Drift between two versions of ONE Solidity file, split into code and comment.
/// `None` when either side could not be lexed — the caller must then treat the
/// file's churn as code rather than assuming it was comment-only.
pub fn file_drift(base: &str, head: &str) -> Option<LineDrift> {
    let (base_code, base_comments) = split_source(base)?;
    let (head_code, head_comments) = split_source(head)?;
    let (code_added, code_removed) = diff_counts(&base_code, &head_code);
    let (comment_added, comment_removed) = diff_counts(&base_comments, &head_comments);
    Some(LineDrift {
        code_added,
        code_removed,
        comment_added,
        comment_removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The case the hunk-based classifier could not do: an edit in the MIDDLE of a
    /// long `/** … */` block, whose opener a diff hunk would never show.
    #[test]
    fn editing_the_middle_of_a_long_natspec_block_is_comment_only() {
        let base = "\
contract C {
    /**
     * Some earlier documentation line.
     * Another earlier line.
     * Yet another line of prose.
     * The old wording of this sentence.
     * Trailing prose line.
     */
    function f() internal {}
}";
        let head = base.replace("The old wording of this sentence.", "The new wording here.");
        let d = file_drift(base, &head).expect("lexes");
        assert!(
            !d.code_changed(),
            "a NatSpec edit must not read as code change, got {d:?}"
        );
        assert!(d.comment_added > 0 && d.comment_removed > 0);
    }

    #[test]
    fn a_line_natspec_edit_is_comment_only() {
        let base = "contract C {\n    /// Old wording.\n    uint256 x;\n}";
        let head =
            "contract C {\n    /// New wording.\n    /// @param x A value.\n    uint256 x;\n}";
        let d = file_drift(base, head).expect("lexes");
        assert!(!d.code_changed());
        assert!(d.comment_added >= 1);
    }

    #[test]
    fn a_real_code_edit_is_code() {
        let base = "contract C {\n    uint256 x = 1;\n}";
        let head = "contract C {\n    uint256 x = 2;\n}";
        let d = file_drift(base, head).expect("lexes");
        assert!(d.code_changed());
        assert_eq!(d.comment_added, 0);
        assert_eq!(d.comment_removed, 0);
    }

    /// A trailing comment change leaves the statement intact: comment churn only.
    #[test]
    fn changing_only_a_trailing_comment_is_not_code_change() {
        let base = "contract C {\n    uint256 x = 1; // old note\n}";
        let head = "contract C {\n    uint256 x = 1; // new note\n}";
        let d = file_drift(base, head).expect("lexes");
        assert!(!d.code_changed(), "the statement is unchanged, got {d:?}");
        assert!(d.comment_added > 0);
    }

    /// …but changing the statement on such a line IS code change.
    #[test]
    fn changing_the_statement_beside_a_comment_is_code_change() {
        let base = "contract C {\n    uint256 x = 1; // note\n}";
        let head = "contract C {\n    uint256 x = 2; // note\n}";
        let d = file_drift(base, head).expect("lexes");
        assert!(d.code_changed());
        assert_eq!(d.comment_added, 0, "the comment itself did not change");
    }

    /// The payoff of using a real lexer: `//` inside a string is NOT a comment,
    /// so changing it is code change. A line-prefix heuristic gets this right by
    /// luck, but a URL-only line would fool a naive "contains //" rule.
    #[test]
    fn a_double_slash_inside_a_string_is_code_not_comment() {
        let base = "contract C {\n    string u = \"https://old.example\";\n}";
        let head = "contract C {\n    string u = \"https://new.example\";\n}";
        let d = file_drift(base, head).expect("lexes");
        assert!(d.code_changed(), "string content is code, got {d:?}");
        assert_eq!(d.comment_added, 0);
        assert_eq!(d.comment_removed, 0);
    }

    /// Reindenting without changing substance is not drift.
    #[test]
    fn pure_reformatting_is_not_code_change() {
        let base = "contract C {\n    uint256   x =  1;\n}";
        let head = "contract C {\n        uint256 x = 1;\n}";
        let d = file_drift(base, head).expect("lexes");
        assert!(!d.code_changed(), "whitespace is not substance, got {d:?}");
    }

    #[test]
    fn adding_code_and_comments_together_counts_both_apart() {
        let base = "contract C {\n    uint256 x;\n}";
        let head = "contract C {\n    /// Doc for y.\n    uint256 x;\n    uint256 y;\n}";
        let d = file_drift(base, head).expect("lexes");
        assert!(d.code_changed());
        assert!(d.comment_added > 0);
    }

    /// Comments are lexical: a file that does not PARSE still yields spans, so a
    /// fragment or a syntactically broken file is still classified.
    #[test]
    fn unparseable_source_still_classifies_comments() {
        let base = "    /// doc\n    function f( {";
        let head = "    /// doc changed\n    function f( {";
        let d = file_drift(base, head).expect("lexes despite parse errors");
        assert!(!d.code_changed());
        assert!(d.comment_added > 0);
    }

    #[test]
    fn identical_sources_have_no_drift() {
        let src = "contract C {\n    /// doc\n    uint256 x = 1;\n}";
        assert_eq!(file_drift(src, src).unwrap(), LineDrift::default());
    }
}
