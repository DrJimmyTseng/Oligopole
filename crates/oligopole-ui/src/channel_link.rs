use crate::theme::{AMBER_PRIMARY, CYAN_ACCENT, EMERALD_ACCENT, TEXT_MUTED};
use egui::Color32;
use std::collections::BTreeMap;

/// Color-coded Bloomberg Launchpad channel linking group (`Channel A`, `Channel B`, `Channel C`, `Unlinked`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LinkChannel {
    ChannelA,
    ChannelB,
    ChannelC,
    Unlinked,
}

impl LinkChannel {
    pub const ALL: [Self; 4] = [
        Self::ChannelA,
        Self::ChannelB,
        Self::ChannelC,
        Self::Unlinked,
    ];

    pub const fn badge_label(self) -> &'static str {
        match self {
            Self::ChannelA => "CH-A",
            Self::ChannelB => "CH-B",
            Self::ChannelC => "CH-C",
            Self::Unlinked => "UNLK",
        }
    }

    pub const fn badge_color(self) -> Color32 {
        match self {
            Self::ChannelA => AMBER_PRIMARY,
            Self::ChannelB => CYAN_ACCENT,
            Self::ChannelC => EMERALD_ACCENT,
            Self::Unlinked => TEXT_MUTED,
        }
    }
}

/// Synchronizes active corporate/instrument tickers across linked panes in `egui_tiles`.
#[derive(Debug, Clone)]
pub struct ChannelLinkRegistry {
    active_tickers: BTreeMap<LinkChannel, String>,
}

impl Default for ChannelLinkRegistry {
    fn default() -> Self {
        let mut active_tickers = BTreeMap::new();
        active_tickers.insert(LinkChannel::ChannelA, "PLYR".to_owned());
        active_tickers.insert(LinkChannel::ChannelB, "IRON".to_owned());
        active_tickers.insert(LinkChannel::ChannelC, "ACME".to_owned());
        Self { active_tickers }
    }
}

impl ChannelLinkRegistry {
    pub fn ticker_for(&self, channel: LinkChannel, fallback: &str) -> String {
        self.active_tickers
            .get(&channel)
            .cloned()
            .unwrap_or_else(|| fallback.to_owned())
    }

    pub fn active_ticker_ref(&self, channel: LinkChannel) -> Option<&str> {
        self.active_tickers.get(&channel).map(String::as_str)
    }

    pub fn set_ticker(&mut self, channel: LinkChannel, ticker: impl Into<String>) {
        if channel != LinkChannel::Unlinked {
            self.active_tickers.insert(channel, ticker.into());
        }
    }
}
