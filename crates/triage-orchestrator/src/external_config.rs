use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HayabusaConfig {
    pub bin: String,
    pub enabled: bool,
    pub csv: bool,
    pub json: bool,
    pub rules: Option<String>,
    pub rules_config: Option<String>,
    pub min_level: Option<String>,
    pub profile: Option<String>,
    pub threads: Option<u32>,
    pub clobber: bool,
    pub sort: bool,
    pub scan_all_evtx_files: bool,
    pub enable_all_rules: bool,
    pub enable_noisy_rules: bool,
    pub enable_deprecated_rules: bool,
    pub enable_unsupported_rules: bool,
    pub proven_rules: bool,
    pub time_offset: Option<String>,
    pub timeline_start: Option<String>,
    pub timeline_end: Option<String>,
}

impl Default for HayabusaConfig {
    fn default() -> Self {
        Self {
            bin: "hayabusa".to_string(),
            enabled: true,
            csv: true,
            json: true,
            rules: None,
            rules_config: None,
            min_level: None,
            profile: None,
            threads: None,
            clobber: false,
            sort: false,
            scan_all_evtx_files: false,
            enable_all_rules: false,
            enable_noisy_rules: false,
            enable_deprecated_rules: false,
            enable_unsupported_rules: false,
            proven_rules: false,
            time_offset: None,
            timeline_start: None,
            timeline_end: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HayabusaOverlay {
    pub bin: Option<String>,
    pub enabled: Option<bool>,
    pub csv: Option<bool>,
    pub json: Option<bool>,
    pub rules: Option<String>,
    pub rules_config: Option<String>,
    pub min_level: Option<String>,
    pub profile: Option<String>,
    pub threads: Option<u32>,
    pub clobber: Option<bool>,
    pub sort: Option<bool>,
    pub scan_all_evtx_files: Option<bool>,
    pub enable_all_rules: Option<bool>,
    pub enable_noisy_rules: Option<bool>,
    pub enable_deprecated_rules: Option<bool>,
    pub enable_unsupported_rules: Option<bool>,
    pub proven_rules: Option<bool>,
    pub time_offset: Option<String>,
    pub timeline_start: Option<String>,
    pub timeline_end: Option<String>,
}

impl HayabusaConfig {
    /// Additive per-field merge: only fields set (`Some`) in `overlay` override this base
    /// value; everything else falls through unchanged (spec: Profiles section).
    pub fn merge_overlay(&self, overlay: &HayabusaOverlay) -> HayabusaConfig {
        HayabusaConfig {
            bin: overlay.bin.clone().unwrap_or_else(|| self.bin.clone()),
            enabled: overlay.enabled.unwrap_or(self.enabled),
            csv: overlay.csv.unwrap_or(self.csv),
            json: overlay.json.unwrap_or(self.json),
            rules: overlay.rules.clone().or_else(|| self.rules.clone()),
            rules_config: overlay
                .rules_config
                .clone()
                .or_else(|| self.rules_config.clone()),
            min_level: overlay.min_level.clone().or_else(|| self.min_level.clone()),
            profile: overlay.profile.clone().or_else(|| self.profile.clone()),
            threads: overlay.threads.or(self.threads),
            clobber: overlay.clobber.unwrap_or(self.clobber),
            sort: overlay.sort.unwrap_or(self.sort),
            scan_all_evtx_files: overlay
                .scan_all_evtx_files
                .unwrap_or(self.scan_all_evtx_files),
            enable_all_rules: overlay.enable_all_rules.unwrap_or(self.enable_all_rules),
            enable_noisy_rules: overlay
                .enable_noisy_rules
                .unwrap_or(self.enable_noisy_rules),
            enable_deprecated_rules: overlay
                .enable_deprecated_rules
                .unwrap_or(self.enable_deprecated_rules),
            enable_unsupported_rules: overlay
                .enable_unsupported_rules
                .unwrap_or(self.enable_unsupported_rules),
            proven_rules: overlay.proven_rules.unwrap_or(self.proven_rules),
            time_offset: overlay
                .time_offset
                .clone()
                .or_else(|| self.time_offset.clone()),
            timeline_start: overlay
                .timeline_start
                .clone()
                .or_else(|| self.timeline_start.clone()),
            timeline_end: overlay
                .timeline_end
                .clone()
                .or_else(|| self.timeline_end.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TakajoConfig {
    pub bin: String,
    pub enabled: bool,
    pub level: Option<String>,
    pub display_table: bool,
}

impl Default for TakajoConfig {
    fn default() -> Self {
        Self {
            bin: "takajo".to_string(),
            enabled: true,
            level: None,
            display_table: false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TakajoOverlay {
    pub bin: Option<String>,
    pub enabled: Option<bool>,
    pub level: Option<String>,
    pub display_table: Option<bool>,
}

impl TakajoConfig {
    pub fn merge_overlay(&self, overlay: &TakajoOverlay) -> TakajoConfig {
        TakajoConfig {
            bin: overlay.bin.clone().unwrap_or_else(|| self.bin.clone()),
            enabled: overlay.enabled.unwrap_or(self.enabled),
            level: overlay.level.clone().or_else(|| self.level.clone()),
            display_table: overlay.display_table.unwrap_or(self.display_table),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProfileOverlay {
    pub hayabusa: HayabusaOverlay,
    pub takajo: TakajoOverlay,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExternalConfig {
    pub hayabusa: HayabusaConfig,
    pub takajo: TakajoConfig,
    pub profiles: HashMap<String, ProfileOverlay>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConfig {
    pub hayabusa: HayabusaConfig,
    pub takajo: TakajoConfig,
}

#[derive(Debug, PartialEq)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConfigError {}

impl ExternalConfig {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|e| ConfigError(format!("invalid config: {e}")))
    }

    /// Apply the named profile (additive overlay) on top of the base tables, then validate
    /// the result. `profile: None` resolves the base tables as-is (spec: Precedence).
    pub fn resolve(&self, profile: Option<&str>) -> Result<ResolvedConfig, ConfigError> {
        let (hayabusa, takajo) = match profile {
            Some(name) => {
                let overlay = self.profiles.get(name).ok_or_else(|| {
                    ConfigError(format!(
                        "unknown profile: {name} (available: {})",
                        self.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
                    ))
                })?;
                (
                    self.hayabusa.merge_overlay(&overlay.hayabusa),
                    self.takajo.merge_overlay(&overlay.takajo),
                )
            }
            None => (self.hayabusa.clone(), self.takajo.clone()),
        };
        if takajo.enabled && !hayabusa.json {
            return Err(ConfigError(
                "takajo.enabled = true requires hayabusa.json = true \
                 (takajo automagic needs Hayabusa's JSONL output)"
                    .to_string(),
            ));
        }
        Ok(ResolvedConfig { hayabusa, takajo })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_EXAMPLE: &str = r#"
[hayabusa]
bin = "hayabusa"
enabled = true
csv = true
json = true
rules = "./rules"
rules_config = "./rules/config"
min_level = "informational"
threads = 4
proven_rules = false

[takajo]
bin = "takajo"
enabled = true
display_table = false
"#;

    #[test]
    fn parses_the_full_example_and_applies_field_values() {
        let cfg = ExternalConfig::parse(FULL_EXAMPLE).unwrap();
        assert_eq!(cfg.hayabusa.bin, "hayabusa");
        assert!(cfg.hayabusa.enabled);
        assert!(cfg.hayabusa.csv);
        assert!(cfg.hayabusa.json);
        assert_eq!(cfg.hayabusa.rules.as_deref(), Some("./rules"));
        assert_eq!(cfg.hayabusa.min_level.as_deref(), Some("informational"));
        assert_eq!(cfg.hayabusa.threads, Some(4));
        assert!(!cfg.hayabusa.proven_rules);
        assert_eq!(cfg.takajo.bin, "takajo");
        assert!(cfg.takajo.enabled);
    }

    #[test]
    fn empty_config_text_resolves_to_documented_defaults() {
        let cfg = ExternalConfig::parse("").unwrap();
        assert_eq!(cfg.hayabusa.bin, "hayabusa");
        assert!(cfg.hayabusa.enabled);
        assert!(cfg.hayabusa.csv);
        assert!(cfg.hayabusa.json);
        assert_eq!(cfg.hayabusa.rules, None);
        assert_eq!(cfg.hayabusa.threads, None);
        assert_eq!(cfg.takajo.bin, "takajo");
        assert!(cfg.takajo.enabled);
        assert_eq!(cfg.takajo.level, None);
        assert!(!cfg.takajo.display_table);
    }

    #[test]
    fn malformed_toml_is_a_config_error() {
        let err = ExternalConfig::parse("this is not [valid toml").unwrap_err();
        assert!(err.0.contains("invalid config"), "got: {}", err.0);
    }

    const WITH_PROFILES: &str = r#"
[hayabusa]
rules = "./rules"
min_level = "informational"

[takajo]
enabled = true

[profiles.quick.hayabusa]
min_level = "high"
proven_rules = true

[profiles.quick.takajo]
enabled = false

[profiles.full-hunt.hayabusa]
enable_all_rules = true
enable_noisy_rules = true
"#;

    #[test]
    fn profile_overlay_overrides_only_the_fields_it_sets() {
        let cfg = ExternalConfig::parse(WITH_PROFILES).unwrap();
        let overlay = &cfg.profiles["quick"];
        let merged_hayabusa = cfg.hayabusa.merge_overlay(&overlay.hayabusa);
        // overridden
        assert_eq!(merged_hayabusa.min_level.as_deref(), Some("high"));
        assert!(merged_hayabusa.proven_rules);
        // inherited from the base table, untouched by the profile
        assert_eq!(merged_hayabusa.rules.as_deref(), Some("./rules"));
        assert!(merged_hayabusa.json); // base default, not mentioned in the profile

        let merged_takajo = cfg.takajo.merge_overlay(&overlay.takajo);
        assert!(!merged_takajo.enabled); // overridden
        assert_eq!(merged_takajo.bin, "takajo"); // inherited

        let full_hunt = &cfg.profiles["full-hunt"];
        let merged_full_hunt = cfg.hayabusa.merge_overlay(&full_hunt.hayabusa);
        assert!(merged_full_hunt.enable_all_rules);
        assert!(merged_full_hunt.enable_noisy_rules);
        assert_eq!(merged_full_hunt.timeline_start, None); // never set anywhere
    }

    #[test]
    fn resolve_with_no_profile_uses_the_base_tables() {
        let cfg = ExternalConfig::parse(FULL_EXAMPLE).unwrap();
        let resolved = cfg.resolve(None).unwrap();
        assert_eq!(
            resolved.hayabusa.min_level.as_deref(),
            Some("informational")
        );
        assert!(resolved.takajo.enabled);
    }

    #[test]
    fn resolve_with_a_named_profile_applies_the_overlay() {
        let cfg = ExternalConfig::parse(WITH_PROFILES).unwrap();
        let resolved = cfg.resolve(Some("quick")).unwrap();
        assert_eq!(resolved.hayabusa.min_level.as_deref(), Some("high"));
        assert!(!resolved.takajo.enabled);
    }

    #[test]
    fn resolve_with_an_unknown_profile_is_an_error() {
        let cfg = ExternalConfig::parse(WITH_PROFILES).unwrap();
        let err = cfg.resolve(Some("does-not-exist")).unwrap_err();
        assert!(err.0.contains("does-not-exist"), "got: {}", err.0);
    }

    #[test]
    fn takajo_enabled_without_hayabusa_json_is_a_config_error() {
        let text = r#"
[hayabusa]
json = false

[takajo]
enabled = true
"#;
        let cfg = ExternalConfig::parse(text).unwrap();
        let err = cfg.resolve(None).unwrap_err();
        assert!(err.0.contains("takajo"), "got: {}", err.0);
        assert!(err.0.contains("hayabusa.json"), "got: {}", err.0);
    }

    #[test]
    fn defaults_both_true_never_trip_the_validation_rule() {
        let cfg = ExternalConfig::parse("").unwrap();
        assert!(cfg.resolve(None).is_ok());
    }
}
