//! The time now by the API's clock, for a guard that compares the time now with a time the API
//! wrote, such as when a purchase was created.
//!
//! The local clock can be wrong, or step between two runs (an NTP correction). The API dated
//! what it wrote by its own clock, and says what that clock reads in each answer's `Date`
//! header, so a guard measures by that `Date` ([`Now::of`]).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::kubernetes::{clock_hint, http_date};

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
    /// `internal` when there is no reading at all: the local clock is set before 1970 and the
    /// answer had no `Date` `hipc` reads. `action` says what could not be done, as `cannot
    /// <action>`.
    pub fn of(date: Option<&str>, local: SystemTime, action: &str) -> Result<Self> {
        let server = date.and_then(http_date);
        let local = local
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|since| since.as_secs());
        let (earliest, latest) = match (server, local) {
            (Some(server), Some(local)) => (server.min(local), server.max(local)),
            (Some(one), _) | (None, Some(one)) => (one, one),
            (None, None) => {
                return Err(Error::internal(format!(
                    "cannot {action}: the system clock is set before 1970, and the API's answer \
                     had no `Date` to read the time from"
                ))
                .with_hint(clock_hint()));
            }
        };
        Ok(Self { earliest, latest })
    }
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
        let now = Now::of(Some(DATE), before_1970, "act").unwrap();
        assert_eq!((now.earliest, now.latest), (SERVER, SERVER));
        let error = Now::of(None, before_1970, "check the thing").unwrap_err();
        assert_eq!(error.code(), crate::Code::Internal);
        assert_eq!(
            error.to_string(),
            "cannot check the thing: the system clock is set before 1970, and the API's answer \
             had no `Date` to read the time from"
        );
        assert!(error.hint().unwrap().starts_with("check the system clock"));
    }
}
