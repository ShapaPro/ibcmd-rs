//! Column values read by the built-in client, owned and typed.

use anyhow::{Result, anyhow, bail};
use tiberius::ColumnData;

/// One column value of a result row.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    /// `bit`, `tinyint`, `smallint`, `int`, `bigint`, and a `decimal` or
    /// `numeric` of scale 0 that fits.
    Int(i64),
    Float(f64),
    /// `char`, `varchar`, `nchar`, `nvarchar`, `text`, `ntext`, `xml`.
    Text(String),
    /// `binary`, `varbinary`, `image`, `timestamp`.
    Binary(Vec<u8>),
    /// Any other type (dates, GUIDs, fractional decimals), as the client
    /// renders it; nothing in ibcmd-rs reads these.
    Other(String),
}

impl SqlValue {
    pub fn from_column(value: ColumnData<'static>) -> Self {
        match value {
            ColumnData::U8(value) => value.map_or(Self::Null, |value| Self::Int(value.into())),
            ColumnData::I16(value) => value.map_or(Self::Null, |value| Self::Int(value.into())),
            ColumnData::I32(value) => value.map_or(Self::Null, |value| Self::Int(value.into())),
            ColumnData::I64(value) => value.map_or(Self::Null, Self::Int),
            ColumnData::Bit(value) => value.map_or(Self::Null, |value| Self::Int(value.into())),
            ColumnData::F32(value) => value.map_or(Self::Null, |value| Self::Float(value.into())),
            ColumnData::F64(value) => value.map_or(Self::Null, Self::Float),
            ColumnData::String(value) => {
                value.map_or(Self::Null, |value| Self::Text(value.into_owned()))
            }
            ColumnData::Binary(value) => {
                value.map_or(Self::Null, |value| Self::Binary(value.into_owned()))
            }
            ColumnData::Xml(value) => value.map_or(Self::Null, |value| {
                Self::Text(value.into_owned().to_string())
            }),
            ColumnData::Numeric(value) => value.map_or(Self::Null, |value| {
                if value.scale() == 0
                    && let Ok(value) = i64::try_from(value.value())
                {
                    return Self::Int(value);
                }
                Self::Other(value.to_string())
            }),
            ColumnData::Guid(value) => {
                value.map_or(Self::Null, |value| Self::Other(value.to_string()))
            }
            other => {
                if column_is_null(&other) {
                    Self::Null
                } else {
                    Self::Other(format!("{other:?}"))
                }
            }
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            _ => None,
        }
    }

    /// The value as sqlcmd's text output would show it: what the parsers of
    /// line-oriented queries (`CONCAT(N'X|', ...)`) read.
    pub fn to_text(&self) -> String {
        match self {
            Self::Null => "NULL".to_owned(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Text(value) | Self::Other(value) => value.clone(),
            Self::Binary(bytes) => {
                let mut text = String::with_capacity(2 + bytes.len() * 2);
                text.push_str("0x");
                for byte in bytes {
                    text.push_str(&format!("{byte:02X}"));
                }
                text
            }
        }
    }
}

fn column_is_null(value: &ColumnData<'static>) -> bool {
    match value {
        ColumnData::DateTime(value) => value.is_none(),
        ColumnData::SmallDateTime(value) => value.is_none(),
        ColumnData::Time(value) => value.is_none(),
        ColumnData::Date(value) => value.is_none(),
        ColumnData::DateTime2(value) => value.is_none(),
        ColumnData::DateTimeOffset(value) => value.is_none(),
        _ => false,
    }
}

/// One result row.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlRow {
    /// Which result set of the batch the row belongs to, from 0.
    pub result_set: usize,
    pub values: Vec<SqlValue>,
}

impl SqlRow {
    pub fn from_row(row: tiberius::Row) -> Self {
        let result_set = row.result_index();
        Self {
            result_set,
            values: row.into_iter().map(SqlValue::from_column).collect(),
        }
    }

    pub fn value(&self, index: usize) -> Result<&SqlValue> {
        self.values
            .get(index)
            .ok_or_else(|| anyhow!("result row has no column {index}"))
    }

    pub fn i64(&self, index: usize) -> Result<i64> {
        match self.value(index)? {
            SqlValue::Int(value) => Ok(*value),
            other => bail!("column {index} is not an integer: {other:?}"),
        }
    }

    pub fn text(&self, index: usize) -> Result<&str> {
        match self.value(index)? {
            SqlValue::Text(value) => Ok(value),
            other => bail!("column {index} is not text: {other:?}"),
        }
    }

    pub fn binary(&self, index: usize) -> Result<&[u8]> {
        match self.value(index)? {
            SqlValue::Binary(value) => Ok(value),
            other => bail!("column {index} is not binary: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use tiberius::ColumnData;
    use tiberius::numeric::Numeric;

    use super::SqlValue;

    #[test]
    fn integers_of_every_width_read_as_i64() {
        assert_eq!(
            SqlValue::from_column(ColumnData::U8(Some(7))),
            SqlValue::Int(7)
        );
        assert_eq!(
            SqlValue::from_column(ColumnData::I16(Some(-2))),
            SqlValue::Int(-2)
        );
        assert_eq!(
            SqlValue::from_column(ColumnData::I32(Some(1 << 20))),
            SqlValue::Int(1 << 20)
        );
        assert_eq!(
            SqlValue::from_column(ColumnData::I64(Some(1 << 40))),
            SqlValue::Int(1 << 40)
        );
        assert_eq!(
            SqlValue::from_column(ColumnData::Bit(Some(true))),
            SqlValue::Int(1)
        );
        assert_eq!(
            SqlValue::from_column(ColumnData::Numeric(Some(Numeric::new_with_scale(42, 0)))),
            SqlValue::Int(42)
        );
        assert_eq!(
            SqlValue::from_column(ColumnData::Numeric(Some(Numeric::new_with_scale(425, 1)))),
            SqlValue::Other("42.5".to_owned())
        );
    }

    #[test]
    fn nulls_of_every_type_read_as_null() {
        for value in [
            ColumnData::I32(None),
            ColumnData::String(None),
            ColumnData::Binary(None),
            ColumnData::DateTime2(None),
            ColumnData::Guid(None),
        ] {
            assert!(SqlValue::from_column(value).is_null());
        }
    }

    #[test]
    fn text_and_binary_render_like_sqlcmd() {
        assert_eq!(
            SqlValue::from_column(ColumnData::String(Some(Cow::Borrowed("Конфигурация"))))
                .to_text(),
            "Конфигурация"
        );
        assert_eq!(
            SqlValue::from_column(ColumnData::Binary(Some(Cow::Borrowed(&[0x0a, 0xff])))).to_text(),
            "0x0AFF"
        );
        assert_eq!(SqlValue::Null.to_text(), "NULL");
    }
}
