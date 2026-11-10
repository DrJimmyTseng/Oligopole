/// Bloomberg Terminal function mnemonics supported in the top command bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMnemonic {
    /// `MM`: Market Monitor quote grid.
    MarketMonitor,
    /// `GP`: Graph Price / OHLCV chart.
    GraphPrice,
    /// `OB`: Level-2 Central Limit Order Book depth & trade tape.
    OrderBook,
    /// `FA`: Financial Analysis (Balance Sheet & Income Statement).
    FinancialAnalysis,
    /// `OPS`: Operations & Supply-Chain WAC DAG inspector.
    OperationsDag,
    /// `EX`: Executive Order & AI COO/CFO Delegation ticket.
    ExecutiveTicket,
}

impl TerminalMnemonic {
    pub const ALL: [Self; 6] = [
        Self::MarketMonitor,
        Self::GraphPrice,
        Self::OrderBook,
        Self::FinancialAnalysis,
        Self::OperationsDag,
        Self::ExecutiveTicket,
    ];

    #[inline]
    pub const fn short_code(self) -> &'static str {
        match self {
            Self::MarketMonitor => "MM",
            Self::GraphPrice => "GP",
            Self::OrderBook => "OB",
            Self::FinancialAnalysis => "FA",
            Self::OperationsDag => "OPS",
            Self::ExecutiveTicket => "EX",
        }
    }
}

/// Parsed Bloomberg command from the top command bar (`<TICKER> [MARKET] <MNEMONIC>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTerminalCommand {
    /// Canonical instrument or corporation ticker (e.g. `"IRON CMDTY"`, `"IRON EQ"`, `"ACME EQ"`, `"PLYR"`).
    pub ticker: String,
    pub mnemonic: TerminalMnemonic,
}

/// Parses a Bloomberg command string such as `"IRON CMDTY GP"`, `"ACME EQ FA"`, `"PLYR OB"`, or `"APEX OPS"`.
pub fn parse_bloomberg_command(input: &str) -> Option<ParsedTerminalCommand> {
    let tokens: Vec<String> = input
        .split_whitespace()
        .map(str::to_uppercase)
        .collect();
    if tokens.is_empty() {
        return None;
    }

    let first = tokens.first()?.clone();
    let last = tokens.last()?.as_str();

    let mnemonic = match last {
        "MM" | "MON" => TerminalMnemonic::MarketMonitor,
        "GP" | "CHART" => TerminalMnemonic::GraphPrice,
        "OB" | "BOOK" | "DEPTH" => TerminalMnemonic::OrderBook,
        "FA" | "FIN" | "BS" => TerminalMnemonic::FinancialAnalysis,
        "OPS" | "DAG" | "SCM" => TerminalMnemonic::OperationsDag,
        "EX" | "TRD" | "ORD" => TerminalMnemonic::ExecutiveTicket,
        _ if tokens.len() == 1 => TerminalMnemonic::FinancialAnalysis,
        _ => return None,
    };

    // Preserve explicit market sector qualification (`CMDTY` vs `EQ`) so `IRON CMDTY OB` and `IRON EQ OB`
    // route to the exact commodity or corporate equity order book!
    let ticker = match tokens.get(1).map(String::as_str) {
        Some("CMDTY" | "COMMODITY") => format!("{first} CMDTY"),
        Some("EQ" | "EQUITY")
            if matches!(
                mnemonic,
                TerminalMnemonic::GraphPrice | TerminalMnemonic::OrderBook
            ) =>
        {
            format!("{first} EQ")
        }
        _ if first == "IRON"
            && matches!(
                mnemonic,
                TerminalMnemonic::GraphPrice | TerminalMnemonic::OrderBook
            )
            && tokens.len() == 2 =>
        {
            // When user types `IRON OB` without sector qualifier, default to `IRON EQ` if they specify `EQ`
            // or `IRON EQ` for equity / `IRON CMDTY` when qualified
            "IRON EQ".to_owned()
        }
        _ => first,
    };

    Some(ParsedTerminalCommand { ticker, mnemonic })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bloomberg_command_variants() {
        assert_eq!(
            parse_bloomberg_command("iron cmdty gp"),
            Some(ParsedTerminalCommand {
                ticker: "IRON CMDTY".to_owned(),
                mnemonic: TerminalMnemonic::GraphPrice,
            })
        );
        assert_eq!(
            parse_bloomberg_command("iron eq ob"),
            Some(ParsedTerminalCommand {
                ticker: "IRON EQ".to_owned(),
                mnemonic: TerminalMnemonic::OrderBook,
            })
        );
        assert_eq!(
            parse_bloomberg_command("ACME EQ FA"),
            Some(ParsedTerminalCommand {
                ticker: "ACME".to_owned(),
                mnemonic: TerminalMnemonic::FinancialAnalysis,
            })
        );
        assert_eq!(
            parse_bloomberg_command("PLYR OB"),
            Some(ParsedTerminalCommand {
                ticker: "PLYR".to_owned(),
                mnemonic: TerminalMnemonic::OrderBook,
            })
        );
        assert_eq!(
            parse_bloomberg_command("ZENI EQ OPS"),
            Some(ParsedTerminalCommand {
                ticker: "ZENI".to_owned(),
                mnemonic: TerminalMnemonic::OperationsDag,
            })
        );
    }
}
