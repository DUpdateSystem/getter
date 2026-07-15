use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ManifestAlgorithm {
    Sha256,
    Sha512,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackageManifest {
    entries: HashMap<(ManifestAlgorithm, String), Vec<String>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("malformed Manifest line {line}: {reason}")]
    Malformed { line: usize, reason: String },
    #[error("duplicate Manifest entry for '{file_name}'")]
    Duplicate { file_name: String },
}

impl PackageManifest {
    pub fn parse(content: &str) -> Result<Self, ManifestError> {
        let mut entries: HashMap<(ManifestAlgorithm, String), Vec<String>> = HashMap::new();
        for (index, raw_line) in content.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }
            let Some(separator) = line.find(char::is_whitespace) else {
                return Err(ManifestError::Malformed {
                    line: line_number,
                    reason: "expected '<hex-digest> <file-name>'".to_owned(),
                });
            };
            let digest = &line[..separator];
            let file_name = line[separator..].trim_start();
            if file_name.is_empty() || file_name.chars().any(char::is_control) {
                return Err(ManifestError::Malformed {
                    line: line_number,
                    reason: "file name is empty or contains a control character".to_owned(),
                });
            }
            let algorithm = match digest.len() {
                64 => ManifestAlgorithm::Sha256,
                128 => ManifestAlgorithm::Sha512,
                _ => {
                    return Err(ManifestError::Malformed {
                        line: line_number,
                        reason: "digest must be 64-hex SHA-256 or 128-hex SHA-512".to_owned(),
                    })
                }
            };
            if !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(ManifestError::Malformed {
                    line: line_number,
                    reason: "digest contains non-hex characters".to_owned(),
                });
            }
            let digest = digest.to_ascii_lowercase();
            let key = (algorithm, file_name.to_owned());
            let existing = entries.entry(key).or_default();
            match algorithm {
                ManifestAlgorithm::Sha256 if existing.contains(&digest) => {
                    return Err(ManifestError::Duplicate {
                        file_name: file_name.to_owned(),
                    });
                }
                ManifestAlgorithm::Sha512 if existing.contains(&digest) => continue,
                _ => existing.push(digest),
            }
        }
        Ok(Self { entries })
    }

    pub fn artifact_sha256(&self, file_name: &str) -> Option<&str> {
        self.artifact_sha256_members(file_name)
            .first()
            .map(String::as_str)
    }

    pub fn artifact_sha256_members(&self, file_name: &str) -> &[String] {
        self.digests(file_name, ManifestAlgorithm::Sha256)
            .unwrap_or_default()
    }

    pub fn source_sha512(&self, file_name: &str) -> Option<&str> {
        self.digests(file_name, ManifestAlgorithm::Sha512)
            .and_then(|digests| digests.first())
            .map(String::as_str)
    }

    pub fn contains_sha512(&self, digest: &str) -> bool {
        self.entries.iter().any(|((algorithm, _), digests)| {
            *algorithm == ManifestAlgorithm::Sha512
                && digests
                    .iter()
                    .any(|entry| entry.eq_ignore_ascii_case(digest))
        })
    }

    fn digests(&self, file_name: &str, algorithm: ManifestAlgorithm) -> Option<&[String]> {
        self.entries
            .get(&(algorithm, file_name.to_owned()))
            .map(Vec::as_slice)
    }
}
