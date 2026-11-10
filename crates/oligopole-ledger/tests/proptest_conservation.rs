#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::inconsistent_digit_grouping,
    clippy::too_many_lines
)]

use oligopole_ledger::{
    CommercialBankFacility, FinancialStatementReport, JournalEntry, LedgerEngine, Money,
    StandardAccountCode, allocate_hamilton,
};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum LedgerAction {
    OriginateLoan {
        firm_idx: usize,
        principal_cents: i128,
    },
    RepayLoan {
        firm_idx: usize,
        amount_cents: i128,
    },
    PayWage {
        firm_idx: usize,
        wage_cents: i128,
    },
    HouseholdPurchase {
        firm_idx: usize,
        spend_cents: i128,
    },
    PayProRataDividend {
        firm_idx: usize,
        payout_cents: i128,
        weights: [u64; 3],
    },
    ReserveAndReleaseCash {
        firm_idx: usize,
        reserve_cents: i128,
    },
    AccrueDailyInterest,
    CloseIncomeStatements,
    DefaultAndWriteOff {
        firm_idx: usize,
    },
}

fn action_strategy() -> impl Strategy<Value = LedgerAction> {
    prop_oneof![
        (0usize..3, 1_00i128..1_000_000_00i128).prop_map(|(firm_idx, principal_cents)| {
            LedgerAction::OriginateLoan {
                firm_idx,
                principal_cents,
            }
        }),
        (0usize..3, 1_00i128..500_000_00i128).prop_map(|(firm_idx, amount_cents)| {
            LedgerAction::RepayLoan {
                firm_idx,
                amount_cents,
            }
        }),
        (0usize..3, 1_00i128..200_000_00i128).prop_map(|(firm_idx, wage_cents)| {
            LedgerAction::PayWage {
                firm_idx,
                wage_cents,
            }
        }),
        (0usize..3, 1_00i128..200_000_00i128).prop_map(|(firm_idx, spend_cents)| {
            LedgerAction::HouseholdPurchase {
                firm_idx,
                spend_cents,
            }
        }),
        (
            0usize..3,
            1_00i128..100_000_00i128,
            [1u64..100, 1u64..100, 1u64..100]
        )
            .prop_map(|(firm_idx, payout_cents, weights)| {
                LedgerAction::PayProRataDividend {
                    firm_idx,
                    payout_cents,
                    weights,
                }
            }),
        (0usize..3, 1_00i128..100_000_00i128).prop_map(|(firm_idx, reserve_cents)| {
            LedgerAction::ReserveAndReleaseCash {
                firm_idx,
                reserve_cents,
            }
        }),
        Just(LedgerAction::AccrueDailyInterest),
        Just(LedgerAction::CloseIncomeStatements),
        (0usize..3).prop_map(|firm_idx| LedgerAction::DefaultAndWriteOff { firm_idx }),
    ]
}

proptest! {
    #[test]
    fn prop_closed_loop_monetary_conservation_under_arbitrary_operations(
        actions in prop::collection::vec(action_strategy(), 1..=80)
    ) {
        let mut ledger = LedgerEngine::new();
        let bank_id = ledger.register_entity("Central & Commercial Bank", true);
        let firms = [
            ledger.register_entity("IronWorks Mining", false),
            ledger.register_entity("Acme Appliances", false),
            ledger.register_entity("Apex Retail", false),
        ];
        let households = [
            ledger.register_entity("Wage Cohort", false),
            ledger.register_entity("Manager Cohort", false),
            ledger.register_entity("Capitalist Cohort", false),
        ];

        let mut bank = CommercialBankFacility::new(bank_id, 600, 200);
        bank.seed_bank_equity_reserves(&mut ledger, 0, Money::from_dollars(10_000_000))
            .unwrap();

        for &f in &firms {
            bank.seed_initial_deposit_endowment(&mut ledger, 0, f, Money::from_dollars(250_000))
                .unwrap();
        }
        for &h in &households {
            bank.seed_initial_deposit_endowment(&mut ledger, 0, h, Money::from_dollars(100_000))
                .unwrap();
        }

        ledger.audit_trial_balance().unwrap();

        for (step, action) in actions.into_iter().enumerate() {
            let tick = (step as u64) + 1;
            match action {
                LedgerAction::OriginateLoan {
                    firm_idx,
                    principal_cents,
                } => {
                    let firm = firms[firm_idx];
                    bank.originate_loan(
                        &mut ledger,
                        tick,
                        firm,
                        Money::from_cents(principal_cents),
                        None,
                    )
                    .unwrap();
                }
                LedgerAction::RepayLoan {
                    firm_idx,
                    amount_cents,
                } => {
                    let firm = firms[firm_idx];
                    let avail = ledger.available_cash(firm).unwrap();
                    let pay = avail.min(Money::from_cents(amount_cents));
                    if pay.is_positive() {
                        bank.repay_loan_principal(&mut ledger, tick, firm, pay)
                            .unwrap();
                    }
                }
                LedgerAction::PayWage {
                    firm_idx,
                    wage_cents,
                } => {
                    let firm = firms[firm_idx];
                    let avail = ledger.available_cash(firm).unwrap();
                    let pay = avail.min(Money::from_cents(wage_cents));
                    if pay.is_positive() {
                        let hh = households[0];
                        let f_wage = ledger
                            .entity_account_id(firm, StandardAccountCode::WageExpense)
                            .unwrap();
                        let f_cash = ledger
                            .entity_account_id(firm, StandardAccountCode::CashAndDeposits)
                            .unwrap();
                        let h_cash = ledger
                            .entity_account_id(hh, StandardAccountCode::CashAndDeposits)
                            .unwrap();
                        let h_rev = ledger
                            .entity_account_id(hh, StandardAccountCode::SalesRevenue)
                            .unwrap();

                        let entry = JournalEntry::new(tick, "Payroll Transfer")
                            .debit(f_wage, pay)
                            .credit(f_cash, pay)
                            .debit(h_cash, pay)
                            .credit(h_rev, pay);
                        ledger.post_entry(&entry).unwrap();
                    }
                }
                LedgerAction::HouseholdPurchase {
                    firm_idx,
                    spend_cents,
                } => {
                    let firm = firms[firm_idx];
                    let hh = households[0];
                    let avail = ledger.available_cash(hh).unwrap();
                    let spend = avail.min(Money::from_cents(spend_cents));
                    if spend.is_positive() {
                        let h_exp = ledger
                            .entity_account_id(hh, StandardAccountCode::ConsumptionExpense)
                            .unwrap();
                        let h_cash = ledger
                            .entity_account_id(hh, StandardAccountCode::CashAndDeposits)
                            .unwrap();
                        let f_cash = ledger
                            .entity_account_id(firm, StandardAccountCode::CashAndDeposits)
                            .unwrap();
                        let f_rev = ledger
                            .entity_account_id(firm, StandardAccountCode::SalesRevenue)
                            .unwrap();

                        let entry = JournalEntry::new(tick, "Retail Purchase")
                            .debit(h_exp, spend)
                            .credit(h_cash, spend)
                            .debit(f_cash, spend)
                            .credit(f_rev, spend);
                        ledger.post_entry(&entry).unwrap();
                    }
                }
                LedgerAction::PayProRataDividend {
                    firm_idx,
                    payout_cents,
                    weights,
                } => {
                    let firm = firms[firm_idx];
                    let avail = ledger.available_cash(firm).unwrap();
                    let payout = avail.min(Money::from_cents(payout_cents));
                    if payout.is_positive() {
                        let weight_pairs = [
                            (households[0], weights[0]),
                            (households[1], weights[1]),
                            (households[2], weights[2]),
                        ];
                        let splits = allocate_hamilton(payout, &weight_pairs).unwrap();
                        let f_re = ledger
                            .entity_account_id(firm, StandardAccountCode::RetainedEarnings)
                            .unwrap();
                        let f_cash = ledger
                            .entity_account_id(firm, StandardAccountCode::CashAndDeposits)
                            .unwrap();

                        let mut entry = JournalEntry::new(tick, "Pro-Rata Dividend")
                            .debit(f_re, payout)
                            .credit(f_cash, payout);

                        for (hh_id, share) in splits {
                            if share.is_positive() {
                                let h_cash = ledger
                                    .entity_account_id(hh_id, StandardAccountCode::CashAndDeposits)
                                    .unwrap();
                                let h_div = ledger
                                    .entity_account_id(hh_id, StandardAccountCode::DividendIncome)
                                    .unwrap();
                                entry = entry.debit(h_cash, share).credit(h_div, share);
                            }
                        }
                        ledger.post_entry(&entry).unwrap();
                    }
                }
                LedgerAction::ReserveAndReleaseCash {
                    firm_idx,
                    reserve_cents,
                } => {
                    let firm = firms[firm_idx];
                    let avail = ledger.available_cash(firm).unwrap();
                    let lock = avail.min(Money::from_cents(reserve_cents));
                    if lock.is_positive() {
                        ledger.reserve_cash(firm, lock).unwrap();
                        ledger.audit_trial_balance().unwrap();
                        ledger.release_reserved_cash(firm, lock).unwrap();
                    }
                }
                LedgerAction::AccrueDailyInterest => {
                    for &f in &firms {
                        bank.accrue_and_collect_daily_loan_interest(&mut ledger, tick, f)
                            .unwrap();
                        bank.accrue_and_pay_daily_deposit_interest(&mut ledger, tick, f)
                            .unwrap();
                    }
                    for &h in &households {
                        bank.accrue_and_pay_daily_deposit_interest(&mut ledger, tick, h)
                            .unwrap();
                    }
                }
                LedgerAction::CloseIncomeStatements => {
                    ledger
                        .close_income_statement_for_entity(tick, bank_id)
                        .unwrap();
                    for &f in &firms {
                        ledger.close_income_statement_for_entity(tick, f).unwrap();
                    }
                    for &h in &households {
                        ledger.close_income_statement_for_entity(tick, h).unwrap();
                    }
                }
                LedgerAction::DefaultAndWriteOff { firm_idx } => {
                    let firm = firms[firm_idx];
                    bank.liquidate_and_write_off_default(&mut ledger, tick, firm)
                        .unwrap();
                }
            }

            // Invariant must hold after EVERY SINGLE operation!
            ledger.audit_trial_balance().unwrap();
        }

        // Verify financial statement generation for all entities
        for &f in &firms {
            let report = FinancialStatementReport::generate(&ledger, f, 999).unwrap();
            prop_assert_eq!(
                report.total_assets,
                report
                    .total_liabilities
                    .checked_add(report.total_equity_including_net_income)
                    .unwrap()
            );
        }
    }
}
