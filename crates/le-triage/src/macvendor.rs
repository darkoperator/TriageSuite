//! MAC OUI -> vendor lookup (LECmd Resources/MACs.txt parity).

use std::collections::HashMap;
use std::sync::OnceLock;

/// The bundled IEEE OUI table (tab-separated `OUIHEX<TAB>Vendor` per line).
const MACS_TXT: &str = include_str!("../../../resources/lnk/MACs.txt");

fn table() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut m = HashMap::new();
        for line in MACS_TXT.lines() {
            if let Some((oui, vendor)) = line.split_once('\t') {
                let oui = oui.trim();
                if !oui.is_empty() {
                    // LECmd LoadMacs (Program.cs:85) does line.ToUpperInvariant()
                    // on the WHOLE line before splitting, so both the key AND the
                    // vendor value are uppercased — MACVendor renders e.g.
                    // "MICROSOFT", not "Microsoft". Match that exactly.
                    m.entry(oui.to_uppercase())
                        .or_insert_with(|| vendor.trim().to_uppercase());
                }
            }
        }
        m
    })
}

/// Look up the vendor for a MAC address string like "00:14:22:0d:94:04".
/// Key = first three octets, uppercased, separators removed (LECmd semantics:
/// Program.cs GetVendorFromMac). Returns "(Unknown vendor)" on miss.
pub fn vendor_for_mac(mac: &str) -> String {
    if mac.is_empty() {
        return "(Unknown vendor)".to_string();
    }
    let key: String = mac
        .split(':')
        .take(3)
        .collect::<Vec<_>>()
        .join("")
        .to_uppercase();
    table()
        .get(&key)
        .cloned()
        .unwrap_or_else(|| "(Unknown vendor)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_oui_resolves() {
        // 3CD92B -> "HP" (verified via grep '^3CD92B' resources/lnk/MACs.txt)
        assert_eq!(vendor_for_mac("3C:D9:2B:00:00:00"), "HP");
    }

    #[test]
    fn hyperv_oui_resolves_uppercased() {
        // 00155D -> "Microsoft" in the file; LECmd uppercases the whole line,
        // so MACVendor renders "MICROSOFT" (matches the LECmd fixture exactly).
        assert_eq!(vendor_for_mac("00:15:5d:7c:01:15"), "MICROSOFT");
    }

    #[test]
    fn unknown_oui_is_unknown_vendor() {
        assert_eq!(vendor_for_mac("FF:FF:FF:00:00:00"), "(Unknown vendor)");
    }

    #[test]
    fn empty_mac_is_unknown() {
        assert_eq!(vendor_for_mac(""), "(Unknown vendor)");
    }
}
