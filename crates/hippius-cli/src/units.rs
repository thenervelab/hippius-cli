//! Exact decimal amounts: the one grammar every amount `hipc` reads or compares is held to, and
//! the money types built on it.
//!
//! An amount is plain digits, then optionally a point and more digits: no sign, no exponent,
//! no spaces, and no `+`, so `+7`, `1e3`, `.5`, and `5.` are refused wherever an amount is
//! taken. No float is involved anywhere: an amount is kept as its digits or as an integer count
//! of its smallest unit, so the amount sent is the amount typed.

use std::cmp::Ordering;
use std::fmt;

/// Base units per credit and per hAlpha, as a power of ten: swagger's "10^18 = 1 credit", and
/// the `decimals` the console reads from the same answers.
pub(crate) const DECIMALS: u32 = 18;

/// Planck in one hAlpha or one credit: 10 to the [`DECIMALS`].
pub(crate) const ONE: u128 = 10_u128.pow(DECIMALS);

/// A decimal in the one grammar: its whole digits and its fraction digits, as written.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Decimal<'a> {
    whole: &'a str,
    fraction: &'a str,
}

impl<'a> Decimal<'a> {
    /// `text` when it is one or more digits, then optionally a point and one or more digits.
    pub(crate) fn parse(text: &'a str) -> Option<Self> {
        let (whole, fraction) = match text.split_once('.') {
            Some((whole, fraction)) if !fraction.is_empty() => (whole, fraction),
            Some(_) => return None,
            None => (text, ""),
        };
        let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        (!whole.is_empty() && digits(whole) && digits(fraction)).then_some(Self { whole, fraction })
    }

    /// The whole digits as written.
    pub(crate) fn whole(self) -> &'a str {
        self.whole
    }

    /// The fraction digits as written, empty when there is no point.
    pub(crate) fn fraction(self) -> &'a str {
        self.fraction
    }

    /// The digits that hold value: the whole part without leading zeros and the fraction
    /// without trailing zeros, so `050.10` is `("50", "1")` and `0` is `("", "")`.
    pub(crate) fn significant(self) -> (&'a str, &'a str) {
        (
            self.whole.trim_start_matches('0'),
            self.fraction.trim_end_matches('0'),
        )
    }

    /// The amount in its normal form: no leading zeros before the point and no trailing zeros
    /// after it, so the question and the body show `50` for `050.00`.
    pub(crate) fn normal(self) -> String {
        let (whole, fraction) = self.significant();
        let whole = if whole.is_empty() { "0" } else { whole };
        if fraction.is_empty() {
            whole.to_owned()
        } else {
            format!("{whole}.{fraction}")
        }
    }
}

/// `text` as a `u64` when it is plain digits: unlike `str::parse`, a `+` is refused.
pub(crate) fn whole_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// A credit amount a flag sends, in its normal form ([`Decimal::normal`]). A credit is 10^18
/// base units, so a finer amount cannot be stored as sent, and an update that compares the
/// answer exactly with the amount sent could never see it: more than 18 digits that hold value
/// after the point are refused. Trailing zeros are dropped first, so the server never gets
/// more than 18 written ones. `what` names the amount with its article (`a cap`).
pub(crate) fn credits(text: &str, what: &str) -> Result<String, String> {
    let Some(amount) = Decimal::parse(text) else {
        return Err(format!(
            "{what} is a decimal number of credits, such as `50` or `20.5`"
        ));
    };
    if amount.significant().1.len() > DECIMALS as usize {
        return Err(format!(
            "{what} has at most {DECIMALS} digits after the point, the smallest part of a credit"
        ));
    }
    Ok(amount.normal())
}

/// Compares two credit amounts exactly, as decimal digits, so `50`, `50.0`, and `050` are the
/// same cap and no float rounding decides whether a change is a raise. `None` when either is
/// not a decimal in the grammar, such as a cap the server sent in another form.
pub(crate) fn compare(new: &str, old: &str) -> Option<Ordering> {
    let (new, old) = (
        Decimal::parse(new)?.significant(),
        Decimal::parse(old)?.significant(),
    );
    Some(
        new.0
            .len()
            .cmp(&old.0.len())
            .then_with(|| new.0.cmp(old.0))
            .then_with(|| new.1.cmp(old.1)),
    )
}

/// An amount in planck, the 18-decimal base unit of hAlpha and of credits: [`ONE`] is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Planck(pub(crate) u128);

impl Planck {
    /// A decimal of `unit`, which has 18 decimals, to its exact count of planck. `examples`
    /// shows the refusal's reader what an amount looks like.
    pub(crate) fn parse(text: &str, unit: &str, examples: &str) -> Result<Self, String> {
        let Some(amount) = Decimal::parse(text) else {
            return Err(format!("an amount is {unit} in digits, such as {examples}"));
        };
        if amount.fraction().len() > DECIMALS as usize {
            return Err(format!(
                "{unit} has {DECIMALS} decimals, so an amount has at most {DECIMALS} digits after \
                 the point"
            ));
        }
        let too_large = || format!("the amount is larger than any {unit} balance");
        let whole: u128 = amount.whole().parse().map_err(|_| too_large())?;
        let fraction: u128 = format!("{:0<18}", amount.fraction())
            .parse()
            .expect("checked: 18 ASCII digits");
        whole
            .checked_mul(ONE)
            .and_then(|w| w.checked_add(fraction))
            .map(Self)
            .ok_or_else(too_large)
    }
}

impl fmt::Display for Planck {
    /// The amount as a decimal, exactly: `1.5`, `0.01`, `2`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fraction = format!("{:018}", self.0 % ONE);
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            write!(f, "{}", self.0 / ONE)
        } else {
            write!(f, "{}.{fraction}", self.0 / ONE)
        }
    }
}

/// An amount of US dollars, in cents.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub(crate) struct Usd {
    cents: u64,
}

impl Usd {
    pub(crate) const fn from_cents(cents: u64) -> Self {
        Self { cents }
    }
}

impl fmt::Display for Usd {
    /// The amount as the top-up routes take it: dollars, a point, and two digits.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:02}", self.cents / 100, self.cents % 100)
    }
}

/// `--amount`: dollars in the grammar, with at most two digits after a point. At most nine
/// digits before the point (arbitrary, far above any top-up), so the cents fit in a `u64`.
pub(crate) fn usd(text: &str) -> Result<Usd, String> {
    let refusal = || "an amount is US dollars to the cent, such as `50` or `12.50`".to_owned();
    let amount = Decimal::parse(text).ok_or_else(refusal)?;
    let (dollars, cents) = (amount.whole(), amount.fraction());
    if dollars.len() > 9 || cents.len() > 2 {
        return Err(refusal());
    }
    let parse = |part: &str| {
        part.parse::<u64>()
            .expect("checked: one to nine ASCII digits")
    };
    // One digit after the point is tenths: `20.5` is 20 dollars and 50 cents.
    let cents = match cents.len() {
        0 => 0,
        1 => parse(cents) * 10,
        _ => parse(cents),
    };
    Ok(Usd::from_cents(parse(dollars) * 100 + cents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decimal_is_digits_with_an_optional_fraction() {
        for (text, whole, fraction, normal) in [
            ("0", "0", "", "0"),
            ("050.10", "050", "10", "50.1"),
            ("20.5", "20", "5", "20.5"),
            ("000.000", "000", "000", "0"),
        ] {
            let amount = Decimal::parse(text).unwrap();
            assert_eq!(
                (amount.whole(), amount.fraction()),
                (whole, fraction),
                "{text}"
            );
            assert_eq!(amount.normal(), normal, "{text}");
        }
        for bad in [
            "", ".", ".5", "5.", "+7", "-7", "1e3", " 1", "1.2.3", "1,5", "١",
        ] {
            assert!(Decimal::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_whole_number_is_plain_digits() {
        assert_eq!(whole_number("007"), Some(7));
        assert_eq!(whole_number(&u64::MAX.to_string()), Some(u64::MAX));
        for bad in ["", "+7", "-7", "7 ", "18446744073709551616"] {
            assert_eq!(whole_number(bad), None, "{bad}");
        }
    }

    #[test]
    fn credits_keep_18_decimals_that_hold_value() {
        assert_eq!(credits("050.500", "a cap"), Ok("50.5".to_owned()));
        let finest = format!("0.{}1", "0".repeat(17));
        assert_eq!(credits(&finest, "a cap"), Ok(finest.clone()));
        assert_eq!(credits(&format!("{finest}000"), "a cap"), Ok(finest));
        assert_eq!(
            credits(&format!("0.{}1", "0".repeat(18)), "a cap"),
            Err(
                "a cap has at most 18 digits after the point, the smallest part of a credit"
                    .to_owned()
            )
        );
        assert_eq!(
            credits("+5", "a cap"),
            Err("a cap is a decimal number of credits, such as `50` or `20.5`".to_owned())
        );
    }

    #[test]
    fn credit_amounts_compare_exactly() {
        assert_eq!(compare("50", "050.0"), Some(Ordering::Equal));
        assert_eq!(compare("50.1", "50.09"), Some(Ordering::Greater));
        assert_eq!(compare("9", "10"), Some(Ordering::Less));
        assert_eq!(compare("0.5", "0"), Some(Ordering::Greater));
        assert_eq!(compare("1", "1e3"), None);
        assert_eq!(compare("x", "1"), None);
    }

    #[test]
    fn planck_reads_a_decimal_of_its_unit() {
        let parse = |text| Planck::parse(text, "hAlpha", "`1.5`");
        assert_eq!(parse("1.5"), Ok(Planck(ONE + ONE / 2)));
        assert_eq!(parse("0.000000000000000001"), Ok(Planck(1)));
        assert_eq!(
            parse("0.0000000000000000001"),
            Err(
                "hAlpha has 18 decimals, so an amount has at most 18 digits after the point"
                    .to_owned()
            )
        );
        assert_eq!(
            parse("1."),
            Err("an amount is hAlpha in digits, such as `1.5`".to_owned())
        );
        assert_eq!(
            parse(&"9".repeat(40)),
            Err("the amount is larger than any hAlpha balance".to_owned())
        );
    }

    #[test]
    fn planck_prints_as_an_exact_decimal() {
        for (planck, text) in [
            (0, "0"),
            (1, "0.000000000000000001"),
            (ONE, "1"),
            (ONE / 100, "0.01"),
            (7_425_000_000_000_000_000, "7.425"),
            (u128::MAX, "340282366920938463463.374607431768211455"),
        ] {
            assert_eq!(Planck(planck).to_string(), text);
        }
    }

    #[test]
    fn usd_reads_every_spelling_of_an_amount_back_to_its_cents() {
        // Every cent up to past both minimums and around both maximums, and a prime stride
        // across the rest, which keeps the run short in a debug build.
        let cents = (0..=200_000)
            .chain(999_000..=1_001_000)
            .chain(9_999_000..=10_001_000)
            .chain((200_001..=99_999_999_999).step_by(999_983));
        for cents in cents {
            let amount = Usd::from_cents(cents);
            let written = amount.to_string();
            assert_eq!(usd(&written), Ok(amount), "{written}");
            if cents < 10_000_000_000 {
                assert_eq!(usd(&format!("0{written}")), Ok(amount), "{written}");
            }
            if cents % 10 == 0 {
                let tenths = &written[..written.len() - 1];
                assert_eq!(usd(tenths), Ok(amount), "{tenths}");
            }
            if cents % 100 == 0 {
                let dollars = (cents / 100).to_string();
                assert_eq!(usd(&dollars), Ok(amount), "{dollars}");
            }
        }
        assert!(usd("1000000000").is_err(), "ten digits before the point");
        assert!(usd("999999999.99").is_ok());
    }
}
