pub mod client;
pub mod credentials;
pub mod error;
pub mod model;
pub mod paths;

pub use client::{
    AnnouncementsApi, CalendarApi, CondoClient, HttpCondoClient, LibraryApi, LoginApi, RawResponse,
    ViolationsApi,
};
pub use credentials::{Credentials, CredentialsError};
pub use error::{ClientError, ParseError};
pub use model::{
    parse_announcement_detail, parse_announcement_list, parse_event_list, parse_file_list,
    parse_violation_filters, parse_violation_page, AnnouncementDetail, AnnouncementSummary,
    Attachment, Entry, Event, FileMeta, ViolationFilterOptions, ViolationPage, ViolationQuery,
    ViolationStatus, ViolationSummary,
};
