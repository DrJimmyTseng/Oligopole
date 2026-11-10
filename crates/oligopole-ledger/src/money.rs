use crate::error::LedgerError;
use crate::u256_math::u256_mul_div_rem;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Sign-flip mask (`1 << 127`) converting two's complement `i128` into
/// lexicographically sortable big-endian `u128` bytes for SQLite `BLOB` indexing.
pub const SIGN_FLIP_MASK: u128 = 1u128 << 127;

/// Minor currency units (`1 cent = 1`, `$1.00 = 100`) stored in strict `i128`.
/// Zero floating-point operations are permitted on `Money`.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize,
)]
pub struct Money(pub i128);

impl Money {
    pub const ZERO: Self = Self(0);
    pub const ONE_CENT: Self = Self(1);
    pub const ONE_DOLLAR: Self = Self(100);
    pub const MAX: Self = Self(i128::MAX);
    pub const MIN: Self = Self(i128::MIN);

    #[inline]
    pub const fn from_cents(cents: i128) -> Self {
        Self(cents)
    }

    #[inline]
    pub const fn from_dollars(dollars: i64) -> Self {
        Self((dollars as i128) * 100)
    }

    #[inline]
    pub const fn cents(self) -> i128 {
        self.0
    }

    #[inline]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }

    #[inline]
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    #[inline]
    pub fn checked_add(self, rhs: Self) -> Result<Self, LedgerError> {
        self.0
            .checked_add(rhs.0)
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    #[inline]
    pub fn checked_sub(self, rhs: Self) -> Result<Self, LedgerError> {
        self.0
            .checked_sub(rhs.0)
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    #[inline]
    pub fn checked_neg(self) -> Result<Self, LedgerError> {
        self.0
            .checked_neg()
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    #[inline]
    pub fn checked_abs(self) -> Result<Self, LedgerError> {
        self.0
            .checked_abs()
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    #[inline]
    pub fn checked_mul_u64(self, rhs: u64) -> Result<Self, LedgerError> {
        self.0
            .checked_mul(i128::from(rhs))
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    #[inline]
    pub fn checked_mul_i128(self, rhs: i128) -> Result<Self, LedgerError> {
        self.0
            .checked_mul(rhs)
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    #[inline]
    pub fn checked_div_euclid(self, rhs: i128) -> Result<Self, LedgerError> {
        if rhs == 0 {
            return Err(LedgerError::DivisionByZero);
        }
        self.0
            .checked_div_euclid(rhs)
            .map(Self)
            .ok_or(LedgerError::ArithmeticOverflow)
    }

    /// Computes `(self * numerator) / denominator` and remainder for non-negative `self`
    /// using widening 256-bit arithmetic so `self * numerator` cannot overflow `i128`.
    #[inline]
    pub fn mul_div_rem_u64(
        self,
        numerator: u64,
        denominator: u128,
    ) -> Result<(Self, u128), LedgerError> {
        if self.0 < 0 {
            return Err(LedgerError::NegativeAmountNotPermitted(self));
        }
        let abs_u = self.0.cast_unsigned();
        let (q, r) = u256_mul_div_rem(abs_u, numerator, denominator)?;
        if q > (i128::MAX.cast_unsigned()) {
            return Err(LedgerError::ArithmeticOverflow);
        }
        Ok((Self(q.cast_signed()), r))
    }

    /// Encodes `Money(i128)` into a 16-byte big-endian array with the MSB sign bit flipped
    /// so SQLite `BLOB` byte-lexicographical ordering (`<`, `>`, `ORDER BY`) matches `Ord`.
    #[inline]
    pub const fn to_sortable_be_bytes(self) -> [u8; 16] {
        (self.0.cast_unsigned() ^ SIGN_FLIP_MASK).to_be_bytes()
    }

    /// Decodes `Money(i128)` from a 16-byte sortable big-endian `BLOB`.
    #[inline]
    pub const fn from_sortable_be_bytes(bytes: [u8; 16]) -> Self {
        Self((u128::from_be_bytes(bytes) ^ SIGN_FLIP_MASK).cast_signed())
    }
}

impl fmt::Debug for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Money({self})")
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs_val = self.0.unsigned_abs();
        let dollars = abs_val / 100;
        let cents = abs_val % 100;
        write!(f, "{sign}${dollars}.{cents:02}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_money_sortable_be_extremes() {
        let values = [
            Money::MIN,
            Money::from_cents(-1_000_000),
            Money::from_cents(-1),
            Money::ZERO,
            Money::ONE_CENT,
            Money::ONE_DOLLAR,
            Money::MAX,
        ];
        for pair in values.windows(2) {
            let [a, b] = pair else { continue };
            assert!(a < b);
            assert!(a.to_sortable_be_bytes() < b.to_sortable_be_bytes());
            assert_eq!(Money::from_sortable_be_bytes(a.to_sortable_be_bytes()), *a);
            assert_eq!(Money::from_sortable_be_bytes(b.to_sortable_be_bytes()), *b);
        }
    }

    proptest! {
        #[test]
        fn prop_money_sortable_be_roundtrip_and_order(a in any::<i128>(), b in any::<i128>()) {
            let ma = Money::from_cents(a);
            let mb = Money::from_cents(b);
            let ba = ma.to_sortable_be_bytes();
            let bb = mb.to_sortable_be_bytes();

            prop_assert_eq!(Money::from_sortable_be_bytes(ba), ma);
            prop_assert_eq!(Money::from_sortable_be_bytes(bb), mb);
            prop_assert_eq!(ma.cmp(&mb), ba.cmp(&bb));
        }
    }
}
