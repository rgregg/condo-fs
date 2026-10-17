use thiserror::Error;

/// A page's markup did not contain what the parser needed.
///
/// Distinct from `ClientError::Parse`, which covers malformed JSON: this means the HTML
/// arrived intact but its structure moved, so the fix is a new selector, not a retry.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("expected element not found: {0}")]
    MissingElement(&'static str),
    /// The element was found, but converting its HTML to Markdown failed. Kept distinct
    /// from `MissingElement` so the two failure modes stay visible in logs and error
    /// messages: this one means the DOM shape was fine but the conversion step itself
    /// broke, not that a selector stopped matching.
    #[error("could not convert element to markdown: {0}")]
    UnconvertibleMarkup(&'static str),
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
    /// An attachment could not be downloaded. The message is always a fixed phrase: it never
    /// carries a URL, the site's `Key`, or a pre-signed storage query.
    #[error("attachment error: {0}")]
    Attachment(String),
}
