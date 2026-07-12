use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::item::Item;
use crate::{Error, Result};

/// Core logic: detects "merge request" comments — a comment whose body
/// contains BOTH a mention of the viewer AND a merge keyword.
/// GitHub search cannot express this same-comment constraint.
pub struct CommentFilter {
    mention: Regex,
    keyword: Regex,
    extra: Vec<Regex>,
}

impl CommentFilter {
    pub fn new(viewer_login: &str, extra_patterns: &[String]) -> Result<Self> {
        let mention = Regex::new(&format!(
            r"(?i)@{}(?:[^\w-]|$)",
            regex::escape(viewer_login)
        ))
        .map_err(|e| Error::Config(format!("bad viewer login: {e}")))?;
        let keyword = Regex::new(r"(?i)(merge|マージ)").expect("static regex");
        let extra = extra_patterns
            .iter()
            .map(|p| {
                Regex::new(p)
                    .map_err(|e| Error::Config(format!("invalid extra pattern {p:?}: {e}")))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            mention,
            keyword,
            extra,
        })
    }

    pub fn is_merge_request(&self, body: &str) -> bool {
        if !self.mention.is_match(body) {
            return false;
        }
        self.keyword.is_match(body) || self.extra.iter().any(|re| re.is_match(body))
    }
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// JSON line handed to a command filter: the item plus its stable `id`.
#[derive(Serialize)]
struct ItemLine<'a> {
    id: String,
    #[serde(flatten)]
    item: &'a Item,
}

/// Runs `sh -c <command>` once per poll (batch, not per item), feeding one
/// JSON object per item on stdin and reading the stable ids to keep from
/// stdout (one per line, plain text). Unknown ids are the caller's problem
/// (they simply match nothing). Non-zero exit and timeout are errors so the
/// caller can keep the section's previous items instead of showing an
/// empty (falsely "all clear") section.
pub async fn run_command_filter(command: &str, items: &[Item]) -> Result<HashSet<String>> {
    run_command_filter_with_timeout(command, items, COMMAND_TIMEOUT).await
}

async fn run_command_filter_with_timeout(
    command: &str,
    items: &[Item],
    timeout: Duration,
) -> Result<HashSet<String>> {
    let mut input = String::new();
    for item in items {
        input.push_str(&serde_json::to_string(&ItemLine {
            id: item.stable_id(),
            item,
        })?);
        input.push('\n');
    }

    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    let output = tokio::time::timeout(timeout, async {
        // Write stdin and read stdout concurrently: if the child produces
        // more output than the pipe buffer holds before it finishes reading
        // stdin, sequential write-then-read would deadlock (child blocks on
        // a full stdout pipe, we block on the stdin write).
        let write = async {
            // The child may exit without reading all input (e.g. `head`);
            // a broken pipe here is not an error.
            let _ = stdin.write_all(input.as_bytes()).await;
            drop(stdin);
        };
        let (_, output) = tokio::join!(write, child.wait_with_output());
        output
    })
    .await
    .map_err(|_| {
        // kill_on_drop reaps the child when the timed-out future is dropped
        Error::Filter(format!(
            "command filter timed out after {}s: {command}",
            timeout.as_secs_f64()
        ))
    })??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            String::new()
        } else {
            format!("\n{}", stderr_tail(stderr))
        };
        return Err(Error::Filter(format!(
            "command filter exited with {}: {command}{detail}",
            output.status
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Keeps the tail of a filter's stderr so a chatty script can't flood the
/// status bar. Bounds by both lines and bytes, splitting on a char boundary.
fn stderr_tail(stderr: &str) -> String {
    const MAX_LINES: usize = 5;
    const MAX_BYTES: usize = 2000;
    let lines: Vec<&str> = stderr.lines().collect();
    let tail = lines[lines.len().saturating_sub(MAX_LINES)..].join("\n");
    if tail.len() <= MAX_BYTES {
        return tail;
    }
    let mut cut = tail.len() - MAX_BYTES;
    while !tail.is_char_boundary(cut) {
        cut += 1;
    }
    format!("…{}", &tail[cut..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> CommentFilter {
        CommentFilter::new("nogu3", &[]).unwrap()
    }

    #[test]
    fn matches_mention_and_english_merge() {
        assert!(filter().is_merge_request("@nogu3 please merge this"));
    }

    #[test]
    fn matches_mention_and_japanese_merge() {
        assert!(filter().is_merge_request("@nogu3 マージお願いします"));
    }

    #[test]
    fn keyword_is_case_insensitive() {
        assert!(filter().is_merge_request("@nogu3 MERGE it"));
    }

    #[test]
    fn rejects_mention_without_keyword() {
        assert!(!filter().is_merge_request("@nogu3 ちょっと見て"));
    }

    #[test]
    fn rejects_keyword_without_mention() {
        assert!(!filter().is_merge_request("merged into main"));
    }

    #[test]
    fn rejects_mention_of_longer_login() {
        // @nogu3x is a different user; \b prevents prefix match
        assert!(!filter().is_merge_request("@nogu3x please merge"));
    }

    #[test]
    fn rejects_mention_of_hyphenated_longer_login() {
        // @nogu3-fork is a different user; hyphen must not count as a boundary
        assert!(!filter().is_merge_request("@nogu3-fork please merge"));
    }

    #[test]
    fn extra_pattern_counts_as_keyword() {
        let f = CommentFilter::new("nogu3", &[r"(?i)ship\s*it".to_string()]).unwrap();
        assert!(f.is_merge_request("@nogu3 ship it!"));
        // extra pattern alone without mention still rejected
        assert!(!f.is_merge_request("ship it!"));
    }

    #[test]
    fn invalid_extra_pattern_is_config_error() {
        assert!(CommentFilter::new("nogu3", &["(".to_string()]).is_err());
    }

    #[test]
    fn mention_is_case_insensitive() {
        assert!(filter().is_merge_request("@NoGu3 please merge"));
    }

    #[test]
    fn matches_mention_at_end_of_body() {
        // exercises the `$` alternative of the mention boundary
        assert!(filter().is_merge_request("please merge @nogu3"));
    }

    use std::time::Duration;

    use crate::item::Item;

    fn pr_item(repo: &str, number: u64) -> Item {
        Item {
            repo: repo.into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: "u".into(),
            pr_author: "a".into(),
            pr_updated_at: "2026-07-02T00:00:00Z".into(),
            pr_created_at: "2026-07-01T00:00:00Z".into(),
            comment: None,
        }
    }

    #[tokio::test]
    async fn command_filter_keeps_ids_printed_to_stdout() {
        let items = vec![pr_item("o/r", 1), pr_item("o/r", 2)];
        // stdin carries one JSON object per item with an "id" field;
        // grep -o extracts the first item's id from it
        let keep = run_command_filter("grep -o 'pr:o/r#1'", &items)
            .await
            .unwrap();
        assert!(keep.contains("pr:o/r#1"));
        assert!(!keep.contains("pr:o/r#2"));
    }

    #[tokio::test]
    async fn command_filter_empty_stdout_keeps_nothing() {
        let keep = run_command_filter("cat > /dev/null", &[pr_item("o/r", 1)])
            .await
            .unwrap();
        assert!(keep.is_empty());
    }

    #[tokio::test]
    async fn command_filter_tolerates_child_not_reading_stdin() {
        // `head -n 1` exits after one line; the resulting broken pipe on the
        // writer side must not be an error
        let items = vec![pr_item("a/a", 1), pr_item("b/b", 2)];
        let keep = run_command_filter("head -n 1 | grep -o 'pr:a/a#1'", &items)
            .await
            .unwrap();
        assert!(keep.contains("pr:a/a#1"));
    }

    #[tokio::test]
    async fn command_filter_nonzero_exit_is_error() {
        let err = run_command_filter("exit 3", &[pr_item("o/r", 1)])
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Filter(m) if m.contains("exited")));
    }

    #[test]
    fn stderr_tail_keeps_only_the_last_lines() {
        let long: String = (1..=20).map(|n| format!("line{n}\n")).collect();
        let tail = stderr_tail(&long);
        assert!(tail.contains("line20"));
        assert!(tail.contains("line16"));
        // earlier lines are dropped
        assert!(!tail.contains("line15"));
        assert!(!tail.contains("line1\n"));
    }

    #[tokio::test]
    async fn command_filter_error_includes_stderr() {
        // a failing filter script's diagnostics must reach the user, not be
        // swallowed — otherwise "exited with 1" is undebuggable. The marker is
        // base64-encoded so it appears only on the child's stderr, never in the
        // command string the error already echoes.
        let err = run_command_filter(
            "echo Ym9vbToxMjMK | base64 -d >&2; exit 1",
            &[pr_item("o/r", 1)],
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, Error::Filter(m) if m.contains("boom:123")),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn command_filter_timeout_is_error() {
        let err = run_command_filter_with_timeout("sleep 5", &[], Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Filter(m) if m.contains("timed out")));
    }

    #[tokio::test]
    async fn command_filter_trims_and_skips_blank_lines() {
        let keep = run_command_filter("printf '  pr:o/r#1  \\n\\n'", &[pr_item("o/r", 1)])
            .await
            .unwrap();
        assert_eq!(keep.len(), 1);
        assert!(keep.contains("pr:o/r#1"));
    }

    #[tokio::test]
    async fn command_filter_handles_output_larger_than_pipe_buffer() {
        // `cat` echoes every input line back; with >64KB of input the child's
        // stdout pipe fills while stdin is still being written, which
        // deadlocked the old sequential write-then-read implementation.
        let items: Vec<Item> = (0..200)
            .map(|i| {
                let mut item = pr_item("o/r", i);
                item.pr_title = "x".repeat(1024);
                item
            })
            .collect();
        let keep = run_command_filter_with_timeout("cat", &items, Duration::from_secs(2))
            .await
            .unwrap();
        // stdout lines are the JSON item lines themselves — they simply
        // match nothing as ids, but the call must succeed without timing out
        assert_eq!(keep.len(), 200);
    }
}
