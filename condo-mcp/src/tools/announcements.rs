use crate::server::CondoMcp;
use condo_api::{AnnouncementSummary, CondoClient};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

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
            posted_at: a.posted_at.format("%Y-%m-%dT%H:%M:%S").to_string(),
            expires_at: a
                .expires_at
                .map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string()),
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
}
