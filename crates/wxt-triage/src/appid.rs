//! Decode the two JSON-bearing columns WxTCmd interprets:
//! * `AppId` — a JSON array of `{Application, Platform}` -> the Executable.
//! * `Payload`/`ClipboardPayload` — a JSON object -> DisplayText, ContentInfo,
//!   DevicePlatform, TimeZone.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct AppIdInfo {
    #[serde(rename = "application", alias = "Application")]
    application: String,
    #[serde(rename = "platform", alias = "Platform")]
    platform: String,
}

/// Fields parsed from a Timeline payload JSON object. Absent keys -> empty.
#[derive(Debug, Default, Deserialize)]
struct PayloadFields {
    #[serde(rename = "displayText", alias = "DisplayText", default)]
    pub display_text: String,
    #[serde(rename = "appDisplayName", alias = "AppDisplayName", default)]
    pub app_display_name: String,
    #[serde(rename = "description", alias = "Description", default)]
    pub description: Option<String>,
    #[serde(rename = "contentUri", alias = "ContentUri", default)]
    pub content_uri: Option<String>,
    #[serde(rename = "devicePlatform", alias = "DevicePlatform", default)]
    pub device_platform: String,
    #[serde(rename = "userTimezone", alias = "UserTimezone", default)]
    pub user_timezone: String,
}

/// Replace a leading `{GUID}` path segment with its GuidMapping description
/// (rejoining the rest of the `\`-separated path). If the first segment is not
/// a brace GUID or is unmapped, the input is returned unchanged.
fn map_leading_guid_segment(app: &str) -> String {
    let mut segs: Vec<String> = app.split('\\').map(|s| s.to_string()).collect();
    if let Some(first) = segs.first() {
        if first.starts_with('{') {
            if let Some(desc) = triage_guidmap::description_for(first) {
                segs[0] = desc.to_string();
                return segs.join("\\");
            }
        }
    }
    app.to_string()
}

/// Derive the Executable from an `AppId` JSON array, following WxTCmd's
/// Activity-table preference order: `windows_win32`/`x_exe_path`, else
/// `windows_universal`, else the first entry. A leading `{GUID}` segment in the
/// chosen Application is mapped via GuidMapping. Returns empty on parse failure
/// or empty array.
pub fn executable_from_appid(app_id_json: &str) -> String {
    let infos: Vec<AppIdInfo> = match serde_json::from_str(app_id_json) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    if infos.is_empty() {
        return String::new();
    }
    let chosen = infos
        .iter()
        .find(|t| t.platform == "windows_win32" || t.platform == "x_exe_path")
        .or_else(|| infos.iter().find(|t| t.platform == "windows_universal"))
        .unwrap_or(&infos[0]);
    map_leading_guid_segment(&chosen.application)
}

/// WxTCmd's ActivityOperation-table Executable rule differs slightly: it only
/// maps when the chosen Application contains ".exe" (otherwise the raw
/// Application string is kept). `windows_win32`/`x_exe_path` first, else first.
pub fn executable_from_appid_operation(app_id_json: &str) -> String {
    let infos: Vec<AppIdInfo> = match serde_json::from_str(app_id_json) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    if infos.is_empty() {
        return String::new();
    }
    let chosen = infos
        .iter()
        .find(|t| t.platform == "windows_win32" || t.platform == "x_exe_path")
        .unwrap_or(&infos[0]);
    if chosen.application.contains(".exe") {
        map_leading_guid_segment(&chosen.application)
    } else {
        chosen.application.clone()
    }
}

/// The DisplayText / ContentInfo / DevicePlatform / TimeZone derived from a
/// payload. `payload` is the ASCII-decoded column. Returns the rendered payload
/// string too: the input if it is JSON, else the literal "(Binary data)".
pub struct DecodedPayload {
    pub rendered_payload: String,
    pub display_text: String,
    pub content_info: String,
    pub device_platform: String,
    pub time_zone: String,
}

/// Parse a payload exactly as WxTCmd does.
pub fn decode_payload(payload: &str) -> DecodedPayload {
    if !payload.starts_with('{') {
        return DecodedPayload {
            rendered_payload: "(Binary data)".to_string(),
            display_text: String::new(),
            content_info: String::new(),
            device_platform: String::new(),
            time_zone: String::new(),
        };
    }
    let f: PayloadFields = match serde_json::from_str(payload) {
        Ok(v) => v,
        // Starts with '{' but not valid JSON: WxTCmd would throw; emit empty
        // fields but keep the original payload text.
        Err(_) => {
            return DecodedPayload {
                rendered_payload: payload.to_string(),
                display_text: String::new(),
                content_info: String::new(),
                device_platform: String::new(),
                time_zone: String::new(),
            };
        }
    };

    let mut display_text = f.display_text.clone();
    let mut content_info = String::new();

    if f.content_uri.is_some() || f.description.is_some() {
        display_text = format!("{} ({})", f.display_text, f.app_display_name);
        let raw_uri = f.content_uri.clone().unwrap_or_default();
        let decoded = url_decode(&raw_uri);
        let desc = f.description.clone().unwrap_or_default();
        content_info = format!("{desc} ({decoded})");
        // ContentUri GUID substitution: WxTCmd checks for a `{...}` and replaces
        // a 36-char GUID at offset 6 (i.e. "abcde{GUID}rest"). Byte offsets are
        // only sliced when they fall on char boundaries — a multi-byte UTF-8
        // ContentUri skips substitution rather than panicking (forensic tools
        // must not crash on a single malformed record).
        if decoded.contains('{')
            && decoded.contains('}')
            && decoded.len() >= 43
            && decoded.is_char_boundary(5)
            && decoded.is_char_boundary(6)
            && decoded.is_char_boundary(42)
            && decoded.is_char_boundary(43)
        {
            let start = &decoded[0..5];
            let guid = &decoded[6..42];
            let end = &decoded[43..];
            let mapped = triage_guidmap::description_for(guid).unwrap_or(guid);
            content_info = format!("{desc} ({start}{mapped}{end})");
        }
    }

    DecodedPayload {
        rendered_payload: payload.to_string(),
        display_text,
        content_info,
        device_platform: f.device_platform,
        time_zone: f.user_timezone,
    }
}

/// Minimal percent-decoding for ContentUri (matches ServiceStack `UrlDecode`
/// for the byte sequences seen in Timeline URIs).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_prefers_win32_platform() {
        let j = r#"[{"Application":"firefox.exe","Platform":"windows_universal"},
                    {"Application":"C:\\app\\foo.exe","Platform":"windows_win32"}]"#;
        assert_eq!(executable_from_appid(j), "C:\\app\\foo.exe");
    }

    #[test]
    fn executable_falls_back_to_first_when_no_preferred() {
        let j = r#"[{"Application":"only.exe","Platform":"some_other"}]"#;
        assert_eq!(executable_from_appid(j), "only.exe");
    }

    #[test]
    fn executable_empty_on_bad_json() {
        assert_eq!(executable_from_appid("not json"), "");
        assert_eq!(executable_from_appid("[]"), "");
    }

    #[test]
    fn operation_executable_keeps_non_exe_raw() {
        let j = r#"[{"Application":"Microsoft.Windows.Shell","Platform":"windows_win32"}]"#;
        assert_eq!(
            executable_from_appid_operation(j),
            "Microsoft.Windows.Shell"
        );
    }

    #[test]
    fn payload_binary_when_not_json() {
        let d = decode_payload("AAAAraw bytes");
        assert_eq!(d.rendered_payload, "(Binary data)");
        assert_eq!(d.display_text, "");
    }

    #[test]
    fn payload_json_extracts_fields() {
        let p = r#"{"displayText":"My Doc","devicePlatform":"Windows","userTimezone":"America/New_York"}"#;
        let d = decode_payload(p);
        assert_eq!(d.rendered_payload, p);
        assert_eq!(d.display_text, "My Doc");
        assert_eq!(d.device_platform, "Windows");
        assert_eq!(d.time_zone, "America/New_York");
        assert_eq!(d.content_info, "");
    }

    #[test]
    fn payload_with_content_uri_builds_content_info_and_displaytext() {
        let p = r#"{"displayText":"Doc","appDisplayName":"Word","description":"open",
                    "contentUri":"file:///C:/x.docx"}"#;
        let d = decode_payload(p);
        assert_eq!(d.display_text, "Doc (Word)");
        assert_eq!(d.content_info, "open (file:///C:/x.docx)");
    }

    #[test]
    fn content_uri_with_multibyte_does_not_panic() {
        // The decoded ContentUri starts with three 2-byte `é` chars, so byte
        // offset 5 (a guarded slice boundary) falls in the MIDDLE of a char.
        // Without the is_char_boundary guard, `&decoded[0..5]` panics. With it,
        // substitution is skipped and the un-substituted content_info returned.
        let p = r#"{"description":"d","contentUri":"ééé{6D809377-6AF0-444B-8957-A3773F02200E}/x"}"#;
        let d = decode_payload(p);
        // No panic. Substitution skipped → content_info is the un-substituted
        // "d (<decoded uri>)" form (note: url_decode leaves the literal text).
        assert!(d.content_info.starts_with("d ("));
        assert!(d.content_info.contains("ééé{"));
    }
}
