//! A minimal proleptic-Gregorian civil date.
//!
//! sunsetscan only ever needs three things from a calendar: parse `YYYY-MM-DD`,
//! print `YYYY-MM-DD`, and subtract two dates to get a whole number of days.
//! That is a few dozen lines of well-known arithmetic, so it lives here rather
//! than pulling in a date crate (see CONTRIBUTING for the dependency policy).
//!
//! # Time zone
//!
//! [`Date::today_utc`] returns the current date in UTC, not in the local zone.
//! Resolving a local zone from the standard library alone is not possible, and
//! the alternative -- a date crate whose local-offset lookup is itself unsound
//! in multi-threaded processes -- buys at most a one-day correction to a
//! horizon measured in weeks. Runs that need an exact reference point should
//! pass `--as-of`, which is also what makes CI output reproducible.

use std::fmt;

/// A calendar date with no time and no zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: i32,
    month: u32,
    day: u32,
}

impl Date {
    /// Build a date, rejecting out-of-range months and days (leap years
    /// included).
    pub fn new(year: i32, month: u32, day: u32) -> Option<Date> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Date { year, month, day })
    }

    /// Parse a strict `YYYY-MM-DD` string. Anything else is an error, on
    /// purpose: accepting loose formats here would make `--as-of` ambiguous.
    pub fn parse(s: &str) -> Result<Date, String> {
        let bytes = s.as_bytes();
        let shaped = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
        if !shaped {
            return Err(format!("expected a date as YYYY-MM-DD, got '{s}'"));
        }
        let year: i32 = s[0..4].parse().map_err(|_| bad(s))?;
        let month: u32 = s[5..7].parse().map_err(|_| bad(s))?;
        let day: u32 = s[8..10].parse().map_err(|_| bad(s))?;
        Date::new(year, month, day).ok_or_else(|| format!("'{s}' is not a real calendar date"))
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u32 {
        self.month
    }

    pub fn day(self) -> u32 {
        self.day
    }

    /// Today's date in UTC. See the module docs for why this is not local time.
    pub fn today_utc() -> Date {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            // A clock set before 1970 is not worth a panic; fall back to the
            // epoch, which makes every sunset look past due rather than clean.
            .unwrap_or(0);
        Date::from_days_since_epoch(secs.div_euclid(86_400))
    }

    /// Days since 1970-01-01, using Howard Hinnant's `days_from_civil`.
    pub fn days_since_epoch(self) -> i64 {
        let y = self.year as i64 - i64::from(self.month <= 2);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400; // 0..=399
        let m = self.month as i64;
        let d = self.day as i64;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1; // 0..=365
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // 0..=146096
        era * 146_097 + doe - 719_468
    }

    /// Inverse of [`Date::days_since_epoch`] (`civil_from_days`).
    pub fn from_days_since_epoch(days: i64) -> Date {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097; // 0..=146096
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // 0..=399
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
        let mp = (5 * doy + 2) / 153; // 0..=11
        let d = doy - (153 * mp + 2) / 5 + 1; // 1..=31
        let m = mp + if mp < 10 { 3 } else { -9 }; // 1..=12
        Date {
            year: (y + i64::from(m <= 2)) as i32,
            month: m as u32,
            day: d as u32,
        }
    }

    /// Whole days from `self` to `other`. Negative when `other` is in the past.
    pub fn days_until(self, other: Date) -> i64 {
        other.days_since_epoch() - self.days_since_epoch()
    }
}

fn bad(s: &str) -> String {
    format!("expected a date as YYYY-MM-DD, got '{s}'")
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_round_trip() {
        let d = Date::parse("2026-08-26").unwrap();
        assert_eq!(d.to_string(), "2026-08-26");
        assert_eq!((d.year(), d.month(), d.day()), (2026, 8, 26));
    }

    #[test]
    fn rejects_malformed_and_impossible_dates() {
        for bad in [
            "2026-8-26",
            "26-08-26",
            "2026/08/26",
            "",
            "2026-08-26T00:00:00Z",
            "2026-13-01",
            "2026-00-10",
            "2026-02-30",
            "2026-04-31",
            "2026-01-00",
        ] {
            assert!(Date::parse(bad).is_err(), "{bad} should not parse");
        }
        assert!(Date::parse("2024-02-29").is_ok(), "2024 is a leap year");
        assert!(Date::parse("2026-02-29").is_err(), "2026 is not");
        assert!(Date::parse("2000-02-29").is_ok(), "2000 is a leap year");
        assert!(Date::parse("1900-02-29").is_err(), "1900 is not");
    }

    #[test]
    fn epoch_arithmetic_matches_known_points() {
        assert_eq!(Date::parse("1970-01-01").unwrap().days_since_epoch(), 0);
        assert_eq!(Date::parse("1970-01-02").unwrap().days_since_epoch(), 1);
        assert_eq!(Date::parse("1969-12-31").unwrap().days_since_epoch(), -1);
        assert_eq!(Date::parse("2000-03-01").unwrap().days_since_epoch(), 11017);
    }

    #[test]
    fn days_since_epoch_round_trips_over_a_long_span() {
        // Every day from 1968 through 2100, both directions.
        let start = Date::parse("1968-01-01").unwrap().days_since_epoch();
        let end = Date::parse("2100-12-31").unwrap().days_since_epoch();
        for n in start..=end {
            let d = Date::from_days_since_epoch(n);
            assert_eq!(d.days_since_epoch(), n);
            assert_eq!(Date::parse(&d.to_string()).unwrap(), d);
        }
    }

    #[test]
    fn days_until_is_signed_and_spans_months() {
        let as_of = Date::parse("2026-08-11").unwrap();
        let sunset = Date::parse("2026-08-26").unwrap();
        assert_eq!(as_of.days_until(sunset), 15);
        assert_eq!(sunset.days_until(as_of), -15);
        assert_eq!(as_of.days_until(as_of), 0);
        assert_eq!(
            Date::parse("2026-12-31")
                .unwrap()
                .days_until(Date::parse("2027-01-01").unwrap()),
            1
        );
    }

    #[test]
    fn today_utc_is_a_sane_date() {
        let today = Date::today_utc();
        assert!(today.year() >= 2024, "clock looks wrong: {today}");
        assert_eq!(Date::parse(&today.to_string()).unwrap(), today);
    }
}
