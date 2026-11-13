//! UTC instants shown in an analyst-chosen time zone (feature `zones`).
//!
//! The suite stores and exports UTC only. OxideLens shows those instants in a
//! display zone the analyst picks, and every zone calculation it does goes
//! through this module, so the DST rules live in one place with their edges
//! tested. A zone is never guessed: every function takes the IANA name it is
//! given and answers absent, or an error, for one it does not know.
//!
//! Two text forms come in:
//!
//! - **UTC text**, `YYYY-MM-DDTHH:MM:SS[.f]Z` with 1 to 9 fraction digits, as
//!   the suite writes it (seven digits) and as OxideLens generates it.
//! - **Local text**, `YYYY-MM-DD HH:MM:SS`, a wall-clock time typed by the
//!   analyst in the display zone.
//!
//! Instants this module generates are R1 text: `YYYY-MM-DDTHH:MM:SS.ffffffZ`,
//! six fraction digits and `Z`. Years outside 0000 to 9999 cannot be written
//! with four digits and come back absent rather than wrapped or signed.
//!
//! The zone rules are chrono-tz's copy of tzdb, which expands recurring DST
//! rules through 2099; from 2100 on each zone keeps its last offset (standard
//! time, for the zones that observe DST). An instant that late in evidence is
//! almost certainly a misread field rather than a real time.
//!
//! Every function is total: malformed text, an unknown zone or an instant at
//! the edge of the range gives `None`, an error or an empty list, never a
//! panic (`tests/zone.rs` sweeps them).

use std::sync::OnceLock;

use chrono::{
    DateTime, Datelike, Days, MappedLocalTime, NaiveDate, NaiveDateTime, NaiveTime, Offset,
    TimeDelta, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

/// The names tzdb links to `Etc/UTC`. A time in one of these zones is UTC and
/// its offset is written `Z`; a zone that merely sits at offset zero for part
/// of the year (`Europe/London` in winter) is written `+00:00`.
const UTC_NAMES: &[&str] = &[
    "UTC",
    "Etc/UTC",
    "Etc/UCT",
    "Etc/Universal",
    "Etc/Zulu",
    "UCT",
    "Universal",
    "Zulu",
];

/// Wider than any UTC offset tzdb has ever recorded (the local mean times of
/// Asia/Manila, -15:56, and America/Metlakatla, +15:13, are the extremes). A
/// local wall-clock time is always within this distance of its instant.
const WIDEST_OFFSET: TimeDelta = TimeDelta::hours(16);

/// Every IANA zone name this build knows (chrono-tz's copy of tzdb, links
/// included), sorted, with `UTC` first.
pub fn names() -> &'static [&'static str] {
    static NAMES: OnceLock<Vec<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut all: Vec<&'static str> = chrono_tz::TZ_VARIANTS
            .iter()
            .map(|tz| tz.name())
            .filter(|name| *name != "UTC")
            .collect();
        all.sort_unstable();
        all.dedup();
        all.insert(0, "UTC");
        all
    })
}

/// Whether `name` is an IANA zone name this build knows, spelled exactly
/// (case and all).
pub fn is_zone(name: &str) -> bool {
    lookup(name).is_some()
}

/// A UTC instant as the wall clock in a zone showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// `YYYY-MM-DD HH:MM:SS` followed by the input's fraction exactly as it
    /// was written (`.2040000` stays seven digits; no fraction stays none).
    pub local: String,
    /// The zone's offset from UTC at that instant: `-05:00`, `+05:30`, or `Z`
    /// in UTC itself. Seconds are added only for an offset that has them
    /// (pre-1900 local mean time, `-04:56:02`).
    pub offset: String,
}

/// Show the UTC text `utc_text` in `zone`. `None` when the text is not UTC
/// text, the zone is unknown, or the local year falls outside 0000 to 9999.
///
/// The fraction is copied from the input, never re-rounded: an offset is a
/// whole number of seconds, so converting cannot change it.
pub fn render_utc_text(utc_text: &str, zone: &str) -> Option<Rendered> {
    let parsed = parse_utc(utc_text)?;
    let tz = lookup(zone)?;
    let local = tz.from_utc_datetime(&parsed.at);
    let wall = local.naive_local();
    if !four_digit_year(wall.year()) {
        return None;
    }
    let offset = if UTC_NAMES.contains(&tz.name()) {
        "Z".to_string()
    } else {
        offset_text(local.offset().fix().local_minus_utc())
    };
    Some(Rendered {
        local: format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}{}",
            wall.year(),
            wall.month(),
            wall.day(),
            wall.hour(),
            wall.minute(),
            wall.second(),
            parsed.fraction,
        ),
        offset,
    })
}

/// Why a local wall-clock time has no single UTC instant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocalError {
    /// Not `YYYY-MM-DD HH:MM:SS`, not a real calendar time, or an instant
    /// whose UTC year falls outside 0000 to 9999.
    #[error("not a local time: expected YYYY-MM-DD HH:MM:SS between years 0000 and 9999")]
    BadText,
    /// The zone is not an IANA name this build knows.
    #[error("not a time zone this build knows")]
    UnknownZone,
    /// The clock went back over this time, so it happened twice. Both
    /// instants are given (R1 text) so the analyst can choose one.
    #[error("that local time happened twice in this zone, at {earlier} and at {later}; give the UTC time")]
    Ambiguous { earlier: String, later: String },
    /// The clock jumped forward over this time, so it never happened.
    #[error("that local time never happened in this zone: the clock skipped it")]
    Nonexistent,
}

/// The UTC instant (R1 text) at which `zone`'s wall clock read `local`
/// (`YYYY-MM-DD HH:MM:SS`). A time the clock showed twice or skipped is
/// refused, never resolved by guessing.
pub fn local_to_utc(local: &str, zone: &str) -> Result<String, LocalError> {
    let wall = parse_local(local).ok_or(LocalError::BadText)?;
    let tz = lookup(zone).ok_or(LocalError::UnknownZone)?;
    match tz.from_local_datetime(&wall) {
        MappedLocalTime::Single(at) => r1(at.naive_utc()).ok_or(LocalError::BadText),
        MappedLocalTime::Ambiguous(a, b) => {
            let (a, b) = (a.naive_utc(), b.naive_utc());
            let (earlier, later) = if a <= b { (a, b) } else { (b, a) };
            Err(LocalError::Ambiguous {
                earlier: r1(earlier).ok_or(LocalError::BadText)?,
                later: r1(later).ok_or(LocalError::BadText)?,
            })
        }
        MappedLocalTime::None => Err(LocalError::Nonexistent),
    }
}

/// The starts of local days in `zone` (R1 text, ascending) that cover the
/// closed span `from_utc..=to_utc`: the start of the local day holding
/// `from_utc`, then every `step_days`-th local day after it whose start is no
/// later than `to_utc`. At most `cap` are returned.
///
/// A day starts at local midnight, or, where the clock skips midnight, at the
/// first instant after the gap. A day the clock skipped entirely (Samoa,
/// 2011-12-30) starts when the next one does, and that instant is listed once.
///
/// Empty when either text is not UTC text, the zone is unknown, the span is
/// reversed, or `step_days` or `cap` is zero.
pub fn local_midnights(
    from_utc: &str,
    to_utc: &str,
    zone: &str,
    step_days: u32,
    cap: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    let (Some(from), Some(to), Some(tz)) = (parse_utc(from_utc), parse_utc(to_utc), lookup(zone))
    else {
        return out;
    };
    let (from, to) = (from.at, to.at);
    if step_days == 0 || cap == 0 || from > to {
        return out;
    }
    let step = Days::new(u64::from(step_days));
    let mut day = tz.from_utc_datetime(&from).date_naive();
    let mut last: Option<NaiveDateTime> = None;
    while out.len() < cap {
        // No day whose midnight is this far past `to` can start inside the
        // span; this bounds the loop even for days with no start of their own.
        let midnight = day.and_time(NaiveTime::MIN);
        if midnight
            .checked_sub_signed(WIDEST_OFFSET)
            .is_none_or(|earliest| earliest > to)
        {
            break;
        }
        if let Some(start) = day_start(tz, day) {
            if start > to {
                break;
            }
            if last.is_none_or(|previous| start > previous) {
                // Only a start before year 0000 fails here; the days after it
                // may still be writable, so it is skipped rather than ending
                // the list.
                if let Some(text) = r1(start) {
                    out.push(text);
                }
                last = Some(start);
            }
        }
        match day.checked_add_days(step) {
            Some(next) => day = next,
            None => break,
        }
    }
    out
}

/// `zone`'s offset from UTC, in seconds east, at the instant `utc_text`.
pub fn utc_offset_seconds(utc_text: &str, zone: &str) -> Option<i32> {
    let parsed = parse_utc(utc_text)?;
    let tz = lookup(zone)?;
    Some(
        tz.offset_from_utc_datetime(&parsed.at)
            .fix()
            .local_minus_utc(),
    )
}

/// The IANA zone CLDR names as the golden zone for a Windows time zone name
/// (the registry's `TimeZoneKeyName`, such as `Eastern Standard Time`). Exact
/// match; `None` for a name the table does not hold.
pub fn windows_to_iana(windows_name: &str) -> Option<&'static str> {
    WINDOWS_ZONES
        .iter()
        .find(|(windows, _)| *windows == windows_name)
        .map(|(_, iana)| *iana)
}

/// Windows time zone name to IANA zone, transcribed from Unicode CLDR release
/// 49 (tag `release-49` of github.com/unicode-org/cldr),
/// `common/supplemental/windowsZones.xml`: every `mapZone` with
/// `territory="001"` (the golden zone for that Windows name), in the file's
/// order. 142 entries. The IANA spellings are CLDR's, which keep tzdb's older
/// names (`Asia/Calcutta`, `Asia/Katmandu`, `Europe/Kiev`); chrono-tz knows
/// them all as links, which the unit test below checks.
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC-11", "Etc/GMT+11"),
    ("Aleutian Standard Time", "America/Adak"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Marquesas Standard Time", "Pacific/Marquesas"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("UTC-09", "Etc/GMT+9"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("UTC-08", "Etc/GMT+8"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time (Mexico)", "America/Mazatlan"),
    ("Mountain Standard Time", "America/Denver"),
    ("British Columbia Standard Time", "America/Vancouver"),
    ("Yukon Standard Time", "America/Whitehorse"),
    ("Alberta Standard Time", "America/Edmonton"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Standard Time", "America/Chicago"),
    ("Easter Island Standard Time", "Pacific/Easter"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Canada Central Standard Time", "America/Regina"),
    ("Manitoba Standard Time", "America/Winnipeg"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("Eastern Standard Time", "America/New_York"),
    ("Haiti Standard Time", "America/Port-au-Prince"),
    ("Cuba Standard Time", "America/Havana"),
    ("US Eastern Standard Time", "America/Indianapolis"),
    ("Turks And Caicos Standard Time", "America/Grand_Turk"),
    ("Paraguay Standard Time", "America/Asuncion"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Central Brazilian Standard Time", "America/Cuiaba"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("Tocantins Standard Time", "America/Araguaina"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("Argentina Standard Time", "America/Buenos_Aires"),
    ("Greenland Standard Time", "America/Godthab"),
    ("Montevideo Standard Time", "America/Montevideo"),
    ("Magallanes Standard Time", "America/Punta_Arenas"),
    ("Saint Pierre Standard Time", "America/Miquelon"),
    ("Bahia Standard Time", "America/Bahia"),
    ("UTC-02", "Etc/GMT+2"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("UTC", "Etc/UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("Sao Tome Standard Time", "Africa/Sao_Tome"),
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("Jordan Standard Time", "Asia/Amman"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Middle East Standard Time", "Asia/Beirut"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("Syria Standard Time", "Asia/Damascus"),
    ("West Bank Standard Time", "Asia/Hebron"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("South Sudan Standard Time", "Africa/Juba"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Sudan Standard Time", "Africa/Khartoum"),
    ("Libya Standard Time", "Africa/Tripoli"),
    ("Namibia Standard Time", "Africa/Windhoek"),
    ("Arabic Standard Time", "Asia/Baghdad"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Belarus Standard Time", "Europe/Minsk"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Astrakhan Standard Time", "Europe/Astrakhan"),
    ("Azerbaijan Standard Time", "Asia/Baku"),
    ("Russia Time Zone 3", "Europe/Samara"),
    ("Mauritius Standard Time", "Indian/Mauritius"),
    ("Saratov Standard Time", "Europe/Saratov"),
    ("Georgian Standard Time", "Asia/Tbilisi"),
    ("Volgograd Standard Time", "Europe/Volgograd"),
    ("Caucasus Standard Time", "Asia/Yerevan"),
    ("Afghanistan Standard Time", "Asia/Kabul"),
    ("West Asia Standard Time", "Asia/Tashkent"),
    ("Ekaterinburg Standard Time", "Asia/Yekaterinburg"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("Qyzylorda Standard Time", "Asia/Qyzylorda"),
    ("India Standard Time", "Asia/Calcutta"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Katmandu"),
    ("Central Asia Standard Time", "Asia/Bishkek"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("Omsk Standard Time", "Asia/Omsk"),
    ("Myanmar Standard Time", "Asia/Rangoon"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("Altai Standard Time", "Asia/Barnaul"),
    ("W. Mongolia Standard Time", "Asia/Hovd"),
    ("North Asia Standard Time", "Asia/Krasnoyarsk"),
    ("N. Central Asia Standard Time", "Asia/Novosibirsk"),
    ("Tomsk Standard Time", "Asia/Tomsk"),
    ("China Standard Time", "Asia/Shanghai"),
    ("North Asia East Standard Time", "Asia/Irkutsk"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Ulaanbaatar Standard Time", "Asia/Ulaanbaatar"),
    ("Aus Central W. Standard Time", "Australia/Eucla"),
    ("Transbaikal Standard Time", "Asia/Chita"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("North Korea Standard Time", "Asia/Pyongyang"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Yakutsk Standard Time", "Asia/Yakutsk"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("West Pacific Standard Time", "Pacific/Port_Moresby"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("Vladivostok Standard Time", "Asia/Vladivostok"),
    ("Lord Howe Standard Time", "Australia/Lord_Howe"),
    ("Bougainville Standard Time", "Pacific/Bougainville"),
    ("Russia Time Zone 10", "Asia/Srednekolymsk"),
    ("Magadan Standard Time", "Asia/Magadan"),
    ("Norfolk Standard Time", "Pacific/Norfolk"),
    ("Sakhalin Standard Time", "Asia/Sakhalin"),
    ("Central Pacific Standard Time", "Pacific/Guadalcanal"),
    ("Russia Time Zone 11", "Asia/Kamchatka"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("UTC+12", "Etc/GMT-12"),
    ("Fiji Standard Time", "Pacific/Fiji"),
    ("Chatham Islands Standard Time", "Pacific/Chatham"),
    ("UTC+13", "Etc/GMT-13"),
    ("Tonga Standard Time", "Pacific/Tongatapu"),
    ("Samoa Standard Time", "Pacific/Apia"),
    ("Line Islands Standard Time", "Pacific/Kiritimati"),
];

fn lookup(name: &str) -> Option<Tz> {
    name.parse().ok()
}

fn four_digit_year(year: i32) -> bool {
    (0..=9999).contains(&year)
}

/// Parsed UTC text: the instant, and the fraction exactly as written
/// (`".2040000"`, or empty when there was none).
struct UtcText<'a> {
    at: NaiveDateTime,
    fraction: &'a str,
}

/// `YYYY-MM-DDTHH:MM:SS[.f{1,9}]Z`, ASCII digits only.
fn parse_utc(text: &str) -> Option<UtcText<'_>> {
    let bytes = text.as_bytes();
    // 19 for the date and time, up to 10 for the fraction, 1 for the Z.
    if !(20..=30).contains(&bytes.len()) {
        return None;
    }
    let (fields, rest) = bytes.split_at(19);
    let whole = parse_fields(fields, b'T')?;
    let (fraction, zulu) = rest.split_at(rest.len() - 1);
    if zulu != b"Z" {
        return None;
    }
    let nanos = match fraction.strip_prefix(b".") {
        None if fraction.is_empty() => 0,
        None => return None,
        Some(digits) if (1..=9).contains(&digits.len()) => {
            let scale = 10u32.checked_pow(9 - digits.len() as u32)?;
            number(digits)?.checked_mul(scale)?
        }
        Some(_) => return None,
    };
    Some(UtcText {
        at: whole.with_nanosecond(nanos)?,
        // Every byte checked above is ASCII, so this range is on char
        // boundaries; `get` keeps it total regardless.
        fraction: text.get(19..text.len() - 1)?,
    })
}

/// `YYYY-MM-DD HH:MM:SS`, ASCII digits only.
fn parse_local(text: &str) -> Option<NaiveDateTime> {
    parse_fields(text.as_bytes(), b' ')
}

/// The 19-byte `YYYY-MM-DD?HH:MM:SS` core, with `separator` between the date
/// and the time. Rejects any field that is not a real calendar value.
fn parse_fields(bytes: &[u8], separator: u8) -> Option<NaiveDateTime> {
    let b: &[u8; 19] = bytes.try_into().ok()?;
    if b[4] != b'-' || b[7] != b'-' || b[10] != separator || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year = i32::try_from(number(&b[0..4])?).ok()?;
    let date = NaiveDate::from_ymd_opt(year, number(&b[5..7])?, number(&b[8..10])?)?;
    date.and_hms_opt(
        number(&b[11..13])?,
        number(&b[14..16])?,
        number(&b[17..19])?,
    )
}

/// A run of ASCII digits (at most nine, so it fits a `u32`).
fn number(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() || digits.len() > 9 {
        return None;
    }
    digits.iter().try_fold(0u32, |acc, &byte| {
        if byte.is_ascii_digit() {
            acc.checked_mul(10)?.checked_add(u32::from(byte - b'0'))
        } else {
            None
        }
    })
}

/// R1 text for a UTC instant: `YYYY-MM-DDTHH:MM:SS.ffffffZ`. `None` outside
/// years 0000 to 9999.
fn r1(at: NaiveDateTime) -> Option<String> {
    if !four_digit_year(at.year()) {
        return None;
    }
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        // A leap-second nanosecond count (>= 1e9) is never built here; clamp
        // so even one could not widen the field.
        at.nanosecond().min(999_999_999) / 1_000,
    ))
}

/// `+HH:MM`, or `+HH:MM:SS` for an offset with seconds.
fn offset_text(seconds_east: i32) -> String {
    let sign = if seconds_east < 0 { '-' } else { '+' };
    let total = seconds_east.unsigned_abs();
    let (hours, minutes, seconds) = (total / 3600, total % 3600 / 60, total % 60);
    if seconds == 0 {
        format!("{sign}{hours:02}:{minutes:02}")
    } else {
        format!("{sign}{hours:02}:{minutes:02}:{seconds:02}")
    }
}

/// The first UTC instant of local `day` in `tz`: local midnight (the earlier
/// one if midnight happened twice), or the end of the gap if the clock
/// skipped midnight.
fn day_start(tz: Tz, day: NaiveDate) -> Option<NaiveDateTime> {
    let midnight = day.and_time(NaiveTime::MIN);
    match tz.from_local_datetime(&midnight) {
        MappedLocalTime::Single(at) => Some(at.naive_utc()),
        MappedLocalTime::Ambiguous(a, b) => Some(a.naive_utc().min(b.naive_utc())),
        MappedLocalTime::None => first_instant_reading_at_least(tz, midnight),
    }
}

/// The earliest UTC instant at which `tz`'s wall clock reads `wall` or later,
/// for a `wall` inside a gap. Transitions fall on whole seconds, so a binary
/// search over seconds within [`WIDEST_OFFSET`] of `wall` finds it exactly.
fn first_instant_reading_at_least(tz: Tz, wall: NaiveDateTime) -> Option<NaiveDateTime> {
    let reads_at_least = |second: i64| -> Option<bool> {
        let at = DateTime::<Utc>::from_timestamp(second, 0)?.naive_utc();
        Some(tz.from_utc_datetime(&at).naive_local() >= wall)
    };
    let mut low = wall
        .checked_sub_signed(WIDEST_OFFSET)?
        .and_utc()
        .timestamp();
    let mut high = wall
        .checked_add_signed(WIDEST_OFFSET)?
        .and_utc()
        .timestamp();
    // The search needs the clock before `wall` at `low` and past it at `high`.
    if reads_at_least(low)? || !reads_at_least(high)? {
        return None;
    }
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if reads_at_least(middle)? {
            high = middle;
        } else {
            low = middle;
        }
    }
    DateTime::<Utc>::from_timestamp(high, 0).map(|at| at.naive_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLDR's golden zones must all resolve here, or `windows_to_iana` would
    /// hand OxideLens a name `render_utc_text` then refuses.
    #[test]
    fn every_cldr_target_is_a_known_zone() {
        for (windows, iana) in WINDOWS_ZONES {
            assert!(is_zone(iana), "{windows} -> {iana}");
        }
    }

    /// CLDR release 49 has 142 `territory="001"` entries, one per Windows name.
    #[test]
    fn the_cldr_table_is_whole_and_has_no_duplicate_names() {
        assert_eq!(WINDOWS_ZONES.len(), 142);
        let mut windows: Vec<&str> = WINDOWS_ZONES.iter().map(|(w, _)| *w).collect();
        windows.sort_unstable();
        windows.dedup();
        assert_eq!(windows.len(), WINDOWS_ZONES.len());
    }

    #[test]
    fn offsets_with_seconds_keep_them() {
        assert_eq!(offset_text(0), "+00:00");
        assert_eq!(offset_text(-17_762), "-04:56:02");
        assert_eq!(offset_text(19_800), "+05:30");
        assert_eq!(offset_text(i32::MIN), "-596523:14:08");
    }
}
