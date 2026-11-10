//! The list shapes of the console API, and the list envelope `hipc` prints for them
//! (`TENETS.md` tenet 6).
//!
//! A list endpoint answers either a bare JSON array, or Django REST framework's
//! `{count, next, previous, results}` when it paginates. [`page`] reads either into a
//! [`Page`]. [`envelope`] and [`Page::envelope`] build
//! `{"items": [...], "next_page_token": <string or null>}`. The token is the `page` number
//! of the `next` link, as a decimal string, so `--page-token` can ask for it. A DRF page's
//! other fields, `count` among them, stay beside `items`, as the fields of a list answered
//! inside an object do; its `next` and `previous` links do not, because the token
//! says where the list goes on.
//!
//! One route pages by cursor instead: `GET /api/runners/jobs/` answers
//! `{results, next_before}`. [`cursor_page`] reads that shape, and only where a caller asks
//! for it, so an object that happens to hold `results` is never taken for a cursor page.
//! Its token is the cursor, as a decimal string.

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::phrase::quote;
use crate::url::without_query_values;

/// One page of a list answer.
#[derive(Debug)]
pub struct Page {
    /// The items on this page.
    pub items: Vec<Value>,
    /// The `next` link of a paginated answer, or `None` on the last page and for a bare
    /// array.
    pub next: Option<String>,
    /// The answer's other fields, such as `count`, kept beside the items in the envelope:
    /// everything but `results` and the `next` and `previous` links. Empty for a bare
    /// array.
    pub fields: Map<String, Value>,
}

/// Reads a list answer: a bare array, or a paginated `{results, next}` object.
///
/// # Errors
///
/// `internal` for any other shape.
pub fn page(answer: Value) -> Result<Page> {
    match answer {
        Value::Array(items) => Ok(Page {
            items,
            next: None,
            fields: Map::new(),
        }),
        Value::Object(mut map) => match map.shift_remove("results") {
            Some(Value::Array(items)) => page_beside(items, map),
            _ => Err(not_a_list()),
        },
        _ => Err(not_a_list()),
    }
}

/// Reads a list answered inside an object, `items` taken from it and `fields` the rest,
/// such as `{buckets, count}`: its `next` and `previous` links are read as [`page`] reads
/// DRF's, so a route that adds pages is caught where it is read whole, rather than its
/// `next` link printed beside a `next_page_token` of null.
///
/// # Errors
///
/// `internal` when `next` is neither a string nor null.
pub fn page_beside(items: Vec<Value>, mut fields: Map<String, Value>) -> Result<Page> {
    let next = match fields.shift_remove("next") {
        None | Some(Value::Null) => None,
        Some(Value::String(next)) => Some(next),
        // Treating a malformed link as the last page would print a short list as if it
        // were complete.
        Some(_) => return Err(not_a_list()),
    };
    fields.shift_remove("previous");
    Ok(Page {
        items,
        next,
        fields,
    })
}

impl Page {
    /// The `page` number of the `next` link, the token `--page-token` takes.
    ///
    /// # Errors
    ///
    /// `internal` when there is a `next` link without a numeric `page` parameter: it cannot
    /// be expressed as a token, and dropping it would hide that more pages exist.
    pub fn next_page_token(&self) -> Result<Option<String>> {
        let Some(next) = &self.next else {
            return Ok(None);
        };
        page_number(next).map(Some).ok_or_else(|| {
            Error::internal(format!(
                "the API's `next` link {} has no page number",
                quote(without_query_values(next))
            ))
        })
    }

    /// This page as the envelope `hipc` prints for one page of a longer list.
    ///
    /// # Errors
    ///
    /// As [`Page::next_page_token`].
    pub fn envelope(self) -> Result<Value> {
        let token = self.next_page_token()?;
        Ok(page_envelope(self.items, token, self.fields))
    }

    /// The envelope of an answer `hipc` reads as the whole list, as
    /// [`Page::into_complete`] reads it, with the answer's other fields beside the items.
    ///
    /// # Errors
    ///
    /// As [`Page::into_complete`].
    pub fn complete_envelope(mut self, path: &str) -> Result<Value> {
        let fields = std::mem::take(&mut self.fields);
        Ok(envelope_beside(self.into_complete(path)?, fields))
    }

    /// The items of an answer `hipc` reads as the whole list: a catalog, or the rows a
    /// guard must all see (tenet 6). `path` is the route read, as `hipc api` takes it, such
    /// as `/accounts/`, for the hint. The hint has no `--all`: `hipc api --all` pages only
    /// `results` or a bare array, and many of these lists are answered inside an object.
    ///
    /// # Errors
    ///
    /// `internal` when the answer has a `next` link. Dropping it would print, or decide
    /// on, a short list as if it were complete; the Hub double-charge guard would miss an
    /// active plan on page 2.
    pub fn into_complete(self, path: &str) -> Result<Vec<Value>> {
        match self.next {
            None => Ok(self.items),
            Some(next) => Err(Error::internal(format!(
                "the API split a list into pages, and this command reads it whole: its `next` \
                 link is {}",
                quote(without_query_values(&next))
            ))
            .with_hint(format!(
                "run `hipc api GET {path}` to see the answer and its `next` link"
            ))),
        }
    }
}

/// One page of a cursor list, `{"results": [...], "next_before": <integer or null>}`.
#[derive(Debug)]
pub struct CursorPage {
    /// The items on this page.
    pub items: Vec<Value>,
    /// The server's `next_before` as a decimal string, or `None` on the last page: the
    /// token `--page-token` takes, which goes back as `?before=`.
    pub next_page_token: Option<String>,
    /// The answer's other fields, such as `count`, kept beside the items in the envelope,
    /// as [`Page`] keeps them.
    pub fields: Map<String, Value>,
}

impl CursorPage {
    /// This page as the envelope `hipc` prints for one page of a longer list.
    pub fn envelope(self) -> Value {
        page_envelope(self.items, self.next_page_token, self.fields)
    }
}

/// Reads a cursor page: an object with a `results` array and a `next_before` that is a
/// non-negative integer or null. The console types `next_before` as `number | null` and
/// never omits it.
///
/// # Errors
///
/// `internal` for any other shape, a missing `next_before` included: reading it as the
/// last page would print a short list as if it were complete.
pub fn cursor_page(answer: Value) -> Result<CursorPage> {
    let Value::Object(mut map) = answer else {
        return Err(not_a_list());
    };
    let Some(Value::Array(items)) = map.shift_remove("results") else {
        return Err(not_a_list());
    };
    let next_page_token = match map.shift_remove("next_before") {
        Some(Value::Null) => None,
        // As a number, so the token is one spelling of the cursor whatever the server
        // wrote (`812`, not `812.0` or `8.12e2`, which `as_u64` refuses).
        Some(Value::Number(cursor)) => match cursor.as_u64() {
            Some(cursor) => Some(cursor.to_string()),
            None => return Err(not_a_cursor(&format!("`{cursor}`"))),
        },
        // The kind, not the value: a server string or object has no place in the message.
        Some(other) => {
            let kind = match other {
                Value::String(_) => "a string",
                Value::Bool(_) => "a boolean",
                Value::Array(_) => "an array",
                _ => "an object",
            };
            return Err(not_a_cursor(kind));
        }
        None => {
            return Err(Error::internal(
                "the API answered a cursor list without `next_before`",
            ));
        }
    };
    Ok(CursorPage {
        items,
        next_page_token,
        fields: map,
    })
}

fn not_a_cursor(found: &str) -> Error {
    Error::internal(format!(
        "the API's `next_before` is {found}, not a cursor: an integer that is not negative, \
         or null"
    ))
}

/// The list envelope `hipc` prints for a complete list: every item, and no next page.
pub fn envelope(items: Vec<Value>) -> Value {
    page_envelope(items, None, Map::new())
}

/// [`envelope`], with the other fields of the server's object kept beside the items rather
/// than dropped (fields are not dropped to make a prettier document). `items`
/// and `next_page_token` win over a server field of the same name.
pub fn envelope_beside(items: Vec<Value>, fields: Map<String, Value>) -> Value {
    page_envelope(items, None, fields)
}

/// The envelope, after the server's `fields`. `items` and `next_page_token` win over a
/// server field of the same name.
fn page_envelope(items: Vec<Value>, token: Option<String>, fields: Map<String, Value>) -> Value {
    let mut out = fields;
    out.insert("items".to_owned(), Value::Array(items));
    out.insert(
        "next_page_token".to_owned(),
        token.map_or(Value::Null, Value::String),
    );
    Value::Object(out)
}

/// The numeric `page` query parameter of a URL.
fn page_number(url: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    let query = query.split_once('#').map_or(query, |(q, _)| q);
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == "page")
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        // As a number, so `page=01` and `page=1` are one page to the loop checks. A page
        // number too large for u64 is no page number.
        .and_then(|value| value.parse::<u64>().ok())
        .map(|number| number.to_string())
}

fn not_a_list() -> Error {
    Error::internal("the API answered a list request with something other than a list")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A `next` link the server wrote can carry a signed token in its query, so the
    /// messages that quote it show the names only.
    #[test]
    fn a_quoted_next_link_has_its_query_values_redacted() {
        let signed = "https://h/api/x/?sig=s3cr3t&cursor=abc";
        let no_number = Page {
            items: Vec::new(),
            next: Some(signed.to_owned()),
            fields: Map::new(),
        };
        let message = no_number.next_page_token().unwrap_err().to_string();
        assert!(!message.contains("s3cr3t"), "{message}");
        assert!(message.contains("sig=<redacted>"), "{message}");
        let paginated = Page {
            items: Vec::new(),
            next: Some(signed.to_owned()),
            fields: Map::new(),
        };
        let error = paginated.into_complete("/x/").unwrap_err();
        let message = error.to_string();
        assert!(!message.contains("s3cr3t"), "{message}");
        assert!(message.contains("sig=<redacted>"), "{message}");
        // The hint names the route read, so it can be run as written.
        assert_eq!(
            error.hint(),
            Some("run `hipc api GET /x/` to see the answer and its `next` link")
        );
    }

    /// `preserve_order` makes `Map::remove` a `swap_remove`, which moves the last field into
    /// the gap; the server's other fields keep their order.
    #[test]
    fn the_fields_beside_the_items_keep_the_servers_order() {
        let names = |fields: &Map<String, Value>| fields.keys().cloned().collect::<Vec<_>>();
        let answer = json!({
            "a": 1, "results": [], "b": 2, "next": null, "c": 3, "previous": null, "d": 4
        });
        assert_eq!(names(&page(answer).unwrap().fields), ["a", "b", "c", "d"]);
        let answer = json!({"a": 1, "results": [], "b": 2, "next_before": null, "c": 3, "d": 4});
        assert_eq!(
            names(&cursor_page(answer).unwrap().fields),
            ["a", "b", "c", "d"]
        );
    }

    #[test]
    fn a_bare_array_is_one_page() {
        let page = page(json!([1, 2])).unwrap();
        assert_eq!(page.items, [json!(1), json!(2)]);
        assert_eq!(page.next, None);
    }

    #[test]
    fn a_paginated_answer_carries_its_next_link() {
        let answer = json!({
            "count": 3,
            "next": "https://api.hippius.com/api/ssh-keys/?page=2",
            "previous": null,
            "results": [{"id": 1}]
        });
        let page = page(answer).unwrap();
        assert_eq!(page.items, [json!({"id": 1})]);
        assert_eq!(
            page.next.as_deref(),
            Some("https://api.hippius.com/api/ssh-keys/?page=2")
        );
        let last = super::page(json!({"next": null, "results": []})).unwrap();
        assert_eq!(last.next, None);
        let odd = super::page(json!({"next": 5, "results": []})).unwrap_err();
        assert_eq!(odd.code().as_str(), "internal");
    }

    #[test]
    fn other_shapes_are_not_lists() {
        for bad in [
            json!({"items": []}),
            json!({"results": {}}),
            json!("x"),
            json!(null),
            json!(3),
        ] {
            let error = page(bad.clone()).unwrap_err();
            assert_eq!(error.code().as_str(), "internal", "{bad}");
        }
    }

    #[test]
    fn only_a_numeric_page_parameter_is_a_token() {
        for (next, token) in [
            ("https://x/a/?page_size=5&page=12", Some("12")),
            ("https://x/a/?page=3#frag", Some("3")),
            ("https://x/a/?page=", None),
            ("https://x/a/?page=abc", None),
            ("https://x/a/?pages=2", None),
            ("https://x/a/", None),
            ("https://x/a/?page=007", Some("7")),
            ("https://x/a/?page=99999999999999999999", None),
        ] {
            assert_eq!(page_number(next).as_deref(), token, "{next}");
        }
    }

    #[test]
    fn one_page_envelope_carries_the_next_token_or_refuses_a_link_without_one() {
        let with_next = Page {
            items: vec![json!(1)],
            next: Some("https://x/a/?page=2".into()),
            fields: Map::new(),
        };
        assert_eq!(
            with_next.envelope().unwrap(),
            json!({"items": [1], "next_page_token": "2"})
        );
        let last = Page {
            items: vec![],
            next: None,
            fields: Map::new(),
        };
        assert_eq!(
            last.envelope().unwrap(),
            json!({"items": [], "next_page_token": null})
        );
        let cursor = Page {
            items: vec![],
            next: Some("https://x/a/?cursor=abc".into()),
            fields: Map::new(),
        };
        assert_eq!(cursor.envelope().unwrap_err().code().as_str(), "internal");
    }

    #[test]
    fn a_cursor_page_carries_its_cursor_as_a_decimal_token() {
        let page = cursor_page(json!({"results": [{"id": 1}], "next_before": 812})).unwrap();
        assert_eq!(page.items, [json!({"id": 1})]);
        assert_eq!(page.next_page_token.as_deref(), Some("812"));
        assert_eq!(
            page.envelope(),
            json!({"items": [{"id": 1}], "next_page_token": "812"})
        );
        let last = cursor_page(json!({"results": [], "next_before": null})).unwrap();
        assert_eq!(
            last.envelope(),
            json!({"items": [], "next_page_token": null})
        );
    }

    /// The answer's other fields stay beside `items`, as a page's do; the cursor does not.
    #[test]
    fn a_cursor_page_keeps_the_other_fields_of_the_answer() {
        let page = cursor_page(json!({
            "results": [{"id": 1}],
            "next_before": 812,
            "count": 3,
            "items": "the server's",
        }))
        .unwrap();
        assert_eq!(
            page.envelope(),
            json!({"count": 3, "items": [{"id": 1}], "next_page_token": "812"})
        );
    }

    #[test]
    fn a_cursor_page_refuses_any_other_shape() {
        for bad in [
            json!([]),
            json!({"results": []}),
            json!({"results": {}, "next_before": null}),
            json!({"next_before": 3}),
            json!({"results": [], "next_before": "812"}),
            json!({"results": [], "next_before": -1}),
            json!({"results": [], "next_before": true}),
            json!({"results": [], "next_before": [9]}),
            json!({"results": [], "next_before": {"id": 9}}),
            json!({"results": [], "next_before": 1.5}),
            json!({"count": 1, "next": null, "previous": null, "results": []}),
        ] {
            let error = cursor_page(bad.clone()).unwrap_err();
            assert_eq!(error.code().as_str(), "internal", "{bad}");
        }
        let error = cursor_page(json!({"results": []})).unwrap_err();
        assert_eq!(
            error.message(),
            "the API answered a cursor list without `next_before`"
        );
        let error = cursor_page(json!({"results": [], "next_before": "9"})).unwrap_err();
        assert_eq!(
            error.message(),
            "the API's `next_before` is a string, not a cursor: an integer that is not \
             negative, or null"
        );
    }

    /// A DRF page keeps `count` and any other field beside `items`, as the keyed
    /// lists keep theirs. `next` becomes the token and `previous` is left out: both are
    /// links, and the token says where the list goes on.
    #[test]
    fn a_drf_page_keeps_its_other_fields_beside_the_items() {
        let answer = json!({
            "count": 57,
            "next": "https://x/a/?page=2",
            "previous": "https://x/a/?page=1",
            "total_credits": "9.50",
            "results": [{"id": 1}]
        });
        assert_eq!(
            page(answer.clone()).unwrap().envelope().unwrap(),
            json!({
                "count": 57,
                "total_credits": "9.50",
                "items": [{"id": 1}],
                "next_page_token": "2"
            })
        );
        let complete = json!({"count": 1, "next": null, "previous": null, "results": [1]});
        assert_eq!(
            page(complete).unwrap().complete_envelope("/a/").unwrap(),
            json!({"count": 1, "items": [1], "next_page_token": null})
        );
        let error = page(answer).unwrap().complete_envelope("/a/").unwrap_err();
        assert_eq!(error.code().as_str(), "internal");
        // The envelope's own names win over a server field of the same name.
        let clash = json!({"items": "x", "next_page_token": "9", "next": null, "results": []});
        assert_eq!(
            page(clash).unwrap().envelope().unwrap(),
            json!({"items": [], "next_page_token": null})
        );
        assert!(page(json!([1])).unwrap().fields.is_empty());
    }

    #[test]
    fn envelope_has_items_then_the_token() {
        assert_eq!(
            envelope(vec![json!(1)]),
            json!({"items": [1], "next_page_token": null})
        );
        let doc = envelope(vec![]);
        let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["items", "next_page_token"]);
    }
}
