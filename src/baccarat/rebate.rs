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
    AllExceptMainBetTie { rate: f64 },
}

impl RebateRule {
    /// 根据下注类型和假设的牌局结果返回返水比例。
    ///
    /// 这里的 outcome 不代表下注前已经知道真实结果。
    /// EV 计算会分别传入 Player、Banker、Tie 三种可能结果，
    /// 再把它们按照各自概率加权。
    pub const fn rate_for(self, bet: MainBet, outcome: RoundOutcome) -> f64 {
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
}
