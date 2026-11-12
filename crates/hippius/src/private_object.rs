//! A private file that holds one JSON object, such as the DNS credentials, App Engine secrets,
//! and cluster key files: read once through [`PrivateFile::load`], held to its checks, and
//! parsed with every name and string cleared when dropped.

use std::fmt;
use std::path::Path;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::private_file::{Guarded, PrivateFile};
use crate::secret::clear_owned;

/// Why a private file is not one JSON object of distinct names.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ObjectProblem {
    /// The object names an entry twice: which copy counts would depend on the parser.
    Repeated,
    /// The file is not one JSON object.
    NotAnObject,
}

impl ObjectProblem {
    /// The problem as a refusal says it after the file's name; `entry` is what the object's
    /// names stand for, such as `a field`.
    pub(crate) fn describe(self, entry: &str) -> String {
        match self {
            Self::Repeated => format!("names {entry} twice"),
            Self::NotAnObject => "is not a JSON object".to_owned(),
        }
    }
}

/// Reads the private file at `path` ([`PrivateFile::load`], held to its checks) as one JSON
/// object. The bytes read are cleared on the way out, whatever the outcome; each caller words
/// its own refusal of an [`ObjectProblem`] and of what the object holds.
///
/// # Errors
///
/// The refusals of [`PrivateFile::load`], and `usage` with `hint` when nothing is at the path.
/// That message does not repeat the path: a secret passed in place of the file that holds it
/// names no file, and quoting it would copy the secret into a log (tenet 9).
pub(crate) fn load_private_object(
    path: &Path,
    guarded: Guarded,
    hint: &str,
) -> Result<std::result::Result<Fields, ObjectProblem>> {
    let Some(bytes) = PrivateFile::load(path, guarded)? else {
        return Err(Error::usage(
            format!(
                "{} {} names does not exist; the path is not repeated here, in case it is a \
                 secret passed in place of the file",
                guarded.what,
                guarded.named_by.name()
            ),
            hint,
        ));
    };
    let bytes = Zeroizing::new(bytes);
    Ok(match serde_json::from_slice::<UniqueObject>(&bytes) {
        Ok(UniqueObject::Object(fields)) => Ok(fields),
        Ok(UniqueObject::Repeated) => Err(ObjectProblem::Repeated),
        Err(_) => Err(ObjectProblem::NotAnObject),
    })
}

/// The file as a JSON object, or the fact that it names a field twice. Any other JSON value is
/// a deserialization error. What was read is cleared when either is dropped or the read
/// fails: a field named twice and its earlier value included.
enum UniqueObject {
    Object(Fields),
    Repeated,
}

/// The fields of a private file's JSON object, every name and string in them cleared when
/// dropped ([`clear_owned`]), so a refusal that returns before a field is taken out leaves no
/// copy of it in freed memory.
pub(crate) struct Fields(Map<String, Value>);

impl std::ops::Deref for Fields {
    type Target = Map<String, Value>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Fields {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Fields {
    fn drop(&mut self) {
        clear_owned(Value::Object(std::mem::take(&mut self.0)));
    }
}

impl<'de> Deserialize<'de> for UniqueObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = UniqueObject;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<UniqueObject, A::Error> {
                // Cleared when dropped, so a syntax error later in the file, which returns
                // here, leaves nothing read before it behind.
                let mut fields = Fields(Map::new());
                let mut repeated = false;
                while let Some((name, value)) = map.next_entry::<String, Value>()? {
                    // The whole object is still read, so a syntax error later in the file
                    // is reported as one. A second copy is not kept: it is cleared now, and
                    // the first when `fields` is dropped.
                    if fields.contains_key(&name) {
                        repeated = true;
                        drop(Zeroizing::new(name));
                        clear_owned(value);
                    } else {
                        fields.insert(name, value);
                    }
                }
                Ok(if repeated {
                    UniqueObject::Repeated
                } else {
                    UniqueObject::Object(fields)
                })
            }
        }
        deserializer.deserialize_map(Object)
    }
}
