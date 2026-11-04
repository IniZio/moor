//! Binding a value as its parameter's Postgres type, and reading a column as a storage class.

use bytes::BytesMut;
use postgres::types::{to_sql_checked, FromSql, IsNull, ToSql, Type};

use crate::store::db::{Cause, Value};

/// A value bound as its parameter's type.
#[derive(Debug)]
pub(super) struct Bind<'a>(pub(super) &'a Value);

impl ToSql for Bind<'_> {
    fn to_sql(&self, ty: &Type, out: &mut BytesMut) -> std::result::Result<IsNull, Cause> {
        match (self.0, ty) {
            (Value::Null, _) => Ok(IsNull::Yes),
            (Value::Integer(i), &Type::INT8) => i.to_sql(ty, out),
            (Value::Integer(i), &Type::INT4) => i32::try_from(*i)?.to_sql(ty, out),
            (Value::Integer(i), &Type::INT2) => i16::try_from(*i)?.to_sql(ty, out),
            (Value::Integer(i), &Type::BOOL) => (*i != 0).to_sql(ty, out),
            (Value::Real(f), &Type::FLOAT8) => f.to_sql(ty, out),
            (Value::Text(s), ty) if <&str as ToSql>::accepts(ty) => s.as_str().to_sql(ty, out),
            (Value::Blob(b), &Type::BYTEA) => b.as_slice().to_sql(ty, out),
            (v, ty) => Err(format!("cannot bind {:?} as {ty}", v.kind()).into()),
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

/// A column read as the `SQLite` storage class that holds it.
pub(super) struct Read(pub(super) Value);

impl<'a> FromSql<'a> for Read {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> std::result::Result<Self, Cause> {
        let value = match *ty {
            Type::INT8 => Value::Integer(i64::from_sql(ty, raw)?),
            Type::INT4 => Value::Integer(i32::from_sql(ty, raw)?.into()),
            Type::INT2 => Value::Integer(i16::from_sql(ty, raw)?.into()),
            Type::BOOL => Value::Integer(bool::from_sql(ty, raw)?.into()),
            Type::FLOAT8 => Value::Real(f64::from_sql(ty, raw)?),
            Type::FLOAT4 => Value::Real(f32::from_sql(ty, raw)?.into()),
            Type::NUMERIC => numeric(raw)?,
            Type::BYTEA => Value::Blob(Vec::<u8>::from_sql(ty, raw)?),
            _ if <String as FromSql>::accepts(ty) => Value::Text(String::from_sql(ty, raw)?),
            _ => return Err(format!("cannot read a {ty} column").into()),
        };
        Ok(Self(value))
    }

    fn from_sql_null(_: &Type) -> std::result::Result<Self, Cause> {
        Ok(Self(Value::Null))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

/// A binary `numeric`, as an integer when it is whole and fits, as `SUM` over integers is on
/// `SQLite`, and as a real otherwise.
pub(super) fn numeric(raw: &[u8]) -> std::result::Result<Value, Cause> {
    let word = |i: usize| -> std::result::Result<i16, Cause> {
        match raw.get(i * 2..i * 2 + 2) {
            Some(&[hi, lo]) => Ok(i16::from_be_bytes([hi, lo])),
            _ => Err("short numeric".into()),
        }
    };
    let ndigits = word(0)?;
    let weight = i32::from(word(1)?);
    let negative = match word(2)?.cast_unsigned() {
        0x0000 => false,
        0x4000 => true,
        _ => return Err("numeric is NaN or infinite".into()),
    };
    let mut whole = Some(0i64);
    let mut real = 0f64;
    for i in 0..i32::from(ndigits) {
        let d = word(4 + usize::try_from(i)?)?;
        real += f64::from(d) * 10_000f64.powi(weight - i);
        if i <= weight {
            whole = whole.and_then(|n| n.checked_mul(10_000)?.checked_add(i64::from(d)));
        } else if d != 0 {
            whole = None;
        }
    }
    for _ in i32::from(ndigits)..=weight {
        whole = whole.and_then(|n| n.checked_mul(10_000));
    }
    Ok(match (whole, negative) {
        (Some(n), false) => Value::Integer(n),
        (Some(n), true) => Value::Integer(-n),
        (None, false) => Value::Real(real),
        (None, true) => Value::Real(-real),
    })
}
