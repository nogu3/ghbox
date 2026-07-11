use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub db_path: PathBuf,
    pub sections: Vec<Section>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 300,
            db_path: default_db_path(),
            sections: default_sections(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub title: String,
    pub query: String,
    #[serde(default = "default_columns")]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub filter: SectionFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Repo,
    Number,
    Title,
    Author,
    Comment,
    Updated,
    Created,
}

/// Per-section filter. Deserialized via `FilterSpec` so unknown/misplaced
/// keys inside `filter = { ... }` are config errors (serde's internally
/// tagged enums do not support deny_unknown_fields).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(try_from = "FilterSpec")]
pub enum SectionFilter {
    #[default]
    None,
    CommentMention {
        extra_patterns: Vec<String>,
    },
    Command {
        command: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilterSpec {
    #[serde(rename = "type")]
    kind: String,
    extra_patterns: Option<Vec<String>>,
    command: Option<String>,
}

impl TryFrom<FilterSpec> for SectionFilter {
    type Error = String;

    fn try_from(spec: FilterSpec) -> std::result::Result<Self, String> {
        match spec.kind.as_str() {
            "none" => {
                if spec.extra_patterns.is_some() || spec.command.is_some() {
                    return Err("filter type \"none\" takes no other keys".into());
                }
                Ok(SectionFilter::None)
            }
            "comment-mention" => {
                if spec.command.is_some() {
                    return Err("filter type \"comment-mention\" does not take `command`".into());
                }
                Ok(SectionFilter::CommentMention {
                    extra_patterns: spec.extra_patterns.unwrap_or_default(),
                })
            }
            "command" => {
                if spec.extra_patterns.is_some() {
                    return Err("filter type \"command\" does not take `extra_patterns`".into());
                }
                let command = spec
                    .command
                    .ok_or_else(|| "filter type \"command\" requires `command`".to_string())?;
                Ok(SectionFilter::Command { command })
            }
            other => Err(format!(
                "unknown filter type {other:?} (expected \"none\", \"comment-mention\", or \"command\")"
            )),
        }
    }
}

/// Built-in sections used when the config file has no `sections` key.
/// Reproduces the pre-config behavior (merge requests + review requests).
fn default_sections() -> Vec<Section> {
    vec![
        Section {
            title: "マージ依頼".into(),
            query: "is:pr is:open mentions:@me".into(),
            columns: vec![
                Column::Repo,
                Column::Number,
                Column::Title,
                Column::Author,
                Column::Comment,
            ],
            filter: SectionFilter::CommentMention {
                extra_patterns: Vec::new(),
            },
        },
        Section {
            title: "レビュー依頼".into(),
            query: "is:pr is:open review-requested:@me".into(),
            columns: default_columns(),
            filter: SectionFilter::None,
        },
    ]
}

fn default_columns() -> Vec<Column> {
    vec![
        Column::Repo,
        Column::Number,
        Column::Title,
        Column::Author,
        Column::Updated,
    ]
}

fn default_db_path() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join(".local")
                .join("share")
        })
        .join("ghbox")
        .join("state.db")
}

fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("ghbox")
        .join("config.toml")
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let config = if path.exists() {
            let text = std::fs::read_to_string(path)?;
            toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))?
        } else {
            Self::default()
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.sections.is_empty() {
            return Err(Error::Config(
                "sections must not be empty (omit the key entirely for the defaults)".into(),
            ));
        }
        for section in &self.sections {
            if let SectionFilter::CommentMention { extra_patterns } = &section.filter {
                for pattern in extra_patterns {
                    regex::Regex::new(pattern).map_err(|e| {
                        Error::Config(format!(
                            "section {:?}: invalid extra pattern {pattern:?}: {e}",
                            section.title
                        ))
                    })?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_text: &str) -> Result<Config> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, toml_text).unwrap();
        Config::load_from(&path)
    }

    #[test]
    fn missing_file_yields_default_two_sections() {
        let cfg = Config::load_from(Path::new("/nonexistent/config.toml")).unwrap();
        assert_eq!(cfg.poll_interval_secs, 300);
        assert!(cfg.db_path.ends_with("ghbox/state.db"));
        assert_eq!(cfg.sections.len(), 2);
        assert_eq!(cfg.sections[0].title, "マージ依頼");
        assert!(matches!(
            cfg.sections[0].filter,
            SectionFilter::CommentMention { .. }
        ));
        assert_eq!(cfg.sections[1].filter, SectionFilter::None);
    }

    #[test]
    fn sections_key_omitted_uses_defaults() {
        let cfg = parse("poll_interval_secs = 60\n").unwrap();
        assert_eq!(cfg.poll_interval_secs, 60);
        assert_eq!(cfg.sections.len(), 2);
    }

    #[test]
    fn explicit_empty_sections_is_error() {
        assert!(parse("sections = []\n").is_err());
    }

    #[test]
    fn full_section_parses() {
        let cfg = parse(
            r#"
[[sections]]
title = "マージ依頼"
query = "is:pr is:open mentions:@me"
columns = ["repo", "number", "title", "author", "comment"]
filter = { type = "comment-mention", extra_patterns = ["(?i)ship\\s*it"] }

[[sections]]
title = "自分が関わるPR"
query = "is:pr is:open involves:@me"
filter = { type = "command", command = "jq -r .id" }
"#,
        )
        .unwrap();
        assert_eq!(cfg.sections.len(), 2);
        assert_eq!(cfg.sections[0].columns[4], Column::Comment);
        assert_eq!(
            cfg.sections[0].filter,
            SectionFilter::CommentMention {
                extra_patterns: vec!["(?i)ship\\s*it".into()]
            }
        );
        assert_eq!(
            cfg.sections[1].filter,
            SectionFilter::Command {
                command: "jq -r .id".into()
            }
        );
        // columns omitted → defaults
        assert_eq!(cfg.sections[1].columns, default_columns());
    }

    #[test]
    fn filter_omitted_is_none() {
        let cfg = parse("[[sections]]\ntitle = \"t\"\nquery = \"q\"\n").unwrap();
        assert_eq!(cfg.sections[0].filter, SectionFilter::None);
    }

    #[test]
    fn unknown_column_is_error() {
        let err = parse("[[sections]]\ntitle = \"t\"\nquery = \"q\"\ncolumns = [\"bogus\"]\n");
        assert!(err.is_err());
    }

    #[test]
    fn unknown_filter_type_is_error() {
        let err =
            parse("[[sections]]\ntitle = \"t\"\nquery = \"q\"\nfilter = { type = \"bogus\" }\n");
        assert!(err.is_err());
    }

    #[test]
    fn misplaced_filter_key_is_error() {
        // command key on a comment-mention filter must be rejected
        let err = parse(
            "[[sections]]\ntitle = \"t\"\nquery = \"q\"\nfilter = { type = \"comment-mention\", command = \"x\" }\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn command_filter_requires_command() {
        let err =
            parse("[[sections]]\ntitle = \"t\"\nquery = \"q\"\nfilter = { type = \"command\" }\n");
        assert!(err.is_err());
    }

    #[test]
    fn invalid_extra_pattern_is_error() {
        let err = parse(
            "[[sections]]\ntitle = \"t\"\nquery = \"q\"\nfilter = { type = \"comment-mention\", extra_patterns = [\"(\"] }\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn removed_top_level_extra_patterns_is_error() {
        // the pre-rewrite top-level key must now be a typo error
        assert!(parse("extra_patterns = [\"ship it\"]\n").is_err());
    }

    #[test]
    fn unknown_key_is_error() {
        assert!(parse("typo_key = 1\n").is_err());
    }
}
