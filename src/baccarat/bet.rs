//! 标准百家乐主注的净赔付规则。

use super::RoundOutcome;

/// 可下注的三种主注。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MainBet {
    /// 闲家获胜时下注赢。
    Player,
    /// 庄家获胜时下注赢。
    Banker,
    /// 双方最终点数相同时下注赢。
    Tie,
}

/// 三种主注的净赔付配置。
///
/// 所有赔付值都表示“每下注 1 单位的净盈利”，不包含退回的本金：
/// 例如 `0.95` 表示赢得 0.95 单位，`-1.0` 表示输掉本金，`0.0`
/// 表示和局时 Push。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MainBetRules {
    player_payout: f64,
    banker_payout: f64,
    tie_payout: f64,
}

impl MainBetRules {
    /// 使用自定义的闲、庄、和净赔付创建主注规则。
    ///
    /// 参数仍然表示每下注 1 单位的净盈利，不包含本金返还。例如 `8.0`
    /// 表示和注赢得 8 个单位本金之外的净盈利。
    pub const fn with_payouts(player_payout: f64, banker_payout: f64, tie_payout: f64) -> Self {
        Self {
            player_payout,
            banker_payout,
            tie_payout,
        }
    }

    /// 创建项目当前采用的标准百家乐主注赔付：闲 1:1、庄 0.95:1、和 8:1。
    pub const fn standard() -> Self {
        Self::with_payouts(1.0, 0.95, 8.0)
    }

    /// 返回闲注获胜时的净赔付。
    pub const fn player_payout(self) -> f64 {
        self.player_payout
    }

    /// 返回庄注获胜时的净赔付。
    pub const fn banker_payout(self) -> f64 {
        self.banker_payout
    }

    /// 返回和注获胜时的净赔付。
    pub const fn tie_payout(self) -> f64 {
        self.tie_payout
    }

    /// 结算一笔主注，返回相对于本金的净盈利。
    pub const fn settle(self, bet: MainBet, outcome: RoundOutcome) -> f64 {
        match (bet, outcome) {
            (MainBet::Player, RoundOutcome::Player) => self.player_payout,
            (MainBet::Player, RoundOutcome::Banker) => -1.0,
            (MainBet::Player, RoundOutcome::Tie) => 0.0,
            (MainBet::Banker, RoundOutcome::Player) => -1.0,
            (MainBet::Banker, RoundOutcome::Banker) => self.banker_payout,
            (MainBet::Banker, RoundOutcome::Tie) => 0.0,
            (MainBet::Tie, RoundOutcome::Player | RoundOutcome::Banker) => -1.0,
            (MainBet::Tie, RoundOutcome::Tie) => self.tie_payout,
        }
    }
}

impl Default for MainBetRules {
    fn default() -> Self {
        Self::standard()
    }
}

#[cfg(test)]
mod tests {
    use super::{MainBet, MainBetRules};
    use crate::RoundOutcome;

    #[test]
    fn standard_rules_use_net_payouts() {
        let rules = MainBetRules::standard();

        assert_eq!(rules.player_payout(), 1.0);
        assert_eq!(rules.banker_payout(), 0.95);
        assert_eq!(rules.tie_payout(), 8.0);
    }

    #[test]
    fn custom_rules_can_change_each_net_payout() {
        let rules = MainBetRules::with_payouts(1.0, 1.0, 9.0);

        assert_eq!(rules.player_payout(), 1.0);
        assert_eq!(rules.banker_payout(), 1.0);
        assert_eq!(rules.tie_payout(), 9.0);
    }

    #[test]
    fn settles_all_main_bet_outcome_combinations() {
        let rules = MainBetRules::standard();
        let cases = [
            (MainBet::Player, RoundOutcome::Player, 1.0),
            (MainBet::Player, RoundOutcome::Banker, -1.0),
            (MainBet::Player, RoundOutcome::Tie, 0.0),
            (MainBet::Banker, RoundOutcome::Player, -1.0),
            (MainBet::Banker, RoundOutcome::Banker, 0.95),
            (MainBet::Banker, RoundOutcome::Tie, 0.0),
            (MainBet::Tie, RoundOutcome::Player, -1.0),
            (MainBet::Tie, RoundOutcome::Banker, -1.0),
            (MainBet::Tie, RoundOutcome::Tie, 8.0),
        ];

        for (bet, outcome, expected) in cases {
            assert_eq!(rules.settle(bet, outcome), expected);
        }
    }
}
