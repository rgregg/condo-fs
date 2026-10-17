use crate::server::CondoMcp;
use condo_api::{AnnouncementDetail, AnnouncementSummary, AnnouncementsApi, Attachment};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

/// Format a naive local timestamp the same way across every announcement tool.
fn format_timestamp(d: chrono::NaiveDateTime) -> String {
    d.format("%Y-%m-%dT%H:%M:%S").to_string()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListAnnouncementsParams {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Announcement {
    pub id: u64,
    pub title: String,
    /// Local time, no zone — the association's own clock.
    pub posted_at: String,
    /// Absent when the announcement has no expiry.
    pub expires_at: Option<String>,
}

impl From<AnnouncementSummary> for Announcement {
    fn from(a: AnnouncementSummary) -> Announcement {
        Announcement {
            id: a.id,
            title: a.title,
            posted_at: format_timestamp(a.posted_at),
            expires_at: a.expires_at.map(format_timestamp),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetAnnouncementParams {
    /// Announcement id, as returned by list_announcements.
    pub id: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AnnouncementFull {
    pub id: u64,
    pub title: String,
    /// Local time, no zone — the association's own clock. Absent if the page did not
    /// carry a readable posted date.
    pub posted_at: Option<String>,
    /// Absent when the announcement has no expiry.
    pub expires_at: Option<String>,
    /// Who posted it — often a committee or role rather than a person.
    pub posted_by: Option<String>,
    /// The announcement text, converted from the site's HTML to Markdown.
    pub body: String,
    pub attachments: Vec<AnnouncementAttachment>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AnnouncementAttachment {
    pub name: String,
    pub uploaded: Option<String>,
}

impl From<Attachment> for AnnouncementAttachment {
    fn from(a: Attachment) -> AnnouncementAttachment {
        AnnouncementAttachment {
            name: a.name,
            uploaded: a.uploaded.map(format_timestamp),
        }
    }
}

impl From<AnnouncementDetail> for AnnouncementFull {
    fn from(d: AnnouncementDetail) -> AnnouncementFull {
        AnnouncementFull {
            id: d.id,
            title: d.title,
            posted_at: d.posted_at.map(format_timestamp),
            expires_at: d.expires_at.map(format_timestamp),
            posted_by: d.posted_by,
            body: d.body_markdown,
            attachments: d.attachments.into_iter().map(Into::into).collect(),
        }
    }
}

#[rmcp::tool_router(router = announcements_router, vis = "pub")]
impl CondoMcp {
    #[tool(
        name = "list_announcements",
        description = "List the condo association's currently-posted announcements, newest \
                       first. Returns each announcement's id, title, posting time, and expiry \
                       if it has one; pass an id to get_announcement for the full text. \
                       IMPORTANT: this returns only announcements that are currently posted \
                       and unexpired. Older announcements exist but are reachable only through \
                       a form this read-only server cannot submit, so absence from this list \
                       does not mean an announcement never existed."
    )]
    pub async fn list_announcements(
        &self,
        Parameters(_): Parameters<ListAnnouncementsParams>,
    ) -> Result<Json<Vec<Announcement>>, ErrorData> {
        let items = self
            .with_client(move |c| c.list_announcements())
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(items.into_iter().map(Announcement::from).collect()))
    }

    #[tool(
        name = "get_announcement",
        description = "Get one condo association announcement in full, by the id returned \
                       from list_announcements. Returns the title, who posted it, when it was \
                       posted and expires, the full text converted to Markdown, and a list \
                       of attachments (name and upload date; contents cannot be downloaded, \
                       so do not report them as unreadable files). Some announcements are \
                       posted as a single image with no other text — for those, the body \
                       will be only a Markdown image link, which is the complete announcement, \
                       not a broken or truncated one; its contents cannot be fetched, so do \
                       not report it as unreadable or try to download the URL."
    )]
    pub async fn get_announcement(
        &self,
        Parameters(params): Parameters<GetAnnouncementParams>,
    ) -> Result<Json<AnnouncementFull>, ErrorData> {
        let id = params.id;
        let detail = self
            .with_client(move |c| c.get_announcement(id))
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(AnnouncementFull::from(detail)))
    }
}
