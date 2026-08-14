//! RFC 3339 parsing, and nothing else.
//!
//! GitHub timestamps every run as `2026-08-14T21:22:03Z`. That is the only
//! shape we need to read, and pulling in a date-time crate — with its parser,
//! its timezone database, and its transitive dependencies — to handle one
//! fixed-width format would be a poor trade in a helper that is meant to stay
//! small and start instantly.
//!
//! Days-from-civil is Howard Hinnant's algorithm, which is exact for the whole
//! proleptic Gregorian calendar and needs no lookup tables.

/// Parse an RFC 3339 timestamp into Unix seconds.
///
/// Accepts `Z`, `+HH:MM` and `-HH:MM` offsets, and tolerates fractional
/// seconds by ignoring them. Returns `None` for anything else rather than
/// guessing — a misparsed timestamp shows the user a run that finished in 1970.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }

    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;

    if !matches!(bytes.get(4), Some(b'-'))
        || !matches!(bytes.get(7), Some(b'-'))
        || !matches!(bytes.get(10), Some(b'T' | b't' | b' '))
        || !matches!(bytes.get(13), Some(b':'))
        || !matches!(bytes.get(16), Some(b':'))
    {
        return None;
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=60).contains(&second) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let mut epoch = days * 86_400 + hour * 3600 + minute * 60 + second;

    // Skip fractional seconds, then read the offset if there is one.
    let mut rest = text.get(19..)?;
    if let Some(stripped) = rest.strip_prefix('.') {
        let digits = stripped
            .as_bytes()
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        rest = stripped.get(digits..)?;
    }

    match rest.as_bytes().first() {
        None | Some(b'Z' | b'z') => Some(epoch),
        Some(sign @ (b'+' | b'-')) => {
            let offset_hour: i64 = rest.get(1..3)?.parse().ok()?;
            let offset_minute: i64 = rest.get(4..6)?.parse().ok()?;
            if !(0..=23).contains(&offset_hour) || !(0..=59).contains(&offset_minute) {
                return None;
            }
            let offset = offset_hour * 3600 + offset_minute * 60;
            // A timestamp at +02:00 is two hours *earlier* in UTC.
            if *sign == b'+' {
                epoch -= offset;
            } else {
                epoch += offset;
            }
            Some(epoch)
        }
        _ => None,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
///
/// Every division here is exact integer calendar arithmetic straight out of
/// the published algorithm; rewriting it to satisfy a lint would be the
/// riskiest possible edit to the one function that must stay bit-correct.
#[allow(clippy::integer_division, reason = "exact calendar arithmetic")]
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    // Tests assert; a failed assertion is the point, so the panic-free
    // lints that guard the poll loop are relaxed here and only here.
    #![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn the_epoch_is_zero() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn a_known_github_timestamp_parses() {
        // 2026-08-14T21:22:03Z, cross-checked against `date -u -d ... +%s`.
        assert_eq!(parse_rfc3339("2026-08-14T21:22:03Z"), Some(1_786_742_523));
    }

    #[test]
    fn leap_days_are_handled() {
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(parse_rfc3339("2000-02-29T00:00:00Z"), Some(951_782_400));
    }

    #[test]
    fn fractional_seconds_are_ignored() {
        assert_eq!(
            parse_rfc3339("2024-01-15T10:30:00.123456Z"),
            parse_rfc3339("2024-01-15T10:30:00Z")
        );
    }

    #[test]
    fn offsets_shift_in_the_right_direction() {
        let utc = parse_rfc3339("2024-01-15T12:00:00Z").expect("utc parses");
        let plus = parse_rfc3339("2024-01-15T14:00:00+02:00").expect("positive offset parses");
        let minus = parse_rfc3339("2024-01-15T10:00:00-02:00").expect("negative offset parses");
        assert_eq!(plus, utc, "+02:00 is two hours ahead of UTC");
        assert_eq!(minus, utc, "-02:00 is two hours behind UTC");
    }

    #[test]
    fn leap_seconds_do_not_crash() {
        assert!(parse_rfc3339("2016-12-31T23:59:60Z").is_some());
    }

    #[test]
    fn malformed_input_is_rejected_rather_than_guessed() {
        for bad in [
            "",
            "not a date",
            "2024-01-15",
            "2024-13-01T00:00:00Z",
            "2024-01-32T00:00:00Z",
            "2024-01-15T25:00:00Z",
            "2024-01-15T10:61:00Z",
            "2024/01/15T10:00:00Z",
            "2024-01-15X10:00:00Z",
        ] {
            assert!(parse_rfc3339(bad).is_none(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn dates_before_the_epoch_go_negative() {
        let value = parse_rfc3339("1969-12-31T23:59:59Z").expect("parses");
        assert_eq!(value, -1);
    }
}
