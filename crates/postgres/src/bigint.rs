//! An unsigned number on its way into a `bigint`.
//!
//! The signedness is this database's, so the narrowing lives here rather than
//! in the domain types that cross it.

use std::error::Error;

use tokio_postgres::types::{IsNull, ToSql, Type, to_sql_checked};

/// A `u64` bound for a `bigint` column, refusing past `i64::MAX` rather than
/// wrapping to a negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bigint(pub u64);

impl ToSql for Bigint {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut bytes::BytesMut,
    ) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
        i64::try_from(self.0)?.to_sql(ty, out)
    }

    fn accepts(ty: &Type) -> bool {
        <i64 as ToSql>::accepts(ty)
    }

    to_sql_checked!();
}

#[cfg(test)]
mod tests {
    use tokio_postgres::types::{ToSql, Type};

    use super::Bigint;

    fn encoded(value: u64) -> Result<Vec<u8>, String> {
        let mut out = bytes::BytesMut::new();

        Bigint(value)
            .to_sql(&Type::INT8, &mut out)
            .map(|_| out.to_vec())
            .map_err(|refusal| refusal.to_string())
    }

    #[test]
    fn sends_a_number_a_bigint_can_hold() {
        assert_eq!(encoded(1), Ok(1_i64.to_be_bytes().to_vec()));
        assert_eq!(
            encoded(11_297_108_109),
            Ok(11_297_108_109_i64.to_be_bytes().to_vec())
        );
    }

    #[test]
    fn takes_the_largest_bigint_there_is() {
        let largest = i64::MAX.unsigned_abs();

        assert_eq!(encoded(largest), Ok(i64::MAX.to_be_bytes().to_vec()));
    }

    #[test]
    fn refuses_a_number_a_cast_would_turn_negative() {
        assert!(encoded(i64::MAX.unsigned_abs() + 1).is_err());
        assert!(encoded(u64::MAX).is_err());
    }

    #[test]
    fn goes_where_an_i64_goes() {
        assert!(<Bigint as ToSql>::accepts(&Type::INT8));
        assert!(!<Bigint as ToSql>::accepts(&Type::TEXT));
    }
}
