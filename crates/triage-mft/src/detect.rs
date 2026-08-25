use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactType {
    Mft,
    UsnJournal,
    Boot,
    Sds,
    LogFile,
    Unknown,
}

pub fn detect_by_path(path: impl AsRef<Path>) -> ArtifactType {
    let file_name = path
        .as_ref()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let decoded = percent_encoding::percent_decode_str(file_name)
        .decode_utf8_lossy()
        .to_ascii_lowercase();

    match decoded.as_str() {
        "$mft" => ArtifactType::Mft,
        "$boot" => ArtifactType::Boot,
        "$logfile" => ArtifactType::LogFile,
        "$secure:$sds" | "$sds" => ArtifactType::Sds,
        "$usnjrnl:$j" | "$j" => ArtifactType::UsnJournal,
        _ => ArtifactType::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_plain_artifact_names() {
        assert_eq!(detect_by_path("$MFT"), ArtifactType::Mft);
        assert_eq!(detect_by_path("$Boot"), ArtifactType::Boot);
        assert_eq!(detect_by_path("$LogFile"), ArtifactType::LogFile);
    }

    #[test]
    fn detects_url_encoded_collection_names() {
        assert_eq!(detect_by_path("$UsnJrnl%3A$J"), ArtifactType::UsnJournal);
        assert_eq!(detect_by_path("$Secure%3A$SDS"), ArtifactType::Sds);
    }

    #[test]
    fn unknown_for_unrecognized_names() {
        assert_eq!(detect_by_path("settings.dat"), ArtifactType::Unknown);
    }
}
