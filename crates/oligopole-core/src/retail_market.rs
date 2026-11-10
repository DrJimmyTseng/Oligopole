use crate::error::CoreError;
use crate::household::{HouseholdCohort, RetailStoreOffer};
use crate::ids::{CohortId, CorpId, SkuId};
use oligopole_ledger::{Money, allocate_units_hamilton};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Summary of a single store-to-cohort retail transaction settled during Sub-Phase 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetailSaleSettlement {
    pub seller_corp_id: CorpId,
    pub buyer_cohort_id: CohortId,
    pub sku_id: SkuId,
    pub units_sold: u64,
    pub unit_price: Money,
    pub gross_revenue: Money,
}

/// Computes deterministic B2C retail clearing across all `cohorts` and `offers`.
///
/// 1. Each cohort splits its daily retail budget across active stores using MNL utility (`allocate_retail_expenditure`).
/// 2. Each cohort's store dollar allocation is converted into discrete desired units `desired_units = budget_for_store / unit_price`.
/// 3. If total desired units across all cohorts at a store exceed `offer.available_units`, the store's `available_units`
///    are rationed pro-rata across cohorts via `allocate_units_hamilton` (sorted by `(Remainder DESC, CohortId ASC)`).
pub fn compute_retail_clearing_plan(
    sku_id: SkuId,
    cohorts: &[(CohortId, &HouseholdCohort, Money)],
    offers: &[RetailStoreOffer],
    reference_price: Money,
) -> Result<Vec<RetailSaleSettlement>, CoreError> {
    if offers.is_empty() || cohorts.is_empty() {
        return Ok(Vec::new());
    }

    let mut offers_by_corp: BTreeMap<CorpId, RetailStoreOffer> = BTreeMap::new();
    for &offer in offers {
        if offer.available_units > 0 && offer.unit_price.is_positive() {
            offers_by_corp.insert(offer.seller_corp_id, offer);
        }
    }
    if offers_by_corp.is_empty() {
        return Ok(Vec::new());
    }

    // Map: seller_corp_id -> Vec<(cohort_id, desired_units)>
    let mut store_demands: BTreeMap<CorpId, Vec<(CohortId, u64)>> = BTreeMap::new();

    for &(cohort_id, cohort, available_cash) in cohorts {
        let budget = cohort.compute_daily_retail_budget(available_cash)?;
        if !budget.is_positive() {
            continue;
        }
        let store_budgets =
            cohort.allocate_retail_expenditure(budget, offers, reference_price)?;

        for (corp_id, allocated_money) in store_budgets {
            let Some(offer) = offers_by_corp.get(&corp_id) else {
                continue;
            };
            let desired_i128 = allocated_money
                .checked_div_euclid(offer.unit_price.cents())?
                .cents();
            let desired_units = u64::try_from(desired_i128).unwrap_or(0);
            if desired_units > 0 {
                store_demands
                    .entry(corp_id)
                    .or_default()
                    .push((cohort_id, desired_units));
            }
        }
    }

    let mut settlements = Vec::new();

    for (corp_id, cohort_requests) in store_demands {
        let Some(offer) = offers_by_corp.get(&corp_id) else {
            continue;
        };
        let total_requested: u64 = cohort_requests.iter().map(|&(_, q)| q).sum();
        if total_requested == 0 {
            continue;
        }

        let fulfilled_list = if total_requested <= offer.available_units {
            cohort_requests
        } else {
            allocate_units_hamilton(offer.available_units, &cohort_requests)?
        };

        for (cohort_id, units) in fulfilled_list {
            if units == 0 {
                continue;
            }
            let gross_revenue = offer.unit_price.checked_mul_u64(units)?;
            settlements.push(RetailSaleSettlement {
                seller_corp_id: corp_id,
                buyer_cohort_id: cohort_id,
                sku_id,
                units_sold: units,
                unit_price: offer.unit_price,
                gross_revenue,
            });
        }
    }

    Ok(settlements)
}
