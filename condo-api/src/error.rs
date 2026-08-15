use thiserror::Error;

/// A page's markup did not contain what the parser needed.
///
/// Distinct from `ClientError::Parse`, which covers malformed JSON: this means the HTML
/// arrived intact but its structure moved, so the fix is a new selector, not a retry.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("expected element not found: {0}")]
    MissingElement(&'static str),
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("authentication failed")]
    Auth,
    #[error("parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found")]
    NotFound,
    #[error("page structure error: {0}")]
    PageStructure(#[from] ParseError),
}
