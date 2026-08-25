use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct EventMap {
    #[serde(rename = "Channel")]
    pub channel: String,
    #[serde(rename = "EventId")]
    pub event_id: u32,
    #[serde(rename = "Description", default)]
    pub description: String,
    #[serde(rename = "Maps", default)]
    pub entries: Vec<MapEntry>,
}

/// One entry in the `Maps:` list of a Zimmerman `.map` file.
///
/// Each entry names an output property (`Property`), a template string
/// (`PropertyValue`) that may contain `%varname%` placeholders, and a list of
/// XPath extractions (`Values`) whose results are substituted into the template.
#[derive(Debug, Deserialize)]
pub struct MapEntry {
    #[serde(rename = "Property")]
    pub property: String,
    #[serde(rename = "PropertyValue", default)]
    pub property_value: String,
    #[serde(rename = "Values", default)]
    pub values: Vec<MapValue>,
}

#[derive(Debug, Deserialize)]
pub struct MapValue {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Value")]
    pub value: String,
}

/// Resolve a single map entry against the raw event XML.
///
/// Extracts each named value via its XPath selector and substitutes `%name%`
/// placeholders in `PropertyValue`. Returns the filled template, or the first
/// extracted value when `PropertyValue` is empty.
pub fn resolve_entry(entry: &MapEntry, xml: &str) -> String {
    let mut result = entry.property_value.clone();
    for val in &entry.values {
        let extracted = normalize_binxml_value(&extract_xpath(xml, &val.value));
        result = result.replace(&format!("%{}%", val.name), &extracted);
    }
    if result.is_empty() {
        entry
            .values
            .first()
            .map(|v| normalize_binxml_value(&extract_xpath(xml, &v.value)))
            .unwrap_or_default()
    } else {
        result
    }
}

/// Render a binxml-derived scalar the way EvtxECmd does, so PayloadData and
/// Payload values match: hex integers (`0x…`) uppercased, GUIDs lowercased.
/// The `evtx` crate emits the opposite casing for both. Anything else is
/// returned unchanged.
pub(crate) fn normalize_binxml_value(s: &str) -> String {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return format!("0x{}", hex.to_ascii_uppercase());
        }
    }
    if is_guid(s) {
        return s.to_ascii_lowercase();
    }
    s.to_string()
}

/// True for a canonical 8-4-4-4-12 hyphenated GUID (hex digits + hyphens).
fn is_guid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    b.iter().enumerate().all(|(i, &c)| match i {
        8 | 13 | 18 | 23 => c == b'-',
        _ => c.is_ascii_hexdigit(),
    })
}

pub struct MapIndex {
    index: HashMap<(String, u32), EventMap>,
}

impl MapIndex {
    pub fn load(maps_dir: &Path) -> Self {
        let mut index = HashMap::new();

        if !maps_dir.is_dir() {
            return Self { index };
        }

        let entries = match std::fs::read_dir(maps_dir) {
            Ok(e) => e,
            Err(_) => return Self { index },
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("map") {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            match serde_yml::from_str::<EventMap>(&content) {
                Ok(map) => {
                    let key = (map.channel.clone(), map.event_id);
                    index.insert(key, map);
                }
                Err(e) => {
                    eprintln!("warning: skipping malformed map {}: {e}", path.display());
                }
            }
        }

        Self { index }
    }

    /// Build a `MapIndex` from `(name, yaml-content)` pairs — used for the
    /// compile-time-embedded corpus. Malformed maps warn to stderr and are skipped.
    pub fn from_contents<'a>(items: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut index = std::collections::HashMap::new();
        for (name, content) in items {
            match serde_yml::from_str::<EventMap>(content) {
                Ok(map) => {
                    index.insert((map.channel.clone(), map.event_id), map);
                }
                Err(e) => eprintln!("warning: skipping malformed map {name}: {e}"),
            }
        }
        Self { index }
    }

    pub fn lookup(&self, channel: &str, event_id: u32) -> Option<&EventMap> {
        self.index.get(&(channel.to_owned(), event_id))
    }
}

// Evaluates a limited subset of XPath used in Zimmerman Maps:
//   /Event/System/Computer
//   /Event/EventData/Data[@Name="SubjectUserName"]
pub fn extract_xpath(xml: &str, xpath: &str) -> String {
    extract_xpath_inner(xml, xpath).unwrap_or_default()
}

fn extract_xpath_inner(xml: &str, xpath: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();

    let segments: Vec<&str> = xpath.trim_start_matches('/').split('/').collect();

    // Skip first segment if it matches the root element local name
    let start = if segments
        .first()
        .is_some_and(|s| parse_segment_name(s) == root.tag_name().name())
    {
        1
    } else {
        0
    };

    let mut node = root;
    for seg in &segments[start..] {
        let (name, attr_filter) = parse_segment(seg);
        node = node
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == name)
            .find(|n| match attr_filter {
                Some((attr_name, attr_val)) => n.attribute(attr_name) == Some(attr_val),
                None => true,
            })?;
    }

    Some(node.text().unwrap_or("").to_string())
}

fn parse_segment_name(segment: &str) -> &str {
    segment.split('[').next().unwrap_or(segment)
}

// Returns (element_name, Option<(attr_name, attr_value)>)
fn parse_segment(segment: &str) -> (&str, Option<(&str, &str)>) {
    if let Some(bracket_pos) = segment.find('[') {
        let name = &segment[..bracket_pos];
        let predicate = &segment[bracket_pos + 1..segment.len() - 1]; // strip [ and ]
                                                                      // predicate: @Name="SubjectUserName"
        if let Some(eq_pos) = predicate.find('=') {
            let attr_name = predicate[1..eq_pos].trim(); // strip leading @
            let attr_val = predicate[eq_pos + 1..].trim().trim_matches('"');
            return (name, Some((attr_name, attr_val)));
        }
    }
    (segment, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    const SAMPLE_MAP_YAML: &str = r#"
Author: Test
Channel: Security
EventId: 4624
Description: An account was successfully logged on
Maps:
  -
    Property: PayloadData1
    PropertyValue: "%username%"
    Values:
      -
        Name: username
        Value: /Event/EventData/Data[@Name="SubjectUserName"]
  -
    Property: PayloadData2
    PropertyValue: "%computer%"
    Values:
      -
        Name: computer
        Value: /Event/System/Computer
"#;

    const SAMPLE_XML: &str = r#"<Event xmlns="http://schemas.microsoft.com/win/2004/08/events/event">
  <System>
    <Provider Name="Microsoft-Windows-Security-Auditing"/>
    <EventID>4624</EventID>
    <Channel>Security</Channel>
    <Computer>WORKSTATION01</Computer>
    <TimeCreated SystemTime="2026-01-01T00:00:00.000000Z"/>
    <EventRecordID>1</EventRecordID>
  </System>
  <EventData>
    <Data Name="SubjectUserName">SYSTEM</Data>
    <Data Name="SubjectDomainName">NT AUTHORITY</Data>
    <Data Name="SubjectLogonId">0x3e7</Data>
  </EventData>
</Event>"#;

    #[test]
    fn loads_map_from_directory() {
        let dir = tempdir().unwrap();
        let map_path = dir.path().join("Security_4624.map");
        let mut f = std::fs::File::create(&map_path).unwrap();
        write!(f, "{SAMPLE_MAP_YAML}").unwrap();

        let index = MapIndex::load(dir.path());
        let map = index.lookup("Security", 4624);
        assert!(map.is_some());
        assert_eq!(
            map.unwrap().description,
            "An account was successfully logged on"
        );
        assert_eq!(map.unwrap().entries.len(), 2);
    }

    #[test]
    fn lookup_returns_none_for_unknown_event() {
        let dir = tempdir().unwrap();
        let index = MapIndex::load(dir.path());
        assert!(index.lookup("Security", 9999).is_none());
    }

    #[test]
    fn empty_index_for_nonexistent_dir() {
        let index = MapIndex::load(Path::new("/nonexistent/maps/dir"));
        assert!(index.lookup("Security", 4624).is_none());
    }

    #[test]
    fn resolve_entry_substitutes_template() {
        let entry = MapEntry {
            property: "PayloadData1".into(),
            property_value: "Target: %domain%\\%user%".into(),
            values: vec![
                MapValue {
                    name: "domain".into(),
                    value: r#"/Event/EventData/Data[@Name="SubjectDomainName"]"#.into(),
                },
                MapValue {
                    name: "user".into(),
                    value: r#"/Event/EventData/Data[@Name="SubjectUserName"]"#.into(),
                },
            ],
        };
        let result = resolve_entry(&entry, SAMPLE_XML);
        assert_eq!(result, "Target: NT AUTHORITY\\SYSTEM");
    }

    #[test]
    fn extract_xpath_data_by_attribute() {
        let val = extract_xpath(
            SAMPLE_XML,
            r#"/Event/EventData/Data[@Name="SubjectUserName"]"#,
        );
        assert_eq!(val, "SYSTEM");
    }

    #[test]
    fn extract_xpath_simple_path() {
        let val = extract_xpath(SAMPLE_XML, "/Event/System/Computer");
        assert_eq!(val, "WORKSTATION01");
    }

    #[test]
    fn extract_xpath_missing_returns_empty() {
        let val = extract_xpath(SAMPLE_XML, r#"/Event/EventData/Data[@Name="NoSuchField"]"#);
        assert_eq!(val, "");
    }
}
