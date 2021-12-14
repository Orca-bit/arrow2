use std::sync::Arc;

use chrono::Datelike;

// Ideally this trait should not be needed and both `csv` and `csv_async` crates would share
// the same `ByteRecord` struct. Unfortunately, they do not and thus we must use generics
// over this trait and materialize the generics for each struct.
pub(crate) trait ByteRecordGeneric {
    fn get(&self, index: usize) -> Option<&[u8]>;
}

use crate::{
    array::*,
    datatypes::*,
    error::{ArrowError, Result},
    record_batch::RecordBatch,
    temporal_conversions,
    types::{NativeType, NaturalDataType},
};

use super::utils::RFC3339;

#[inline]
fn to_utf8(bytes: &[u8]) -> Option<&str> {
    simdutf8::basic::from_utf8(bytes).ok()
}

#[inline]
fn deserialize_primitive<T, B: ByteRecordGeneric, F>(
    rows: &[B],
    column: usize,
    datatype: DataType,
    op: F,
) -> Arc<dyn Array>
where
    T: NativeType + NaturalDataType + lexical_core::FromLexical,
    F: Fn(&[u8]) -> Option<T>,
{
    let iter = rows.iter().map(|row| match row.get(column) {
        Some(bytes) => {
            if bytes.is_empty() {
                return None;
            }
            op(bytes)
        }
        None => None,
    });
    Arc::new(PrimitiveArray::<T>::from_trusted_len_iter(iter).to(datatype))
}

#[inline]
fn significant_bytes(bytes: &[u8]) -> usize {
    bytes.iter().map(|byte| (*byte != b'0') as usize).sum()
}

/// Deserializes bytes to a single i128 representing a decimal
/// The decimal precision and scale are not checked.
#[inline]
fn deserialize_decimal(bytes: &[u8], precision: usize, scale: usize) -> Option<i128> {
    let mut a = bytes.split(|x| *x == b'.');
    let lhs = a.next();
    let rhs = a.next();
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => lexical_core::parse::<i128>(lhs).ok().and_then(|x| {
            lexical_core::parse::<i128>(rhs)
                .ok()
                .map(|y| (x, lhs, y, rhs))
                .and_then(|(lhs, lhs_b, rhs, rhs_b)| {
                    let lhs_s = significant_bytes(lhs_b);
                    let rhs_s = significant_bytes(rhs_b);
                    if lhs_s + rhs_s > precision || rhs_s > scale {
                        None
                    } else {
                        Some((lhs, rhs, rhs_s))
                    }
                })
                .map(|(lhs, rhs, rhs_s)| lhs * 10i128.pow(rhs_s as u32) + rhs)
        }),
        (None, Some(rhs)) => {
            if rhs.len() != precision || rhs.len() != scale {
                return None;
            }
            lexical_core::parse::<i128>(rhs).ok()
        }
        (Some(lhs), None) => {
            if lhs.len() != precision || scale != 0 {
                return None;
            }
            lexical_core::parse::<i128>(lhs).ok()
        }
        (None, None) => None,
    }
}

#[inline]
fn deserialize_boolean<B, F>(rows: &[B], column: usize, op: F) -> Arc<dyn Array>
where
    B: ByteRecordGeneric,
    F: Fn(&[u8]) -> Option<bool>,
{
    let iter = rows.iter().map(|row| match row.get(column) {
        Some(bytes) => {
            if bytes.is_empty() {
                return None;
            }
            op(bytes)
        }
        None => None,
    });
    Arc::new(BooleanArray::from_trusted_len_iter(iter))
}

#[inline]
fn deserialize_utf8<O: Offset, B: ByteRecordGeneric>(rows: &[B], column: usize) -> Arc<dyn Array> {
    let iter = rows.iter().map(|row| match row.get(column) {
        Some(bytes) => to_utf8(bytes),
        None => None,
    });
    Arc::new(Utf8Array::<O>::from_trusted_len_iter(iter))
}

#[inline]
fn deserialize_binary<O: Offset, B: ByteRecordGeneric>(
    rows: &[B],
    column: usize,
) -> Arc<dyn Array> {
    let iter = rows.iter().map(|row| row.get(column));
    Arc::new(BinaryArray::<O>::from_trusted_len_iter(iter))
}

#[inline]
fn deserialize_datetime<T: chrono::TimeZone>(string: &str, tz: &T) -> Option<i64> {
    let mut parsed = chrono::format::Parsed::new();
    let fmt = chrono::format::StrftimeItems::new(RFC3339);
    if chrono::format::parse(&mut parsed, string, fmt).is_ok() {
        parsed
            .to_datetime()
            .map(|x| x.naive_utc())
            .map(|x| tz.from_utc_datetime(&x))
            .map(|x| x.timestamp_nanos())
            .ok()
    } else {
        None
    }
}

/// Deserializes `column` of `rows` into an [`Array`] of [`DataType`] `datatype`.
#[inline]
pub(crate) fn deserialize_column<B: ByteRecordGeneric>(
    rows: &[B],
    column: usize,
    datatype: DataType,
    _line_number: usize,
) -> Result<Arc<dyn Array>> {
    use DataType::*;
    Ok(match datatype {
        Boolean => deserialize_boolean(rows, column, |bytes| {
            if bytes.eq_ignore_ascii_case(b"false") {
                Some(false)
            } else if bytes.eq_ignore_ascii_case(b"true") {
                Some(true)
            } else {
                None
            }
        }),
        Int8 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<i8>(bytes).ok()
        }),
        Int16 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<i16>(bytes).ok()
        }),
        Int32 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<i32>(bytes).ok()
        }),
        Int64 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<i64>(bytes).ok()
        }),
        UInt8 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<u8>(bytes).ok()
        }),
        UInt16 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<u16>(bytes).ok()
        }),
        UInt32 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<u32>(bytes).ok()
        }),
        UInt64 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<u64>(bytes).ok()
        }),
        Float32 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<f32>(bytes).ok()
        }),
        Float64 => deserialize_primitive(rows, column, datatype, |bytes| {
            lexical_core::parse::<f64>(bytes).ok()
        }),
        Date32 => deserialize_primitive(rows, column, datatype, |bytes| {
            to_utf8(bytes)
                .and_then(|x| x.parse::<chrono::NaiveDate>().ok())
                .map(|x| x.num_days_from_ce() - temporal_conversions::EPOCH_DAYS_FROM_CE)
        }),
        Date64 => deserialize_primitive(rows, column, datatype, |bytes| {
            to_utf8(bytes)
                .and_then(|x| x.parse::<chrono::NaiveDateTime>().ok())
                .map(|x| x.timestamp_millis())
        }),
        Timestamp(TimeUnit::Nanosecond, None) => {
            deserialize_primitive(rows, column, datatype, |bytes| {
                to_utf8(bytes)
                    .and_then(|x| x.parse::<chrono::NaiveDateTime>().ok())
                    .map(|x| x.timestamp_nanos())
            })
        }
        Timestamp(TimeUnit::Microsecond, None) => {
            deserialize_primitive(rows, column, datatype, |bytes| {
                to_utf8(bytes)
                    .and_then(|x| x.parse::<chrono::NaiveDateTime>().ok())
                    .map(|x| x.timestamp_nanos() / 1000)
            })
        }
        Timestamp(time_unit, None) => deserialize_primitive(rows, column, datatype, |bytes| {
            to_utf8(bytes)
                .and_then(|x| x.parse::<chrono::NaiveDateTime>().ok())
                .map(|x| x.timestamp_nanos())
                .map(|x| match time_unit {
                    TimeUnit::Second => x / 1_000_000_000,
                    TimeUnit::Millisecond => x / 1_000_000,
                    TimeUnit::Microsecond => x / 1_000,
                    TimeUnit::Nanosecond => x,
                })
        }),
        Timestamp(time_unit, Some(ref tz)) => {
            let tz = temporal_conversions::parse_offset(tz)?;
            deserialize_primitive(rows, column, datatype, |bytes| {
                to_utf8(bytes)
                    .and_then(|x| deserialize_datetime(x, &tz))
                    .map(|x| match time_unit {
                        TimeUnit::Second => x / 1_000_000_000,
                        TimeUnit::Millisecond => x / 1_000_000,
                        TimeUnit::Microsecond => x / 1_000,
                        TimeUnit::Nanosecond => x,
                    })
            })
        }
        Decimal(precision, scale) => deserialize_primitive(rows, column, datatype, |x| {
            deserialize_decimal(x, precision, scale)
        }),
        Utf8 => deserialize_utf8::<i32, _>(rows, column),
        LargeUtf8 => deserialize_utf8::<i64, _>(rows, column),
        Binary => deserialize_binary::<i32, _>(rows, column),
        LargeBinary => deserialize_binary::<i64, _>(rows, column),
        other => {
            return Err(ArrowError::NotYetImplemented(format!(
                "Deserializing type \"{:?}\" is not implemented",
                other
            )))
        }
    })
}

/// Deserializes rows [`ByteRecord`] into a [`RecordBatch`].
/// Note that this is a convenience function: column deserialization
/// is trivially parallelizable (e.g. rayon).
pub(crate) fn deserialize_batch<F, B: ByteRecordGeneric>(
    rows: &[B],
    fields: &[Field],
    projection: Option<&[usize]>,
    line_number: usize,
    deserialize_column: F,
) -> Result<RecordBatch>
where
    F: Fn(&[B], usize, DataType, usize) -> Result<Arc<dyn Array>>,
{
    let projection: Vec<usize> = match projection {
        Some(v) => v.to_vec(),
        None => fields.iter().enumerate().map(|(i, _)| i).collect(),
    };
    let projected_fields: Vec<Field> = projection.iter().map(|i| fields[*i].clone()).collect();

    let schema = Arc::new(Schema::new(projected_fields));

    if rows.is_empty() {
        return Ok(RecordBatch::new_empty(schema));
    }

    let columns = projection
        .iter()
        .map(|column| {
            let column = *column;
            let field = &fields[column];
            let data_type = field.data_type();
            deserialize_column(rows, column, data_type.clone(), line_number)
        })
        .collect::<Result<Vec<_>>>()?;

    RecordBatch::try_new(schema, columns)
}