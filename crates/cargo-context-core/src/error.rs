use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("regex compile error: {0}")]
    Regex(#[from] regex::Error),

    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("budget exceeded: {actual} tokens > {limit}")]
    BudgetExceeded { actual: usize, limit: usize },

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("not yet implemented: {0}")]
    NotImplemented(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
