pub mod announcements;
pub mod calendar;
pub mod library;
pub mod violations;

/// Format a naive local timestamp the same way across every tool.
pub(crate) fn format_timestamp(d: chrono::NaiveDateTime) -> String {
    d.format("%Y-%m-%dT%H:%M:%S").to_string()
}
