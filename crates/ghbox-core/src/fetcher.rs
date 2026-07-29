//! Stateful fetch orchestrator: GitHub search for query sections, direct
//! enumeration for repo sections, and a diff-based comment cache so
//! steady-state polls only refetch comments of PRs whose `updatedAt`
//! moved. Lives across polls; the frontend holds one instance.

use std::collections::{HashMap, HashSet};

use crate::config::{Section, SectionFilter};
use crate::github::{self, PrData};
use crate::item::CommentInfo;
use crate::repo_fetch::{self, PrRef};
use crate::{Error, Result};

pub struct FetchOutcome {
    pub viewer_login: String,
    /// Parallel to the `Section` slice passed to `fetch`. `Err` means this
    /// section's candidates could not be fetched this poll (the frontend
    /// keeps its previous items).
    pub sections: Vec<std::result::Result<Vec<PrData>, String>>,
    /// Partial-failure warnings surfaced alongside usable data.
    pub errors: Vec<String>,
}

pub(crate) struct CachedComments {
    pub updated_at: String,
    pub comments: Vec<CommentInfo>,
}

#[derive(Default)]
pub struct Fetcher {
    /// `repo#number` → comments as of the recorded `updatedAt`.
    comment_cache: HashMap<String, CachedComments>,
}

fn key_of(pr: &PrData) -> String {
    format!("{}#{}", pr.repo, pr.pr_number)
}

/// Union of the search result (first, so its comment-carrying entries win
/// dedup) and the section's repo enumerations.
pub(crate) fn merge_candidates(search: Option<&[PrData]>, repo_lists: &[&[PrData]]) -> Vec<PrData> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for pr in search
        .unwrap_or_default()
        .iter()
        .chain(repo_lists.iter().flat_map(|l| l.iter()))
    {
        if seen.insert(key_of(pr)) {
            out.push(pr.clone());
        }
    }
    out
}

/// PRs that still need a comment fetch: no comments yet, and the cache has
/// no entry at this `updatedAt`.
pub(crate) fn plan_comment_fetches(
    cache: &HashMap<String, CachedComments>,
    candidates: &[PrData],
) -> Vec<PrRef> {
    candidates
        .iter()
        .filter(|pr| !pr.comments_fetched)
        .filter(|pr| {
            cache
                .get(&key_of(pr))
                .is_none_or(|c| c.updated_at != pr.pr_updated_at)
        })
        .map(|pr| PrRef {
            repo: pr.repo.clone(),
            number: pr.pr_number,
        })
        .collect()
}

pub(crate) fn attach_comments(prs: &mut [PrData], cache: &HashMap<String, CachedComments>) {
    for pr in prs.iter_mut().filter(|p| !p.comments_fetched) {
        if let Some(c) = cache.get(&key_of(pr))
            && c.updated_at == pr.pr_updated_at
        {
            pr.comments = c.comments.clone();
            pr.comments_fetched = true;
        }
    }
}

/// Drops cache entries for PRs that are no longer open — but only within
/// repos whose enumeration succeeded this poll; a failed repo's PRs may
/// well still be open.
pub(crate) fn evict_stale(
    cache: &mut HashMap<String, CachedComments>,
    live_keys: &HashSet<String>,
    succeeded_repos: &HashSet<String>,
) {
    cache.retain(|key, _| {
        let repo = key.rsplit_once('#').map(|(r, _)| r).unwrap_or(key);
        !succeeded_repos.contains(repo) || live_keys.contains(key)
    });
}

fn wants_comments(section: &Section) -> bool {
    matches!(section.filter, SectionFilter::CommentMention { .. })
}

impl Fetcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn fetch(&mut self, token: &str, sections: &[Section]) -> Result<FetchOutcome> {
        let mut warnings: Vec<String> = Vec::new();

        // 1. Search, for the query-carrying subset.
        let search_subset: Vec<(usize, &Section)> = sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.query.is_some())
            .collect();
        let search_result = if search_subset.is_empty() {
            None
        } else {
            let subset: Vec<&Section> = search_subset.iter().map(|(_, s)| *s).collect();
            Some(github::fetch_sections(token, &subset).await)
        };
        // section index → position in the search Fetched
        let search_pos: HashMap<usize, usize> = search_subset
            .iter()
            .enumerate()
            .map(|(pos, (idx, _))| (*idx, pos))
            .collect();

        // 2. Enumeration, once per unique repo (order-stable for tests/logs).
        let unique_repos: Vec<&str> = {
            let mut seen = HashSet::new();
            sections
                .iter()
                .flat_map(|s| s.repos.iter().map(String::as_str))
                .filter(|r| seen.insert(*r))
                .collect()
        };
        let mut repo_results: HashMap<&str, Result<Vec<PrData>>> = HashMap::new();
        let mut enum_viewer: Option<String> = None;
        for repo in &unique_repos {
            match repo_fetch::fetch_repo_open_prs(token, repo).await {
                Ok(r) => {
                    enum_viewer = Some(r.viewer_login);
                    warnings.extend(r.errors);
                    repo_results.insert(repo, Ok(r.prs));
                }
                Err(e) => {
                    repo_results.insert(repo, Err(e));
                }
            }
        }

        // 3. Viewer identity; without any successful request there is no
        // usable result at all — surface the first error as the fetch error.
        let viewer_login = match (&search_result, enum_viewer) {
            (Some(Ok(f)), _) => f.viewer_login.clone(),
            (_, Some(v)) => v,
            (Some(Err(_)), None) => {
                let Some(Err(e)) = search_result else {
                    unreachable!()
                };
                return Err(e);
            }
            (None, None) => {
                let first_err = repo_results
                    .into_values()
                    .find_map(|r| r.err())
                    .unwrap_or_else(|| Error::Api("no sources configured".into()));
                return Err(first_err);
            }
        };
        if let Some(Ok(f)) = &search_result {
            warnings.extend(f.errors.iter().cloned());
        }

        // 4. Per-section candidates (search ∪ repos), or the section's error.
        let mut out_sections: Vec<std::result::Result<Vec<PrData>, String>> = Vec::new();
        for (idx, section) in sections.iter().enumerate() {
            if search_pos.contains_key(&idx)
                && let Some(Err(e)) = &search_result
            {
                out_sections.push(Err(format!("search failed: {e}")));
                continue;
            }
            if let Some(repo_err) =
                section
                    .repos
                    .iter()
                    .find_map(|r| match repo_results.get(r.as_str()) {
                        Some(Err(e)) => Some(format!("{e}")),
                        _ => None,
                    })
            {
                out_sections.push(Err(repo_err));
                continue;
            }
            let search_prs: Option<&[PrData]> = search_pos.get(&idx).map(|pos| {
                let Some(Ok(f)) = &search_result else {
                    unreachable!("search errors handled above")
                };
                f.sections[*pos].as_slice()
            });
            let repo_lists: Vec<&[PrData]> = section
                .repos
                .iter()
                .map(|r| {
                    let Some(Ok(prs)) = repo_results.get(r.as_str()) else {
                        unreachable!("repo errors handled above")
                    };
                    prs.as_slice()
                })
                .collect();
            out_sections.push(Ok(merge_candidates(search_prs, &repo_lists)));
        }

        // 5. Comment diff-fetch for comment-mention sections.
        let mut needed: Vec<PrRef> = Vec::new();
        let mut needed_keys = HashSet::new();
        let mut key_updated_at: HashMap<String, String> = HashMap::new();
        for (section, candidates) in sections.iter().zip(&out_sections) {
            if !wants_comments(section) {
                continue;
            }
            let Ok(candidates) = candidates else { continue };
            for r in plan_comment_fetches(&self.comment_cache, candidates) {
                if needed_keys.insert(r.key()) {
                    needed.push(r);
                }
            }
            for pr in candidates {
                key_updated_at.insert(key_of(pr), pr.pr_updated_at.clone());
            }
        }
        let comment_failure: Option<String> = if needed.is_empty() {
            None
        } else {
            match repo_fetch::fetch_comments(token, &needed).await {
                Ok((map, errs)) => {
                    warnings.extend(errs);
                    for r in &needed {
                        let key = r.key();
                        // absent from the map = PR vanished mid-fetch; an
                        // empty entry still marks it as fetched
                        let comments = map.get(&key).cloned().unwrap_or_default();
                        if let Some(updated_at) = key_updated_at.get(&key) {
                            self.comment_cache.insert(
                                key,
                                CachedComments {
                                    updated_at: updated_at.clone(),
                                    comments,
                                },
                            );
                        }
                    }
                    None
                }
                Err(e) => Some(format!("comment fetch failed: {e}")),
            }
        };
        for (section, candidates) in sections.iter().zip(out_sections.iter_mut()) {
            if !wants_comments(section) {
                continue;
            }
            let Ok(prs) = candidates else { continue };
            attach_comments(prs, &self.comment_cache);
            if prs.iter().any(|p| !p.comments_fetched) {
                // a comment-mention section missing comments would show
                // false negatives; degrade it to a section error instead
                let msg = comment_failure
                    .clone()
                    .unwrap_or_else(|| "comment fetch incomplete".into());
                *candidates = Err(msg);
            }
        }

        // 6. Evict cache entries of PRs no longer open.
        let mut live_keys = HashSet::new();
        let mut succeeded_repos = HashSet::new();
        for repo in &unique_repos {
            if let Some(Ok(prs)) = repo_results.get(repo) {
                succeeded_repos.insert((*repo).to_string());
                live_keys.extend(prs.iter().map(key_of));
            }
        }
        evict_stale(&mut self.comment_cache, &live_keys, &succeeded_repos);

        Ok(FetchOutcome {
            viewer_login,
            sections: out_sections,
            errors: warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::PrData;
    use crate::item::{CommentInfo, PrState};

    fn pr(repo: &str, number: u64, updated_at: &str, comments_fetched: bool) -> PrData {
        PrData {
            repo: repo.into(),
            pr_number: number,
            pr_title: "t".into(),
            pr_url: "u".into(),
            pr_author: "a".into(),
            pr_updated_at: updated_at.into(),
            pr_created_at: "2026-01-01T00:00:00Z".into(),
            state: PrState::Open,
            comments: vec![],
            review_requested: vec![],
            comments_fetched,
        }
    }

    fn cached(updated_at: &str, ids: &[i64]) -> CachedComments {
        CachedComments {
            updated_at: updated_at.into(),
            comments: ids
                .iter()
                .map(|&id| CommentInfo {
                    id,
                    author: "a".into(),
                    body: "b".into(),
                    created_at: "2026-01-01T00:00:00Z".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn merge_candidates_dedups_preferring_search_version() {
        let search = vec![pr("o/r", 1, "2026-07-01T00:00:00Z", true)];
        let repo_list = vec![
            pr("o/r", 1, "2026-07-01T00:00:00Z", false),
            pr("o/r", 2, "2026-07-01T00:00:00Z", false),
        ];
        let merged = merge_candidates(Some(&search), &[&repo_list]);
        assert_eq!(merged.len(), 2);
        let one = merged.iter().find(|p| p.pr_number == 1).unwrap();
        assert!(one.comments_fetched, "search version (with comments) wins");
    }

    #[test]
    fn merge_candidates_dedups_across_repo_lists() {
        let list = vec![pr("o/r", 1, "2026-07-01T00:00:00Z", false)];
        let merged = merge_candidates(None, &[&list, &list]);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn plan_comment_fetches_skips_fetched_and_fresh_cache() {
        let mut cache = HashMap::new();
        cache.insert("o/r#1".to_string(), cached("2026-07-01T00:00:00Z", &[9]));
        cache.insert("o/r#2".to_string(), cached("2026-06-01T00:00:00Z", &[8]));
        let candidates = vec![
            pr("o/r", 1, "2026-07-01T00:00:00Z", false), // cache fresh — skip
            pr("o/r", 2, "2026-07-01T00:00:00Z", false), // cache stale — fetch
            pr("o/r", 3, "2026-07-01T00:00:00Z", false), // uncached — fetch
            pr("o/r", 4, "2026-07-01T00:00:00Z", true),  // already has comments — skip
        ];
        let refs = plan_comment_fetches(&cache, &candidates);
        let keys: Vec<String> = refs.iter().map(|r| r.key()).collect();
        assert_eq!(keys, vec!["o/r#2".to_string(), "o/r#3".to_string()]);
    }

    #[test]
    fn attach_comments_fills_from_fresh_cache_only() {
        let mut cache = HashMap::new();
        cache.insert("o/r#1".to_string(), cached("2026-07-01T00:00:00Z", &[9]));
        cache.insert("o/r#2".to_string(), cached("2026-06-01T00:00:00Z", &[8]));
        let mut prs = vec![
            pr("o/r", 1, "2026-07-01T00:00:00Z", false),
            pr("o/r", 2, "2026-07-01T00:00:00Z", false),
        ];
        attach_comments(&mut prs, &cache);
        assert!(prs[0].comments_fetched);
        assert_eq!(prs[0].comments[0].id, 9);
        assert!(!prs[1].comments_fetched, "stale cache must not attach");
    }

    #[test]
    fn evict_stale_only_touches_succeeded_repos() {
        let mut cache = HashMap::new();
        cache.insert("o/r#1".to_string(), cached("t", &[]));
        cache.insert("o/r#2".to_string(), cached("t", &[]));
        cache.insert("x/y#5".to_string(), cached("t", &[]));
        let live: HashSet<String> = ["o/r#1".to_string()].into();
        let succeeded: HashSet<String> = ["o/r".to_string()].into();
        evict_stale(&mut cache, &live, &succeeded);
        assert!(cache.contains_key("o/r#1"), "still open — kept");
        assert!(!cache.contains_key("o/r#2"), "closed — evicted");
        assert!(
            cache.contains_key("x/y#5"),
            "repo x/y failed this poll — must not evict"
        );
    }
}
