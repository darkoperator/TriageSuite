//! The output row types — the contract between the Chromium and Firefox
//! parsers.
//!
//! One struct per dataset, shared by both families, rather than a row type per
//! family. If each family owned its own row type the columns would drift, which
//! is precisely how a unified-event model ends up with an untyped attribute bag
//! papering over the difference. Sharing forces the union to be explicit: a
//! column only one family has is `Option`/empty for the other, and the doc
//! comment says which.
//!
//! Serde field declaration order is CSV column order. Every record opens with
//! `Browser`, `Browser Channel`, `Profile` and closes with `Notes`,
//! `Source File` — discriminators first because `Scope::UserSpecific` merges
//! every browser and profile of one user into a single file, which is
//! unreadable without them on the left.
//!
//! The `csv` crate does not support `#[serde(flatten)]`, so those five columns
//! are literal fields in every struct rather than a nested type. A
//! header-contract test holds them in line.

use serde::Serialize;
use triage_core::timestamp::WinTimestamp;

/// Chromium `urls` + `visits`, Firefox `moz_places` + `moz_historyvisits`.
///
/// One row per visit, plus one row per URL that has no surviving visit rows —
/// see `Record Type`. A `urls` row with `visit_count = 12` and no `visits` rows
/// is a deletion indicator and must never be dropped.
#[derive(Debug, Default, Serialize)]
pub struct HistoryRecord {
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Browser Channel")]
    pub channel: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    /// `Visit` for a `visits` / `moz_historyvisits` row; `URL Only` for a
    /// `urls` / `moz_places` row whose visit rows are gone.
    #[serde(rename = "Record Type")]
    pub record_type: &'static str,
    /// `visits.visit_time` (WebKit us) / `moz_historyvisits.visit_date`
    /// (PRTime us). Empty on `URL Only` rows.
    #[serde(rename = "Visit Time")]
    pub visit_time: WinTimestamp,
    /// `urls.url` / `moz_places.url`. Empty when a visit's URL foreign key is
    /// dangling, which `Notes` then records.
    #[serde(rename = "URL")]
    pub url: String,
    /// `urls.title` / `moz_places.title`.
    #[serde(rename = "Title")]
    pub title: String,
    /// Decoded core transition type / `moz_historyvisits.visit_type`.
    #[serde(rename = "Visit Type")]
    pub visit_type: String,
    /// Decoded qualifier bits of `visits.transition`, `|`-joined. Chromium only.
    #[serde(rename = "Transition Qualifiers")]
    pub transition_qualifiers: String,
    /// The raw, undecoded transition value, so a qualifier this crate does not
    /// yet know about is still recoverable from the output.
    #[serde(rename = "Transition Raw")]
    pub transition_raw: Option<i64>,
    /// Decoded `moz_historyvisits.source`: how the navigation was initiated
    /// (`Organic`, `Sponsored`, `Bookmarked`, `Searched`). Firefox only, and
    /// only on a visit row — it describes the navigation, not where the record
    /// came from, so it says nothing about syncing or importing.
    #[serde(rename = "Visit Source")]
    pub visit_source: String,
    /// `visits.visit_duration` (microseconds) rendered as seconds. Chromium only.
    #[serde(rename = "Visit Duration (s)")]
    pub visit_duration_secs: Option<f64>,
    /// URL-level aggregate: `urls.visit_count` / `moz_places.visit_count`.
    #[serde(rename = "Visit Count")]
    pub visit_count: Option<i64>,
    /// `urls.typed_count` (a count) / `moz_places.typed` (0 or 1). The two
    /// families genuinely differ in meaning here; documented in the tool page.
    #[serde(rename = "Typed Count")]
    pub typed_count: Option<i64>,
    /// URL-level aggregate: `urls.last_visit_time` / `moz_places.last_visit_date`.
    #[serde(rename = "Last Visit Time")]
    pub last_visit_time: WinTimestamp,
    /// `urls.hidden` / `moz_places.hidden`.
    #[serde(rename = "Hidden")]
    pub hidden: String,
    /// `visits.from_visit` / `moz_historyvisits.from_visit` — the referring
    /// visit, which is what makes a redirect chain reconstructable.
    #[serde(rename = "From Visit ID")]
    pub from_visit_id: Option<i64>,
    /// `visits.opener_visit`. Chromium only.
    #[serde(rename = "Opener Visit ID")]
    pub opener_visit_id: Option<i64>,
    /// `visits.id` / `moz_historyvisits.id`.
    #[serde(rename = "Visit ID")]
    pub visit_id: Option<i64>,
    /// `urls.id` / `moz_places.id`.
    #[serde(rename = "URL ID")]
    pub url_id: Option<i64>,
    /// `moz_places.frecency`. Firefox only.
    #[serde(rename = "Frecency")]
    pub frecency: Option<i64>,
    #[serde(rename = "Notes")]
    pub notes: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

impl HistoryRecord {
    pub const VISIT: &'static str = "Visit";
    pub const URL_ONLY: &'static str = "URL Only";
}

/// Chromium `downloads` + `downloads_url_chains`, Firefox download annotations.
///
/// Chromium-shaped, because it is a genuine superset: Firefox records no MIME
/// type, referrer, tab URL, danger type or opened flag, and does not separate
/// received from total bytes. Those columns are empty for Firefox rows rather
/// than being split into a second dataset an analyst would have to join.
#[derive(Debug, Default, Serialize)]
pub struct DownloadRecord {
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Browser Channel")]
    pub channel: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    /// `Download`, or `Orphan URL Chain` for a `downloads_url_chains` group
    /// whose parent `downloads` row is gone — a deletion indicator.
    #[serde(rename = "Record Type")]
    pub record_type: &'static str,
    /// `downloads.start_time` (WebKit us).
    #[serde(rename = "Start Time")]
    pub start_time: WinTimestamp,
    /// `downloads.end_time`. Zero means still in progress or never finished,
    /// and renders empty rather than as an epoch.
    #[serde(rename = "End Time")]
    pub end_time: WinTimestamp,
    /// `downloads.last_access_time`. Chromium only.
    #[serde(rename = "Last Access Time")]
    pub last_access_time: WinTimestamp,
    /// `downloads.target_path` — where the file was meant to land.
    #[serde(rename = "Target Path")]
    pub target_path: String,
    /// `downloads.current_path` — the in-flight `.crdownload`. Chromium only.
    #[serde(rename = "Current Path")]
    pub current_path: String,
    /// First `downloads_url_chains.url` (chain_index 0): the originating URL.
    #[serde(rename = "Download URL")]
    pub download_url: String,
    /// The whole redirect chain in `chain_index` order, ` -> `-joined. The
    /// chain is often the only record of where a download really came from.
    #[serde(rename = "URL Chain")]
    pub url_chain: String,
    #[serde(rename = "Referrer")]
    pub referrer: String,
    #[serde(rename = "Tab URL")]
    pub tab_url: String,
    #[serde(rename = "Tab Referrer URL")]
    pub tab_referrer_url: String,
    #[serde(rename = "Site URL")]
    pub site_url: String,
    #[serde(rename = "MIME Type")]
    pub mime_type: String,
    #[serde(rename = "Original MIME Type")]
    pub original_mime_type: String,
    #[serde(rename = "Received Bytes")]
    pub received_bytes: Option<i64>,
    #[serde(rename = "Total Bytes")]
    pub total_bytes: Option<i64>,
    /// Decoded `downloads.state`.
    #[serde(rename = "State")]
    pub state: String,
    /// Decoded `downloads.danger_type` — the Safe Browsing verdict.
    #[serde(rename = "Danger Type")]
    pub danger_type: String,
    /// Decoded `downloads.interrupt_reason`.
    #[serde(rename = "Interrupt Reason")]
    pub interrupt_reason: String,
    #[serde(rename = "Opened")]
    pub opened: String,
    /// `downloads.by_ext_id` and `by_ext_name`: a download initiated by an
    /// extension rather than the user.
    #[serde(rename = "By Extension")]
    pub by_extension: String,
    /// `downloads.hash` rendered as lowercase hex. Chrome populates it only for
    /// some download types, so empty does not mean "no hash was computed".
    #[serde(rename = "Hash (SHA-256)")]
    pub hash_sha256: String,
    #[serde(rename = "ETag")]
    pub etag: String,
    /// `downloads.last_modified` — the raw HTTP header text, kept verbatim
    /// rather than reparsed, because its format is server-controlled.
    #[serde(rename = "Last Modified Header")]
    pub last_modified_header: String,
    #[serde(rename = "GUID")]
    pub guid: String,
    #[serde(rename = "Download ID")]
    pub download_id: Option<i64>,
    #[serde(rename = "Notes")]
    pub notes: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

impl DownloadRecord {
    pub const DOWNLOAD: &'static str = "Download";
    pub const ORPHAN_CHAIN: &'static str = "Orphan URL Chain";
}

/// Chromium `Preferences` -> `extensions.settings`, Firefox `extensions.json`.
///
/// `Install Location` is the column to look at first: `Unpacked (Load)` on a
/// managed endpoint means somebody side-loaded an extension from disk rather
/// than installing it from a store.
#[derive(Debug, Default, Serialize)]
pub struct ExtensionRecord {
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Browser Channel")]
    pub channel: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    /// The `extensions.settings` map key / `addons[].id`.
    #[serde(rename = "Extension ID")]
    pub extension_id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "Enabled")]
    pub enabled: String,
    /// Decoded `state` / the Firefox disable flags summarized.
    #[serde(rename = "State")]
    pub state: String,
    /// Decoded `disable_reasons` bitmask. Chromium only.
    #[serde(rename = "Disable Reasons")]
    pub disable_reasons: String,
    /// Decoded `location`. `Unpacked (Load)` is the sideload indicator.
    #[serde(rename = "Install Location")]
    pub install_location: String,
    #[serde(rename = "From Store")]
    pub from_store: String,
    #[serde(rename = "Source URL")]
    pub source_url: String,
    #[serde(rename = "Install Time")]
    pub install_time: WinTimestamp,
    #[serde(rename = "Update Time")]
    pub update_time: WinTimestamp,
    #[serde(rename = "Install Path")]
    pub install_path: String,
    /// `|`-joined API permissions.
    #[serde(rename = "Permissions")]
    pub permissions: String,
    /// `|`-joined host permissions — which sites the extension can read.
    #[serde(rename = "Host Permissions")]
    pub host_permissions: String,
    #[serde(rename = "Manifest Version")]
    pub manifest_version: Option<i64>,
    /// Decoded `signedState`. Firefox only; `Missing` on a release build means
    /// a policy-installed or tampered add-on.
    #[serde(rename = "Signed State")]
    pub signed_state: String,
    #[serde(rename = "Add-on Type")]
    pub addon_type: String,
    #[serde(rename = "Notes")]
    pub notes: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

/// Chromium `Bookmarks` (a JSON tree), Firefox `moz_bookmarks`.
///
/// Folders and separators are emitted as well as URLs: a folder named "exfil"
/// with a date added is evidence, and dropping non-URL nodes would also lose
/// the folder path that gives the URLs their context.
#[derive(Debug, Default, Serialize)]
pub struct BookmarkRecord {
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Browser Channel")]
    pub channel: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    /// `URL`, `Folder` or `Separator`.
    #[serde(rename = "Type")]
    pub node_type: String,
    /// Chromium's `roots` key (`bookmark_bar`, `other`, `synced`) or the
    /// Firefox root's friendly name.
    #[serde(rename = "Root")]
    pub root: String,
    /// `/`-joined folder titles from the root down to but excluding this node.
    #[serde(rename = "Folder Path")]
    pub folder_path: String,
    #[serde(rename = "Title")]
    pub title: String,
    #[serde(rename = "URL")]
    pub url: String,
    /// Chromium stores this as WebKit microseconds in a decimal *string*.
    #[serde(rename = "Date Added")]
    pub date_added: WinTimestamp,
    #[serde(rename = "Date Modified")]
    pub date_modified: WinTimestamp,
    /// Chromium only.
    #[serde(rename = "Date Last Used")]
    pub date_last_used: WinTimestamp,
    #[serde(rename = "GUID")]
    pub guid: String,
    #[serde(rename = "Bookmark ID")]
    pub bookmark_id: Option<i64>,
    /// Firefox only; Chromium's tree structure is implicit.
    #[serde(rename = "Parent ID")]
    pub parent_id: Option<i64>,
    /// Index among its siblings.
    #[serde(rename = "Position")]
    pub position: Option<i64>,
    /// Depth below the root; a direct child of a root is 1.
    #[serde(rename = "Depth")]
    pub depth: Option<i64>,
    /// Firefox keyword shortcut. Firefox only.
    #[serde(rename = "Keyword")]
    pub keyword: String,
    #[serde(rename = "Notes")]
    pub notes: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

/// Chromium `Login Data` -> `logins`, Firefox `logins.json`.
///
/// Metadata only. The password itself — plaintext, ciphertext, hex or base64 —
/// is never emitted in any column of any dataset, and a test greps the whole
/// output to prove it.
///
/// Chromium stores the username in the clear while Firefox encrypts it with
/// NSS, so `Username` plus `Username Encrypted` is the honest union: a blank
/// username with the flag set means "there is one, we chose not to decrypt it",
/// which is not the same as "there is no username".
#[derive(Debug, Default, Serialize)]
pub struct LoginRecord {
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Browser Channel")]
    pub channel: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    #[serde(rename = "Origin URL")]
    pub origin_url: String,
    #[serde(rename = "Action URL")]
    pub action_url: String,
    #[serde(rename = "Signon Realm")]
    pub signon_realm: String,
    /// Plaintext on Chromium; always empty on Firefox.
    #[serde(rename = "Username")]
    pub username: String,
    #[serde(rename = "Username Encrypted")]
    pub username_encrypted: String,
    /// Whether a credential exists at all — never its content.
    #[serde(rename = "Password Present")]
    pub password_present: String,
    /// `v10`/`v11`/`v20`/`DPAPI` on Chromium, `NSS` on Firefox. Never the
    /// ciphertext.
    #[serde(rename = "Password Encryption")]
    pub password_encryption: String,
    /// A length, not the content.
    #[serde(rename = "Password Length")]
    pub password_length: Option<i64>,
    #[serde(rename = "Username Element")]
    pub username_element: String,
    #[serde(rename = "Password Element")]
    pub password_element: String,
    #[serde(rename = "Times Used")]
    pub times_used: Option<i64>,
    #[serde(rename = "Date Created")]
    pub date_created: WinTimestamp,
    #[serde(rename = "Date Last Used")]
    pub date_last_used: WinTimestamp,
    #[serde(rename = "Date Password Modified")]
    pub date_password_modified: WinTimestamp,
    /// Shared/received credentials. Chromium only.
    #[serde(rename = "Date Received")]
    pub date_received: WinTimestamp,
    #[serde(rename = "Scheme")]
    pub scheme: String,
    /// A "never save for this site" entry, which has no credential at all.
    #[serde(rename = "Blocklisted")]
    pub blocklisted: String,
    #[serde(rename = "Federation URL")]
    pub federation_url: String,
    #[serde(rename = "Display Name")]
    pub display_name: String,
    #[serde(rename = "Login ID")]
    pub login_id: Option<i64>,
    /// Firefox only.
    #[serde(rename = "GUID")]
    pub guid: String,
    #[serde(rename = "Notes")]
    pub notes: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

/// Chromium `Web Data` -> `autofill`, Firefox `moz_formhistory`.
///
/// The cleanest one-to-one pair in the tool, with one trap: Chromium stores
/// these timestamps as unix **seconds** while Firefox uses PRTime
/// **microseconds**. Same dataset, two epochs.
#[derive(Debug, Default, Serialize)]
pub struct AutofillRecord {
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Browser Channel")]
    pub channel: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    /// `autofill.name` / `moz_formhistory.fieldname` — the form field.
    #[serde(rename = "Field Name")]
    pub field_name: String,
    /// What was typed. Plaintext in both families.
    #[serde(rename = "Value")]
    pub value: String,
    /// `autofill.count` / `moz_formhistory.timesUsed`.
    #[serde(rename = "Use Count")]
    pub use_count: Option<i64>,
    #[serde(rename = "First Used")]
    pub first_used: WinTimestamp,
    #[serde(rename = "Last Used")]
    pub last_used: WinTimestamp,
    /// Firefox only.
    #[serde(rename = "GUID")]
    pub guid: String,
    /// Firefox only — Chromium's `autofill` table is keyed by (name, value)
    /// with no stable row id.
    #[serde(rename = "Entry ID")]
    pub entry_id: Option<i64>,
    #[serde(rename = "Notes")]
    pub notes: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

/// Chromium `cookies`, Firefox `moz_cookies`.
///
/// The two families differ more here than anywhere else: Chromium encrypts the
/// value (AES-GCM v10/v11, App-Bound v20, or DPAPI on older profiles) while
/// Firefox stores it in the clear. Rather than one nullable `Value` that means
/// two different things, the encryption state is explicit and filterable, so a
/// Chromium row can never be mistaken for an empty cookie.
///
/// No decryption is attempted and no ciphertext is ever emitted.
#[derive(Debug, Default, Serialize)]
pub struct CookieRecord {
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Browser Channel")]
    pub channel: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    /// `cookies.host_key` / `moz_cookies.host`.
    #[serde(rename = "Host")]
    pub host: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Path")]
    pub path: String,
    /// Plaintext only. Empty on Chromium >= 80, where the real value lives in
    /// `encrypted_value`; always populated on Firefox.
    #[serde(rename = "Value")]
    pub value: String,
    /// Whether a non-empty `encrypted_value` blob is present. An empty `Value`
    /// with this set to True means "there is a value, we chose not to decrypt
    /// it" — materially different from "the cookie has no value".
    #[serde(rename = "Value Encrypted")]
    pub value_encrypted: String,
    /// `v10`, `v11`, `v20`, `DPAPI` or `Unknown`, from the ciphertext prefix
    /// alone. v20 is App-Bound and machine-bound, so it is not decryptable from
    /// a dead capture by any tool.
    #[serde(rename = "Encryption Scheme")]
    pub encryption_scheme: String,
    /// Length of the ciphertext, or of the plaintext when unencrypted. A size,
    /// never the content.
    #[serde(rename = "Value Length")]
    pub value_length: Option<i64>,
    #[serde(rename = "Created")]
    pub created: WinTimestamp,
    #[serde(rename = "Last Accessed")]
    pub last_accessed: WinTimestamp,
    /// `cookies.last_update_utc`, Chromium schema v14+ only.
    #[serde(rename = "Last Updated")]
    pub last_updated: WinTimestamp,
    /// Chromium `expires_utc` is WebKit microseconds; Firefox `expiry` is unix
    /// seconds. Same concept, different unit, in the same dataset.
    #[serde(rename = "Expires")]
    pub expires: WinTimestamp,
    #[serde(rename = "Session Cookie")]
    pub session_cookie: String,
    #[serde(rename = "Secure")]
    pub secure: String,
    #[serde(rename = "HTTP Only")]
    pub http_only: String,
    #[serde(rename = "SameSite")]
    pub same_site: String,
    #[serde(rename = "Priority")]
    pub priority: String,
    #[serde(rename = "Source Scheme")]
    pub source_scheme: String,
    #[serde(rename = "Source Port")]
    pub source_port: Option<i64>,
    /// CHIPS partitioned-cookie key. Chromium only.
    #[serde(rename = "Top Frame Site")]
    pub top_frame_site: String,
    /// Container / partition key. Firefox only.
    #[serde(rename = "Origin Attributes")]
    pub origin_attributes: String,
    #[serde(rename = "Notes")]
    pub notes: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

/// Chromium `keyword_search_terms`, Firefox `moz_places_metadata` search
/// queries.
///
/// The term a person typed is often more use than the URL it produced, and it
/// survives in this table even when the corresponding history rows have been
/// cleared. One row per search execution — the same term searched twice is two
/// rows, because each is a separate act.
#[derive(Debug, Default, Serialize)]
pub struct KeywordSearchRecord {
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Browser Channel")]
    pub channel: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    /// Which table the row came from, since the three sources carry very
    /// different timestamp fidelity: `keyword_search_terms`,
    /// `moz_places_metadata` or `moz_inputhistory`.
    #[serde(rename = "Search Source")]
    pub search_source: String,
    /// The visit at which the search was run. Empty when the search term
    /// outlived its history rows — which is itself worth seeing.
    #[serde(rename = "Search Time")]
    pub search_time: WinTimestamp,
    /// `keyword_search_terms.term`, case preserved as typed.
    #[serde(rename = "Search Term")]
    pub search_term: String,
    /// `keyword_search_terms.lower_term`, Chromium's normalized form.
    #[serde(rename = "Search Term (Lower)")]
    pub search_term_lower: String,
    /// The results-page URL the search produced.
    #[serde(rename = "Search URL")]
    pub search_url: String,
    /// Host of `Search URL` — a projection, not an inference: the provider.
    #[serde(rename = "Search Engine Host")]
    pub search_engine_host: String,
    #[serde(rename = "Page Title")]
    pub page_title: String,
    #[serde(rename = "Last Visit Time")]
    pub last_visit_time: WinTimestamp,
    #[serde(rename = "Visit Count")]
    pub visit_count: Option<i64>,
    /// `keyword_search_terms.keyword_id`, which points at the configured search
    /// engine in `Web Data`. Chromium only.
    #[serde(rename = "Keyword ID")]
    pub keyword_id: Option<i64>,
    #[serde(rename = "URL ID")]
    pub url_id: Option<i64>,
    #[serde(rename = "Visit ID")]
    pub visit_id: Option<i64>,
    #[serde(rename = "Notes")]
    pub notes: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

#[cfg(test)]
pub(crate) mod header_test_support {
    use serde::Serialize;

    /// The CSV header a record type produces, for the attribution-column
    /// contract test.
    pub fn headers<T: Serialize + Default>() -> Vec<String> {
        let mut writer = csv::Writer::from_writer(Vec::new());
        writer.serialize(T::default()).unwrap();
        let bytes = writer.into_inner().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let first = text.lines().next().unwrap_or_default().to_string();
        let mut reader = csv::Reader::from_reader(first.as_bytes());
        reader
            .headers()
            .unwrap()
            .iter()
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::header_test_support::headers;
    use super::*;

    /// Every dataset must be pivotable by browser and profile, and every row
    /// must say where it came from. Without this test the five shared columns
    /// drift as the structs are edited, because `csv` cannot flatten them into
    /// a shared type.
    /// Assert one record type honours the shared column contract. Each new
    /// dataset adds a call below.
    fn assert_attribution_contract<T: Serialize + Default>(what: &str) {
        let header = headers::<T>();
        assert_eq!(
            &header[..3],
            &["Browser", "Browser Channel", "Profile"],
            "{what}: leading attribution columns"
        );
        assert_eq!(
            &header[header.len() - 2..],
            &["Notes", "Source File"],
            "{what}: trailing provenance columns"
        );
    }

    /// Every dataset must be pivotable by browser and profile, and every row
    /// must say where it came from. Without this the five shared columns drift
    /// as the structs are edited, because `csv` cannot flatten them into a
    /// shared type.
    #[test]
    fn every_record_shares_the_attribution_column_contract() {
        assert_attribution_contract::<HistoryRecord>("History");
        assert_attribution_contract::<DownloadRecord>("Downloads");
        assert_attribution_contract::<KeywordSearchRecord>("Keyword Searches");
        assert_attribution_contract::<CookieRecord>("Cookies");
        assert_attribution_contract::<AutofillRecord>("Autofill");
        assert_attribution_contract::<LoginRecord>("Logins");
        assert_attribution_contract::<BookmarkRecord>("Bookmarks");
        assert_attribution_contract::<ExtensionRecord>("Extensions");
    }

    #[test]
    fn history_columns_are_in_the_documented_order() {
        let header = headers::<HistoryRecord>();
        assert_eq!(
            header,
            vec![
                "Browser",
                "Browser Channel",
                "Profile",
                "Record Type",
                "Visit Time",
                "URL",
                "Title",
                "Visit Type",
                "Transition Qualifiers",
                "Transition Raw",
                "Visit Source",
                "Visit Duration (s)",
                "Visit Count",
                "Typed Count",
                "Last Visit Time",
                "Hidden",
                "From Visit ID",
                "Opener Visit ID",
                "Visit ID",
                "URL ID",
                "Frecency",
                "Notes",
                "Source File",
            ]
        );
    }

    /// An unset timestamp must render as an empty cell, never as the epoch.
    #[test]
    fn an_unset_timestamp_is_an_empty_cell() {
        let mut writer = csv::Writer::from_writer(Vec::new());
        writer.serialize(HistoryRecord::default()).unwrap();
        let out = String::from_utf8(writer.into_inner().unwrap()).unwrap();
        let row = out.lines().nth(1).unwrap();
        assert!(row.starts_with(",,,,,"), "got: {row}");
        assert!(!row.contains("1601"), "got: {row}");
        assert!(!row.contains("1970"), "got: {row}");
    }
}
