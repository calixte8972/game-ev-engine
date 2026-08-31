//! 返水规则及“下注方向 + 牌局结果”到返水率的映射。
//!
//! 返水是按下注额额外返还的收益。`rate = 0.015` 表示每下注 1 单位，在符合
//! 条件的结果下额外获得 0.015 单位。下注前不知道真实结果，因此 EV 层会把
//! 三种可能结果分别调用 [`RebateRule::rate_for`]，再按各自概率加权。

use crate::{MainBet, RoundOutcome};

/// 返水规则。
///
/// rate 使用小数表示，例如 0.015 表示 1.5%。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RebateRule {
    /// 不提供返水。
    None,
    /// Player、Banker 遇到和局不返水，其他情况都返水。
    ///
    /// 按当前约定，Tie 注的三种结果都属于“其他情况”，因此 Tie 注
    /// 无论最终结果是什么都会获得返水。
    AllExceptMainBetTie {
        /// 用小数表示的返水率；例如 1.5% 写成 0.015。
        rate: f64,
    },
}

impl RebateRule {
    /// 根据下注类型和假设的牌局结果返回返水比例。
    ///
    /// 这里的 outcome 不代表下注前已经知道真实结果。
    /// EV 计算会分别传入 Player、Banker、Tie 三种可能结果，
    /// 再把它们按照各自概率加权。
    pub const fn rate_for(self, bet: MainBet, outcome: RoundOutcome) -> f64 {
        // 外层 match 先判断有没有返水；内层 match 再处理有返水规则下的例外组合。
        match self {
            Self::None => 0.0,
            Self::AllExceptMainBetTie { rate } => match (bet, outcome) {
                // Player/Banker 注遇到和局只 Push，不产生返水。
                (MainBet::Player | MainBet::Banker, RoundOutcome::Tie) => 0.0,
                // 其他组合都产生 rate 比例的返水。
                _ => rate,
            },
        }
    }

    /// 返回任意边注每下注 1 单位可以获得的返水比例。
    ///
    /// 当前业务约定只有“庄/闲主注遇到和局”不返水。大小、对子、幸运 6/7、
    /// 龙宝等边注不属于这个例外，因此无论边注最后赢、输还是 Push，都按
    /// 实际下注额获得同一个返水比例。把这条规则集中在这里，可以保证策略
    /// EV、凯利金额和真实回放结算使用完全相同的口径。
    pub const fn rate_for_side_bet(self) -> f64 {
        match self {
            Self::None => 0.0,
            Self::AllExceptMainBetTie { rate } => rate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RebateRule;
    use crate::{MainBet, RoundOutcome};

    #[test]
    fn no_rebate_rule_always_returns_zero() {
        let rule = RebateRule::None;

        for bet in [MainBet::Player, MainBet::Banker, MainBet::Tie] {
            for outcome in [
                RoundOutcome::Player,
                RoundOutcome::Banker,
                RoundOutcome::Tie,
            ] {
                assert_eq!(rule.rate_for(bet, outcome), 0.0);
            }
        }
    }

    #[test]
    fn main_bets_do_not_receive_rebate_on_tie() {
        let rule = RebateRule::AllExceptMainBetTie { rate: 0.015 };

        assert_eq!(rule.rate_for(MainBet::Player, RoundOutcome::Tie), 0.0);
        assert_eq!(rule.rate_for(MainBet::Banker, RoundOutcome::Tie), 0.0);
    }

    #[test]
    fn tie_bet_receives_rebate_for_all_outcomes() {
        let rule = RebateRule::AllExceptMainBetTie { rate: 0.015 };

        for outcome in [
            RoundOutcome::Player,
            RoundOutcome::Banker,
            RoundOutcome::Tie,
        ] {
            assert_eq!(rule.rate_for(MainBet::Tie, outcome), 0.015);
        }
    }

    #[test]
    fn every_side_bet_outcome_receives_rebate() {
        let rule = RebateRule::AllExceptMainBetTie { rate: 0.015 };

        assert_eq!(rule.rate_for_side_bet(), 0.015);
        assert_eq!(RebateRule::None.rate_for_side_bet(), 0.0);
    }
}
