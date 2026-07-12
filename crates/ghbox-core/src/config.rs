use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{Error, Result};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub db_path: PathBuf,
    pub sections: Vec<Section>,
    pub theme: Theme,
    pub keybindings: Keybindings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 300,
            db_path: default_db_path(),
            sections: default_sections(),
            theme: Theme::default(),
            keybindings: Keybindings::default(),
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

/// ratatui named color (lowercase in config) or "#rrggbb".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeColor {
    Named(NamedColor),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
}

impl std::str::FromStr for ThemeColor {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                let r = u8::from_str_radix(&hex[0..2], 16).expect("checked hex");
                let g = u8::from_str_radix(&hex[2..4], 16).expect("checked hex");
                let b = u8::from_str_radix(&hex[4..6], 16).expect("checked hex");
                return Ok(ThemeColor::Rgb(r, g, b));
            }
            return Err(format!("invalid color {s:?}: expected #rrggbb"));
        }
        use NamedColor::*;
        let named = match s {
            "black" => Black,
            "red" => Red,
            "green" => Green,
            "yellow" => Yellow,
            "blue" => Blue,
            "magenta" => Magenta,
            "cyan" => Cyan,
            "gray" => Gray,
            "darkgray" => DarkGray,
            "lightred" => LightRed,
            "lightgreen" => LightGreen,
            "lightyellow" => LightYellow,
            "lightblue" => LightBlue,
            "lightmagenta" => LightMagenta,
            "lightcyan" => LightCyan,
            "white" => White,
            _ => return Err(format!("unknown color {s:?}")),
        };
        Ok(ThemeColor::Named(named))
    }
}

impl<'de> serde::Deserialize<'de> for ThemeColor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub tab_active: ThemeColor,
    pub tab_inactive: ThemeColor,
    pub border: ThemeColor,
    pub selection_bg: ThemeColor,
    pub selection_fg: ThemeColor,
    pub table_header: ThemeColor,
    pub status_bar: ThemeColor,
}

impl Default for Theme {
    fn default() -> Self {
        use NamedColor::*;
        Self {
            tab_active: ThemeColor::Named(Yellow),
            tab_inactive: ThemeColor::Named(DarkGray),
            border: ThemeColor::Named(DarkGray),
            selection_bg: ThemeColor::Named(Blue),
            selection_fg: ThemeColor::Named(White),
            table_header: ThemeColor::Named(Cyan),
            status_bar: ThemeColor::Named(DarkGray),
        }
    }
}

/// A key a config action can be bound to: a single character or a named
/// special key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySpec {
    Char(char),
    Tab,
    BackTab,
    Enter,
    Up,
    Down,
    Left,
    Right,
    Esc,
}

impl std::str::FromStr for KeySpec {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "tab" => Ok(KeySpec::Tab),
            "backtab" => Ok(KeySpec::BackTab),
            "enter" => Ok(KeySpec::Enter),
            "up" => Ok(KeySpec::Up),
            "down" => Ok(KeySpec::Down),
            "left" => Ok(KeySpec::Left),
            "right" => Ok(KeySpec::Right),
            "esc" => Ok(KeySpec::Esc),
            _ => {
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(KeySpec::Char(c)),
                    _ => Err(format!(
                        "invalid key {s:?}: expected one character or tab/backtab/enter/up/down/left/right/esc"
                    )),
                }
            }
        }
    }
}

impl std::fmt::Display for KeySpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySpec::Char(c) => write!(f, "{c}"),
            KeySpec::Tab => write!(f, "tab"),
            KeySpec::BackTab => write!(f, "backtab"),
            KeySpec::Enter => write!(f, "enter"),
            KeySpec::Up => write!(f, "up"),
            KeySpec::Down => write!(f, "down"),
            KeySpec::Left => write!(f, "left"),
            KeySpec::Right => write!(f, "right"),
            KeySpec::Esc => write!(f, "esc"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for KeySpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// One config action's keys. Accepts a single key string or a non-empty
/// array of key strings, so `open = "o"` and `down = ["down", "j"]` both work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding(pub Vec<KeySpec>);

impl<'de> serde::Deserialize<'de> for KeyBinding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = KeyBinding;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a key string or a non-empty array of key strings")
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<KeyBinding, E> {
                let spec = s.parse::<KeySpec>().map_err(E::custom)?;
                Ok(KeyBinding(vec![spec]))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<KeyBinding, A::Error> {
                let mut keys = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    keys.push(s.parse::<KeySpec>().map_err(serde::de::Error::custom)?);
                }
                if keys.is_empty() {
                    return Err(serde::de::Error::custom(
                        "keybinding must have at least one key",
                    ));
                }
                Ok(KeyBinding(keys))
            }
        }
        d.deserialize_any(V)
    }
}

impl std::fmt::Display for KeyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, spec) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, "/")?;
            }
            write!(f, "{spec}")?;
        }
        Ok(())
    }
}

impl KeyBinding {
    /// The binding's primary key — the first one listed. A `KeyBinding` is
    /// always non-empty (defaults are non-empty and deserialize rejects `[]`).
    pub fn primary(&self) -> KeySpec {
        self.0[0]
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Keybindings {
    pub up: KeyBinding,
    pub down: KeyBinding,
    pub next_section: KeyBinding,
    pub prev_section: KeyBinding,
    pub open: KeyBinding,
    pub done: KeyBinding,
    pub refresh: KeyBinding,
    pub quit: KeyBinding,
}

impl Default for Keybindings {
    fn default() -> Self {
        Self {
            up: KeyBinding(vec![KeySpec::Up, KeySpec::Char('k')]),
            down: KeyBinding(vec![KeySpec::Down, KeySpec::Char('j')]),
            next_section: KeyBinding(vec![KeySpec::Right, KeySpec::Char('l')]),
            prev_section: KeyBinding(vec![KeySpec::Left, KeySpec::Char('h')]),
            open: KeyBinding(vec![KeySpec::Char('o')]),
            done: KeyBinding(vec![KeySpec::Char('d')]),
            refresh: KeyBinding(vec![KeySpec::Char('r')]),
            quit: KeyBinding(vec![KeySpec::Char('q')]),
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
        let kb = &self.keybindings;
        let bindings = [
            ("up", &kb.up),
            ("down", &kb.down),
            ("next_section", &kb.next_section),
            ("prev_section", &kb.prev_section),
            ("open", &kb.open),
            ("done", &kb.done),
            ("refresh", &kb.refresh),
            ("quit", &kb.quit),
        ];
        for (i, (name_a, binding_a)) in bindings.iter().enumerate() {
            for (name_b, binding_b) in &bindings[i + 1..] {
                for key in &binding_a.0 {
                    if binding_b.0.contains(key) {
                        return Err(Error::Config(format!(
                            "keybindings: {name_a} and {name_b} are both bound to \"{key}\""
                        )));
                    }
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

    #[test]
    fn theme_defaults_and_partial_override() {
        let cfg = parse("[theme]\ntab_active = \"red\"\n").unwrap();
        assert_eq!(cfg.theme.tab_active, ThemeColor::Named(NamedColor::Red));
        // omitted keys keep defaults
        assert_eq!(cfg.theme.selection_bg, ThemeColor::Named(NamedColor::Blue));
    }

    #[test]
    fn hex_color_parses() {
        let cfg = parse("[theme]\nborder = \"#1a2B3c\"\n").unwrap();
        assert_eq!(cfg.theme.border, ThemeColor::Rgb(0x1a, 0x2b, 0x3c));
    }

    #[test]
    fn invalid_color_is_error() {
        assert!(parse("[theme]\nborder = \"mauve\"\n").is_err());
        assert!(parse("[theme]\nborder = \"#12345\"\n").is_err());
    }

    #[test]
    fn keybindings_defaults_and_partial_override() {
        let cfg = parse("[keybindings]\nquit = \"x\"\n").unwrap();
        assert_eq!(cfg.keybindings.quit, KeyBinding(vec![KeySpec::Char('x')]));
        assert_eq!(
            cfg.keybindings.next_section,
            KeyBinding(vec![KeySpec::Right, KeySpec::Char('l')])
        );
    }

    #[test]
    fn special_key_names_parse() {
        let cfg = parse("[keybindings]\nquit = \"esc\"\nopen = \"o\"\n").unwrap();
        assert_eq!(cfg.keybindings.quit, KeyBinding(vec![KeySpec::Esc]));
        assert_eq!(cfg.keybindings.open, KeyBinding(vec![KeySpec::Char('o')]));
    }

    #[test]
    fn keybinding_accepts_string_and_array() {
        let cfg = parse("[keybindings]\ndown = [\"down\", \"j\"]\nopen = \"o\"\n").unwrap();
        assert_eq!(
            cfg.keybindings.down,
            KeyBinding(vec![KeySpec::Down, KeySpec::Char('j')])
        );
        assert_eq!(cfg.keybindings.open, KeyBinding(vec![KeySpec::Char('o')]));
    }

    #[test]
    fn keybinding_empty_array_is_error() {
        assert!(parse("[keybindings]\nquit = []\n").is_err());
    }

    #[test]
    fn keybinding_defaults_are_arrow_first_with_vim() {
        let cfg = parse("").unwrap();
        assert_eq!(
            cfg.keybindings.next_section,
            KeyBinding(vec![KeySpec::Right, KeySpec::Char('l')])
        );
        assert_eq!(
            cfg.keybindings.prev_section,
            KeyBinding(vec![KeySpec::Left, KeySpec::Char('h')])
        );
        assert_eq!(
            cfg.keybindings.down,
            KeyBinding(vec![KeySpec::Down, KeySpec::Char('j')])
        );
        assert_eq!(cfg.keybindings.open, KeyBinding(vec![KeySpec::Char('o')]));
    }

    #[test]
    fn invalid_key_name_is_error() {
        assert!(parse("[keybindings]\nquit = \"ctrl-q\"\n").is_err());
    }

    #[test]
    fn duplicate_keybinding_is_error() {
        let err = parse("[keybindings]\nquit = \"j\"\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("down") && msg.contains("quit"), "got: {msg}");
    }

    #[test]
    fn key_spec_display_roundtrip() {
        assert_eq!(KeySpec::Char('k').to_string(), "k");
        assert_eq!(KeySpec::BackTab.to_string(), "backtab");
    }

    #[test]
    fn left_right_keys_parse() {
        let cfg =
            parse("[keybindings]\nnext_section = \"right\"\nprev_section = \"left\"\n").unwrap();
        assert_eq!(
            cfg.keybindings.next_section,
            KeyBinding(vec![KeySpec::Right])
        );
        assert_eq!(
            cfg.keybindings.prev_section,
            KeyBinding(vec![KeySpec::Left])
        );
        assert_eq!(KeySpec::Left.to_string(), "left");
        assert_eq!(KeySpec::Right.to_string(), "right");
    }

    #[test]
    fn keybinding_primary_returns_first_key() {
        let cfg = parse("[keybindings]\ndown = [\"down\", \"j\"]\n").unwrap();
        assert_eq!(cfg.keybindings.down.primary(), KeySpec::Down);
    }
}
