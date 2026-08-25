//! Map a shell item to SBECmd's ShellType string. Spellings are fixture-pinned
//! (Task 10); these cover every class type seen across the 10 reference fixtures.

use triage_shellitems::ShellItem;

pub fn shell_type_name(item: &ShellItem) -> String {
    match item.class_type {
        // 0x00 — variable item (FTP/HTTP container, property-view networks, etc.).
        // Sub-type 0xAFBB00B5 (network computer/workgroup, body[2..6]) → SBECmd
        // names these "Variable: Users property view" because they contain a
        // PropertyStore blob with System.ItemNameDisplay. All other 0x00 sub-types
        // get the generic "Variable" name.
        0x00 => {
            let sig = item
                .raw
                .get(2..6)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            if sig == Some(0xafbb00b5) {
                "Variable: Users property view".to_string()
            } else {
                "Variable".to_string()
            }
        }
        // 0x01 — control-panel category item (e.g. "User Accounts").
        0x01 => "Control Panel Category".to_string(),
        // 0x1F — root folder backed by a GUID (e.g. "This PC", "Quick Access").
        0x1F => "Root folder: GUID".to_string(),
        // 0x23 / 0x25 / 0x2F — drive letter or volume (e.g. "C:").
        0x23 | 0x25 | 0x2F => "Drive letter".to_string(),
        // 0x2E — root folder sub-item backed by a GUID (e.g. special folders under
        // "This PC" like Downloads, Documents, Desktop).  EZ-tools `ShellBag0x2e.cs`
        // names these "Root folder: GUID" — identical to 0x1F items.
        0x2E => "Root folder: GUID".to_string(),
        // 0x30..=0x3F — file/directory entries.
        // Bit 0x02 of the class byte distinguishes directory (clear) from file (set).
        0x30..=0x3F => {
            if item.class_type & 0x02 != 0 {
                "File".to_string()
            } else {
                "Directory".to_string()
            }
        }
        // 0x40..=0x4F — network location (share, UNC path, workgroup).
        0x40..=0x4F => "Network location".to_string(),
        // 0x07 / 0x52 / 0x7E — zip file contents (various class bytes used by
        // different Windows versions / zip formats for items inside archives).
        0x07 | 0x52 | 0x7E => "Zip file contents".to_string(),
        // 0x61 — URI item (ms-gamingoverlay://, etc.).
        0x61 => "URI".to_string(),
        // 0x71 — users property view (CLSID_SearchFolder, property-view items).
        0x71 => "Users property view".to_string(),
        // 0x74 — Users Files Folder / delegate item.
        // SBECmd names these "Variable: Users property view".
        0x74 => "Variable: Users property view".to_string(),
        // 0xC0..=0xFF — high-bit network variants (UNC share entries: class byte
        // has MSB set, indicating a "remote" or "connected" network resource).
        // SBECmd calls all of these "Network location" (same as 0x40..=0x4F).
        0xC0..=0xFF => "Network location".to_string(),
        // Unknown class: SBECmd renders "Type ID: 0x{byte_hex}" (uppercase hex).
        ct => format!("Type ID: 0x{ct:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(ct: u8) -> ShellItem {
        ShellItem {
            class_type: ct,
            value: String::new(),
            extension: None,
            modified: None,
            raw: vec![],
        }
    }

    #[test]
    fn names_match_sbecmd() {
        assert_eq!(shell_type_name(&item(0x00)), "Variable");
        assert_eq!(shell_type_name(&item(0x01)), "Control Panel Category");
        assert_eq!(shell_type_name(&item(0x1F)), "Root folder: GUID");
        assert_eq!(shell_type_name(&item(0x23)), "Drive letter");
        assert_eq!(shell_type_name(&item(0x2E)), "Root folder: GUID");
        assert_eq!(shell_type_name(&item(0x31)), "Directory");
        assert_eq!(shell_type_name(&item(0x32)), "File");
        assert_eq!(shell_type_name(&item(0x41)), "Network location");
        assert_eq!(shell_type_name(&item(0x07)), "Zip file contents");
        assert_eq!(shell_type_name(&item(0x52)), "Zip file contents");
        assert_eq!(shell_type_name(&item(0x7E)), "Zip file contents");
        assert_eq!(shell_type_name(&item(0x71)), "Users property view");
        assert_eq!(
            shell_type_name(&item(0x74)),
            "Variable: Users property view"
        );
        // Unknown types use "Type ID: 0xXX" (uppercase) to match SBECmd.
        assert_eq!(shell_type_name(&item(0x99)), "Type ID: 0x99");
        assert_eq!(shell_type_name(&item(0xAB)), "Type ID: 0xAB");
    }
}
