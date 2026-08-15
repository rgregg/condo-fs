use crate::server::CondoMcp;
use chrono::{Duration, NaiveDate, Utc};
use condo_api::{CondoClient, Event};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListEventsParams {
    /// Inclusive start of the range, as YYYY-MM-DD. Defaults to today.
    pub start: Option<String>,
    /// Inclusive end of the range, as YYYY-MM-DD: bookings anywhere on this day are
    /// included. Defaults to 60 days after today.
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

/// Parse a YYYY-MM-DD date.
fn parse_date(s: &str) -> Result<NaiveDate, ErrorData> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
        ErrorData::invalid_params(
            format!("Could not read {s:?} as a date. Use YYYY-MM-DD, e.g. 2026-08-01."),
            None,
        )
    })
}

/// Convert a date to Unix seconds at *UTC* midnight.
///
/// This is never treated as an exact cutoff on its own: the amenity feed's own
/// timestamps are naive association-local time with no zone attached, so UTC midnight for
/// a given date can be many hours away from that date's local midnight. Every caller of
/// this function widens the resulting bound (asymmetrically — see `list_events` below,
/// which covers the full UTC-12..UTC+14 offset range) and then filters the parsed events
/// by comparing their naive local date/range against the request, not against this value.
fn utc_midnight(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .expect("midnight is always valid")
        .and_utc()
        .timestamp()
}

#[rmcp::tool_router(router = calendar_router, vis = "pub")]
impl CondoMcp {
    #[tool(
        name = "list_events",
        description = "List bookings on the condo association's amenity calendar between two \
                       dates, inclusive on both ends — reservations of shared facilities such \
                       as shelters, docks, and the clubhouse. Dates are YYYY-MM-DD and default \
                       to today through 60 days out. Results are sorted by start time. Titles \
                       name the facility booked, not the resident who booked it, and booking \
                       details are not available. IMPORTANT: this covers amenity bookings only. \
                       Board meetings, committee meetings, classes, and other community events \
                       live on a separate community events calendar that this server does not \
                       yet expose — an empty or unrelated result here does not mean no such \
                       event exists."
    )]
    pub async fn list_events(
        &self,
        Parameters(params): Parameters<ListEventsParams>,
    ) -> Result<Json<Vec<CalendarEvent>>, ErrorData> {
        let today = Utc::now().date_naive();
        let start_date = match params.start {
            Some(ref s) => parse_date(s)?,
            // The feed carries no timezone, so "today" can only be approximated from
            // this server's UTC clock. Back it up a day so a UTC-vs-association-local
            // mismatch can never silently exclude bookings that are still "today" in
            // the association's own timezone; the range is widened and filtered the
            // same way an explicit start date is, below.
            None => today - Duration::days(1),
        };
        let end_date = match params.end {
            Some(ref s) => parse_date(s)?,
            None => today + Duration::days(60),
        };
        if end_date < start_date {
            return Err(ErrorData::invalid_params(
                "The end date must not be before the start date.".to_string(),
                None,
            ));
        }

        // The outbound query is Unix seconds with no timezone, but the feed's own
        // timestamps are naive association-local time. Widen the fetched window so no
        // booking that overlaps [start_date, end_date] can fall outside it regardless of
        // the association's real UTC offset, then filter the parsed events below back
        // down to the exact requested days.
        //
        // The widening is asymmetric because `utc_midnight` always measures from UTC, not
        // from the association's (unknown) local offset. Checked against the real-world
        // offset range UTC-12..UTC+14:
        //   - start edge, worst case UTC+14: utc_midnight(start_date - 1) lands at local
        //     (start_date - 1)T14:00 — still before local start_date's midnight. One day
        //     back covers every offset in range.
        //   - end edge, worst case UTC-12: utc_midnight(end_date + 1) — one day forward —
        //     lands at local end_date T12:00: only halfway through end_date's day, so an
        //     evening booking on end_date is still missed (exactly the bug this fixes).
        //     utc_midnight(end_date + 2) — two days forward — lands at local
        //     (end_date + 1)T12:00, which is past local end_date's midnight rollover, so
        //     it is required, not merely conservative.
        let query_start = utc_midnight(start_date - Duration::days(1));
        let query_end = utc_midnight(end_date + Duration::days(2));
        let events = self
            .with_client(move |c| c.list_events(query_start, query_end))
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(
            events
                .into_iter()
                .filter(|e| {
                    // Keep on overlap with [start_date, end_date], not just on the start
                    // date: a multi-day booking (e.g. a marina dock reserved for several
                    // days) can start before the requested range and still belong in it.
                    let event_end = e.end.unwrap_or(e.start).date();
                    e.start.date() <= end_date && event_end >= start_date
                })
                .map(CalendarEvent::from)
                .collect(),
        ))
    }
}
