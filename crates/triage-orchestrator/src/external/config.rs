use serde::Deserialize;
use std::collections::HashMap;

/// Pick a required field: the overlay wins whenever it set the field at all,
/// including when it set it to the same value as the base.
///
/// Generic rather than inlined into the macro so the `.clone()` is on an opaque
/// `T` — an inlined `overlay.enabled.clone()` on an `Option<bool>` would trip
/// clippy's `clone_on_copy` under CI's `-D warnings`.
fn pick<T: Clone>(overlay: &Option<T>, base: &T) -> T {
    overlay.clone().unwrap_or_else(|| base.clone())
}

/// Pick an optional field. An overlay that leaves it unset falls through to the
/// base; there is deliberately no way to un-set a base value from an overlay,
/// matching the "additive per-field merge" contract.
fn pick_opt<T: Clone>(overlay: &Option<T>, base: &Option<T>) -> Option<T> {
    overlay.clone().or_else(|| base.clone())
}

/// Declare one external tool's TOML table as a single field list, generating the
/// three types that always travel together: the typed config struct with concrete
/// defaults, the all-`Option` profile-overlay struct, and the additive per-field
/// `merge_overlay`.
///
/// `req <name>: <ty> = <default>;` is a field with a built-in default — the tool
/// always receives a value. `opt <name>: <ty>;` is a field that means "don't pass
/// that flag" when unset, so the tool's own default applies and the schema doesn't
/// drift when the tool changes it across versions.
///
/// Field declaration order is preserved verbatim, because `deny_unknown_fields`'
/// "expected one of `bin`, `enabled`, ..." error text enumerates fields in
/// declaration order and that text is user-facing.
macro_rules! tool_table {
    ($cfg:ident / $ovl:ident { $($body:tt)* }) => {
        tool_table!(@munch $cfg / $ovl ; [] [] [] [] ; $($body)*);
    };

    (@munch $cfg:ident / $ovl:ident ;
     [$($cf:tt)*] [$($of:tt)*] [$($df:tt)*] [$($mf:tt)*] ;
     req $field:ident : $ty:ty = $default:expr ; $($rest:tt)*) => {
        tool_table!(@munch $cfg / $ovl ;
            [$($cf)* pub $field: $ty,]
            [$($of)* pub $field: Option<$ty>,]
            [$($df)* $field: $default,]
            [$($mf)* $field = pick,]
            ; $($rest)*);
    };

    (@munch $cfg:ident / $ovl:ident ;
     [$($cf:tt)*] [$($of:tt)*] [$($df:tt)*] [$($mf:tt)*] ;
     opt $field:ident : $ty:ty ; $($rest:tt)*) => {
        tool_table!(@munch $cfg / $ovl ;
            [$($cf)* pub $field: Option<$ty>,]
            [$($of)* pub $field: Option<$ty>,]
            [$($df)* $field: None,]
            [$($mf)* $field = pick_opt,]
            ; $($rest)*);
    };

    // Terminal arm. The merge accumulator is matched as `field = picker,` pairs
    // rather than as a finished expression: `self` and `overlay` may only be
    // written in the same expansion as the `fn merge_overlay` that binds them,
    // and each recursive `@munch` step is a separate expansion.
    (@munch $cfg:ident / $ovl:ident ;
     [$($cf:tt)*] [$($of:tt)*] [$($df:tt)*]
     [$($mfield:ident = $mpick:ident,)*] ; ) => {
        #[derive(Debug, Clone, PartialEq, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $cfg { $($cf)* }

        impl Default for $cfg {
            fn default() -> Self {
                Self { $($df)* }
            }
        }

        #[derive(Debug, Clone, Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $ovl { $($of)* }

        impl $cfg {
            /// Additive per-field merge: only fields set (`Some`) in `overlay` override
            /// this base value; everything else falls through unchanged.
            pub fn merge_overlay(&self, overlay: &$ovl) -> $cfg {
                $cfg {
                    $( $mfield: $mpick(&overlay.$mfield, &self.$mfield), )*
                }
            }
        }
    };
}

tool_table! {
    HayabusaConfig / HayabusaOverlay {
        req bin: String = "hayabusa".to_string();
        req enabled: bool = true;
        req csv: bool = true;
        req json: bool = true;
        req logon_summary: bool = true;
        opt rules: String;
        opt rules_config: String;
        opt min_level: String;
        opt profile: String;
        opt threads: u32;
        req clobber: bool = false;
        req sort: bool = false;
        req scan_all_evtx_files: bool = false;
        req enable_all_rules: bool = false;
        req enable_noisy_rules: bool = false;
        req enable_deprecated_rules: bool = false;
        req enable_unsupported_rules: bool = false;
        req proven_rules: bool = false;
        opt time_offset: String;
        opt timeline_start: String;
        opt timeline_end: String;
    }
}

tool_table! {
    TakajoConfig / TakajoOverlay {
        req bin: String = "takajo".to_string();
        req enabled: bool = true;
        opt level: String;
        req display_table: bool = false;
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

#[derive(Debug, Clone, Default, PartialEq)]
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
    /// the result. `profile: None` resolves the base tables as-is.
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
        let resolved = ResolvedConfig { hayabusa, takajo };
        // Each tool validates its own rules against the fully-merged config; a
        // rule that spans two tables (Takajo needing Hayabusa's JSONL) belongs to
        // whichever tool the requirement is *for*. Registry order decides which
        // error surfaces first, so it is deterministic.
        for tool in crate::external::registry::ALL {
            tool.validate(&resolved)?;
        }
        Ok(resolved)
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

    /// The `tool_table!` macro must not cost anything in error quality: a typo'd
    /// field still has to name itself, list the valid fields in declaration order,
    /// and point at the line in the user's file. This is why the macro generates
    /// typed structs rather than routing tables through `toml::Value`, which
    /// would discard the span.
    #[test]
    fn unknown_field_error_names_the_field_the_alternatives_and_the_line() {
        let err = ExternalConfig::parse("[hayabusa]\nbin = \"h\"\nrulez = \"./r\"\n").unwrap_err();
        assert!(err.0.contains("rulez"), "got: {}", err.0);
        assert!(err.0.contains("unknown field"), "got: {}", err.0);
        assert!(err.0.contains("line 3"), "got: {}", err.0);
        // Declaration order, which is what the macro's field list preserves.
        let alternatives = err.0.find("expected one of").expect("alternatives listed");
        let tail = &err.0[alternatives..];
        let bin = tail.find("`bin`").expect("bin listed");
        let enabled = tail.find("`enabled`").expect("enabled listed");
        let rules = tail.find("`rules`").expect("rules listed");
        assert!(bin < enabled && enabled < rules, "got: {}", err.0);
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
