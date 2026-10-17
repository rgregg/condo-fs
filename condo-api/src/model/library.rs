use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Folder {
        id: u64,
        name: String,
    },
    File {
        id: u64,
        key: String,
        name: String,
        date: String,
        thumbnail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    pub filename: Option<String>,
}

#[derive(Deserialize)]
struct RawRow {
    #[serde(rename = "ID")]
    id: u64,
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "Thumbnail")]
    thumbnail: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Date")]
    date: String,
    #[serde(rename = "Link")]
    link: String,
}

pub fn parse_file_list(json: &str) -> Result<Vec<Entry>, serde_json::Error> {
    let rows: Vec<RawRow> = serde_json::from_str(json)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            // `Options` is a permissions bitmask, NOT a type discriminator — folders
            // appear with Options 0 or 2. The reliable signal is the Link URL:
            // folders point at view-folder, files at view-file/download-file.
            if r.link.contains("view-folder") {
                Some(Entry::Folder {
                    id: r.id,
                    name: r.name,
                })
            } else if r.link.contains("view-file") || r.link.contains("download-file") {
                Some(Entry::File {
                    id: r.id,
                    key: r.key,
                    name: r.name,
                    date: r.date,
                    thumbnail: r.thumbnail,
                })
            } else {
                log::warn!("skipping row {} with unrecognized Link {:?}", r.id, r.link);
                None
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_folder_rows() {
        let json = std::fs::read_to_string("tests/fixtures/folders.json").unwrap();
        let entries = parse_file_list(&json).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0],
            Entry::Folder {
                id: 162100,
                name: "Board of Directors".into()
            }
        );
    }

    #[test]
    fn options_zero_folders_are_still_folders() {
        // Regression: `Options` is a permissions bitmask, not a type discriminator.
        // Folders can have Options=0 and must not be dropped.
        let json = std::fs::read_to_string("tests/fixtures/folders.json").unwrap();
        let entries = parse_file_list(&json).unwrap();
        assert_eq!(
            entries[2],
            Entry::Folder {
                id: 141025,
                name: "Governing Documents".into()
            }
        );
    }

    #[test]
    fn parses_file_rows_keeping_raw_name() {
        let json = std::fs::read_to_string("tests/fixtures/files.json").unwrap();
        let entries = parse_file_list(&json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            Entry::File {
                id: 5369528,
                key: "9E825A05-B799-4A3A-8635-9C9B19A66ADB".into(),
                name: "01/09/25 Board Minutes".into(),
                date: "2025-01-18 02:41:25".into(),
                thumbnail: "/shared/images/icons/pdf-128x128.png".into(),
            }
        );
    }
}
