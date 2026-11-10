use crate::error::CoreError;
use crate::ids::{CorpId, InstrumentId, OrderId, SkuId};
use oligopole_ledger::{EntityId, Money};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Market sector classification for Bloomberg terminal tickers (`IRON CMDTY`, `ACME EQ`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum InstrumentKind {
    Commodity { sku_id: SkuId },
    Equity { corp_id: CorpId },
}

/// Direction of an order on the Central Limit Order Book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum OrderSide {
    Bid,
    Ask,
}

/// A resting or incoming limit order in the Central Limit Order Book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitOrder {
    pub order_id: OrderId,
    pub instrument_id: InstrumentId,
    pub trader_entity_id: EntityId,
    pub side: OrderSide,
    pub limit_price: Money,
    pub initial_qty: u64,
    pub remaining_qty: u64,
    pub submitted_tick: u64,
}

/// Execution fill emitted when a Bid and Ask match in price-time priority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeFill {
    pub tick: u64,
    pub instrument_id: InstrumentId,
    pub buyer_entity_id: EntityId,
    pub seller_entity_id: EntityId,
    pub bid_order_id: OrderId,
    pub ask_order_id: OrderId,
    pub execution_price: Money,
    /// The buyer's original `limit_price` so exact `limit_price * qty` cash escrow is released
    /// even when the buyer receives price improvement (`execution_price < buyer_limit_price`).
    pub buyer_limit_price: Money,
    pub qty: u64,
}

/// Standard Open-High-Low-Close-Volume bar for `egui_plot` candlesticks and SQLite rollups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OhlcvBar {
    pub bar_tick: u64,
    pub open: Money,
    pub high: Money,
    pub low: Money,
    pub close: Money,
    pub volume: u64,
}

/// Aggregated Level-2 order book depth row for `egui_extras::TableBuilder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderBookDepthLevel {
    pub price: Money,
    pub total_qty: u64,
    pub order_count: u32,
}

/// Deterministic Price-Time Priority Central Limit Order Book (CLOB) for a single instrument.
///
/// Uses `BTreeMap<Money, VecDeque<LimitOrder>>` for canonical iteration and serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentralLimitOrderBook {
    pub instrument_id: InstrumentId,
    pub ticker: String,
    pub kind: InstrumentKind,
    pub last_price: Money,
    pub bids: BTreeMap<Money, VecDeque<LimitOrder>>,
    pub asks: BTreeMap<Money, VecDeque<LimitOrder>>,
    pub current_day_bar: Option<OhlcvBar>,
    pub history_bars: VecDeque<OhlcvBar>,
    pub recent_fills: VecDeque<TradeFill>,
    max_history_bars: usize,
}

impl CentralLimitOrderBook {
    pub fn new(
        instrument_id: InstrumentId,
        ticker: impl Into<String>,
        kind: InstrumentKind,
        reference_price: Money,
    ) -> Self {
        Self {
            instrument_id,
            ticker: ticker.into(),
            kind,
            last_price: reference_price,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            current_day_bar: None,
            history_bars: VecDeque::with_capacity(360),
            recent_fills: VecDeque::with_capacity(64),
            max_history_bars: 720,
        }
    }

    pub fn best_bid(&self) -> Option<Money> {
        self.bids
            .iter()
            .rev()
            .find(|(_, q)| !q.is_empty())
            .map(|(&p, _)| p)
    }

    pub fn best_ask(&self) -> Option<Money> {
        self.asks
            .iter()
            .find(|(_, q)| !q.is_empty())
            .map(|(&p, _)| p)
    }

    pub fn mid_or_last_price(&self) -> Money {
        match (self.best_bid(), self.best_ask()) {
            (Some(b), Some(a)) => Money::from_cents(i128::midpoint(b.cents(), a.cents())),
            _ => self.last_price,
        }
    }

    /// Returns top `max_levels` Bid depth levels sorted highest price first.
    pub fn top_bid_levels(&self, max_levels: usize) -> Vec<OrderBookDepthLevel> {
        self.bids
            .iter()
            .rev()
            .filter(|(_, q)| !q.is_empty())
            .take(max_levels)
            .map(|(&price, queue)| OrderBookDepthLevel {
                price,
                total_qty: queue.iter().map(|o| o.remaining_qty).sum(),
                order_count: u32::try_from(queue.len()).unwrap_or(u32::MAX),
            })
            .collect()
    }

    /// Returns top `max_levels` Ask depth levels sorted lowest price first.
    pub fn top_ask_levels(&self, max_levels: usize) -> Vec<OrderBookDepthLevel> {
        self.asks
            .iter()
            .filter(|(_, q)| !q.is_empty())
            .take(max_levels)
            .map(|(&price, queue)| OrderBookDepthLevel {
                price,
                total_qty: queue.iter().map(|o| o.remaining_qty).sum(),
                order_count: u32::try_from(queue.len()).unwrap_or(u32::MAX),
            })
            .collect()
    }

    /// Inserts a new `LimitOrder` (whose collateral has already been reserved by the caller)
    /// and matches it immediately against resting opposite-side orders in price-time priority.
    ///
    /// Self-trade prevention: if the resting maker order belongs to the same `trader_entity_id`,
    /// matching stops or skips to prevent wash trades.
    pub fn submit_and_match(
        &mut self,
        mut incoming: LimitOrder,
    ) -> Result<Vec<TradeFill>, CoreError> {
        if incoming.remaining_qty == 0 || !incoming.limit_price.is_positive() {
            return Err(CoreError::InvalidOrder {
                reason: "order quantity and limit_price must be strictly positive".to_owned(),
            });
        }

        let mut fills = Vec::new();

        match incoming.side {
            OrderSide::Bid => {
                // Match against lowest-priced Asks where ask_price <= incoming.limit_price
                let matching_prices: Vec<Money> = self
                    .asks
                    .range(..=incoming.limit_price)
                    .map(|(&p, _)| p)
                    .collect();

                for ask_price in matching_prices {
                    if incoming.remaining_qty == 0 {
                        break;
                    }
                    let Some(queue) = self.asks.get_mut(&ask_price) else {
                        continue;
                    };

                    let mut idx = 0usize;
                    while idx < queue.len() && incoming.remaining_qty > 0 {
                        let Some(resting_ask) = queue.get_mut(idx) else {
                            break;
                        };
                        if resting_ask.trader_entity_id == incoming.trader_entity_id {
                            idx += 1;
                            continue;
                        }

                        let match_qty = incoming.remaining_qty.min(resting_ask.remaining_qty);
                        incoming.remaining_qty -= match_qty;
                        resting_ask.remaining_qty -= match_qty;

                        let fill = TradeFill {
                            tick: incoming.submitted_tick,
                            instrument_id: self.instrument_id,
                            buyer_entity_id: incoming.trader_entity_id,
                            seller_entity_id: resting_ask.trader_entity_id,
                            bid_order_id: incoming.order_id,
                            ask_order_id: resting_ask.order_id,
                            execution_price: ask_price,
                            buyer_limit_price: incoming.limit_price,
                            qty: match_qty,
                        };
                        fills.push(fill);

                        if resting_ask.remaining_qty == 0 {
                            queue.remove(idx);
                        } else {
                            idx += 1;
                        }
                    }

                    if queue.is_empty() {
                        self.asks.remove(&ask_price);
                    }
                }

                if incoming.remaining_qty > 0 {
                    self.bids
                        .entry(incoming.limit_price)
                        .or_default()
                        .push_back(incoming);
                }
            }
            OrderSide::Ask => {
                // Match against highest-priced Bids where bid_price >= incoming.limit_price
                let matching_prices: Vec<Money> = self
                    .bids
                    .range(incoming.limit_price..)
                    .rev()
                    .map(|(&p, _)| p)
                    .collect();

                for bid_price in matching_prices {
                    if incoming.remaining_qty == 0 {
                        break;
                    }
                    let Some(queue) = self.bids.get_mut(&bid_price) else {
                        continue;
                    };

                    let mut idx = 0usize;
                    while idx < queue.len() && incoming.remaining_qty > 0 {
                        let Some(resting_bid) = queue.get_mut(idx) else {
                            break;
                        };
                        if resting_bid.trader_entity_id == incoming.trader_entity_id {
                            idx += 1;
                            continue;
                        }

                        let match_qty = incoming.remaining_qty.min(resting_bid.remaining_qty);
                        incoming.remaining_qty -= match_qty;
                        resting_bid.remaining_qty -= match_qty;

                        let fill = TradeFill {
                            tick: incoming.submitted_tick,
                            instrument_id: self.instrument_id,
                            buyer_entity_id: resting_bid.trader_entity_id,
                            seller_entity_id: incoming.trader_entity_id,
                            bid_order_id: resting_bid.order_id,
                            ask_order_id: incoming.order_id,
                            execution_price: bid_price,
                            buyer_limit_price: resting_bid.limit_price,
                            qty: match_qty,
                        };
                        fills.push(fill);

                        if resting_bid.remaining_qty == 0 {
                            queue.remove(idx);
                        } else {
                            idx += 1;
                        }
                    }

                    if queue.is_empty() {
                        self.bids.remove(&bid_price);
                    }
                }

                if incoming.remaining_qty > 0 {
                    self.asks
                        .entry(incoming.limit_price)
                        .or_default()
                        .push_back(incoming);
                }
            }
        }

        for fill in &fills {
            self.record_fill_on_ohlcv(fill);
        }

        Ok(fills)
    }

    /// Removes and returns a resting order by `order_id` so the caller can release its reservation escrow.
    pub fn cancel_order(&mut self, order_id: OrderId) -> Result<LimitOrder, CoreError> {
        let mut empty_bid_price = None;
        for (&price, queue) in &mut self.bids {
            if let Some(pos) = queue.iter().position(|o| o.order_id == order_id) {
                let removed = queue
                    .remove(pos)
                    .ok_or(CoreError::UnknownOrder(order_id))?;
                if queue.is_empty() {
                    empty_bid_price = Some(price);
                }
                if let Some(p) = empty_bid_price {
                    self.bids.remove(&p);
                }
                return Ok(removed);
            }
        }

        let mut empty_ask_price = None;
        for (&price, queue) in &mut self.asks {
            if let Some(pos) = queue.iter().position(|o| o.order_id == order_id) {
                let removed = queue
                    .remove(pos)
                    .ok_or(CoreError::UnknownOrder(order_id))?;
                if queue.is_empty() {
                    empty_ask_price = Some(price);
                }
                if let Some(p) = empty_ask_price {
                    self.asks.remove(&p);
                }
                return Ok(removed);
            }
        }

        Err(CoreError::UnknownOrder(order_id))
    }

    /// Cancels all resting orders submitted by `trader_entity_id` (used before posting refreshed market-making quotes).
    pub fn cancel_all_for_entity(&mut self, trader_entity_id: EntityId) -> Vec<LimitOrder> {
        let mut cancelled = Vec::new();

        self.bids.retain(|_, queue| {
            let mut i = 0usize;
            while i < queue.len() {
                if queue
                    .get(i)
                    .is_some_and(|o| o.trader_entity_id == trader_entity_id)
                {
                    if let Some(removed) = queue.remove(i) {
                        cancelled.push(removed);
                    }
                } else {
                    i += 1;
                }
            }
            !queue.is_empty()
        });

        self.asks.retain(|_, queue| {
            let mut i = 0usize;
            while i < queue.len() {
                if queue
                    .get(i)
                    .is_some_and(|o| o.trader_entity_id == trader_entity_id)
                {
                    if let Some(removed) = queue.remove(i) {
                        cancelled.push(removed);
                    }
                } else {
                    i += 1;
                }
            }
            !queue.is_empty()
        });

        cancelled
    }

    fn record_fill_on_ohlcv(&mut self, fill: &TradeFill) {
        self.last_price = fill.execution_price;
        match &mut self.current_day_bar {
            Some(bar) if bar.bar_tick == fill.tick => {
                if fill.execution_price > bar.high {
                    bar.high = fill.execution_price;
                }
                if fill.execution_price < bar.low {
                    bar.low = fill.execution_price;
                }
                bar.close = fill.execution_price;
                bar.volume = bar.volume.saturating_add(fill.qty);
            }
            _ => {
                if let Some(prev) = self.current_day_bar.take() {
                    self.push_history_bar(prev);
                }
                self.current_day_bar = Some(OhlcvBar {
                    bar_tick: fill.tick,
                    open: fill.execution_price,
                    high: fill.execution_price,
                    low: fill.execution_price,
                    close: fill.execution_price,
                    volume: fill.qty,
                });
            }
        }

        if self.recent_fills.len() >= 64 {
            self.recent_fills.pop_front();
        }
        self.recent_fills.push_back(fill.clone());
    }

    /// Finalizes the daily OHLCV bar at the end of Sub-Phase 3 (carrying forward `last_price` with 0 volume if no trades occurred).
    pub fn finalize_daily_bar(&mut self, tick: u64) -> OhlcvBar {
        let bar = match self.current_day_bar.take() {
            Some(b) if b.bar_tick == tick => b,
            Some(older) => {
                self.push_history_bar(older);
                OhlcvBar {
                    bar_tick: tick,
                    open: self.last_price,
                    high: self.last_price,
                    low: self.last_price,
                    close: self.last_price,
                    volume: 0,
                }
            }
            None => OhlcvBar {
                bar_tick: tick,
                open: self.last_price,
                high: self.last_price,
                low: self.last_price,
                close: self.last_price,
                volume: 0,
            },
        };
        self.push_history_bar(bar);
        bar
    }

    fn push_history_bar(&mut self, bar: OhlcvBar) {
        if self.history_bars.len() >= self.max_history_bars {
            self.history_bars.pop_front();
        }
        self.history_bars.push_back(bar);
    }
}
