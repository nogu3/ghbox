#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GitHub API error: {0}")]
    Api(String),
    #[error("failed to get token via `gh auth token`: {0}")]
    Token(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("db schema error: {0}")]
    Schema(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
