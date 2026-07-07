use std::path::Path;
use thiserror::Error;

#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

// Manual Debug so the password is never written to logs or panics.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("reading credentials file: {0}")]
    Io(#[from] std::io::Error),
    #[error("missing {0} in credentials file")]
    Missing(&'static str),
}

impl Credentials {
    pub fn from_file(path: &Path) -> Result<Credentials, CredentialsError> {
        let contents = std::fs::read_to_string(path)?;
        let mut username = None;
        let mut password = None;
        for line in contents.lines() {
            let line = line.trim_end_matches(['\r', '\n']);
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim().to_ascii_uppercase().as_str() {
                "USERNAME" => username = Some(value.to_string()),
                "PASSWORD" => password = Some(value.to_string()),
                _ => {}
            }
        }
        Ok(Credentials {
            username: username.ok_or(CredentialsError::Missing("USERNAME"))?,
            password: password.ok_or(CredentialsError::Missing("PASSWORD"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_username_and_password_with_special_chars() {
        let c = Credentials::from_file(Path::new("tests/fixtures/credentials.txt")).unwrap();
        assert_eq!(c.username, "ryan@example.com");
        assert_eq!(c.password, "p@ss^word#1 two");
    }

    #[test]
    fn debug_redacts_password() {
        let c = Credentials {
            username: "u@e.com".into(),
            password: "supersecret".into(),
        };
        let rendered = format!("{c:?}");
        assert!(rendered.contains("u@e.com"));
        assert!(!rendered.contains("supersecret"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn missing_password_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("creds.txt");
        std::fs::write(&p, "USERNAME=only@example.com\n").unwrap();
        assert!(matches!(
            Credentials::from_file(&p),
            Err(CredentialsError::Missing("PASSWORD"))
        ));
    }
}
