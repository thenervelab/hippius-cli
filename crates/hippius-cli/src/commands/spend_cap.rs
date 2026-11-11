//! The question a monthly spend cap update asks, for `hipc compute runners limit update` and
//! `hipc cdn cap update`: a change that may let more of the balance be spent is confirmed
//! (tenet 5), and one that cannot is sent unasked.

use std::cmp::Ordering;

use hippius::phrase;

use crate::commands::credits::amount;

/// A spend cap, as the question names it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Cap<'a> {
    /// The cap, such as "the monthly runner spend cap".
    pub(crate) named: &'a str,
    /// What the read is, which holds `monthly_cap_credits`, such as "the limit".
    pub(crate) read: &'static str,
    /// Who may spend more once it is raised, such as "runners".
    pub(crate) spender: &'static str,
}

/// The cap an update sets.
#[derive(Debug, Clone, Copy)]
pub(crate) enum NewCap<'a> {
    /// This many credits, in the parser's normal form.
    Credits(&'a str),
    /// No cap.
    None,
}

/// What the read says of the cap in force.
#[derive(Debug)]
pub(crate) enum InForce {
    /// `null`: no cap, or a server default; which is unconfirmed.
    Null,
    /// No amount: `monthly_cap_credits` is missing, or the answer is not an object.
    Unknown,
    /// `monthly_cap_credits`, as text, which [`amount`] reads as an amount or not.
    Credits(String),
}

/// The action and the consequence of the question an update asks, or `None` when it sends
/// unasked. `order` is how the new cap compares with the cap in force, `None` when there is
/// no amount to compare (the CDN cap compares its exact base units first, so it may know the
/// order with no `monthly_cap_credits`).
pub(crate) fn question(
    cap: Cap<'_>,
    new: NewCap<'_>,
    in_force: &InForce,
    order: Option<Ordering>,
) -> Option<(String, String)> {
    let Cap {
        named,
        read,
        spender,
    } = cap;
    let more = format!("{spender} may then spend more of the balance each month");
    let unknown = format!(
        "if the cap in force is higher than it shows, {spender} may then spend more of the \
         balance each month"
    );
    let no_amount = format!("{read} has no amount in `monthly_cap_credits`");
    let not_an_amount = |old: &str| {
        format!(
            "where `monthly_cap_credits` is {}, which is not an amount of credits",
            phrase::quote(old)
        )
    };
    let new = match new {
        NewCap::None => {
            return match in_force {
                // Removing a cap that already reads `null` sends the same `null` back.
                InForce::Null => None,
                InForce::Unknown => Some((
                    format!("remove {named}, whose current value is unknown: {no_amount}"),
                    unknown,
                )),
                InForce::Credits(old) if amount(old).is_some() => Some((
                    format!("remove {named} of {} credits", phrase::quote(old)),
                    more,
                )),
                InForce::Credits(old) => {
                    Some((format!("remove {named}, {}", not_an_amount(old)), unknown))
                }
            };
        }
        NewCap::Credits(new) => new,
    };
    let set = format!("set {named} to `{new}` credits");
    match (in_force, order) {
        // Whether `null` means no cap, rather than a server default, is unconfirmed, so a
        // cap set over it is asked about.
        (InForce::Null, _) => Some((
            format!("{set}, where {read} shows `null`, unset or a server default"),
            unknown,
        )),
        (_, Some(Ordering::Less | Ordering::Equal)) => None,
        (InForce::Credits(old), Some(Ordering::Greater)) => Some((
            format!(
                "raise {named} from {} to `{new}` credits",
                phrase::quote(old)
            ),
            more,
        )),
        (InForce::Unknown, Some(Ordering::Greater)) => {
            Some((format!("raise {named} to `{new}` credits"), more))
        }
        (InForce::Credits(old), None) => Some((format!("{set}, {}", not_an_amount(old)), unknown)),
        (InForce::Unknown, None) => Some((
            format!("{set}, where the cap in force is unknown: {no_amount}"),
            unknown,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNERS: Cap<'static> = Cap {
        named: "the runner cap",
        read: "the limit",
        spender: "runners",
    };

    const MORE: &str = "runners may then spend more of the balance each month";

    const UNKNOWN: &str = "if the cap in force is higher than it shows, runners may then spend \
                           more of the balance each month";

    fn credits(text: &str) -> InForce {
        InForce::Credits(text.to_owned())
    }

    /// A cut, or the same cap, is sent unasked, and so is a removal over `null`; a raise says
    /// spending may grow, and a change over a cap that cannot be compared says it may.
    #[test]
    fn only_a_change_that_may_spend_more_is_asked() {
        let ask = |new, in_force: &InForce, order| question(RUNNERS, new, in_force, order);
        let asked =
            |action: &str, consequence: &str| Some((action.to_owned(), consequence.to_owned()));
        let ten = NewCap::Credits("10");
        assert_eq!(ask(ten, &credits("20"), Some(Ordering::Less)), None);
        assert_eq!(ask(ten, &credits("10"), Some(Ordering::Equal)), None);
        assert_eq!(ask(ten, &InForce::Unknown, Some(Ordering::Less)), None);
        assert_eq!(ask(NewCap::None, &InForce::Null, None), None);
        assert_eq!(
            ask(ten, &credits("5"), Some(Ordering::Greater)),
            asked("raise the runner cap from `5` to `10` credits", MORE)
        );
        assert_eq!(
            ask(ten, &InForce::Unknown, Some(Ordering::Greater)),
            asked("raise the runner cap to `10` credits", MORE)
        );
        assert_eq!(
            ask(ten, &InForce::Null, Some(Ordering::Less)),
            asked(
                "set the runner cap to `10` credits, where the limit shows `null`, unset or a \
                 server default",
                UNKNOWN
            )
        );
        assert_eq!(
            ask(ten, &credits("x"), None),
            asked(
                "set the runner cap to `10` credits, where `monthly_cap_credits` is `x`, which \
                 is not an amount of credits",
                UNKNOWN
            )
        );
        assert_eq!(
            ask(ten, &InForce::Unknown, None),
            asked(
                "set the runner cap to `10` credits, where the cap in force is unknown: the \
                 limit has no amount in `monthly_cap_credits`",
                UNKNOWN
            )
        );
        assert_eq!(
            ask(NewCap::None, &credits("5"), None),
            asked("remove the runner cap of `5` credits", MORE)
        );
        assert_eq!(
            ask(NewCap::None, &credits("x"), None),
            asked(
                "remove the runner cap, where `monthly_cap_credits` is `x`, which is not an \
                 amount of credits",
                UNKNOWN
            )
        );
        assert_eq!(
            ask(NewCap::None, &InForce::Unknown, None),
            asked(
                "remove the runner cap, whose current value is unknown: the limit has no amount \
                 in `monthly_cap_credits`",
                UNKNOWN
            )
        );
    }
}
