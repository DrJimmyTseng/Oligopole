use crate::channel_link::{ChannelLinkRegistry, LinkChannel};
use crate::command_bar::TerminalMnemonic;
use crate::theme::{AMBER_PRIMARY, ASK_RED, BID_GREEN, CYAN_ACCENT, EMERALD_ACCENT, TEXT_MUTED};
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use egui_extras::{Column, TableBuilder};
use egui_plot::{Line, Plot, PlotPoints};
use oligopole_core::{
    CorporationRole, CorporationSnapshot, InstrumentSnapshot, OrderSide, PlayerCommand, WorldView,
};
use oligopole_ledger::{Money, StandardAccountCode};
use tokio::sync::mpsc;

/// A dockable pane instance inside the `egui_tiles` Bloomberg Terminal workspace.
#[derive(Debug, Clone)]
pub struct TerminalPane {
    pub mnemonic: TerminalMnemonic,
    pub channel: LinkChannel,
    pub local_ticker: String,
    pub order_qty_input: u64,
    pub order_price_dollars: i64,
    pub loan_amount_dollars: i64,
}

impl TerminalPane {
    pub fn new(mnemonic: TerminalMnemonic, channel: LinkChannel, default_ticker: &str) -> Self {
        Self {
            mnemonic,
            channel,
            local_ticker: default_ticker.to_owned(),
            order_qty_input: 50,
            order_price_dollars: 42,
            loan_amount_dollars: 50_000,
        }
    }

    pub fn active_ticker(&self, registry: &ChannelLinkRegistry) -> String {
        if self.channel == LinkChannel::Unlinked {
            self.local_ticker.clone()
        } else {
            registry.ticker_for(self.channel, &self.local_ticker)
        }
    }

    pub fn title(&self, registry: &ChannelLinkRegistry) -> String {
        let ticker = self.active_ticker(registry);
        let mnem = match self.mnemonic {
            TerminalMnemonic::MarketMonitor => "MM • MARKET MONITOR",
            TerminalMnemonic::GraphPrice => "GP • PRICE & VOLUME",
            TerminalMnemonic::OrderBook => "OB • L2 DEPTH & TAPE",
            TerminalMnemonic::FinancialAnalysis => "FA • FINANCIALS",
            TerminalMnemonic::OperationsDag => "OPS • SUPPLY DAG",
            TerminalMnemonic::ExecutiveTicket => "EX • EXEC TICKET",
        };
        format!("[{}] {ticker} — {mnem}", self.channel.badge_label())
    }

    pub fn render(
        &mut self,
        ui: &mut Ui,
        view: &WorldView,
        registry: &mut ChannelLinkRegistry,
        cmd_tx: &mpsc::UnboundedSender<PlayerCommand>,
    ) {
        // Render top channel badge selector bar inside each pane
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(self.channel.badge_label())
                    .color(self.channel.badge_color())
                    .strong(),
            );
            for ch in LinkChannel::ALL {
                if ui
                    .selectable_label(self.channel == ch, ch.badge_label())
                    .clicked()
                {
                    self.channel = ch;
                }
            }
            ui.separator();
            let active = self.active_ticker(registry);
            ui.label(RichText::new(format!("ACTIVE: {active}")).color(AMBER_PRIMARY));
        });
        ui.separator();

        match self.mnemonic {
            TerminalMnemonic::MarketMonitor => {
                Self::render_market_monitor(ui, view, self.channel, registry);
            }
            TerminalMnemonic::GraphPrice => {
                let ticker = self.active_ticker(registry);
                Self::render_price_chart(ui, view, &ticker);
            }
            TerminalMnemonic::OrderBook => {
                let ticker = self.active_ticker(registry);
                Self::render_order_book(ui, view, &ticker);
            }
            TerminalMnemonic::FinancialAnalysis => {
                let ticker = self.active_ticker(registry);
                Self::render_financial_statements(ui, view, &ticker);
            }
            TerminalMnemonic::OperationsDag => {
                Self::render_operations_dag(ui, view);
            }
            TerminalMnemonic::ExecutiveTicket => {
                let ticker = self.active_ticker(registry);
                self.render_executive_ticket(ui, view, &ticker, cmd_tx);
            }
        }
    }

    fn render_market_monitor(
        ui: &mut Ui,
        view: &WorldView,
        target_channel: LinkChannel,
        registry: &mut ChannelLinkRegistry,
    ) {
        ui.label(
            RichText::new("CORPORATE EQUITIES & OPERATING METRICS (Click row to link ticker)")
                .color(CYAN_ACCENT)
                .strong(),
        );

        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(65.0))
            .column(Column::initial(160.0))
            .column(Column::initial(85.0))
            .column(Column::initial(95.0))
            .column(Column::initial(95.0))
            .column(Column::remainder())
            .header(18.0, |mut header| {
                header.col(|ui| {
                    ui.strong("TICKER");
                });
                header.col(|ui| {
                    ui.strong("NAME");
                });
                header.col(|ui| {
                    ui.strong("SELL PX");
                });
                header.col(|ui| {
                    ui.strong("CASH");
                });
                header.col(|ui| {
                    ui.strong("NET INC");
                });
                header.col(|ui| {
                    ui.strong("EQ LAST");
                });
            })
            .body(|mut body| {
                for corp in &view.corporations {
                    body.row(20.0, |mut row| {
                        row.col(|ui| {
                            if ui
                                .button(RichText::new(&corp.ticker).color(AMBER_PRIMARY).strong())
                                .clicked()
                            {
                                let ch = if target_channel == LinkChannel::Unlinked {
                                    LinkChannel::ChannelA
                                } else {
                                    target_channel
                                };
                                registry.set_ticker(ch, corp.ticker.clone());
                            }
                        });
                        row.col(|ui| {
                            ui.label(&corp.name);
                        });
                        row.col(|ui| {
                            ui.label(format!("{}", corp.posted_selling_price));
                        });
                        row.col(|ui| {
                            ui.label(format!("{}", corp.cash_balance));
                        });
                        row.col(|ui| {
                            let ni = corp.financial_statement.net_income;
                            let color = if ni.is_negative() { ASK_RED } else { BID_GREEN };
                            ui.colored_label(color, format!("{ni}"));
                        });
                        row.col(|ui| {
                            ui.label(format!("{}", corp.share_price));
                        });
                    });
                }
            });
    }

    fn find_instrument<'a>(view: &'a WorldView, ticker: &str) -> Option<&'a InstrumentSnapshot> {
        let upper = ticker.to_uppercase();
        view.instruments
            .iter()
            .find(|i| i.ticker == upper || i.ticker.starts_with(&upper))
            .or_else(|| view.instruments.first())
    }

    fn find_corporation<'a>(view: &'a WorldView, ticker: &str) -> Option<&'a CorporationSnapshot> {
        let upper = ticker.to_uppercase();
        view.corporations
            .iter()
            .find(|c| c.ticker == upper || upper.starts_with(&c.ticker))
            .or_else(|| view.corporations.first())
    }

    #[allow(clippy::cast_precision_loss)]
    fn render_price_chart(ui: &mut Ui, view: &WorldView, ticker: &str) {
        let Some(inst) = Self::find_instrument(view, ticker) else {
            ui.label("No instrument available.");
            return;
        };

        ui.horizontal(|ui| {
            ui.label(RichText::new(&inst.ticker).color(AMBER_PRIMARY).strong());
            ui.label(format!("LAST: {}", inst.last_price));
            if let Some(bid) = inst.best_bid {
                ui.colored_label(BID_GREEN, format!("BID: {bid}"));
            }
            if let Some(ask) = inst.best_ask {
                ui.colored_label(ASK_RED, format!("ASK: {ask}"));
            }
        });

        let points: PlotPoints = inst
            .ohlcv_bars
            .iter()
            .map(|bar| {
                let x = bar.bar_tick as f64;
                let y = (bar.close.cents() as f64) / 100.0;
                [x, y]
            })
            .collect();

        Plot::new(format!("gp_plot_{}", inst.ticker))
            .height(ui.available_height().max(120.0))
            .show(ui, |plot_ui| {
                plot_ui.line(
                    Line::new(points)
                        .color(AMBER_PRIMARY)
                        .name(format!("{} Close ($)", inst.ticker)),
                );
            });
    }

    fn render_order_book(ui: &mut Ui, view: &WorldView, ticker: &str) {
        let Some(inst) = Self::find_instrument(view, ticker) else {
            ui.label("No order book available.");
            return;
        };

        ui.label(
            RichText::new(format!("{} • CLOB DEPTH & RECENT FILLS", inst.ticker))
                .color(CYAN_ACCENT)
                .strong(),
        );

        ui.columns(2, |cols| {
            if let Some(left) = cols.first_mut() {
                left.colored_label(BID_GREEN, "BIDS (Price / Qty)");
                for lvl in &inst.top_bids {
                    left.label(format!(
                        "{}  ×  {} ({})",
                        lvl.price, lvl.total_qty, lvl.order_count
                    ));
                }
            }
            if let Some(right) = cols.get_mut(1) {
                right.colored_label(ASK_RED, "ASKS (Price / Qty)");
                for lvl in &inst.top_asks {
                    right.label(format!(
                        "{}  ×  {} ({})",
                        lvl.price, lvl.total_qty, lvl.order_count
                    ));
                }
            }
        });

        ui.separator();
        ui.label(RichText::new("RECENT TRADE FILLS").color(TEXT_MUTED));
        for fill in inst.recent_fills.iter().rev().take(6) {
            ui.label(format!(
                "T{} | {} units @ {}",
                fill.tick, fill.qty, fill.execution_price
            ));
        }
    }

    fn render_financial_statements(ui: &mut Ui, view: &WorldView, ticker: &str) {
        let Some(corp) = Self::find_corporation(view, ticker) else {
            ui.label("No corporation found.");
            return;
        };
        let st = &corp.financial_statement;

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} ({}) — GAAP DOUBLE-ENTRY BOOKS", corp.name, corp.ticker))
                    .color(AMBER_PRIMARY)
                    .strong(),
            );
            ui.colored_label(
                EMERALD_ACCENT,
                format!(
                    "Assets ({}) == L + E ({})",
                    st.total_assets,
                    st.total_liabilities
                        .checked_add(st.total_equity_including_net_income)
                        .unwrap_or(Money::ZERO)
                ),
            );
        });

        TableBuilder::new(ui)
            .striped(true)
            .column(Column::initial(210.0))
            .column(Column::remainder())
            .header(18.0, |mut h| {
                h.col(|ui| {
                    ui.strong("ACCOUNT");
                });
                h.col(|ui| {
                    ui.strong("NORMAL BALANCE");
                });
            })
            .body(|mut body| {
                for code in [
                    StandardAccountCode::CashAndDeposits,
                    StandardAccountCode::InventoryRaw,
                    StandardAccountCode::InventoryFinished,
                    StandardAccountCode::PropertyPlantEquipment,
                    StandardAccountCode::LoansPayable,
                    StandardAccountCode::CommonStock,
                    StandardAccountCode::RetainedEarnings,
                    StandardAccountCode::SalesRevenue,
                    StandardAccountCode::CostOfGoodsSold,
                    StandardAccountCode::WageExpense,
                    StandardAccountCode::InterestExpense,
                ] {
                    let bal = st.line_items.get(&code).copied().unwrap_or(Money::ZERO);
                    body.row(18.0, |mut row| {
                        row.col(|ui| {
                            ui.label(format!("{code:?}"));
                        });
                        row.col(|ui| {
                            ui.label(format!("{bal}"));
                        });
                    });
                }
            });
    }

    fn render_operations_dag(ui: &mut Ui, view: &WorldView) {
        ui.label(
            RichText::new("SUPPLY-CHAIN WAC INVENTORY DAG (Tier 1 CLOB -> Tier 2 B2B -> Tier 3 Retail MNL)")
                .color(EMERALD_ACCENT)
                .strong(),
        );

        let desired_size = Vec2::new(ui.available_width().max(320.0), 180.0);
        let (rect, _resp) = ui.allocate_exact_size(desired_size, Sense::hover());
        let painter = ui.painter_at(rect);

        painter.rect_filled(rect, 4.0, Color32::from_rgb(14, 18, 24));

        for (idx, corp) in view.corporations.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let col_x = rect.left() + 20.0 + (idx as f32) * 145.0;
            let box_rect = Rect::from_min_size(Pos2::new(col_x, rect.top() + 25.0), Vec2::new(130.0, 125.0));
            let border_color = match corp.role {
                CorporationRole::ResourceExtractor { .. } => AMBER_PRIMARY,
                CorporationRole::Manufacturer { .. } => CYAN_ACCENT,
                CorporationRole::Retailer { .. } => EMERALD_ACCENT,
            };
            painter.rect_stroke(
                box_rect,
                4.0_f32,
                Stroke::new(1.5_f32, border_color),
                egui::StrokeKind::Inside,
            );

            let total_inv_units: u64 = corp.inventory.values().map(|l| l.total_qty()).sum();
            let info = format!(
                "{}\nCap: {}/d\nPx: {}\nWAC Qty: {}",
                corp.ticker, corp.daily_capacity, corp.posted_selling_price, total_inv_units
            );
            painter.text(
                box_rect.left_top() + Vec2::new(8.0, 8.0),
                egui::Align2::LEFT_TOP,
                info,
                egui::FontId::monospace(11.5),
                Color32::WHITE,
            );
        }
    }

    fn render_executive_ticket(
        &mut self,
        ui: &mut Ui,
        view: &WorldView,
        ticker: &str,
        cmd_tx: &mpsc::UnboundedSender<PlayerCommand>,
    ) {
        let Some(corp) = Self::find_corporation(view, ticker) else {
            ui.label("Select a corporation to issue executive commands.");
            return;
        };

        ui.label(
            RichText::new(format!("EXECUTIVE COMMAND TICKET • {} ({})", corp.name, corp.ticker))
                .color(AMBER_PRIMARY)
                .strong(),
        );
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("PERSONA: {}", corp.persona_kind.badge_label()))
                    .color(CYAN_ACCENT)
                    .strong(),
            );
            ui.separator();
            ui.label(format!(
                "Shares Out: {} | Mkt Cap: {}",
                corp.total_shares_outstanding, corp.market_cap
            ));
        });

        ui.horizontal(|ui| {
            ui.label(format!("Current Selling Price: {}", corp.posted_selling_price));
            if ui.button("- $2.00").clicked() {
                let new_px = Money::from_cents((corp.posted_selling_price.cents() - 200).max(100));
                let _ = cmd_tx.send(PlayerCommand::SetCorporationPrice {
                    corp_id: corp.corp_id,
                    new_price: new_px,
                });
            }
            if ui.button("+ $2.00").clicked() {
                let new_px = Money::from_cents(corp.posted_selling_price.cents() + 200);
                let _ = cmd_tx.send(PlayerCommand::SetCorporationPrice {
                    corp_id: corp.corp_id,
                    new_price: new_px,
                });
            }
        });

        ui.separator();
        ui.label(RichText::new("ENDOGENOUS COMMERCIAL BANK CREDIT FACILITY").color(CYAN_ACCENT));
        ui.horizontal(|ui| {
            ui.label(format!("Loans Payable: {}", corp.loans_payable));
            if ui.button("Draw $50k Loan").clicked() {
                let _ = cmd_tx.send(PlayerCommand::RequestBankLoan {
                    corp_id: corp.corp_id,
                    principal: Money::from_dollars(50_000),
                });
            }
            if ui.button("Repay $25k Loan").clicked() {
                let _ = cmd_tx.send(PlayerCommand::RepayBankLoan {
                    corp_id: corp.corp_id,
                    amount: Money::from_dollars(25_000),
                });
            }
        });

        ui.separator();
        let mode_badge = if corp.delegation.accepts_slow_tick_ai() {
            "MODE: AI AUTOPILOT ACTIVE"
        } else {
            "MODE: MANUAL EXECUTIVE CONTROL"
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("AI COO/CFO DELEGATION (`Clef-Flash` / `System0`)")
                    .color(EMERALD_ACCENT),
            );
            ui.separator();
            ui.label(RichText::new(mode_badge).color(AMBER_PRIMARY).strong());
        });
        let mut del_price = corp.delegation.delegate_pricing_to_ai;
        let mut del_capex = corp.delegation.delegate_capex_and_financing_to_ai;
        let mut del_proc = corp.delegation.delegate_procurement_to_ai;
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut del_price, "AI Pricing").changed();
            changed |= ui.checkbox(&mut del_capex, "AI CapEx/CFO").changed();
            changed |= ui.checkbox(&mut del_proc, "AI Procurement").changed();
            if ui.button("Full Manual").clicked() {
                del_price = false;
                del_capex = false;
                del_proc = false;
                changed = true;
            }
            if ui.button("Full AI Autopilot").clicked() {
                del_price = true;
                del_capex = true;
                del_proc = true;
                changed = true;
            }
        });
        if changed {
            let _ = cmd_tx.send(PlayerCommand::UpdateAiDelegation {
                corp_id: corp.corp_id,
                delegate_pricing: del_price,
                delegate_capex: del_capex,
                delegate_procurement: del_proc,
            });
        }

        if let Some(eq_id) = corp.equity_instrument_id {
            ui.separator();
            ui.label(
                RichText::new("CORPORATE SHARE BUYBACK / SECONDARY EQUITY CLOB")
                    .color(AMBER_PRIMARY),
            );
            ui.horizontal(|ui| {
                ui.label("Qty:");
                ui.add(egui::DragValue::new(&mut self.order_qty_input).range(1..=10_000));
                ui.label("Limit ($):");
                ui.add(egui::DragValue::new(&mut self.order_price_dollars).range(1..=10_000));
                if ui.button("BUYBACK SHARES (BID)").clicked() {
                    let _ = cmd_tx.send(PlayerCommand::SubmitClobOrder {
                        trader_entity_id: corp.entity_id,
                        instrument_id: eq_id,
                        side: OrderSide::Bid,
                        limit_price: Money::from_dollars(self.order_price_dollars),
                        qty: self.order_qty_input,
                    });
                }
            });
        }
    }
}
