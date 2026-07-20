//! Batched blob fetching.
//!
//! Splitting drift into code vs comment needs both versions of every changed
//! source file, which is two blobs per file. Fetched one-by-one over the REST
//! contents API that is a request per blob; a repo with 63 changed files since
//! its audit anchor costs 126 round trips. GitHub's GraphQL API can alias many
//! `object(expression: "<ref>:<path>")` lookups into one document, so the same
//! work becomes a handful of requests.
//!
//! The query building and response parsing live here as pure functions over
//! strings; `main.rs` owns the `gh` invocation.

use std::collections::BTreeMap;

/// One blob to fetch: the ref to read it at, and the path within the repo.
pub type Want = (String, String);

/// How many blobs to request per query. GraphQL bills by node count and the
/// response carries whole file bodies, so this trades round trips against
/// response size rather than maximising either.
pub const BATCH: usize = 40;

/// Escape a Rust string for embedding in a GraphQL double-quoted string.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// The alias naming one want within its batch. GraphQL aliases must match
/// `[_A-Za-z][_0-9A-Za-z]*`, so an index — not the path — is the identifier.
fn alias(i: usize) -> String {
    format!("b{i}")
}

/// Build one GraphQL document fetching every want in a batch from one repo.
///
/// `isTruncated` is selected alongside `text` because GitHub silently caps blob
/// text: a truncated body would otherwise parse as a shorter-but-valid file and
/// read as spurious deletions.
pub fn blob_query(org: &str, repo: &str, wants: &[Want]) -> String {
    let mut q = format!(
        "query{{repository(owner:\"{}\",name:\"{}\"){{",
        escape(org),
        escape(repo)
    );
    for (i, (git_ref, path)) in wants.iter().enumerate() {
        q.push_str(&format!(
            "{}:object(expression:\"{}:{}\"){{...on Blob{{text isTruncated}}}}",
            alias(i),
            escape(git_ref),
            escape(path)
        ));
    }
    q.push_str("}}");
    q
}

/// Map a batch's response back onto its wants.
///
/// A want is absent from the result when the path does not exist at that ref,
/// when the object is not a text blob, or when GitHub truncated it. Absence
/// means "unknown", which callers charge to code — so no failure mode here can
/// present as "this file did not change".
pub fn parse_blob_response(json: &str, wants: &[Want]) -> BTreeMap<Want, String> {
    let mut out = BTreeMap::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return out;
    };
    let repo = &v["data"]["repository"];
    for (i, want) in wants.iter().enumerate() {
        let node = &repo[alias(i)];
        if node["isTruncated"].as_bool() == Some(true) {
            continue;
        }
        if let Some(text) = node["text"].as_str() {
            out.insert(want.clone(), text.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(r: &str, p: &str) -> Want {
        (r.to_string(), p.to_string())
    }

    #[test]
    fn the_query_aliases_every_want_under_one_repository() {
        let q = blob_query(
            "rainlanguage",
            "rain.math.binary",
            &[w("abc", "src/A.sol"), w("def", "src/B.sol")],
        );
        assert!(q.contains("repository(owner:\"rainlanguage\",name:\"rain.math.binary\")"));
        assert!(q.contains("b0:object(expression:\"abc:src/A.sol\")"));
        assert!(q.contains("b1:object(expression:\"def:src/B.sol\")"));
        // One repository block, so one request covers both blobs.
        assert_eq!(q.matches("repository(").count(), 1);
    }

    #[test]
    fn the_query_selects_is_truncated_so_a_capped_body_is_detectable() {
        let q = blob_query("o", "r", &[w("sha", "src/A.sol")]);
        assert!(q.contains("...on Blob{text isTruncated}"));
    }

    #[test]
    fn a_quote_in_a_path_cannot_break_out_of_the_query_string() {
        let q = blob_query("o", "r", &[w("sha", "src/we\"ird.sol")]);
        assert!(q.contains("src/we\\\"ird.sol"));
        // The expression string is still exactly one opened-and-closed literal.
        assert!(q.contains("expression:\"sha:src/we\\\"ird.sol\""));
    }

    #[test]
    fn a_response_maps_each_alias_back_to_its_own_want() {
        let wants = vec![w("abc", "src/A.sol"), w("def", "src/B.sol")];
        let got = parse_blob_response(
            r#"{"data":{"repository":{
                 "b0":{"text":"contract A {}","isTruncated":false},
                 "b1":{"text":"contract B {}","isTruncated":false}}}}"#,
            &wants,
        );
        assert_eq!(
            got.get(&wants[0]).map(String::as_str),
            Some("contract A {}")
        );
        assert_eq!(
            got.get(&wants[1]).map(String::as_str),
            Some("contract B {}")
        );
    }

    #[test]
    fn a_path_absent_at_that_ref_is_absent_from_the_map_not_empty() {
        let wants = vec![w("abc", "src/Gone.sol")];
        let got = parse_blob_response(r#"{"data":{"repository":{"b0":null}}}"#, &wants);
        assert!(
            !got.contains_key(&wants[0]),
            "a missing blob must read as unknown, never as an empty file"
        );
    }

    #[test]
    fn a_truncated_blob_is_dropped_rather_than_read_as_a_shorter_file() {
        let wants = vec![w("abc", "src/Huge.sol")];
        let got = parse_blob_response(
            r#"{"data":{"repository":{"b0":{"text":"contract H {","isTruncated":true}}}}"#,
            &wants,
        );
        assert!(
            !got.contains_key(&wants[0]),
            "a capped body would otherwise diff as mass deletions"
        );
    }

    #[test]
    fn a_malformed_or_errored_response_yields_no_blobs_rather_than_panicking() {
        let wants = vec![w("abc", "src/A.sol")];
        assert!(parse_blob_response("not json", &wants).is_empty());
        assert!(
            parse_blob_response(r#"{"errors":[{"message":"rate limited"}]}"#, &wants).is_empty()
        );
    }
}
