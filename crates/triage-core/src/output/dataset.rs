use std::io::Write;

/// JSON framing per tool, matching the corresponding Zimmerman tool
/// (spec section 5.2). `--pretty` changes whitespace only, and only applies
/// to Array framing — NDJSON framing is line-delimited by definition, so
/// pretty-printing is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonFraming {
    /// One JSON object per line.
    Ndjson,
    /// A single JSON array of records.
    Array,
}

/// A named output dataset a tool can emit (e.g. AutomaticDestinations).
#[derive(Debug, Clone, Copy)]
pub struct DatasetSpec {
    /// Stable id used by parsers when routing records.
    pub id: &'static str,
    /// Default output basename without extension, e.g. "StubTriage_Output".
    pub default_basename: &'static str,
    pub framing: JsonFraming,
    /// True for datasets the reference tool emits as CSV only (no JSON file).
    pub csv_only: bool,
    /// Override naming: None = primary dataset, receives --csvf/--jsonf
    /// verbatim; Some(suffix) = derived name `{stem}{suffix}{ext}`
    /// (PECmd: `--csvf foo.csv` -> `foo.csv` + `foo_Timeline.csv`).
    pub override_suffix: Option<&'static str>,
}

/// CSV writer over any io::Write; headers come from serde field renames.
pub struct CsvSink<W: Write> {
    inner: csv::Writer<W>,
}

impl<W: Write> CsvSink<W> {
    pub fn new(w: W) -> Self {
        Self {
            inner: csv::Writer::from_writer(w),
        }
    }

    pub fn write<T: serde::Serialize>(&mut self, record: &T) -> Result<(), std::io::Error> {
        self.inner.serialize(record).map_err(std::io::Error::other)
    }

    pub fn finish(mut self) -> Result<(), std::io::Error> {
        self.inner.flush()
    }
}

/// JSON writer over any io::Write with NDJSON or array framing.
///
/// Unset (JSON null) properties are omitted from each record, matching the
/// Zimmerman tools, which serialize records with ServiceStack.Text's
/// null-omitting `ToJson()` (e.g. PECmd Program.cs). Empty strings are NOT
/// omitted: PECmd distinguishes a C# null property (omitted) from an
/// empty-string one (kept), e.g. a 1601-blanked Volume0Created stays `""`.
pub struct JsonSink<W: Write> {
    out: W,
    framing: JsonFraming,
    pretty: bool,
    count: u64,
}

impl<W: Write> JsonSink<W> {
    pub fn new(out: W, framing: JsonFraming, pretty: bool) -> Self {
        Self {
            out,
            framing,
            pretty,
            count: 0,
        }
    }

    pub fn write<T: serde::Serialize>(&mut self, record: &T) -> Result<(), std::io::Error> {
        let mut value = serde_json::to_value(record)?;
        if let serde_json::Value::Object(map) = &mut value {
            // ServiceStack ToJson equivalence: TOP-LEVEL null properties are
            // omitted; empty strings and nested nulls are kept. Load-bearing
            // contract: WinTimestamp::none() serializes as null, so unset
            // timestamp fields (e.g. PreviousRunN) vanish from JSON here while
            // still rendering as bare empty CSV cells. A field that must
            // appear as JSON null cannot be a top-level Option/none type.
            map.retain(|_, v| !v.is_null());
        }
        let body = if self.pretty && self.framing == JsonFraming::Array {
            serde_json::to_string_pretty(&value)?
        } else {
            serde_json::to_string(&value)?
        };
        match self.framing {
            JsonFraming::Ndjson => writeln!(self.out, "{body}")?,
            JsonFraming::Array => {
                if self.count == 0 {
                    write!(self.out, "[")?;
                } else {
                    write!(self.out, ",")?;
                }
                if self.pretty {
                    write!(self.out, "\n{body}")?;
                } else {
                    write!(self.out, "{body}")?;
                }
            }
        }
        self.count += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), std::io::Error> {
        if self.framing == JsonFraming::Array {
            if self.count == 0 {
                write!(self.out, "[")?;
            }
            if self.pretty {
                writeln!(self.out, "\n]")?;
            } else {
                writeln!(self.out, "]")?;
            }
        }
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Row {
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "SourceFile")]
        source_file: String,
    }

    fn row(n: &str) -> Row {
        Row {
            name: n.into(),
            source_file: "/cap/a".into(),
        }
    }

    #[test]
    fn csv_has_renamed_headers_and_rows() {
        let mut buf = Vec::new();
        {
            let mut w = CsvSink::new(&mut buf);
            w.write(&row("alpha")).unwrap();
            w.write(&row("beta")).unwrap();
            w.finish().unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(text, "Name,SourceFile\nalpha,/cap/a\nbeta,/cap/a\n");
    }

    #[test]
    fn ndjson_framing_one_object_per_line() {
        let mut buf = Vec::new();
        {
            let mut w = JsonSink::new(&mut buf, JsonFraming::Ndjson, false);
            w.write(&row("alpha")).unwrap();
            w.write(&row("beta")).unwrap();
            w.finish().unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(
            text,
            "{\"Name\":\"alpha\",\"SourceFile\":\"/cap/a\"}\n{\"Name\":\"beta\",\"SourceFile\":\"/cap/a\"}\n"
        );
    }

    #[test]
    fn json_omits_null_properties_but_keeps_empty_strings() {
        // Mirrors ServiceStack.Text's ToJson(): null omitted, "" kept,
        // property order = declaration order.
        #[derive(Serialize)]
        struct Sparse {
            #[serde(rename = "A")]
            a: String,
            #[serde(rename = "B")]
            b: Option<String>,
            #[serde(rename = "C")]
            c: String,
        }
        let mut buf = Vec::new();
        {
            let mut w = JsonSink::new(&mut buf, JsonFraming::Ndjson, false);
            w.write(&Sparse {
                a: "x".into(),
                b: None,
                c: String::new(),
            })
            .unwrap();
            w.write(&Sparse {
                a: "y".into(),
                b: Some("z".into()),
                c: "w".into(),
            })
            .unwrap();
            w.finish().unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(
            text,
            "{\"A\":\"x\",\"C\":\"\"}\n{\"A\":\"y\",\"B\":\"z\",\"C\":\"w\"}\n"
        );
    }

    #[test]
    fn array_framing_produces_valid_json_array() {
        let mut buf = Vec::new();
        {
            let mut w = JsonSink::new(&mut buf, JsonFraming::Array, false);
            w.write(&row("alpha")).unwrap();
            w.write(&row("beta")).unwrap();
            w.finish().unwrap();
        }
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["Name"], "alpha");
    }

    #[test]
    fn empty_array_framing_is_valid_json() {
        let mut buf = Vec::new();
        {
            let w = JsonSink::new(&mut buf, JsonFraming::Array, false);
            w.finish().unwrap();
        }
        let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 0);
    }

    #[test]
    fn pretty_is_ignored_for_ndjson_framing() {
        let mut buf = Vec::new();
        {
            let mut w = JsonSink::new(&mut buf, JsonFraming::Ndjson, true);
            w.write(&row("alpha")).unwrap();
            w.finish().unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        assert_eq!(text.lines().count(), 1);
        let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["Name"], "alpha");
    }

    #[test]
    fn pretty_changes_whitespace_only() {
        let mut plain = Vec::new();
        let mut pretty = Vec::new();
        {
            let mut w = JsonSink::new(&mut plain, JsonFraming::Array, false);
            w.write(&row("alpha")).unwrap();
            w.finish().unwrap();
            let mut w = JsonSink::new(&mut pretty, JsonFraming::Array, true);
            w.write(&row("alpha")).unwrap();
            w.finish().unwrap();
        }
        let a: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        let b: serde_json::Value = serde_json::from_slice(&pretty).unwrap();
        assert_eq!(a, b);
        assert!(pretty.len() > plain.len());
    }
}
