//! The values a statement binds, converted the way rusqlite binds them.

use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::ToSql;

/// The values bound to a statement's placeholders, in order.
pub(crate) trait Params {
    /// The bound values.
    fn values(self) -> rusqlite::Result<Vec<Value>>;
}

/// One bound value, converted the way rusqlite binds it.
fn bound(p: &(impl ToSql + ?Sized)) -> rusqlite::Result<Value> {
    Ok(match p.to_sql()? {
        ToSqlOutput::Borrowed(v) => owned(v)?,
        ToSqlOutput::Owned(v) => v,
        _ => {
            return Err(rusqlite::Error::ToSqlConversionFailure(
                "unsupported value".into(),
            ))
        }
    })
}

impl Params for &[&dyn ToSql] {
    fn values(self) -> rusqlite::Result<Vec<Value>> {
        self.iter().map(|p| bound(*p)).collect()
    }
}

impl Params for Vec<Value> {
    fn values(self) -> rusqlite::Result<Vec<Value>> {
        Ok(self)
    }
}

impl Params for () {
    fn values(self) -> rusqlite::Result<Vec<Value>> {
        Ok(Vec::new())
    }
}

impl Params for [&(dyn ToSql + Send + Sync); 0] {
    fn values(self) -> rusqlite::Result<Vec<Value>> {
        Ok(Vec::new())
    }
}

/// `Params` for arrays of one to `N` values.
macro_rules! array_params {
    ($($n:literal)+) => {$(
        impl<T: ToSql> Params for [T; $n] {
            fn values(self) -> rusqlite::Result<Vec<Value>> {
                self.iter().map(bound).collect()
            }
        }
    )+};
}
array_params!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16);

/// `Params` for tuples of one to sixteen values.
macro_rules! tuple_params {
    ($(($($t:ident $i:tt),+))+) => {$(
        impl<$($t: ToSql),+> Params for ($($t,)+) {
            fn values(self) -> rusqlite::Result<Vec<Value>> {
                Ok(vec![$(bound(&self.$i)?),+])
            }
        }
    )+};
}
tuple_params! {
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
    (A 0, B 1, C 2, D 3)
    (A 0, B 1, C 2, D 3, E 4)
    (A 0, B 1, C 2, D 3, E 4, F 5)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11)
}

/// `v` as an owned value; text that is not UTF-8 is refused as rusqlite refuses it.
pub(super) fn owned(v: ValueRef<'_>) -> rusqlite::Result<Value> {
    Ok(match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::Integer(i),
        ValueRef::Real(f) => Value::Real(f),
        ValueRef::Text(t) => Value::Text(
            std::str::from_utf8(t)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?
                .to_owned(),
        ),
        ValueRef::Blob(b) => Value::Blob(b.to_vec()),
    })
}
