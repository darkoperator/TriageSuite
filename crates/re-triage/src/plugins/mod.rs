//! Plugin registry + RECmd GetPluginsToActivate auto-match. A plugin activates
//! when one of its key paths (lowercased, wildcard-aware) matches the current
//! key path, or its ValueName equals the entry's ValueName.

use triage_registry::plugin::RegistryPlugin;

// One module per ported plugin (Phase 2). Bam is the worked example.
pub mod app_compat_cache;
pub mod app_compat_flags2;
pub mod app_paths;
pub mod bam;
pub mod cid_size_mru;
pub mod device_classes;
pub mod etw;
pub mod file_exts;
pub mod firewall_rules;
pub mod first_folder;
pub mod icon_layouts;
pub mod jumplist_data;
pub mod known_networks;
pub mod last_visited_pidl_mru;
pub mod network_adapters;
pub mod network_setup2;
pub mod office_mru;
pub mod open_save_pidl_mru;
pub mod products;
pub mod profile_list;
pub mod radar;
pub mod recent_docs;
pub mod scsi;
pub mod services;
pub mod shimcache;
pub mod task_cache;
pub mod taskband;
pub mod time_zone_info;
pub mod trusted_documents;
pub mod typed_urls;
pub mod uninstall;
pub mod user_assist;
pub mod volume_info_cache;
pub mod windows_app;
pub mod word_wheel_query;

/// All ported plugins. Phase 2 appends one entry per plugin the captures fire.
pub fn registry() -> Vec<Box<dyn RegistryPlugin>> {
    vec![
        Box::new(bam::BamDam),
        Box::new(app_paths::AppPaths),
        Box::new(uninstall::UnInstall),
        Box::new(profile_list::ProfileList),
        Box::new(products::Products),
        Box::new(typed_urls::TypedURLs),
        Box::new(word_wheel_query::WordWheelQuery),
        Box::new(cid_size_mru::CIDSizeMRU),
        Box::new(first_folder::FirstFolder),
        Box::new(task_cache::TaskCache),
        Box::new(radar::Radar),
        Box::new(known_networks::KnownNetworks),
        Box::new(volume_info_cache::VolumeInfoCache),
        Box::new(services::Services),
        Box::new(firewall_rules::FirewallRules),
        Box::new(etw::Etw),
        Box::new(time_zone_info::TimeZoneInfo),
        Box::new(network_setup2::NetworkSetup2),
        Box::new(network_adapters::NetworkAdapters),
        Box::new(device_classes::DeviceClasses),
        Box::new(scsi::Scsi),
        Box::new(file_exts::FileExts),
        Box::new(office_mru::OfficeMru),
        Box::new(app_compat_flags2::AppCompatFlags2),
        Box::new(trusted_documents::TrustedDocuments),
        Box::new(icon_layouts::IconLayouts),
        Box::new(jumplist_data::JumplistData),
        Box::new(recent_docs::RecentDocs),
        Box::new(open_save_pidl_mru::OpenSavePidlMru),
        Box::new(last_visited_pidl_mru::LastVisitedPidlMru),
        Box::new(user_assist::UserAssist),
        Box::new(windows_app::WindowsApp),
        Box::new(app_compat_cache::AppCompatCache),
        Box::new(taskband::Taskband),
    ]
}

/// Plugins whose key paths match `key_path` (or whose ValueName matches
/// `entry_value_name`). Mirrors RECmd GetPluginsToActivate.
pub fn plugins_to_activate<'a>(
    plugins: &'a [Box<dyn RegistryPlugin>],
    key_path: &str,
    entry_value_name: Option<&str>,
) -> Vec<&'a dyn RegistryPlugin> {
    let kp = key_path.to_lowercase();
    plugins
        .iter()
        .filter(|p| {
            p.key_paths().iter().any(|kpat| {
                let pat = kpat.to_lowercase();
                if pat.contains('*') {
                    crate::batch::wildcard_match(&pat, &kp)
                } else {
                    pat == kp
                }
            }) || match (p.value_name(), entry_value_name) {
                (Some(pv), Some(ev)) => pv.eq_ignore_ascii_case(ev),
                _ => false,
            }
        })
        .map(|b| b.as_ref())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bam_matches_usersettings_path() {
        let plugins = registry();
        let hits = plugins_to_activate(
            &plugins,
            r"ControlSet001\Services\bam\State\UserSettings\S-1-5-21-1004336348",
            None,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].plugin_name(), "BamDam");
    }

    #[test]
    fn unrelated_path_no_match() {
        let plugins = registry();
        assert!(
            plugins_to_activate(&plugins, r"Microsoft\Windows\CurrentVersion\Run", None).is_empty()
        );
    }
}
