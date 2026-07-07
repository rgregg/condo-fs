use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            '/' => '-',
            '\0' => '_',
            _ => c,
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Icons are named like `pdf-128x128.png`; the leading token is the type.
pub fn infer_extension(thumbnail: &str) -> Option<&'static str> {
    let file = thumbnail.rsplit('/').next().unwrap_or(thumbnail);
    let token = file.split(['-', '.']).next().unwrap_or("");
    match token {
        "pdf" => Some("pdf"),
        "doc" => Some("doc"),
        "docx" => Some("docx"),
        "xls" => Some("xls"),
        "xlsx" => Some("xlsx"),
        "ppt" => Some("ppt"),
        "pptx" => Some("pptx"),
        "txt" => Some("txt"),
        "csv" => Some("csv"),
        "jpg" | "jpeg" => Some("jpg"),
        "png" => Some("png"),
        "gif" => None, // folder.gif and generic icons — never a real file type here
        "zip" => Some("zip"),
        _ => None,
    }
}

pub fn file_display_name(raw: &str, thumbnail: &str) -> String {
    let base = sanitize_name(raw);
    match infer_extension(thumbnail) {
        Some(ext) if !base.to_ascii_lowercase().ends_with(&format!(".{ext}")) => {
            format!("{base}.{ext}")
        }
        _ => base,
    }
}

pub fn resolve_collisions(names: Vec<String>) -> Vec<String> {
    use std::collections::HashMap;
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let key = name.to_ascii_lowercase();
        let count = seen.entry(key).or_insert(0);
        *count += 1;
        if *count == 1 {
            out.push(name);
        } else {
            // insert " (N)" before the extension if any
            let (stem, ext) = match name.rsplit_once('.') {
                Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
                _ => (name.clone(), String::new()),
            };
            out.push(format!("{stem} ({count}){ext}"));
        }
    }
    out
}

pub fn parse_condo_date(date: &str) -> SystemTime {
    // Format: YYYY-MM-DD HH:MM:SS, treated as UTC.
    fn parse(date: &str) -> Option<u64> {
        let (d, t) = date.split_once(' ')?;
        let mut dp = d.split('-');
        let year: i64 = dp.next()?.parse().ok()?;
        let month: i64 = dp.next()?.parse().ok()?;
        let day: i64 = dp.next()?.parse().ok()?;
        let mut tp = t.split(':');
        let hh: i64 = tp.next()?.parse().ok()?;
        let mm: i64 = tp.next()?.parse().ok()?;
        let ss: i64 = tp.next()?.parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        // days from civil (Howard Hinnant's algorithm)
        let y = if month <= 2 { year - 1 } else { year };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146097 + doe - 719468;
        let secs = days * 86400 + hh * 3600 + mm * 60 + ss;
        if secs < 0 {
            None
        } else {
            Some(secs as u64)
        }
    }
    match parse(date) {
        Some(secs) => UNIX_EPOCH + Duration::from_secs(secs),
        None => UNIX_EPOCH,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_slashes() {
        assert_eq!(
            sanitize_name("01/09/25 Board Minutes"),
            "01-09-25 Board Minutes"
        );
        assert_eq!(sanitize_name("  spaced  "), "spaced");
        assert_eq!(sanitize_name("/"), "-");
        assert_eq!(sanitize_name(""), "_");
    }

    #[test]
    fn infers_extension_from_thumbnail() {
        assert_eq!(
            infer_extension("/shared/images/icons/pdf-128x128.png"),
            Some("pdf")
        );
        assert_eq!(infer_extension("/shared/images/icons/folder.gif"), None);
        assert_eq!(infer_extension("/x/doc-128x128.png"), Some("doc"));
    }

    #[test]
    fn file_display_name_appends_extension() {
        assert_eq!(
            file_display_name(
                "01/09/25 Board Minutes",
                "/shared/images/icons/pdf-128x128.png"
            ),
            "01-09-25 Board Minutes.pdf"
        );
        // does not double up if already present
        assert_eq!(
            file_display_name("report.pdf", "/shared/images/icons/pdf-128x128.png"),
            "report.pdf"
        );
        // unknown icon: no extension
        assert_eq!(
            file_display_name("thing", "/shared/images/icons/mystery.gif"),
            "thing"
        );
    }

    #[test]
    fn resolves_duplicate_names() {
        let out = resolve_collisions(vec![
            "a.pdf".into(),
            "a.pdf".into(),
            "b".into(),
            "a.pdf".into(),
        ]);
        assert_eq!(out, vec!["a.pdf", "a (2).pdf", "b", "a (3).pdf"]);
    }

    #[test]
    fn parses_condo_date() {
        let t = parse_condo_date("2025-01-18 02:41:25");
        let secs = t.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, 1737168085); // 2025-01-18T02:41:25Z
        assert_eq!(parse_condo_date("garbage"), UNIX_EPOCH);
        assert_eq!(parse_condo_date(""), UNIX_EPOCH);
    }
}
