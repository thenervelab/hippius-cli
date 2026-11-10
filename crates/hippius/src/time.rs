//! Time as the API writes it and as `hipc` measures it: the HTTP `Date` and RFC 3339 forms,
//! read into seconds since 1970, and the time now by the API's clock.
//!
//! The local clock can be wrong, or step between two runs (an NTP correction). The API dated
//! what it wrote by its own clock, and says what that clock reads in each answer's `Date`
//! header, so a guard that compares the time now with a time the API wrote, such as when a
//! purchase was created, measures by that `Date` ([`Now::of`]).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// The time now, in seconds since 1970, as two readings: the earlier and the later of the
/// API's `Date` and the local clock, or the one clock there is. Which clock is right is not
/// known, and two API servers' clocks can differ too, so a guard reads the one that refuses
/// more: neither a local clock that runs fast nor a `Date` ahead of time lets a repeat
/// through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Now {
    /// The earlier reading: for the age of something dated before now, which is then never
    /// more than either clock says.
    pub earliest: u64,
    /// The later reading: for a time recorded now that a later run measures from, which is
    /// then never earlier than either clock says.
    pub latest: u64,
}

impl Now {
    /// The time now, from `date`, an answer's `Date` header ([`crate::Response::date`]),
    /// and `local`, the local clock. A `Date` that is not an IMF-fixdate is no reading, and a
    /// local clock set before 1970 is none either.
    ///
    /// # Errors
    ///
    /// `internal` when the local clock is set before 1970, whatever the `Date`: a `Date` alone
    /// could be ahead of time and open a guard's window. `action` says what could not be done, as `cannot
    /// <action>`.
    pub fn of(date: Option<&str>, local: SystemTime, action: &str) -> Result<Self> {
        let server = date.and_then(http_date);
        let local = local
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|since| since.as_secs());
        let (earliest, latest) = match (server, local) {
            (Some(server), Some(local)) => (server.min(local), server.max(local)),
            (None, Some(local)) => (local, local),
            // A `Date` alone is not enough: with nothing to check it against, a `Date` ahead
            // of time would open the window, so a local clock that is not a time refuses.
            (_, None) => {
                return Err(Error::internal(format!(
                    "cannot {action}: the system clock is set before 1970"
                ))
                .with_hint(clock_hint()));
            }
        };
        Ok(Self { earliest, latest })
    }
}

/// The hint of a refusal to date a signature: the clock is the one thing the caller can fix,
/// and a server whose `Date` is far off is a bug to report, so the hint keeps both halves of
/// the `internal` hint it replaces.
pub(crate) fn clock_hint() -> String {
    format!(
        "check the system clock, then run this again with `--debug` and report what it \
         prints at `{}`",
        crate::error::ISSUES_URL
    )
}

/// Seconds since the Unix epoch of `date`, an HTTP `Date` in the IMF-fixdate form that RFC
/// 9110 makes servers send (section 5.6.7, <https://www.rfc-editor.org/rfc/rfc9110#section-5.6.7>),
/// such as `Sun, 06 Nov 1994 08:49:37 GMT`, or `None` for any other text. The two obsolete
/// forms that section lets a recipient read are left out on purpose: no server `hipc` talks to
/// sends them, and without a time the caller uses its own clock. The day is checked against
/// the month's length (no 31 Feb), and the weekday is not checked against the date.
pub(crate) fn http_date(date: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_weekday, rest) = date.split_once(", ")?;
    let fields: Vec<&str> = rest.split(' ').collect();
    let [day, month, year, time, "GMT"] = fields[..] else {
        return None;
    };
    let clock: Vec<&str> = time.split(':').collect();
    let [hour, minute, second] = clock[..] else {
        return None;
    };
    let month = MONTHS.iter().position(|m| *m == month)? as u64 + 1;
    let [day, hour, minute, second] = [day, hour, minute, second].map(|text| digits(text, 2));
    let (day, hour, minute, second) = (day?, hour?, minute?, second?);
    civil_seconds(digits(year, 4)?, month, day, [hour, minute, second])
}

/// Seconds since the Unix epoch of a UTC date and time of day, or `None` for a date before
/// 1970 or one that does not exist: a month outside 1 to 12, a day past the month's length
/// (no 31 Feb, and 29 Feb only in a leap year), an hour past 23, or a minute past 59. A second
/// of 60 is a leap second, which both IMF-fixdate and RFC 3339 allow.
fn civil_seconds(year: u64, month: u64, day: u64, [hour, minute, second]: [u64; 3]) -> Option<u64> {
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let last_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if year < 1970 || !(1..=last_day).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_since_epoch(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// `text` as a number when it is exactly `width` ASCII digits.
fn digits(text: &str, width: usize) -> Option<u64> {
    if text.len() != width || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The seconds since 1970 of an RFC 3339 time as the API writes it,
/// `YYYY-MM-DDTHH:MM:SS`, an optional fraction, and `Z` or an offset `+HH:MM` or `-HH:MM`, with
/// `T` and `Z` in either case;
/// `None` for any other text, a date or an offset that does not exist (`02-31`, `+24:00`), or
/// a time before 1970.
pub fn rfc3339_seconds(text: &str) -> Option<u64> {
    rfc3339(text).map(|(seconds, _)| seconds)
}

/// [`rfc3339_seconds`] with the fraction of a second, in nanoseconds: digits past the ninth
/// are dropped, so two times that differ only there compare equal.
pub(crate) fn rfc3339(text: &str) -> Option<(u64, u32)> {
    // RFC 3339 section 5.6 allows a lowercase `t` and `z`.
    let (date, rest) = text.split_once(['T', 't'])?;
    let (time, zone) = rest.split_at(rest.find(['Z', 'z', '+', '-'])?);
    let date: Vec<&str> = date.split('-').collect();
    let [year, month, day] = date[..] else {
        return None;
    };
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let nanos = format!("{:0<9}", &fraction[..fraction.len().min(9)])
        .parse()
        .ok()?;
    let clock: Vec<&str> = clock.split(':').collect();
    let [hour, minute, second] = clock[..] else {
        return None;
    };
    let local = civil_seconds(
        digits(year, 4)?,
        digits(month, 2)?,
        digits(day, 2)?,
        [digits(hour, 2)?, digits(minute, 2)?, digits(second, 2)?],
    )?;
    let offset = if zone.eq_ignore_ascii_case("Z") {
        0
    } else {
        let (sign, hm) = zone.split_at(1);
        let (h, m) = hm.split_once(':')?;
        let (h, m) = (digits(h, 2)?, digits(m, 2)?);
        if h > 23 || m > 59 {
            return None;
        }
        let seconds = i64::try_from(h * 3600 + m * 60).ok()?;
        match sign {
            "+" => seconds,
            "-" => -seconds,
            _ => return None,
        }
    };
    let seconds = u64::try_from(i64::try_from(local).ok()? - offset).ok()?;
    Some((seconds, nanos))
}

/// The days from 1970-01-01 to `year`-`month`-`day` in the proleptic Gregorian calendar, for a
/// year from 1970: Howard Hinnant's `days_from_civil`
/// (<https://howardhinnant.github.io/date_algorithms.html#days_from_civil>), which counts in
/// 400-year eras of 146097 days from a year that starts in March, so that a leap day falls at
/// the end of its year. 719468 is the day of 1970-01-01 in that count.
fn days_since_epoch(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The proleptic Gregorian date `day` days after 1970-01-01, as `YYYY-MM-DD` (Howard
/// Hinnant's `civil_from_days`).
pub fn date_of_unix_day(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const DATE: &str = "Sun, 06 Nov 1994 08:49:37 GMT";
    const SERVER: u64 = 784_111_777;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn two_clocks_give_both_readings_however_close() {
        let now = Now::of(Some(DATE), at(SERVER), "act").unwrap();
        assert_eq!((now.earliest, now.latest), (SERVER, SERVER));
        let now = Now::of(Some(DATE), at(SERVER + 30), "act").unwrap();
        assert_eq!((now.earliest, now.latest), (SERVER, SERVER + 30));
        let now = Now::of(Some(DATE), at(SERVER + 301), "act").unwrap();
        assert_eq!((now.earliest, now.latest), (SERVER, SERVER + 301));
        let now = Now::of(Some(DATE), at(SERVER - 3_600), "act").unwrap();
        assert_eq!((now.earliest, now.latest), (SERVER - 3_600, SERVER));
    }

    #[test]
    fn one_clock_alone_is_the_time() {
        for date in [
            None,
            Some("yesterday"),
            Some("Sun, 06 Nov 1994 08:49:37 UTC"),
        ] {
            let now = Now::of(date, at(SERVER + 9_999), "act").unwrap();
            assert_eq!((now.earliest, now.latest), (SERVER + 9_999, SERVER + 9_999));
        }
        let before_1970 = UNIX_EPOCH - Duration::from_secs(1);
        for date in [None, Some(DATE)] {
            let error = Now::of(date, before_1970, "check the thing").unwrap_err();
            assert_eq!(error.code(), crate::Code::Internal);
            assert_eq!(
                error.to_string(),
                "cannot check the thing: the system clock is set before 1970"
            );
            assert!(error.hint().unwrap().starts_with("check the system clock"));
        }
    }

    #[test]
    fn an_rfc3339_time_is_a_real_date_with_a_real_offset() {
        for (text, seconds, nanos) in [
            ("2024-02-29T00:00:00Z", 1_709_164_800, 0),
            ("2000-02-29T00:00:00Z", 951_782_400, 0),
            ("2026-12-31T23:59:59Z", 1_798_761_599, 0),
            ("2026-11-03T10:00:00.5Z", 1_793_700_000, 500_000_000),
            (
                "2026-11-03T10:00:00.123456789123Z",
                1_793_700_000,
                123_456_789,
            ),
            ("2026-11-03T13:59:00+03:59", 1_793_700_000, 0),
            ("2026-11-03T10:00:00-00:00", 1_793_700_000, 0),
            // RFC 3339 allows `t` and `z` in lowercase.
            ("2026-11-03t10:00:00.5z", 1_793_700_000, 500_000_000),
        ] {
            assert_eq!(rfc3339(text), Some((seconds, nanos)), "{text}");
        }
        for bad in [
            "2026-02-29T00:00:00Z",
            "2100-02-29T00:00:00Z",
            "2026-02-31T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-11-03T10:00:00+24:00",
            "2026-11-03T10:00:00+99:99",
            "2026-11-03T10:00:00+02:60",
            "2026-11-03T10:00:00+2:00",
            "2026-11-03T10:00:00+002:00",
            "2026-1-03T10:00:00Z",
            "2026-11-3T10:00:00Z",
            "26-11-03T10:00:00Z",
            "2026-11-03T1:00:00Z",
            "2026-11-03T10:00:00:00Z",
            "2026-11-03-01T10:00:00Z",
            "2026-11-03T10:00:00Z02:00",
        ] {
            assert_eq!(rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_http_date_is_read_in_the_imf_fixdate_form_only() {
        for (date, seconds) in [
            ("Thu, 01 Jan 1970 00:00:00 GMT", 0),
            ("Sun, 06 Nov 1994 08:49:37 GMT", 784_111_777),
            ("Tue, 29 Feb 2000 12:00:00 GMT", 951_825_600),
            ("Sun, 01 Nov 2026 23:59:59 GMT", 1_793_577_599),
            ("Mon, 01 Mar 2100 00:00:00 GMT", 4_107_542_400),
            // A leap second, which IMF-fixdate allows.
            ("Wed, 31 Dec 2016 23:59:60 GMT", 1_483_228_800),
        ] {
            assert_eq!(http_date(date), Some(seconds), "{date}");
        }
        for date in [
            "",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 06 Nov 1994 08:49:37 GMT ",
            "Sun, 06 Nov 1994 08:49 GMT",
            "Sun, 06 Nov 1994 08:49:37:00 GMT",
            "Sun, 06 Now 1994 08:49:37 GMT",
            "Sun, 6 Nov 1994 08:49:37 GMT",
            "Sun, +6 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 94 08:49:37 GMT",
            "Sun, 06 Nov 1994 8:49:37 GMT",
            "Sun, 06 Nov 1994 08:4x:37 GMT",
            "Sun, 06 Nov 1994 08:49:3 GMT",
            "Wed, 31 Dec 1969 23:59:59 GMT",
            "Sun, 00 Nov 1994 08:49:37 GMT",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "Sun, 31 Nov 1994 08:49:37 GMT",
            "Thu, 29 Feb 2001 00:00:00 GMT",
            "Mon, 29 Feb 2100 00:00:00 GMT",
            "Thu, 30 Feb 2000 00:00:00 GMT",
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "Sun, 06 Nov 1994 08:60:00 GMT",
            "Sun, 06 Nov 1994 08:49:61 GMT",
        ] {
            assert_eq!(http_date(date), None, "{date}");
        }
    }

    /// Every day from 1970 to 2400 is one day after the one before it, by a calendar written
    /// here from the Gregorian leap rule.
    #[test]
    fn each_day_of_the_calendar_is_one_day_after_the_last() {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let mut expected = 0;
        for year in 1970..=2400 {
            let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
            for (month, name) in MONTHS.iter().enumerate() {
                let days = match month {
                    1 if leap => 29,
                    1 => 28,
                    3 | 5 | 8 | 10 => 30,
                    _ => 31,
                };
                for day in 1..=days {
                    let date = format!("Mon, {day:02} {name} {year} 00:00:00 GMT");
                    assert_eq!(http_date(&date), Some(expected), "{date}");
                    expected += 86_400;
                }
            }
        }
    }

    #[test]
    fn rfc3339_times_read_as_seconds_since_1970() {
        for (text, seconds) in [
            ("1970-01-01T00:00:00Z", 0),
            ("2026-11-03T10:00:00Z", 1_793_700_000),
            ("2026-11-03T10:00:00.123456Z", 1_793_700_000),
            ("2026-11-03T12:00:00+02:00", 1_793_700_000),
            ("2026-11-03T05:30:00-04:30", 1_793_700_000),
            ("2024-02-29T23:59:60Z", 1_709_251_200),
        ] {
            assert_eq!(rfc3339_seconds(text), Some(seconds), "{text}");
        }
        for bad in [
            "",
            "2026-11-03",
            "2026-11-03T10:00:00",
            "1969-12-31T23:59:59Z",
            "1970-01-01T00:00:00+00:01",
            "2026-13-03T10:00:00Z",
            "2026-00-03T10:00:00Z",
            "2026-11-32T10:00:00Z",
            "2026-11-00T10:00:00Z",
            "2026-11-03T24:00:00Z",
            "2026-11-03T10:60:00Z",
            "2026-11-03T10:00:61Z",
            "2026-11-03T10:00:00.Z",
            "2026-11-03T10:00:00.1aZ",
            "2026-11-03T10:00Z",
            "2026-11-03T10:00:00+0200",
            "2026-11-03T10:00:00+xx:00",
            "2026-11-03T10:00:00+02:xx",
            "2026-x1-03T10:00:00Z",
            "2026-11-03T1x:00:00Z",
            "2026-11-03T10:00:00Q",
            "2026-11-03T10:00:00Z02:00",
        ] {
            assert_eq!(rfc3339_seconds(bad), None, "{bad}");
        }
    }

    /// The date of a day count, checked against dates worked out by hand, a leap day and
    /// the turn of a century included.
    #[test]
    fn a_unix_day_is_its_utc_date() {
        for (day, date) in [
            (0, "1970-01-01"),
            (-1, "1969-12-31"),
            (2_932_896, "9999-12-31"),
            (59, "1970-03-01"),
            (11_016, "2000-02-29"),
            (20_784, "2026-11-27"),
            (47_541, "2100-03-01"),
        ] {
            assert_eq!(date_of_unix_day(day), date, "{day}");
        }
    }
}
