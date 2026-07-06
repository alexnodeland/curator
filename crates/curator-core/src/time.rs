//! Minimal RFC 3339 UTC timestamp formatting.
//!
//! The plane stamps `created` fields (proposals, notes) and computes
//! recency cutoffs as `YYYY-MM-DDTHH:MM:SSZ` strings — the same shape
//! SQLite's `strftime('%Y-%m-%dT%H:%M:%SZ', 'now')` produces, so string
//! comparison is a valid time comparison everywhere in the index. The
//! civil-date arithmetic below is the standard days-from-epoch algorithm
//! (Howard Hinnant's `civil_from_days`), pure and locale-free — no time
//! crate needed for this one fixed format.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Format seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
#[must_use]
pub fn rfc3339_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (h, m, s) = (
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// The current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
#[must_use]
pub fn now_rfc3339_utc() -> String {
    rfc3339_utc(unix_now())
}

/// The current time as seconds since the Unix epoch.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

/// Parse an RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SS[.fff]Z`, or a
/// bare `YYYY-MM-DD`) to seconds since the Unix epoch. Fractional seconds
/// are truncated. `None` for anything else — including non-UTC offsets:
/// every timestamp the plane mints or stores is `Z`-suffixed, and a
/// producer timestamp in another zone should be treated as unparseable
/// rather than silently misread.
#[must_use]
pub fn parse_rfc3339_utc(s: &str) -> Option<u64> {
    let bytes = s.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<u64> {
        let part = s.get(range)?;
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year as i64, month as u32, day as u32);
    let day_secs = u64::try_from(days).ok()? * 86_400;
    if s.len() == 10 {
        return Some(day_secs); // bare date = midnight UTC
    }
    if bytes.len() < 20 || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let (h, m, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if h > 23 || m > 59 || sec > 60 {
        return None;
    }
    // The tail must be `Z`, optionally preceded by `.fff...` (truncated).
    let tail = &s[19..];
    let valid_tail = tail == "Z"
        || (tail.starts_with('.')
            && tail.ends_with('Z')
            && tail[1..tail.len() - 1].bytes().all(|b| b.is_ascii_digit())
            && tail.len() > 2);
    if !valid_tail {
        return None;
    }
    Some(day_secs + h * 3_600 + m * 60 + sec.min(59))
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's
/// `days_from_civil`) — the inverse of [`civil_from_days`].
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let mp = u64::from(if m > 2 { m - 3 } else { m + 9 }); // [0, 11]
    let doy = (153 * mp + 2) / 5 + u64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

/// Civil `(year, month, day)` from days since 1970-01-01 (proleptic
/// Gregorian). Exact for the entire representable range.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants_format_exactly() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(rfc3339_utc(86_400), "1970-01-02T00:00:00Z");
        // Leap year day: 2024-02-29.
        assert_eq!(rfc3339_utc(1_709_164_800), "2024-02-29T00:00:00Z");
        // Contract example instant: 2026-07-03T09:15:00Z.
        assert_eq!(rfc3339_utc(1_783_070_100), "2026-07-03T09:15:00Z");
        // Century non-leap boundary.
        assert_eq!(rfc3339_utc(4_107_542_399), "2100-02-28T23:59:59Z");
        assert_eq!(rfc3339_utc(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn formatting_is_monotonic_in_string_order() {
        // RFC 3339 with fixed width compares lexicographically — the whole
        // index relies on this.
        let mut prev = rfc3339_utc(0);
        for secs in [1u64, 59, 3_600, 86_401, 1_000_000_000, 4_000_000_000] {
            let cur = rfc3339_utc(secs);
            assert!(prev < cur, "{prev} !< {cur}");
            prev = cur;
        }
    }

    #[test]
    fn parse_inverts_format() {
        for secs in [
            0u64,
            86_399,
            86_400,
            1_709_164_800,
            1_783_070_100,
            4_107_542_399,
            4_107_542_400,
        ] {
            let formatted = rfc3339_utc(secs);
            assert_eq!(parse_rfc3339_utc(&formatted), Some(secs), "{formatted}");
        }
    }

    #[test]
    fn parse_accepts_millis_and_bare_dates() {
        assert_eq!(
            parse_rfc3339_utc("2026-07-03T09:15:00.123Z"),
            Some(1_783_070_100)
        );
        assert_eq!(
            parse_rfc3339_utc("2026-07-03"),
            parse_rfc3339_utc("2026-07-03T00:00:00Z")
        );
    }

    #[test]
    fn parse_rejects_junk_and_non_utc() {
        for bad in [
            "",
            "not a date",
            "2026-7-3",
            "2026-07-03T09:15",
            "2026-07-03T09:15:00",
            "2026-07-03T09:15:00+02:00",
            "2026-07-03T09:15:00.Z",
            "2026-13-01T00:00:00Z",
            "2026-07-03T24:00:00Z",
        ] {
            assert_eq!(parse_rfc3339_utc(bad), None, "{bad:?} must not parse");
        }
    }

    #[test]
    fn now_is_well_formed() {
        let now = now_rfc3339_utc();
        assert_eq!(now.len(), 20);
        assert!(now.ends_with('Z'));
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[10..11], "T");
        assert!(now.as_str() > "2026-01-01T00:00:00Z");
    }
}
