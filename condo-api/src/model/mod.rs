pub mod announcements;
pub mod calendar;
pub(crate) mod html;
pub mod library;
pub mod violations;

pub use announcements::{
    parse_announcement_detail, parse_announcement_list, AnnouncementDetail, AnnouncementSummary,
    Attachment,
};
pub use calendar::{parse_event_list, Event};
pub use library::{parse_file_list, Entry, FileMeta};
pub use violations::{
    parse_violation_detail, parse_violation_filters, parse_violation_page, SavedAttachment,
    ViolationAttachment, ViolationDetail, ViolationFilterOptions, ViolationPage, ViolationQuery,
    ViolationStatus, ViolationSummary, ViolationUpdate,
};
