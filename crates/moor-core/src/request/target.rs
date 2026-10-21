//! The Box a request addresses: by name, or by its store id.

use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::name::{BoxId, BoxName};

/// The Box a request addresses. On the wire it is one `box` field: a string is a name, an object
/// `{"id": n}` is an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoxTarget {
    /// A Box name; each request applies its own rule for which Box of that name it means.
    Name(BoxName),
    /// Exactly the Box with this id, in whatever state it is.
    Id(BoxId),
}

impl From<BoxName> for BoxTarget {
    fn from(name: BoxName) -> Self {
        Self::Name(name)
    }
}

impl From<BoxId> for BoxTarget {
    fn from(id: BoxId) -> Self {
        Self::Id(id)
    }
}

impl fmt::Display for BoxTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => f.write_str(name.as_str()),
            Self::Id(id) => write!(f, "id {}", id.get()),
        }
    }
}

impl Serialize for BoxTarget {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Name(name) => name.serialize(s),
            Self::Id(id) => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry("id", id)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for BoxTarget {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(TargetVisitor)
    }
}

/// Reads a `box` value: a string name or an `{"id": n}` object.
struct TargetVisitor;

impl<'de> Visitor<'de> for TargetVisitor {
    type Value = BoxTarget;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(r#"a Box name or {"id": <Box id>}"#)
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<BoxTarget, E> {
        BoxName::parse_allow_bare(s)
            .map(BoxTarget::Name)
            .map_err(E::custom)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<BoxTarget, A::Error> {
        let mut id = None;
        while let Some(key) = map.next_key::<String>()? {
            if key != "id" {
                return Err(de::Error::unknown_field(&key, &["id"]));
            }
            if id.is_some() {
                return Err(de::Error::duplicate_field("id"));
            }
            id = Some(map.next_value::<i64>()?);
        }
        id.map(|v| BoxTarget::Id(BoxId::new(v)))
            .ok_or_else(|| de::Error::missing_field("id"))
    }
}

#[cfg(test)]
mod tests;
