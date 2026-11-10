//! The guard on a write that replaces a whole list: a VM's firewall rules, a CDN zone's
//! cache rules, and a member's grants.
//!
//! A one-entry file meant as "add this entry" drops every other entry, and an agent passing
//! `--yes` never reads the question that says so. So unless `--remove-others` is passed,
//! the command reads the current list first and refuses with `remove_others_required` when
//! the new list leaves out an entry the list has now. The read names the target (tenet 5):
//! it runs after the dry-run check, the account refusal, and the token check, and before
//! the question. With `--remove-others` nothing is compared: a list whose write carries the
//! version its read returns as `If-Match` (a CDN zone's WAF rules and rate limits) is still
//! read, after the question, for that version alone; any other list is not read.
//!
//! Each list says what one entry is through its key ([`Entry::new`]): two entries with
//! equal keys are the same entry. The lists are compared as multisets: each entry of the
//! new list keeps at most one current entry with its key, so two current entries with one
//! key need two in the new list. Fields a key leaves out (a rule's description, a cache
//! rule's actions) are edits of an entry that stays. A list whose order matters puts the
//! position in the key (the cache rules).

use std::collections::HashMap;

use hippius::{Client, Error, Request, Result, phrase};
use serde_json::Value;

use crate::commands::Printed;
use crate::context::{Context, Write};

/// One entry of a list, by its key.
#[derive(Debug)]
pub(crate) struct Entry(Value);

impl Entry {
    /// The entry whose key is `key`: two entries with equal keys are one entry.
    pub(crate) fn new(key: Value) -> Self {
        Self(key)
    }

    /// The key as compact JSON with every object's fields sorted: two keys are equal exactly
    /// when their texts are. `serde_json` keeps the order fields were written in, and its
    /// equality ignores that order, so the text sorts them to agree with it.
    fn text(&self) -> String {
        let mut key = self.0.clone();
        key.sort_all_objects();
        key.to_string()
    }

    /// The entry as a refusal names it: a text key as it is, any other as compact JSON, in
    /// backticks. A key may hold text from the server, which [`phrase::quote`] keeps inside
    /// the literal.
    fn label(&self) -> String {
        match &self.0 {
            Value::String(text) => phrase::quote(text),
            key => phrase::quote(key),
        }
    }
}

/// A write that replaces a whole list, and the read that finds what it would remove.
#[derive(Debug)]
pub(crate) struct Replace {
    /// The write, with its action ([`Request::about`]).
    pub(crate) write: Request,
    /// What follows from the write, for the question.
    pub(crate) consequence: Option<&'static str>,
    /// The new list's entries.
    pub(crate) entries: Vec<Entry>,
    /// `--remove-others`: the write may remove entries, so nothing is compared, and the
    /// list is read only for the version in [`Replace::draft_if_match`].
    pub(crate) remove_others: bool,
    /// The read of the current list, with its action.
    pub(crate) read: Request,
    /// The body a dry run shows for `write` when the read fills part of it, with a
    /// placeholder there, as [`Found::body`] fills it in a real run; `None` when the read
    /// leaves the write as it is.
    pub(crate) draft_body: Option<Value>,
    /// The `If-Match` a dry run shows for `write` when the read gives it, a placeholder, as
    /// [`Found::if_match`] gives it in a real run; `None` when the read gives none. `Some`
    /// also makes a `--remove-others` write read the list, after the question, for that
    /// version ([`replace_list`]).
    pub(crate) draft_if_match: Option<&'static str>,
    /// One entry, as a count names it, such as `inbound rule`: it takes a plain `s` in the
    /// plural.
    pub(crate) noun: &'static str,
    /// What holds the list, as a message names it, such as "the VM `vm_1`".
    pub(crate) owner: String,
}

/// What the read of the current list found.
#[derive(Debug)]
pub(crate) struct Found {
    /// The entries the list has now.
    pub(crate) entries: Vec<Entry>,
    /// A JSON body to send in place of the body of [`Replace::write`], as a firewall PUT
    /// that carries the revision it read; `None` sends that write as it is. Only the body
    /// can change, so the write keeps the route `check_write` and the dry run saw.
    pub(crate) body: Option<Value>,
    /// The version the read saw, sent as `If-Match` on the write, so a list another client
    /// published between the read and the write is refused (412) rather than overwritten;
    /// `None` sends no precondition.
    pub(crate) if_match: Option<String>,
}

impl Found {
    /// What a read found that leaves the write as it is.
    pub(crate) fn new(entries: Vec<Entry>) -> Self {
        Self {
            entries,
            body: None,
            if_match: None,
        }
    }
}

/// Sends `replace.write` and returns the server's body, with [`Found::body`] as its body and
/// [`Found::if_match`] as its `If-Match` when the read gave them.
///
/// Unless `remove_others`, it first sends `replace.read`, finds the current entries in its
/// answer with `current`, and refuses the write when the new list leaves out any of them, all
/// before the question. With `remove_others` and a [`Replace::draft_if_match`], it sends the
/// read after the question, for the version alone: the flag lets the write remove entries,
/// not overwrite a list another client published since. With `remove_others` and no version
/// to send, nothing is read. `current` fails closed: an answer it cannot read is its error
/// ([`crate::commands::unreadable`]), never an empty list, so a shape `hipc` does not expect cannot turn the
/// guard off.
///
/// A dry run reads nothing. When a real run would read, its document is the write with
/// [`Replace::draft_body`] and [`Replace::draft_if_match`], and adds `reads_first`, the read,
/// and `requests`, the read and then the write; otherwise it is the write as it is.
///
/// # Errors
///
/// `remove_others_required` for a write that would remove entries; the refusals of
/// [`Context::check_write`] and [`Context::confirm_write`]; the read's error; and the error
/// of `current`.
pub(crate) fn replace_list(
    ctx: &Context<'_>,
    client: &Client,
    replace: &Replace,
    current: impl FnOnce(Value) -> Result<Found>,
) -> Result<Printed> {
    let reads = !replace.remove_others || replace.draft_if_match.is_some();
    let draft = if reads {
        filled(
            &replace.write,
            replace.draft_body.as_ref(),
            replace.draft_if_match,
        )
    } else {
        replace.write.clone()
    };
    let checked = match ctx.check_write(client, &draft)? {
        Write::DryRun(mut doc) => {
            if reads {
                doc["reads_first"] = ctx.describe(client, &replace.read)?;
                doc["requests"] = ctx.describe_each(client, &[&replace.read, &draft])?;
            }
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let action = replace.write.action();
    let found = if replace.remove_others {
        ctx.confirm_write(checked, &action, replace.consequence)?;
        // Nothing is compared, so the read gives only the version. A read the command needs
        // for itself comes after the question (tenet 5), which also keeps the version fresh.
        if reads {
            Some(current(ctx.send(client, &replace.read)?.document()?)?)
        } else {
            None
        }
    } else {
        let found = current(ctx.send(client, &replace.read)?.document()?)?;
        refuse_removal(ctx, replace, &found)?;
        ctx.confirm_write(checked, &action, replace.consequence)?;
        Some(found)
    };
    let write = match &found {
        Some(found) => filled(
            &replace.write,
            found.body.as_ref(),
            found.if_match.as_deref(),
        ),
        None => replace.write.clone(),
    };
    let body = ctx.send(client, &write)?.document()?;
    Ok(Printed::Document(body))
}

/// Refuses, without `--remove-others`, a new list that leaves out an entry of `found`, the
/// list the read found.
///
/// # Errors
///
/// `remove_others_required`, naming each entry the new list would remove.
pub(crate) fn refuse_removal(ctx: &Context<'_>, replace: &Replace, found: &Found) -> Result<()> {
    if replace.remove_others {
        return Ok(());
    }
    let removed = removed(&found.entries, &replace.entries);
    if removed.is_empty() {
        return Ok(());
    }
    let message = format!(
        "this would remove {} that {} has now, and `--remove-others` was not passed",
        phrase::count(removed.len() as u64, replace.noun),
        replace.owner
    );
    Err(Error::remove_others_required(message, ctx.args(), &removed))
}

/// `write` with `body` as its body and `if_match` as its `If-Match`, each when given: what a
/// read fills in, or a dry run's placeholders for it.
fn filled(write: &Request, body: Option<&Value>, if_match: Option<&str>) -> Request {
    let mut write = write.clone();
    if let Some(body) = body {
        write = write.json(body);
    }
    if let Some(version) = if_match {
        write = write.if_match(version);
    }
    write
}

/// The labels of the entries of `now` that the entries of `new` do not keep, in the order
/// of `now`. Each entry of `new` keeps at most one entry of `now` with its key, so a key
/// held twice now and once in `new` removes one of the two.
///
/// `new` is counted once into a multiset, so the comparison is linear: a WAF list's item file
/// may hold about 70 000 lines (1 MiB), and a search of `new` per entry of `now` would be
/// quadratic.
fn removed(now: &[Entry], new: &[Entry]) -> Vec<String> {
    let mut unused: HashMap<String, usize> = HashMap::with_capacity(new.len());
    for entry in new {
        *unused.entry(entry.text()).or_default() += 1;
    }
    let mut removed = Vec::new();
    for entry in now {
        match unused.get_mut(&entry.text()) {
            Some(count) if *count > 0 => *count -= 1,
            _ => removed.push(entry.label()),
        }
    }
    removed
}

/// `items` as a set: sorted by their JSON text, each once, so two lists that hold the same
/// items in another order or with a repeat compare equal.
pub(crate) fn set(mut items: Vec<Value>) -> Value {
    items.sort_by_key(Value::to_string);
    items.dedup();
    Value::Array(items)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entries(keys: &[Value]) -> Vec<Entry> {
        keys.iter().cloned().map(Entry::new).collect()
    }

    #[test]
    fn an_entry_is_removed_when_no_new_entry_keeps_it() {
        let now = entries(&[json!("a"), json!({"k": 1}), json!("a"), json!("c")]);
        let new = entries(&[json!("c"), json!("d")]);
        assert_eq!(removed(&now, &new), ["`a`", r#"`{"k":1}`"#, "`a`"]);
        assert_eq!(removed(&now, &now), Vec::<String>::new());
        assert_eq!(removed(&[], &new), Vec::<String>::new());
        // A key held twice needs two entries in the new list: one keeps only one.
        let new = entries(&[json!("a"), json!({"k": 1}), json!("c")]);
        assert_eq!(removed(&now, &new), ["`a`"]);
    }

    #[test]
    fn duplicates_count_on_both_sides_and_field_order_does_not() {
        let now = entries(&[
            json!("a"),
            json!("a"),
            json!("a"),
            json!({"x": 1, "y": [2]}),
        ]);
        // Two of three `a` kept, and an extra `b` in the new list keeps nothing.
        let new = entries(&[json!("b"), json!("a"), json!("b"), json!("a")]);
        assert_eq!(removed(&now, &new), ["`a`", r#"`{"x":1,"y":[2]}`"#]);
        // The same object with its fields in another order, at any depth, is the same key.
        let now = entries(&[json!({"m": {"p": 1, "q": 2}, "n": 0})]);
        let new = entries(&[json!({"n": 0, "m": {"q": 2, "p": 1}})]);
        assert_eq!(removed(&now, &new), Vec::<String>::new());
        // A number written another way is another key, as `Value` equality has it.
        let new = entries(&[json!({"n": 0.0, "m": {"q": 2, "p": 1}})]);
        assert_eq!(removed(&now, &new).len(), 1);
        // More `new` entries than `now` ones remove nothing.
        assert_eq!(removed(&now[..0], &now), Vec::<String>::new());
    }

    #[test]
    fn a_label_keeps_server_text_inside_its_literal() {
        assert_eq!(Entry::new(json!("a`b")).label(), r"`a\u{60}b`");
        assert_eq!(Entry::new(json!({"g": "`"})).label(), r#"`{"g":"\u{60}"}`"#);
    }

    #[test]
    fn a_set_ignores_order_and_repeats() {
        assert_eq!(
            set(vec![json!("b"), json!("a"), json!("b")]),
            json!(["a", "b"])
        );
        assert_eq!(set(Vec::new()), json!([]));
    }
}
