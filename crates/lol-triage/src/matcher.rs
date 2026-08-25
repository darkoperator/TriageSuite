use crate::candidate::Candidate;
use crate::refdata::LolRefs;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    #[serde(rename = "SourceTool")]
    pub source_tool: String,
    #[serde(rename = "SourceDataset")]
    pub source_dataset: String,
    #[serde(rename = "SourceFile")]
    pub source_file: String,
    #[serde(rename = "EvidencePath")]
    pub evidence_path: String,
    #[serde(rename = "EvidenceSha1")]
    pub evidence_sha1: String,
    #[serde(rename = "EvidenceTimestamp")]
    pub evidence_timestamp: String,
    #[serde(rename = "MatchType")]
    pub match_type: String,
    #[serde(rename = "ReferenceList")]
    pub reference_list: String,
    #[serde(rename = "ReferenceName")]
    pub reference_name: String,
    #[serde(rename = "ReferenceCategory")]
    pub reference_category: String,
    #[serde(rename = "MitreAttackId")]
    pub mitre_attack_id: String,
    #[serde(rename = "Confidence")]
    pub confidence: String,
}

pub fn match_candidate(c: &Candidate, refs: &LolRefs) -> Vec<Finding> {
    let mut findings = Vec::new();

    if let Some(sha1) = &c.sha1 {
        if let Some(d) = refs.driver_by_hash(sha1) {
            findings.push(build_driver_finding(c, d, "Hash", "High"));
        }
    }
    if findings.is_empty() {
        if let Some(d) = refs.driver_by_basename(&c.basename) {
            findings.push(build_driver_finding(c, d, "Filename", "Medium"));
        }
    }
    if let Some(r) = refs.rmm_by_basename(&c.basename) {
        findings.push(build_rmm_finding(c, r));
    }

    findings
}

fn build_driver_finding(
    c: &Candidate,
    d: &crate::refdata::LolDriverEntry,
    match_type: &str,
    confidence: &str,
) -> Finding {
    Finding {
        source_tool: c.source_tool.to_string(),
        source_dataset: c.source_dataset.to_string(),
        source_file: c.source_file.clone(),
        evidence_path: c.evidence_path.clone(),
        evidence_sha1: c.sha1.clone().unwrap_or_default(),
        evidence_timestamp: c.evidence_timestamp.clone(),
        match_type: match_type.to_string(),
        reference_list: "LOLDrivers".to_string(),
        reference_name: d.tags.first().cloned().unwrap_or_else(|| d.id.clone()),
        reference_category: d.category.clone(),
        mitre_attack_id: d.mitre_id.clone(),
        confidence: confidence.to_string(),
    }
}

fn build_rmm_finding(c: &Candidate, r: &crate::refdata::LolRmmEntry) -> Finding {
    Finding {
        source_tool: c.source_tool.to_string(),
        source_dataset: c.source_dataset.to_string(),
        source_file: c.source_file.clone(),
        evidence_path: c.evidence_path.clone(),
        evidence_sha1: c.sha1.clone().unwrap_or_default(),
        evidence_timestamp: c.evidence_timestamp.clone(),
        match_type: "Filename".to_string(),
        reference_list: "LOLRMM".to_string(),
        reference_name: r.name.clone(),
        reference_category: r.category.clone(),
        mitre_attack_id: String::new(),
        confidence: "Medium".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::refdata::{LolDriverEntry, LolRmmEntry};

    fn refs() -> LolRefs {
        LolRefs::new(
            vec![LolDriverEntry {
                id: "2a6a38ca-f2e6-456e-9ccf-db59d8c80c9e".into(),
                category: "vulnerable driver".into(),
                mitre_id: "T1068".into(),
                tags: vec!["nvflash.sys".into()],
                md5: "ba86e444ae837476e7ccdd06f8867795".into(),
                sha1: "b9c3f4dcc7463cbec84b808d880194bbc304ccd0".into(),
                sha256: "9368e51ec98e2ad20893a5fc21e6a8b20c5bee158d5c49ca58649cff84db9d68".into(),
            }],
            vec![LolRmmEntry {
                name: "KiTTY".into(),
                category: "RAT".into(),
                install_basenames: vec!["kitty.exe".into()],
                sha256_hashes: vec![],
            }],
        )
    }

    fn candidate(basename: &str, sha1: Option<&str>) -> Candidate {
        Candidate {
            source_tool: "AmcacheTriage",
            source_dataset: "FileEntries",
            evidence_path: format!(r"C:\Windows\System32\{basename}"),
            basename: basename.to_string(),
            sha1: sha1.map(str::to_string),
            evidence_timestamp: "2024-01-01T00:00:00.0000000Z".into(),
            source_file: "amcache.csv".into(),
        }
    }

    #[test]
    fn hash_match_is_high_confidence() {
        let c = candidate(
            "nvflash.sys",
            Some("b9c3f4dcc7463cbec84b808d880194bbc304ccd0"),
        );
        let findings = match_candidate(&c, &refs());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].confidence, "High");
        assert_eq!(findings[0].match_type, "Hash");
        assert_eq!(findings[0].reference_list, "LOLDrivers");
        assert_eq!(findings[0].mitre_attack_id, "T1068");
    }

    #[test]
    fn filename_only_match_is_medium_confidence() {
        let c = candidate("nvflash.sys", None);
        let findings = match_candidate(&c, &refs());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].confidence, "Medium");
        assert_eq!(findings[0].match_type, "Filename");
    }

    #[test]
    fn rmm_basename_match() {
        let c = candidate("kitty.exe", None);
        let findings = match_candidate(&c, &refs());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].reference_list, "LOLRMM");
        assert_eq!(findings[0].reference_name, "KiTTY");
        assert_eq!(findings[0].mitre_attack_id, "");
    }

    #[test]
    fn no_match_returns_empty() {
        let c = candidate("notepad.exe", None);
        assert!(match_candidate(&c, &refs()).is_empty());
    }

    /// A SHA-1 that matches nothing must not suppress the filename fallback:
    /// the row still gets a Medium-confidence Filename finding.
    #[test]
    fn unmatched_hash_still_falls_through_to_filename_match() {
        let c = candidate(
            "nvflash.sys",
            Some("0000000000000000000000000000000000000000"),
        );
        let findings = match_candidate(&c, &refs());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].match_type, "Filename");
        assert_eq!(findings[0].confidence, "Medium");
        assert_eq!(findings[0].reference_list, "LOLDrivers");
        // The candidate's own (non-matching) hash is still reported verbatim.
        assert_eq!(
            findings[0].evidence_sha1,
            "0000000000000000000000000000000000000000"
        );
    }

    /// The spec intentionally does not de-duplicate across reference lists: a
    /// basename present in both LOLDrivers and LOLRMM yields one finding each.
    #[test]
    fn basename_in_both_lists_yields_one_finding_per_list() {
        let refs = LolRefs::new(
            vec![LolDriverEntry {
                id: "shared-entry".into(),
                category: "vulnerable driver".into(),
                mitre_id: "T1068".into(),
                tags: vec!["atera.exe".into()],
                md5: String::new(),
                sha1: String::new(),
                sha256: String::new(),
            }],
            vec![LolRmmEntry {
                name: "Atera".into(),
                category: "RMM".into(),
                install_basenames: vec!["atera.exe".into()],
                sha256_hashes: vec![],
            }],
        );
        let c = candidate("atera.exe", None);
        let findings = match_candidate(&c, &refs);
        assert_eq!(findings.len(), 2, "{findings:?}");

        let driver = findings
            .iter()
            .find(|f| f.reference_list == "LOLDrivers")
            .expect("a LOLDrivers finding");
        assert_eq!(driver.reference_name, "atera.exe");
        assert_eq!(driver.mitre_attack_id, "T1068");
        assert_eq!(driver.confidence, "Medium");

        let rmm = findings
            .iter()
            .find(|f| f.reference_list == "LOLRMM")
            .expect("a LOLRMM finding");
        assert_eq!(rmm.reference_name, "Atera");
        assert_eq!(rmm.mitre_attack_id, "");
    }

    /// A candidate with no basename (blank path column in the source CSV) must
    /// never match an empty value in the reference data.
    #[test]
    fn empty_basename_and_hash_produce_no_findings() {
        let refs = LolRefs::new(
            vec![LolDriverEntry {
                id: "blank-fields".into(),
                category: "vulnerable driver".into(),
                mitre_id: "T1068".into(),
                tags: vec!["".into()],
                md5: String::new(),
                sha1: String::new(),
                sha256: String::new(),
            }],
            vec![LolRmmEntry {
                name: "TeamViewer".into(),
                category: "RMM".into(),
                install_basenames: vec!["".into()],
                sha256_hashes: vec![],
            }],
        );
        let c = candidate("", Some(""));
        assert!(match_candidate(&c, &refs).is_empty());
    }
}
