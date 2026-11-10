use serde::{Deserialize, Serialize};
use std::fmt;

/// Canonical 30/360 institutional constants.
pub const DAYS_PER_MONTH: u64 = 30;
pub const DAYS_PER_QUARTER: u64 = 90;
pub const DAYS_PER_YEAR: u64 = 360;

/// Simulation tick counter (`1 Fast Tick == 1 Day` on the 30/360 financial calendar).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash, Serialize, Deserialize,
)]
pub struct Tick(pub u64);

impl Tick {
    pub const ZERO: Self = Self(0);

    #[inline]
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// Returns `true` at the end of every 30-day month (`tick > 0 && tick % 30 == 0`).
    #[inline]
    pub const fn is_month_end(self) -> bool {
        self.0 > 0 && self.0.is_multiple_of(DAYS_PER_MONTH)
    }

    /// Returns `true` at the end of every 90-day quarter (`tick > 0 && tick % 90 == 0`).
    #[inline]
    pub const fn is_quarter_end(self) -> bool {
        self.0 > 0 && self.0.is_multiple_of(DAYS_PER_QUARTER)
    }

    /// Returns `true` at the end of every 360-day year (`tick > 0 && tick % 360 == 0`).
    #[inline]
    pub const fn is_year_end(self) -> bool {
        self.0 > 0 && self.0.is_multiple_of(DAYS_PER_YEAR)
    }

    /// Converts a zero-indexed `Tick` into a human-readable 30/360 calendar date starting at `Year 1, Month 1, Day 1`.
    #[inline]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn to_date360(self) -> Date360 {
        let year = (self.0 / DAYS_PER_YEAR) as u32 + 1;
        let day_of_year = self.0 % DAYS_PER_YEAR;
        let month = (day_of_year / DAYS_PER_MONTH) as u8 + 1;
        let day = (day_of_year % DAYS_PER_MONTH) as u8 + 1;
        Date360 { year, month, day }
    }
}

/// Institutional 30/360 date representation (`month in 1..=12`, `day in 1..=30`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Date360 {
    pub year: u32,
    pub month: u8,
    pub day: u8,
}

impl fmt::Display for Date360 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Y{:02}-M{:02}-D{:02}", self.year, self.month, self.day)
    }
}

/// The 5 deterministic intraday sub-phases executed sequentially on every Fast Tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DailySubPhase {
    /// Sub-Phase 1: Scheduled B2B contract deliveries and spot wholesale replenishment.
    MorningLogisticsAndB2B,
    /// Sub-Phase 2: Factory DAG production recipes transforming raw inputs + labor into finished goods.
    FactoryProduction,
    /// Sub-Phase 3: Commodity and Equity Central Limit Order Book (CLOB) matching and OHLCV bar updates.
    CapitalAndCommodityClob,
    /// Sub-Phase 4: Household Stone-Geary + Padé (2,2) MNL retail purchasing and inventory rationing.
    RetailConsumption,
    /// Sub-Phase 5: Payroll settlement, 30/360 interest accrual, dividend distribution, and Trial Balance audit.
    EndOfDaySettlementAndAudit,
}

impl DailySubPhase {
    pub const ALL: [Self; 5] = [
        Self::MorningLogisticsAndB2B,
        Self::FactoryProduction,
        Self::CapitalAndCommodityClob,
        Self::RetailConsumption,
        Self::EndOfDaySettlementAndAudit,
    ];
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_calendar_360_conversions() {
        assert_eq!(
            Tick(0).to_date360(),
            Date360 {
                year: 1,
                month: 1,
                day: 1
            }
        );
        assert_eq!(
            Tick(29).to_date360(),
            Date360 {
                year: 1,
                month: 1,
                day: 30
            }
        );
        assert!(!Tick(29).is_month_end());
        assert!(Tick(30).is_month_end());
        assert_eq!(
            Tick(30).to_date360(),
            Date360 {
                year: 1,
                month: 2,
                day: 1
            }
        );
        assert!(Tick(90).is_quarter_end());
        assert!(Tick(360).is_year_end());
        assert_eq!(
            Tick(360).to_date360(),
            Date360 {
                year: 2,
                month: 1,
                day: 1
            }
        );
    }
}
