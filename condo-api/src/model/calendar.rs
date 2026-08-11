use chrono::NaiveDateTime;
use serde::Deserialize;

/// One amenity booking from the association's calendar.
///
/// Timestamps are naive local time — the feed carries no zone. Callers that need an
/// absolute instant must apply the association's timezone themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: u64,
    pub title: String,
    pub start: NaiveDateTime,
    /// Absent for zero-length and some all-day entries.
    pub end: Option<NaiveDateTime>,
    pub all_day: bool,
    pub is_holiday: bool,
    /// True when the booking belongs to the authenticated user.
    pub is_mine: bool,
}

#[derive(Deserialize)]
struct RawEvent {
    #[serde(rename = "id")]
    id: u64,
    #[serde(rename = "title")]
    title: String,
    #[serde(rename = "start")]
    start: NaiveDateTime,
    #[serde(rename = "end")]
    end: Option<NaiveDateTime>,
    #[serde(rename = "allDay")]
    all_day: bool,
    #[serde(rename = "isEventOwner")]
    is_event_owner: bool,
    #[serde(rename = "isHoliday")]
    is_holiday: bool,
}

/// Parse the amenity calendar feed, sorted by start time.
///
/// `color`, `textColor`, and `isLinkedBooking` are dropped: the first two are display
/// concerns and the third is an internal flag with no meaning to a caller.
pub fn parse_event_list(json: &str) -> Result<Vec<Event>, serde_json::Error> {
    let rows: Vec<RawEvent> = serde_json::from_str(json)?;
    let mut events: Vec<Event> = rows
        .into_iter()
        .map(|r| Event {
            id: r.id,
            title: r.title,
            start: r.start,
            end: r.end,
            all_day: r.all_day,
            is_holiday: r.is_holiday,
            is_mine: r.is_event_owner,
        })
        .collect();
    // The feed arrives in no particular order; callers summarizing "what's coming up"
    // depend on start order, so sorting is the parser's job.
    events.sort_by_key(|e| e.start);
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_captured_amenity_feed() {
        let json = std::fs::read_to_string("tests/fixtures/amenity_events.json").unwrap();
        let events = parse_event_list(&json).unwrap();
        assert_eq!(events.len(), 4);
        // The parser sorts by start (see events_come_back_sorted_by_start below); the
        // fixture's chronologically-earliest entry is id 7570058 at 2025-08-02T18:00:00,
        // not the first entry as it appears in the fixture file.
        let first = &events[0];
        assert_eq!(first.id, 7570058);
        assert_eq!(first.title, "Half of North Beach Shelter booked");
        assert_eq!(
            first.start,
            chrono::NaiveDate::from_ymd_opt(2025, 8, 2)
                .unwrap()
                .and_hms_opt(18, 0, 0)
                .unwrap()
        );
        assert_eq!(
            first.end.unwrap(),
            chrono::NaiveDate::from_ymd_opt(2025, 8, 2)
                .unwrap()
                .and_hms_opt(23, 0, 0)
                .unwrap()
        );
        assert!(!first.all_day);
        assert!(!first.is_holiday);
        assert!(!first.is_mine);
    }

    #[test]
    fn a_null_end_is_allowed() {
        // Observed for zero-length and all-day entries; must not fail the whole parse.
        let json = r##"[{"id":1,"title":"Holiday","start":"2026-01-01T00:00:00","end":null,
            "allDay":true,"color":"#fff","textColor":"#000","isEventOwner":false,
            "isLinkedBooking":false,"isHoliday":true}]"##;
        let events = parse_event_list(json).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].end.is_none());
        assert!(events[0].all_day);
        assert!(events[0].is_holiday);
    }

    #[test]
    fn an_empty_feed_parses_to_no_events() {
        assert_eq!(parse_event_list("[]").unwrap().len(), 0);
    }

    #[test]
    fn events_come_back_sorted_by_start() {
        let json = r#"[
            {"id":2,"title":"Later","start":"2026-03-02T10:00:00","end":null,"allDay":false,
             "color":"","textColor":"","isEventOwner":false,"isLinkedBooking":false,"isHoliday":false},
            {"id":1,"title":"Earlier","start":"2026-03-01T10:00:00","end":null,"allDay":false,
             "color":"","textColor":"","isEventOwner":false,"isLinkedBooking":false,"isHoliday":false}
        ]"#;
        let events = parse_event_list(json).unwrap();
        assert_eq!(
            events[0].title, "Earlier",
            "the feed is not sorted; the parser must sort"
        );
    }
}
