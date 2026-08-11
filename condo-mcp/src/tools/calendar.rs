use crate::server::CondoMcp;
use chrono::{Duration, NaiveDate, NaiveDateTime, Utc};
use condo_api::{CondoClient, Event};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListEventsParams {
    /// Inclusive start of the range, as YYYY-MM-DD. Defaults to today.
    pub start: Option<String>,
    /// Exclusive end of the range, as YYYY-MM-DD. Defaults to 60 days after today.
    pub end: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CalendarEvent {
    pub id: u64,
    /// Names the facility booked, never the resident who booked it.
    pub title: String,
    /// Local time, no zone — the association's own clock.
    pub start: String,
    /// Absent for zero-length and some all-day entries.
    pub end: Option<String>,
    pub all_day: bool,
    pub is_holiday: bool,
    /// True when the booking belongs to the authenticated user.
    pub is_mine: bool,
}

impl From<Event> for CalendarEvent {
    fn from(e: Event) -> CalendarEvent {
        CalendarEvent {
            id: e.id,
            title: e.title,
            start: e.start.format("%Y-%m-%dT%H:%M:%S").to_string(),
            end: e.end.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
            all_day: e.all_day,
            is_holiday: e.is_holiday,
            is_mine: e.is_mine,
        }
    }
}

/// Parse a YYYY-MM-DD date into a Unix second count at midnight.
fn parse_day(s: &str) -> Result<i64, ErrorData> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(|d| {
            d.and_hms_opt(0, 0, 0)
                .expect("midnight is always valid")
                .and_utc()
                .timestamp()
        })
        .map_err(|_| {
            ErrorData::invalid_params(
                format!("Could not read {s:?} as a date. Use YYYY-MM-DD, e.g. 2026-08-01."),
                None,
            )
        })
}

#[rmcp::tool_router(router = calendar_router, vis = "pub")]
impl CondoMcp {
    #[tool(
        name = "list_events",
        description = "List bookings on the condo association's amenity calendar between two \
                       dates — reservations of shared facilities such as shelters, docks, and \
                       the clubhouse. Dates are YYYY-MM-DD and default to today through 60 days \
                       out. Results are sorted by start time. Titles name the facility booked, \
                       not the resident who booked it, and booking details are not available."
    )]
    pub async fn list_events(
        &self,
        Parameters(params): Parameters<ListEventsParams>,
    ) -> Result<Json<Vec<CalendarEvent>>, ErrorData> {
        let today: NaiveDateTime = Utc::now().naive_utc().date().and_hms_opt(0, 0, 0).unwrap();
        let start = match params.start {
            Some(ref s) => parse_day(s)?,
            None => today.and_utc().timestamp(),
        };
        let end = match params.end {
            Some(ref s) => parse_day(s)?,
            None => (today + Duration::days(60)).and_utc().timestamp(),
        };
        if end < start {
            return Err(ErrorData::invalid_params(
                "The end date is before the start date.".to_string(),
                None,
            ));
        }
        let events = self
            .with_client(move |c| c.list_events(start, end))
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(events.into_iter().map(CalendarEvent::from).collect()))
    }
}
