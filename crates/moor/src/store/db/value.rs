//! The values a statement binds and a row holds, and the Rust types a column reads as.

use super::{Error, Result};

/// One value a statement binds or a row holds, as one of `SQLite`'s storage classes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    /// No value.
    Null,
    /// A 64-bit integer; a boolean is 0 or 1.
    Integer(i64),
    /// A 64-bit float.
    Real(f64),
    /// UTF-8 text.
    Text(String),
    /// Bytes.
    Blob(Vec<u8>),
}

/// A value's storage class, as an error names what a column held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// No value.
    Null,
    /// An integer.
    Integer,
    /// A float.
    Real,
    /// Text.
    Text,
    /// Bytes.
    Blob,
}

impl Value {
    /// Its storage class.
    pub(crate) const fn kind(&self) -> Kind {
        match self {
            Self::Null => Kind::Null,
            Self::Integer(_) => Kind::Integer,
            Self::Real(_) => Kind::Real,
            Self::Text(_) => Kind::Text,
            Self::Blob(_) => Kind::Blob,
        }
    }
}

/// Why a [`Value`] did not read as a type.
#[derive(Debug)]
pub(crate) enum Unread {
    /// The value's storage class is not one the type reads.
    Type,
    /// The integer does not fit the type.
    OutOfRange(i64),
}

/// A Rust type a column reads as, by `SQLite`'s rules.
pub(crate) trait FromValue: Sized {
    /// `v` as this type.
    fn from_value(v: &Value) -> std::result::Result<Self, Unread>;
}

/// `FromValue` for integer types, read from an integer that fits.
macro_rules! integer_from_value {
    ($($ty:ty)+) => {$(
        impl FromValue for $ty {
            fn from_value(v: &Value) -> std::result::Result<Self, Unread> {
                match v {
                    Value::Integer(i) => Self::try_from(*i).map_err(|_| Unread::OutOfRange(*i)),
                    _ => Err(Unread::Type),
                }
            }
        }
    )+};
}
integer_from_value! { i64 i32 i16 i8 u64 u32 u16 u8 usize }

impl FromValue for Value {
    fn from_value(v: &Value) -> std::result::Result<Self, Unread> {
        Ok(v.clone())
    }
}

impl From<Value> for sea_query::Value {
    fn from(v: Value) -> Self {
        match v {
            Value::Null => Self::BigInt(None),
            Value::Integer(i) => i.into(),
            Value::Real(f) => f.into(),
            Value::Text(t) => t.into(),
            Value::Blob(b) => b.into(),
        }
    }
}

impl FromValue for bool {
    fn from_value(v: &Value) -> std::result::Result<Self, Unread> {
        i64::from_value(v).map(|i| i != 0)
    }
}

impl FromValue for f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "SQLite reads an integer column as a float the same way"
    )]
    fn from_value(v: &Value) -> std::result::Result<Self, Unread> {
        match v {
            Value::Integer(i) => Ok(*i as Self),
            Value::Real(f) => Ok(*f),
            _ => Err(Unread::Type),
        }
    }
}

impl FromValue for String {
    fn from_value(v: &Value) -> std::result::Result<Self, Unread> {
        match v {
            Value::Text(t) => Ok(t.clone()),
            _ => Err(Unread::Type),
        }
    }
}

impl FromValue for Vec<u8> {
    fn from_value(v: &Value) -> std::result::Result<Self, Unread> {
        match v {
            Value::Blob(b) => Ok(b.clone()),
            _ => Err(Unread::Type),
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(v: &Value) -> std::result::Result<Self, Unread> {
        match v {
            Value::Null => Ok(None),
            v => T::from_value(v).map(Some),
        }
    }
}

/// A `sea-query` value as the storage class it binds as.
pub(super) fn bound(v: sea_query::Value) -> Result<Value> {
    use sea_query::Value as Q;
    let int = |i: Option<i64>| i.map_or(Value::Null, Value::Integer);
    Ok(match v {
        Q::Bool(b) => int(b.map(i64::from)),
        Q::TinyInt(i) => int(i.map(i64::from)),
        Q::SmallInt(i) => int(i.map(i64::from)),
        Q::Int(i) => int(i.map(i64::from)),
        Q::BigInt(i) => int(i),
        Q::TinyUnsigned(i) => int(i.map(i64::from)),
        Q::SmallUnsigned(i) => int(i.map(i64::from)),
        Q::Unsigned(i) => int(i.map(i64::from)),
        Q::BigUnsigned(i) => int(i
            .map(i64::try_from)
            .transpose()
            .map_err(|e| Error::Bind(e.to_string()))?),
        Q::Float(f) => f.map_or(Value::Null, |f| Value::Real(f.into())),
        Q::Double(f) => f.map_or(Value::Null, Value::Real),
        Q::String(t) => t.map_or(Value::Null, Value::Text),
        Q::Char(c) => c.map_or(Value::Null, |c| Value::Text(c.into())),
        Q::Bytes(b) => b.map_or(Value::Null, Value::Blob),
        Q::Enum(_) => return Err(Error::Bind("enum values".into())),
    })
}
