//! 标准百家乐主注的期望收益计算。

use super::{MainBet, MainBetRules, OutcomeWeights, RoundOutcome};

/// 三种主注每下注 1 单位时的 EV 结果。
///
/// EV 使用净盈利口径：赢一笔标准闲注得到 `1.0`，输掉一笔下注为
/// `-1.0`，和局 Push 为 `0.0`。因此 EV 不包含本金返还。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MainBetEv {
    /// 闲注每下注 1 单位的净 EV。
    player: f64,
    /// 庄注每下注 1 单位的净 EV。
    banker: f64,
    /// 和注每下注 1 单位的净 EV。
    tie: f64,
}

impl MainBetEv {
    /// 根据结果权重和赔付规则，计算三种主注的 EV。
    ///
    /// 通用公式为：`EV = Σ P(outcome) × net_profit(outcome)`。
    /// Player 和 Tie 只依赖最终输赢；免佣庄还依赖“庄是否以 6 点获胜”，
    /// 所以庄注会单独拆成普通庄赢与庄 6 赢两部分。
    pub fn calculate(weights: OutcomeWeights, rules: MainBetRules) -> Self {
        // OutcomeWeights 内部保留精确整数权重，只在 EV 展示层转换一次 f64。
        let player_probability = weights.player_probability();
        let banker_probability = weights.banker_probability();
        let tie_probability = weights.tie_probability();

        // 庄六点获胜是庄赢的子集。先用整数权重相减，再转换成概率，
        // 可以避免两个浮点概率相减造成额外舍入误差。
        let banker_non_six_probability = (weights.banker_weight()
            - weights.banker_win_on_six_weight()) as f64
            / weights.total_weight() as f64;
        let banker_six_probability = weights.banker_win_on_six_probability();

        // Player 和 Tie 的赔付只看最终 RoundOutcome，因此可复用同一个期望值公式。
        // `rules.settle` 返回的是净盈利：赢返回净赔付，输返回 -1，Push 返回 0。
        let expected_value = |bet| {
            player_probability * rules.settle(bet, RoundOutcome::Player)
                + banker_probability * rules.settle(bet, RoundOutcome::Banker)
                + tie_probability * rules.settle(bet, RoundOutcome::Tie)
        };

        // 庄注单独计算四种互斥贡献：闲赢时输本金、普通庄赢、庄六赢、和局 Push。
        // 标准庄规则下两个庄赢赔付都为 0.95；免佣庄规则下分别为 1.0 和 0.5。
        let banker_loss_ev =
            player_probability * rules.settle(MainBet::Banker, RoundOutcome::Player);
        let banker_non_six_win_ev = banker_non_six_probability * rules.banker_payout_for_total(5);
        let banker_six_win_ev = banker_six_probability * rules.banker_payout_for_total(6);
        let banker_push_ev = tie_probability * rules.settle(MainBet::Banker, RoundOutcome::Tie);
        let banker_ev = banker_loss_ev + banker_non_six_win_ev + banker_six_win_ev + banker_push_ev;

        Self {
            player: expected_value(MainBet::Player),
            banker: banker_ev,
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

    /// 根据下注类型返回对应 EV，方便上层统一遍历三种主注。
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
    ///
    /// 例如 `EV = -0.02` 时，RTP 为 `0.98`，表示理论总返还约为本金的 98%。
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

    #[test]
    fn no_commission_ev_applies_half_payout_to_banker_six_only() {
        let weights = OutcomeWeights::from_detailed_weights(6, 360, 240, 120, 60)
            .expect("测试权重应构成完整分布");
        let result = MainBetEv::calculate(weights, MainBetRules::no_commission());

        assert_close(result.player_ev(), 1.0 / 6.0);
        assert_close(result.banker_ev(), -5.0 / 24.0);
        assert_close(result.tie_ev(), 0.5);
    }
}
