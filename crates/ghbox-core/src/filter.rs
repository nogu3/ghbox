use regex::Regex;

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
        let mention = Regex::new(&format!(r"@{}(?:[^\w-]|$)", regex::escape(viewer_login)))
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
}
