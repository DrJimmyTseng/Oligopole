use crate::error::LedgerError;
use crate::money::Money;
use serde::{Deserialize, Serialize};

/// $O(1)$ Weighted Average Cost (WAC) inventory tracker for a single SKU at an entity.
///
/// Maintains:
/// - `total_qty`: total physical units legally owned by the entity.
/// - `total_book_value`: exact balance-sheet book value (`Money(i128)`) of all `total_qty` units.
/// - `reserved_qty`: units currently locked in open CLOB Ask orders (cannot be consumed or double-sold
///   until either filled or cancelled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WacInventoryLot {
    total_qty: u64,
    total_book_value: Money,
    reserved_qty: u64,
}

impl WacInventoryLot {
    pub const fn new() -> Self {
        Self {
            total_qty: 0,
            total_book_value: Money::ZERO,
            reserved_qty: 0,
        }
    }

    #[inline]
    pub const fn total_qty(self) -> u64 {
        self.total_qty
    }

    #[inline]
    pub const fn total_book_value(self) -> Money {
        self.total_book_value
    }

    #[inline]
    pub const fn reserved_qty(self) -> u64 {
        self.reserved_qty
    }

    /// Quantity available to be consumed in production, sold at retail, or locked into a new CLOB Ask order.
    #[inline]
    pub const fn available_qty(self) -> u64 {
        self.total_qty.saturating_sub(self.reserved_qty)
    }

    /// Current weighted average unit cost rounded down to the nearest cent, or `Money::ZERO` if empty.
    #[inline]
    pub const fn average_unit_cost_floor(self) -> Money {
        if self.total_qty == 0 {
            return Money::ZERO;
        }
        Money::from_cents(self.total_book_value.cents() / (self.total_qty as i128))
    }

    /// Records the addition of `qty` units with total `acquisition_cost >= Money::ZERO`.
    pub fn record_addition(
        &mut self,
        qty: u64,
        acquisition_cost: Money,
    ) -> Result<(), LedgerError> {
        if acquisition_cost.is_negative() {
            return Err(LedgerError::NegativeAmountNotPermitted(acquisition_cost));
        }
        if qty == 0 {
            if !acquisition_cost.is_zero() {
                // Allow capitalizing freight/landing cost into existing positive inventory,
                // or reject if inventory is empty.
                if self.total_qty == 0 {
                    return Err(LedgerError::InsufficientInventoryQty {
                        available: 0,
                        requested: 1,
                    });
                }
                self.total_book_value = self.total_book_value.checked_add(acquisition_cost)?;
            }
            return Ok(());
        }

        let new_qty = self
            .total_qty
            .checked_add(qty)
            .ok_or(LedgerError::ArithmeticOverflow)?;
        let new_book = self.total_book_value.checked_add(acquisition_cost)?;
        self.total_qty = new_qty;
        self.total_book_value = new_book;
        Ok(())
    }

    /// Locks `qty` unreserved units into CLOB Ask escrow without altering book value or legal title.
    pub fn reserve_units(&mut self, qty: u64) -> Result<(), LedgerError> {
        let avail = self.available_qty();
        if qty > avail {
            return Err(LedgerError::InsufficientInventoryQty {
                available: avail,
                requested: qty,
            });
        }
        self.reserved_qty = self
            .reserved_qty
            .checked_add(qty)
            .ok_or(LedgerError::ArithmeticOverflow)?;
        Ok(())
    }

    /// Releases `qty` previously reserved units (e.g. when a CLOB Ask order is cancelled).
    pub fn release_reserved_units(&mut self, qty: u64) -> Result<(), LedgerError> {
        if qty > self.reserved_qty {
            return Err(LedgerError::ExcessiveReservationRelease {
                reserved: u128::from(self.reserved_qty),
                release: u128::from(qty),
            });
        }
        self.reserved_qty -= qty;
        Ok(())
    }

    /// Draws `qty` unreserved units from inventory (for production WIP or retail sale) and returns
    /// the exact `Money` cost of goods sold (COGS) to credit from the Inventory asset account.
    ///
    /// When `qty == self.total_qty`, sweeps the exact remaining `total_book_value` so zero rounding
    /// dust is ever stranded in an empty lot.
    pub fn draw_unreserved_units(&mut self, qty: u64) -> Result<Money, LedgerError> {
        if qty == 0 {
            return Ok(Money::ZERO);
        }
        let avail = self.available_qty();
        if qty > avail {
            return Err(LedgerError::InsufficientInventoryQty {
                available: avail,
                requested: qty,
            });
        }
        self.draw_internal(qty)
    }

    /// Draws `qty` units that were already reserved in CLOB Ask escrow (upon trade fill),
    /// decrementing both `reserved_qty` and `total_qty` and returning the exact WAC COGS.
    pub fn settle_reserved_units(&mut self, qty: u64) -> Result<Money, LedgerError> {
        if qty == 0 {
            return Ok(Money::ZERO);
        }
        if qty > self.reserved_qty {
            return Err(LedgerError::ExcessiveReservationRelease {
                reserved: u128::from(self.reserved_qty),
                release: u128::from(qty),
            });
        }
        self.reserved_qty -= qty;
        self.draw_internal(qty)
    }

    fn draw_internal(&mut self, qty: u64) -> Result<Money, LedgerError> {
        if qty == self.total_qty {
            let cogs = self.total_book_value;
            self.total_qty = 0;
            self.total_book_value = Money::ZERO;
            return Ok(cogs);
        }

        let (cogs, _) = self
            .total_book_value
            .mul_div_rem_u64(qty, u128::from(self.total_qty))?;
        self.total_qty -= qty;
        self.total_book_value = self.total_book_value.checked_sub(cogs)?;
        Ok(cogs)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_wac_lot_depletion_sweeps_exact_dust() {
        let mut lot = WacInventoryLot::new();
        // Add 3 units for $1.00 (100 cents -> 33.333... cents/unit)
        lot.record_addition(3, Money::from_cents(100)).unwrap();
        let c1 = lot.draw_unreserved_units(1).unwrap();
        assert_eq!(c1, Money::from_cents(33));
        assert_eq!(lot.total_qty(), 2);
        assert_eq!(lot.total_book_value(), Money::from_cents(67));

        let c2 = lot.draw_unreserved_units(1).unwrap();
        assert_eq!(c2, Money::from_cents(33));
        assert_eq!(lot.total_qty(), 1);
        assert_eq!(lot.total_book_value(), Money::from_cents(34));

        // Final unit sweeps the exact 34 cents!
        let c3 = lot.draw_unreserved_units(1).unwrap();
        assert_eq!(c3, Money::from_cents(34));
        assert_eq!(lot.total_qty(), 0);
        assert_eq!(lot.total_book_value(), Money::ZERO);
    }

    #[test]
    fn test_wac_reservation_escrow_preserves_cost_on_cancel() {
        let mut lot = WacInventoryLot::new();
        lot.record_addition(10, Money::from_cents(1_003)).unwrap();
        lot.reserve_units(6).unwrap();
        assert_eq!(lot.available_qty(), 4);
        assert_eq!(lot.reserved_qty(), 6);
        assert!(lot.draw_unreserved_units(5).is_err());

        // Cancel 6 reserved units -> book value is untouched at 1,003 cents
        lot.release_reserved_units(6).unwrap();
        assert_eq!(lot.available_qty(), 10);
        assert_eq!(lot.total_book_value(), Money::from_cents(1_003));
    }

    proptest! {
        #[test]
        fn prop_wac_lot_full_depletion_conserves_exact_cost(
            additions in prop::collection::vec((1u64..=10_000u64, 0i128..=1_000_000_000i128), 1..=20),
            draw_fractions in prop::collection::vec(1u8..=10u8, 1..=20),
        ) {
            let mut lot = WacInventoryLot::new();
            let mut total_added_cost = Money::ZERO;
            let mut total_drawn_cost = Money::ZERO;

            for (qty, cents) in additions {
                let cost = Money::from_cents(cents);
                lot.record_addition(qty, cost).unwrap();
                total_added_cost = total_added_cost.checked_add(cost).unwrap();
            }

            for frac in draw_fractions {
                let avail = lot.available_qty();
                if avail == 0 {
                    break;
                }
                let draw_qty = ((u128::from(avail) * u128::from(frac)) / 10).max(1) as u64;
                let draw_qty = draw_qty.min(avail);
                let cogs = lot.draw_unreserved_units(draw_qty).unwrap();
                prop_assert!(!cogs.is_negative());
                total_drawn_cost = total_drawn_cost.checked_add(cogs).unwrap();
            }

            // Drain any remaining units
            let rem = lot.available_qty();
            if rem > 0 {
                let cogs = lot.draw_unreserved_units(rem).unwrap();
                total_drawn_cost = total_drawn_cost.checked_add(cogs).unwrap();
            }

            prop_assert_eq!(lot.total_qty(), 0);
            prop_assert_eq!(lot.total_book_value(), Money::ZERO);
            prop_assert_eq!(total_drawn_cost, total_added_cost);
        }
    }
}
