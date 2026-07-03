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
    fn now_is_well_formed() {
        let now = now_rfc3339_utc();
        assert_eq!(now.len(), 20);
        assert!(now.ends_with('Z'));
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[10..11], "T");
        assert!(now.as_str() > "2026-01-01T00:00:00Z");
    }
}
