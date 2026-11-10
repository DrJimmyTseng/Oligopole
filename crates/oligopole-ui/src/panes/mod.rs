use crate::channel_link::{ChannelLinkRegistry, LinkChannel};
use crate::command_bar::TerminalMnemonic;
use crate::theme::{AMBER_PRIMARY, ASK_RED, BID_GREEN, CYAN_ACCENT, EMERALD_ACCENT, TEXT_MUTED};
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use egui_extras::{Column, TableBuilder};
use egui_plot::{Line, Plot, PlotPoints};
use oligopole_core::{
    CloudTier, CorporationRole, CorporationSnapshot, CrimeOperationKind, InstrumentKind,
    InstrumentSnapshot, OrderSide, PlayerCommand, WireAttachment, WireAttachmentStatus, WorldView,
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
    pub crime_target_ticker: String,
    pub crime_amount_dollars: i64,
    pub launder_amount_dollars: i64,
    pub ipo_shares_input: u64,
    pub ipo_offer_price_dollars: i64,
    pub sme_new_ticker: String,
    pub sme_new_name: String,
    pub wire_recipient_ticker: String,
    pub wire_subject: String,
    pub wire_body: String,
    pub wire_attachment_mode: usize,
    pub wire_offer_qty: u64,
    pub wire_offer_price_dollars: i64,
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
            crime_target_ticker: String::from("APEX"),
            crime_amount_dollars: 15_000,
            launder_amount_dollars: 5_000,
            ipo_shares_input: 25_000,
            ipo_offer_price_dollars: 15,
            sme_new_ticker: String::from("BOLT"),
            sme_new_name: String::from("Bolt Freight SME"),
            wire_recipient_ticker: String::from("APEX"),
            wire_subject: String::from("B2B Supply / Retainer Proposal"),
            wire_body: String::from("Attached 1-click smart contract offer."),
            wire_attachment_mode: 0,
            wire_offer_qty: 20,
            wire_offer_price_dollars: 130,
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
            TerminalMnemonic::CorporateWire => "MSG • CORPORATE WIRE",
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
            TerminalMnemonic::CorporateWire => {
                let ticker = self.active_ticker(registry);
                self.render_corporate_wire(ui, view, &ticker, cmd_tx);
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
            RichText::new("DOMESTIC CORPORATIONS & SME PIPELINE (Click ticker to link)")
                .color(CYAN_ACCENT)
                .strong(),
        );

        let ch = if target_channel == LinkChannel::Unlinked {
            LinkChannel::ChannelA
        } else {
            target_channel
        };

        TableBuilder::new(ui)
            .id_salt("mm_corp_table")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::initial(60.0))
            .column(Column::initial(135.0))
            .column(Column::initial(75.0))
            .column(Column::initial(85.0))
            .column(Column::initial(85.0))
            .column(Column::initial(75.0))
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
                    ui.strong("CLEAN CASH");
                });
                header.col(|ui| {
                    ui.strong("NET INC");
                });
                header.col(|ui| {
                    ui.strong("EQ LAST");
                });
                header.col(|ui| {
                    ui.strong("CLOUD / SC");
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
                            if corp.equity_instrument_id.is_some() {
                                ui.label(format!("{}", corp.share_price));
                            } else {
                                ui.colored_label(TEXT_MUTED, "PRIVATE SME");
                            }
                        });
                        row.col(|ui| {
                            ui.label(format!(
                                "{} ({} SC)",
                                corp.cloud_tier.badge_label(),
                                corp.service_credits
                            ));
                        });
                    });
                }
            });

        ui.separator();
        ui.label(
            RichText::new("GLOBAL DEPOSITARY RECEIPTS, SERVICE CREDITS (PLEX) & COMMODITY BOOKS")
                .color(EMERALD_ACCENT)
                .strong(),
        );
        ui.horizontal_wrapped(|ui| {
            for inst in &view.instruments {
                if matches!(
                    inst.kind,
                    InstrumentKind::DepositaryReceipt { .. }
                        | InstrumentKind::ServiceCredit
                        | InstrumentKind::Commodity { .. }
                ) {
                    let label = format!("{} ({})", inst.ticker, inst.last_price);
                    if ui
                        .button(RichText::new(label).color(AMBER_PRIMARY))
                        .clicked()
                    {
                        registry.set_ticker(ch, inst.ticker.clone());
                    }
                }
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
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "LULD Ref: {} (±{} bps) | Short Int: {} sh",
                    inst.luld_reference_price, inst.luld_band_bps, inst.short_interest_shares
                ))
                .color(TEXT_MUTED),
            );
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

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} • CLOB DEPTH & RECENT FILLS", inst.ticker))
                    .color(CYAN_ACCENT)
                    .strong(),
            );
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "LULD Anchor: {} | Aggregate Short Interest: {} sh",
                    inst.luld_reference_price, inst.short_interest_shares
                ))
                .color(AMBER_PRIMARY),
            );
        });

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
        ui.label(RichText::new("RECENT TRADE FILLS (10 bps Sovereign Rake)").color(TEXT_MUTED));
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
                RichText::new(format!(
                    "{} ({}) — GAAP DOUBLE-ENTRY BOOKS",
                    corp.name, corp.ticker
                ))
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
            if corp.illicit_cash_balance.is_positive() || corp.tax_lien_payable.is_positive() {
                ui.separator();
                ui.colored_label(
                    ASK_RED,
                    format!(
                        "Illicit Cash: {} | Tax Lien: {}",
                        corp.illicit_cash_balance, corp.tax_lien_payable
                    ),
                );
            }
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
                    StandardAccountCode::IllicitCash,
                    StandardAccountCode::InventoryRaw,
                    StandardAccountCode::InventoryFinished,
                    StandardAccountCode::PropertyPlantEquipment,
                    StandardAccountCode::LoansPayable,
                    StandardAccountCode::TaxesPayable,
                    StandardAccountCode::CommonStock,
                    StandardAccountCode::RetainedEarnings,
                    StandardAccountCode::SalesRevenue,
                    StandardAccountCode::CostOfGoodsSold,
                    StandardAccountCode::WageExpense,
                    StandardAccountCode::InterestExpense,
                    StandardAccountCode::TaxAndFineExpense,
                    StandardAccountCode::CloudServiceExpense,
                    StandardAccountCode::SecurityAndFreightExpense,
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
            RichText::new("SUPPLY-CHAIN WAC INVENTORY DAG & SERVICE SHIELDS (Extractor -> Manufacturer -> Retailer / Logistics / Security)")
                .color(EMERALD_ACCENT)
                .strong(),
        );

        let desired_size = Vec2::new(ui.available_width().max(320.0), 190.0);
        let (rect, _resp) = ui.allocate_exact_size(desired_size, Sense::hover());
        let painter = ui.painter_at(rect);

        painter.rect_filled(rect, 4.0, Color32::from_rgb(14, 18, 24));

        for (idx, corp) in view.corporations.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let col_x = rect.left() + 16.0 + (idx as f32) * 142.0;
            let box_rect = Rect::from_min_size(
                Pos2::new(col_x, rect.top() + 20.0),
                Vec2::new(132.0, 145.0),
            );
            let border_color = match corp.role {
                CorporationRole::ResourceExtractor { .. } => AMBER_PRIMARY,
                CorporationRole::Manufacturer { .. } => CYAN_ACCENT,
                CorporationRole::Retailer { .. } => EMERALD_ACCENT,
                CorporationRole::LogisticsTrucking { .. }
                | CorporationRole::PrivateSecurity { .. } => BID_GREEN,
            };
            painter.rect_stroke(
                box_rect,
                4.0_f32,
                Stroke::new(1.5_f32, border_color),
                egui::StrokeKind::Inside,
            );

            let total_inv_units: u64 = corp.inventory.values().map(|l| l.total_qty()).sum();
            let shield_tag = if corp.security_shield_until_tick > view.tick.0
                || corp.logistics_shield_until_tick > view.tick.0
            {
                "SHIELDED"
            } else if corp.ransomware_degraded_until_tick > view.tick.0 {
                "RANSOM-60%"
            } else {
                "UNSHIELDED"
            };
            let info = format!(
                "{} ({})\nCap: {}/d\nPx: {}\nWAC Qty: {}\n{}",
                corp.ticker,
                corp.cloud_tier.badge_label(),
                corp.daily_capacity,
                corp.posted_selling_price,
                total_inv_units,
                shield_tag
            );
            painter.text(
                box_rect.left_top() + Vec2::new(6.0, 6.0),
                egui::Align2::LEFT_TOP,
                info,
                egui::FontId::monospace(10.5),
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

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "EXECUTIVE COMMAND TICKET • {} ({})",
                    corp.name, corp.ticker
                ))
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
                    "Shares Out: {} | Mkt Cap: {} | SC: {}",
                    corp.total_shares_outstanding, corp.market_cap, corp.service_credits
                ));
            });

            ui.horizontal(|ui| {
                ui.label(format!("Current Selling Price: {}", corp.posted_selling_price));
                if ui.button("- $2.00").clicked() {
                    let new_px =
                        Money::from_cents((corp.posted_selling_price.cents() - 200).max(100));
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
            ui.label(
                RichText::new("SOVEREIGN CLOUD WEB SERVICES TIER (Monthly SC or VC Sink)")
                    .color(CYAN_ACCENT),
            );
            ui.horizontal(|ui| {
                for tier in CloudTier::ALL {
                    if ui
                        .selectable_label(corp.subscribed_cloud_tier == tier, tier.badge_label())
                        .clicked()
                    {
                        let _ = cmd_tx.send(PlayerCommand::SetCloudSubscriptionTier {
                            corp_id: corp.corp_id,
                            tier,
                        });
                    }
                }
            });

            ui.separator();
            ui.label(
                RichText::new("ENDOGENOUS COMMERCIAL BANK CREDIT FACILITY").color(CYAN_ACCENT),
            );
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
            ui.label(
                RichText::new("SME INCORPORATION & IPO LISTING PIPELINE / SHORT-LOCATE VAULT")
                    .color(EMERALD_ACCENT),
            );
            if let Some(eq_id) = corp.equity_instrument_id {
                ui.horizontal(|ui| {
                    ui.label("Qty:");
                    ui.add(egui::DragValue::new(&mut self.order_qty_input).range(1..=10_000));
                    ui.label("Limit ($):");
                    ui.add(egui::DragValue::new(&mut self.order_price_dollars).range(1..=10_000));
                    if ui.button("BUYBACK (BID)").clicked() {
                        let _ = cmd_tx.send(PlayerCommand::SubmitClobOrder {
                            trader_entity_id: corp.entity_id,
                            instrument_id: eq_id,
                            side: OrderSide::Bid,
                            limit_price: Money::from_dollars(self.order_price_dollars),
                            qty: self.order_qty_input,
                        });
                    }
                    if ui.button("SHORT BORROW & SELL").clicked() {
                        if let Some(target_c) =
                            Self::find_corporation(view, &self.crime_target_ticker)
                        {
                            let _ = cmd_tx.send(PlayerCommand::BorrowSharesToShort {
                                borrower_entity_id: corp.entity_id,
                                corp_id: target_c.corp_id,
                                shares: self.order_qty_input,
                                sell_limit_price: Money::from_dollars(self.order_price_dollars),
                            });
                        }
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label("IPO Shares:");
                    ui.add(egui::DragValue::new(&mut self.ipo_shares_input).range(1_000..=100_000));
                    ui.label("Offer ($):");
                    ui.add(egui::DragValue::new(&mut self.ipo_offer_price_dollars).range(5..=1_000));
                    if ui
                        .button(
                            RichText::new("LAUNCH IPO AUCTION (300 bps Fee)")
                                .color(AMBER_PRIMARY)
                                .strong(),
                        )
                        .clicked()
                    {
                        let _ = cmd_tx.send(PlayerCommand::LaunchIpoAuction {
                            corp_id: corp.corp_id,
                            primary_shares_to_issue: self.ipo_shares_input,
                            offer_price: Money::from_dollars(self.ipo_offer_price_dollars),
                        });
                    }
                });
            }

            ui.horizontal(|ui| {
                ui.label("Found SME Ticker:");
                ui.add(egui::TextEdit::singleline(&mut self.sme_new_ticker).desired_width(55.0));
                ui.label("Name:");
                ui.add(egui::TextEdit::singleline(&mut self.sme_new_name).desired_width(120.0));
                if let Some((&sku_id, _)) = view.sku_codes.iter().next() {
                    if ui.button("Found Logistics SME ($100k)").clicked() {
                        let _ = cmd_tx.send(PlayerCommand::FoundPrivateSme {
                            founder_entity_id: corp.entity_id,
                            ticker: self.sme_new_ticker.clone(),
                            name: self.sme_new_name.clone(),
                            role: CorporationRole::LogisticsTrucking {
                                freight_sku: sku_id,
                            },
                            seed_capital: Money::from_dollars(100_000),
                        });
                    }
                    if ui.button("Found Security SME ($100k)").clicked() {
                        let _ = cmd_tx.send(PlayerCommand::FoundPrivateSme {
                            founder_entity_id: corp.entity_id,
                            ticker: self.sme_new_ticker.clone(),
                            name: self.sme_new_name.clone(),
                            role: CorporationRole::PrivateSecurity {
                                protected_sku: sku_id,
                            },
                            seed_capital: Money::from_dollars(100_000),
                        });
                    }
                }
            });

            ui.separator();
            ui.label(
                RichText::new("SYNDICATE COVERT OPERATIONS & FORENSIC CASH LAUNDERING")
                    .color(ASK_RED)
                    .strong(),
            );
            ui.horizontal(|ui| {
                ui.label("Target Corp:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.crime_target_ticker).desired_width(60.0),
                );
                ui.label("Demand ($):");
                ui.add(
                    egui::DragValue::new(&mut self.crime_amount_dollars).range(1_000..=250_000),
                );
                if let Some(target_c) = Self::find_corporation(view, &self.crime_target_ticker) {
                    if ui.button("Extort Retailer").clicked() {
                        let _ = cmd_tx.send(PlayerCommand::ExecuteCrimeOperation {
                            actor_corp_id: corp.corp_id,
                            target_corp_id: target_c.corp_id,
                            kind: CrimeOperationKind::ExtortRetailer {
                                demand_amount: Money::from_dollars(self.crime_amount_dollars),
                            },
                        });
                    }
                    if ui.button("Cyber Ransomware").clicked() {
                        let _ = cmd_tx.send(PlayerCommand::ExecuteCrimeOperation {
                            actor_corp_id: corp.corp_id,
                            target_corp_id: target_c.corp_id,
                            kind: CrimeOperationKind::CyberRansomware {
                                ransom_demand: Money::from_dollars(self.crime_amount_dollars),
                            },
                        });
                    }
                    if ui.button("Wiretap (60d)").clicked() {
                        let _ = cmd_tx.send(PlayerCommand::ExecuteCrimeOperation {
                            actor_corp_id: corp.corp_id,
                            target_corp_id: target_c.corp_id,
                            kind: CrimeOperationKind::CorporateWiretap { duration_ticks: 60 },
                        });
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.colored_label(
                    ASK_RED,
                    format!(
                        "Illicit Cash: {} | Monthly Laundered: {} / B2C Rev: {}",
                        corp.illicit_cash_balance,
                        corp.monthly_laundered_cash,
                        corp.monthly_genuine_b2c_revenue
                    ),
                );
                ui.add(
                    egui::DragValue::new(&mut self.launder_amount_dollars).range(500..=250_000),
                );
                if ui.button("Launder Cash (500 bps Tax)").clicked() {
                    let _ = cmd_tx.send(PlayerCommand::LaunderIllicitCash {
                        corp_id: corp.corp_id,
                        amount: Money::from_dollars(self.launder_amount_dollars),
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
        });
    }

    fn render_corporate_wire(
        &mut self,
        ui: &mut Ui,
        view: &WorldView,
        ticker: &str,
        cmd_tx: &mpsc::UnboundedSender<PlayerCommand>,
    ) {
        let Some(active_corp) = Self::find_corporation(view, ticker) else {
            ui.label("Select a corporation to view its Corporate Wire (`MSG`) inbox.");
            return;
        };

        ui.label(
            RichText::new(format!(
                "INSTANT BLOOMBERG CORPORATE WIRE (`MSG`) • {} ({})",
                active_corp.name, active_corp.ticker
            ))
            .color(AMBER_PRIMARY)
            .strong(),
        );

        ui.horizontal(|ui| {
            ui.label("To Ticker:");
            ui.add(
                egui::TextEdit::singleline(&mut self.wire_recipient_ticker).desired_width(55.0),
            );
            ui.label("Subject:");
            ui.add(egui::TextEdit::singleline(&mut self.wire_subject).desired_width(150.0));
            egui::ComboBox::from_id_salt("wire_att_combo")
                .selected_text(match self.wire_attachment_mode {
                    1 => "B2B Supply Contract",
                    2 => "Extortion Protection Demand",
                    3 => "Security Shield Retainer",
                    _ => "Plain Wire (No Attachment)",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.wire_attachment_mode, 0, "Plain Wire");
                    ui.selectable_value(&mut self.wire_attachment_mode, 1, "B2B Supply Contract");
                    ui.selectable_value(
                        &mut self.wire_attachment_mode,
                        2,
                        "Extortion Protection Demand",
                    );
                    ui.selectable_value(
                        &mut self.wire_attachment_mode,
                        3,
                        "Security Shield Retainer",
                    );
                });
            if ui
                .button(RichText::new("SEND WIRE").color(CYAN_ACCENT).strong())
                .clicked()
            {
                if let Some(recip) = Self::find_corporation(view, &self.wire_recipient_ticker) {
                    let attachment = match self.wire_attachment_mode {
                        1 => view.sku_codes.keys().next().copied().map(|sku_id| {
                            WireAttachment::B2BContractOffer {
                                sku_id,
                                daily_qty: self.wire_offer_qty.max(1),
                                unit_price: Money::from_dollars(
                                    self.wire_offer_price_dollars.max(1),
                                ),
                                duration_days: 30,
                            }
                        }),
                        2 => Some(WireAttachment::SyndicateExtortionDemand {
                            demand_amount: Money::from_dollars(
                                self.wire_offer_price_dollars.max(1) * 100,
                            ),
                            immunity_days: 30,
                        }),
                        3 => Some(WireAttachment::ServiceRetainerOffer {
                            is_security: true,
                            daily_fee: Money::from_dollars(self.wire_offer_price_dollars.max(1) * 10),
                            duration_days: 30,
                        }),
                        _ => None,
                    };
                    let _ = cmd_tx.send(PlayerCommand::SendWireMessage {
                        sender_corp_id: active_corp.corp_id,
                        recipient_corp_id: recip.corp_id,
                        subject: self.wire_subject.clone(),
                        body: self.wire_body.clone(),
                        attachment,
                    });
                }
            }
        });

        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if view.wire_messages.is_empty() {
                ui.colored_label(
                    TEXT_MUTED,
                    "No encrypted corporate wires in current authorized view.",
                );
            }
            for msg in view.wire_messages.iter().rev() {
                let sender_t = view
                    .corporations
                    .iter()
                    .find(|c| c.corp_id == msg.sender_corp_id)
                    .map_or("???", |c| c.ticker.as_str());
                let recip_t = view
                    .corporations
                    .iter()
                    .find(|c| c.corp_id == msg.recipient_corp_id)
                    .map_or("???", |c| c.ticker.as_str());

                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            AMBER_PRIMARY,
                            format!("T{} | {} -> {}", msg.tick, sender_t, recip_t),
                        );
                        ui.strong(&msg.subject);
                        ui.colored_label(CYAN_ACCENT, format!("[{:?}]", msg.status));
                    });
                    ui.label(&msg.body);
                    if let Some(att) = &msg.attachment {
                        ui.colored_label(EMERALD_ACCENT, format!("SMART ATTACHMENT: {att:?}"));
                        if msg.status == WireAttachmentStatus::Pending
                            && msg.recipient_corp_id == active_corp.corp_id
                        {
                            ui.horizontal(|ui| {
                                if ui
                                    .button(
                                        RichText::new("✓ ACCEPT & SETTLE")
                                            .color(BID_GREEN)
                                            .strong(),
                                    )
                                    .clicked()
                                {
                                    let _ = cmd_tx.send(PlayerCommand::RespondWireAttachment {
                                        responder_corp_id: active_corp.corp_id,
                                        message_id: msg.message_id,
                                        accept: true,
                                    });
                                }
                                if ui
                                    .button(RichText::new("✗ REJECT").color(ASK_RED))
                                    .clicked()
                                {
                                    let _ = cmd_tx.send(PlayerCommand::RespondWireAttachment {
                                        responder_corp_id: active_corp.corp_id,
                                        message_id: msg.message_id,
                                        accept: false,
                                    });
                                }
                            });
                        }
                    }
                });
            }
        });
    }
}
