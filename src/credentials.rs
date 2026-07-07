use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
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
