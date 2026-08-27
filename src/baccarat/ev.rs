//! 标准百家乐主注的期望收益计算。

use super::{MainBet, MainBetRules, OutcomeWeights, RoundOutcome};

/// 三种主注每下注 1 单位时的 EV 结果。
///
/// EV 使用净盈利口径：赢一笔标准闲注得到 `1.0`，输掉一笔下注为
/// `-1.0`，和局 Push 为 `0.0`。因此 EV 不包含本金返还。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MainBetEv {
    player: f64,
    banker: f64,
    tie: f64,
}

impl MainBetEv {
    /// 根据结果权重和赔付规则，计算三种主注的 EV。
    pub fn calculate(weights: OutcomeWeights, rules: MainBetRules) -> Self {
        let player_probability = weights.player_probability();
        let banker_probability = weights.banker_probability();
        let tie_probability = weights.tie_probability();

        let expected_value = |bet| {
            player_probability * rules.settle(bet, RoundOutcome::Player)
                + banker_probability * rules.settle(bet, RoundOutcome::Banker)
                + tie_probability * rules.settle(bet, RoundOutcome::Tie)
        };

        Self {
            player: expected_value(MainBet::Player),
            banker: expected_value(MainBet::Banker),
            tie: expected_value(MainBet::Tie),
        }
    }

    /// 返回闲注 EV。
    pub const fn player_ev(self) -> f64 {
        self.player
    }

    /// 返回庄注 EV。
    pub const fn banker_ev(self) -> f64 {
        self.banker
    }

    /// 返回和注 EV。
    pub const fn tie_ev(self) -> f64 {
        self.tie
    }

    /// 根据下注类型返回对应 EV。
    pub const fn ev(self, bet: MainBet) -> f64 {
        match bet {
            MainBet::Player => self.player,
            MainBet::Banker => self.banker,
            MainBet::Tie => self.tie,
        }
    }

    /// 返回 House Edge。
    ///
    /// House Edge 是玩家 EV 的相反数，因此玩家长期平均亏损时该值为正。
    pub const fn house_edge(self, bet: MainBet) -> f64 {
        -self.ev(bet)
    }

    /// 返回包含本金返还的 RTP。
    pub const fn return_to_player(self, bet: MainBet) -> f64 {
        1.0 + self.ev(bet)
    }
}

#[cfg(test)]
mod tests {
    use super::MainBetEv;
    use crate::{MainBet, MainBetRules, OutcomeWeights};

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn calculates_ev_from_outcome_weights() {
        let weights =
            OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应构成完整分布");
        let result = MainBetEv::calculate(weights, MainBetRules::standard());

        assert_close(result.player_ev(), 1.0 / 6.0);
        assert_close(result.banker_ev(), -11.0 / 60.0);
        assert_close(result.tie_ev(), 0.5);
    }

    #[test]
    fn derives_house_edge_and_rtp_from_net_ev() {
        let weights =
            OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应构成完整分布");
        let result = MainBetEv::calculate(weights, MainBetRules::standard());

        assert_close(result.ev(MainBet::Player), 1.0 / 6.0);
        assert_close(result.house_edge(MainBet::Player), -1.0 / 6.0);
        assert_close(result.return_to_player(MainBet::Player), 7.0 / 6.0);

        assert_close(result.house_edge(MainBet::Banker), 11.0 / 60.0);
        assert_close(result.return_to_player(MainBet::Banker), 49.0 / 60.0);

        assert_close(result.house_edge(MainBet::Tie), -0.5);
        assert_close(result.return_to_player(MainBet::Tie), 1.5);
    }

    #[test]
    fn custom_payouts_change_ev_without_changing_probabilities() {
        let weights =
            OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应构成完整分布");
        let result = MainBetEv::calculate(weights, MainBetRules::with_payouts(1.0, 1.0, 9.0));

        assert_close(result.player_ev(), 1.0 / 6.0);
        assert_close(result.banker_ev(), -1.0 / 6.0);
        assert_close(result.tie_ev(), 2.0 / 3.0);
    }
}
