use crate::error::LedgerError;

/// Multiplies `a: u128` by `b: u64` into an exact 192-bit intermediate product and
/// divides by `d: u128`, returning `(quotient, remainder)` without intermediate overflow.
///
/// Returns:
/// - `Err(LedgerError::DivisionByZero)` if `d == 0`.
/// - `Err(LedgerError::ArithmeticOverflow)` if `(a * b) / d` exceeds `u128::MAX`.
#[inline]
pub fn u256_mul_div_rem(a: u128, b: u64, d: u128) -> Result<(u128, u128), LedgerError> {
    if d == 0 {
        return Err(LedgerError::DivisionByZero);
    }
    if a == 0 || b == 0 {
        return Ok((0, 0));
    }

    let a_lo = a & 0xFFFF_FFFF_FFFF_FFFF;
    let a_hi = a >> 64;
    let b_u128 = u128::from(b);

    let p_lo = a_lo * b_u128;
    let p_mid = a_hi * b_u128;

    let mid_shifted = p_mid << 64;
    let (lo, overflow) = p_lo.overflowing_add(mid_shifted);
    let carry = u128::from(overflow);
    let hi = (p_mid >> 64) + carry;

    if hi >= d {
        return Err(LedgerError::ArithmeticOverflow);
    }

    let mut rem = hi;
    let mut quot = 0u128;
    let mut shift = 128u32;
    while shift > 0 {
        shift -= 1;
        let bit = (lo >> shift) & 1;
        let carry_out = (rem >> 127) != 0;
        rem = (rem << 1) | bit;
        if carry_out || rem >= d {
            rem = rem.wrapping_sub(d);
            quot |= 1u128 << shift;
        }
    }

    Ok((quot, rem))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_u256_mul_div_rem_basic_and_extremes() {
        assert_eq!(
            u256_mul_div_rem(0, 100, 50),
            Ok((0, 0))
        );
        assert_eq!(
            u256_mul_div_rem(100, 0, 50),
            Ok((0, 0))
        );
        assert_eq!(
            u256_mul_div_rem(10, 3, 0),
            Err(LedgerError::DivisionByZero)
        );
        // Large i128::MAX * u64::MAX / u128::MAX
        let max_i128_u = i128::MAX.cast_unsigned();
        let b = u64::MAX;
        let d = u128::from(b);
        assert_eq!(
            u256_mul_div_rem(max_i128_u, b, d),
            Ok((max_i128_u, 0))
        );
    }

    proptest! {
        #[test]
        fn prop_u256_mul_div_matches_u128_when_no_overflow(
            a in 0u128..=u128::from(u64::MAX),
            b in 0u64..=u64::MAX,
            d in 1u128..=u128::MAX,
        ) {
            let exact_prod = a * u128::from(b);
            let expected_q = exact_prod / d;
            let expected_r = exact_prod % d;
            let (q, r) = u256_mul_div_rem(a, b, d).unwrap();
            prop_assert_eq!(q, expected_q);
            prop_assert_eq!(r, expected_r);
        }

        #[test]
        fn prop_u256_mul_div_identity_when_b_le_d(
            a in 0u128..=u128::MAX,
            b in 1u64..=u64::MAX,
            extra_d in 0u128..=u128::from(u64::MAX),
        ) {
            let d = u128::from(b).saturating_add(extra_d);
            let (q, r) = u256_mul_div_rem(a, b, d).unwrap();
            prop_assert!(q <= a);
            prop_assert!(r < d);
        }
    }
}
