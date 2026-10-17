use crate::server::CondoMcp;
use condo_api::{CondoClient, Entry};
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListFolderParams {
    /// Folder ID to list. Omit to list the top of the File Library.
    pub folder_id: Option<u64>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct FolderEntry {
    /// Either "folder" or "file".
    pub kind: String,
    /// Pass a folder's id back as `folder_id` to descend into it.
    pub id: u64,
    pub name: String,
    /// Upload timestamp as Condo Control reports it; files only.
    pub modified: Option<String>,
}

impl From<Entry> for FolderEntry {
    fn from(e: Entry) -> FolderEntry {
        match e {
            Entry::Folder { id, name } => FolderEntry {
                kind: "folder".into(),
                id,
                name,
                modified: None,
            },
            Entry::File { id, name, date, .. } => FolderEntry {
                kind: "file".into(),
                id,
                name,
                modified: Some(date),
            },
        }
    }
}

#[rmcp::tool_router(router = library_router, vis = "pub")]
impl CondoMcp {
    #[tool(
        name = "list_folder",
        description = "List the folders and files directly inside one File Library folder of the \
                       condo association's document library. Omit folder_id to list the top of the \
                       library. Returns entries in the order Condo Control provides, which is not \
                       sorted. File contents cannot be read yet."
    )]
    pub async fn list_folder(
        &self,
        Parameters(params): Parameters<ListFolderParams>,
    ) -> Result<Json<Vec<FolderEntry>>, ErrorData> {
        let id = params.folder_id.unwrap_or(self.root);
        let entries = self
            .with_client(move |c| c.list_folder(id))
            .await
            .map_err(|e| self.tool_error(e))?;
        Ok(Json(entries.into_iter().map(FolderEntry::from).collect()))
    }
}
