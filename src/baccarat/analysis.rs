//! 面向 CLI、Python 绑定等上层调用者的主注分析结果。
//!
//! 概率枚举器和 EV 层分别解决不同问题：
//!
//! ```text
//! Shoe
//!   └─ calculate_main_outcomes() → OutcomeWeights（庄、闲、和的概率权重）
//!        └─ MainBetEv::calculate() → MainBetEv（三种下注的净 EV）
//!             └─ MainBetAnalysis（组合成上层容易读取的一份结果）
//! ```
//!
//! 本模块是核心算法与 CLI/Python 之间的“结果适配层”。上层不必分别调用
//! 概率函数和 EV 函数，只需调用 [`analyze_main_bets`] 即可获得完整结果。

use crate::Shoe;

use super::{
    MainBet, MainBetEv, MainBetRules, OutcomeWeights, ProbabilityError, RebateRule, RoundOutcome,
    calculate_main_outcomes,
};

/// 单个主注的概率和收益指标。
///
/// 只保存概率与净 EV；House Edge 和 RTP 从 EV 推导，避免同一份结果中
/// 出现互相矛盾的重复字段。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BetMetrics {
    /// 对应结果发生的条件概率，取值通常在 `0.0..=1.0` 之间。
    probability: f64,
    /// 每下注 1 单位的净期望收益，不包含本金。
    ev: f64,
}

impl BetMetrics {
    /// 只允许本模块组装指标，避免上层手工把某个结果的概率与另一个下注的 EV 混在一起。
    const fn new(probability: f64, ev: f64) -> Self {
        Self { probability, ev }
    }

    /// 返回该下注所对应结果的发生概率。
    ///
    /// 例如庄注指标中的概率是 `P(Banker)`，不是“庄注盈利的概率加权值”。
    pub const fn probability(self) -> f64 {
        self.probability
    }

    /// 返回每下注 1 单位的净期望收益。
    ///
    /// `-0.0105` 表示长期平均每下注 1 单位净亏损约 `0.0105` 单位。
    pub const fn ev(self) -> f64 {
        self.ev
    }

    /// 返回赌场优势，即玩家 EV 的相反数。
    ///
    /// 玩家 EV 为负时，House Edge 为正。例如 `EV = -0.01` 时，
    /// `House Edge = 0.01`。
    pub const fn house_edge(self) -> f64 {
        -self.ev
    }

    /// 返回包含本金返还的理论返还率。
    ///
    /// EV 使用净盈利口径，所以需要加回 1 单位本金：`RTP = 1 + EV`。
    pub const fn rtp(self) -> f64 {
        1.0 + self.ev
    }
}
/// 加入返水后的单个下注指标。
///
/// base_ev 是赌场基础赔付带来的 EV；
/// rebate_ev 是根据所有可能结果计算出来的期望返水；
/// effective_ev 是两者之和。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveBetMetrics {
    probability: f64,
    base_ev: f64,
    rebate_ev: f64,
    effective_ev: f64,
}

impl EffectiveBetMetrics {
    /// 返回该下注对应结果的概率。
    pub const fn probability(self) -> f64 {
        self.probability
    }

    /// 返回不考虑返水时的基础 EV。
    pub const fn base_ev(self) -> f64 {
        self.base_ev
    }

    /// 返回返水带来的期望收益。
    pub const fn rebate_ev(self) -> f64 {
        self.rebate_ev
    }

    /// 返回加入返水后的有效 EV。
    pub const fn effective_ev(self) -> f64 {
        self.effective_ev
    }

    /// 根据有效 EV 返回 House Edge。
    pub const fn house_edge(self) -> f64 {
        -self.effective_ev
    }

    /// 根据有效 EV 返回 RTP。
    pub const fn rtp(self) -> f64 {
        1.0 + self.effective_ev
    }
}

/// 根据基础分析和返水规则计算某一个下注的有效 EV。
///
/// 下注前并不知道真实 outcome。这里的 outcome 只是三种假设：
///
/// ```text
/// 假设结果是 Player  → P(Player) × 该结果收益
/// 假设结果是 Banker  → P(Banker) × 该结果收益
/// 假设结果是 Tie     → P(Tie) × 该结果收益
/// ```
///
/// 三项相加后，得到下注前的长期期望收益。
pub fn effective_ev(
    analysis: MainBetAnalysis,
    bet: MainBet,
    rebate: RebateRule,
) -> EffectiveBetMetrics {
    let base_metrics = analysis.metrics(bet);
    let possible_outcomes = [
        (RoundOutcome::Player, analysis.player().probability()),
        (RoundOutcome::Banker, analysis.banker().probability()),
        (RoundOutcome::Tie, analysis.tie().probability()),
    ];

    // 返水也必须按照结果概率加权。
    // 例如 Player 注遇到 Tie 没有返水，所以 Tie 这一项为零。
    let rebate_ev = possible_outcomes
        .iter()
        .map(|(outcome, probability)| *probability * rebate.rate_for(bet, *outcome))
        .sum::<f64>();

    let base_ev = base_metrics.ev();

    EffectiveBetMetrics {
        probability: base_metrics.probability(),
        base_ev,
        rebate_ev,
        effective_ev: base_ev + rebate_ev,
    }
}
/// Player、Banker、Tie 三种主注的一次完整分析。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MainBetAnalysis {
    /// 闲注的概率与收益指标。
    player: BetMetrics,
    /// 庄注的概率与收益指标。
    banker: BetMetrics,
    /// 和注的概率与收益指标。
    tie: BetMetrics,
    /// 三种下注中 EV 最大的一项；最大 EV 也可能仍然小于零。
    optimal_bet: MainBet,
}

impl MainBetAnalysis {
    /// 根据已经计算好的结果权重和赔付规则创建分析结果。
    ///
    /// 这个入口适合已经缓存 `OutcomeWeights` 的调用者：同一牌靴概率不变时，
    /// 可以只更换赔付规则并重新计算 EV，无需再次枚举牌局。
    pub fn from_weights(weights: OutcomeWeights, rules: MainBetRules) -> Self {
        // 概率只由牌靴和发牌规则决定；EV 还需要叠加当前赌场的赔付规则。
        let ev = MainBetEv::calculate(weights, rules);

        // 把同一个下注的概率与 EV 配对，形成上层直接可用的指标对象。
        let player = BetMetrics::new(weights.player_probability(), ev.player_ev());
        let banker = BetMetrics::new(weights.banker_probability(), ev.banker_ev());
        let tie = BetMetrics::new(weights.tie_probability(), ev.tie_ev());

        // 先假定 Player 最优，再依次拿 Banker 和 Tie 的 EV 与当前最大值比较。
        // 这里比较的是 EV，不是结果概率：概率最高的结果未必是赔付后 EV 最高的下注。
        // 只有严格更大时才替换，因此完全相等时采用固定优先顺序
        // Player → Banker → Tie，保证同一输入始终得到同一结果。
        let mut optimal_bet = MainBet::Player;
        let mut optimal_ev = player.ev();

        if banker.ev() > optimal_ev {
            optimal_bet = MainBet::Banker;
            // 后面 Tie 应与当前最大的 Banker EV 比较，所以这里同步更新最大值。
            optimal_ev = banker.ev();
        }
        if tie.ev() > optimal_ev {
            optimal_bet = MainBet::Tie;
        }

        Self {
            player,
            banker,
            tie,
            optimal_bet,
        }
    }

    /// 返回闲注指标。`BetMetrics` 实现了 `Copy`，这里返回值不会转移整个分析对象。
    pub const fn player(self) -> BetMetrics {
        self.player
    }

    /// 返回庄注指标。
    pub const fn banker(self) -> BetMetrics {
        self.banker
    }

    /// 返回和注指标。
    pub const fn tie(self) -> BetMetrics {
        self.tie
    }

    /// 根据下注类型返回对应指标。
    ///
    /// 这个统一入口适合循环和 Python 绑定，调用者无需自己写三次 `match`。
    pub const fn metrics(self, bet: MainBet) -> BetMetrics {
        match bet {
            MainBet::Player => self.player,
            MainBet::Banker => self.banker,
            MainBet::Tie => self.tie,
        }
    }

    /// 返回 EV 最大的主注；最大值仍可能是负数。
    ///
    /// “最优”仅表示三者中数学期望最大，不表示该下注能够保证盈利。
    pub const fn optimal_bet(self) -> MainBet {
        self.optimal_bet
    }

    /// 返回最优主注对应的 EV。
    ///
    /// 不单独保存 `optimal_ev`，而是根据 `optimal_bet` 从对应指标读取，
    /// 从结构上避免“最优下注是 Banker，但最优 EV 却来自 Player”的矛盾状态。
    pub const fn optimal_ev(self) -> f64 {
        self.metrics(self.optimal_bet).ev()
    }

    pub fn effective_metrics(self, bet: MainBet, rebate: RebateRule) -> EffectiveBetMetrics {
        effective_ev(self, bet, rebate)
    }

    pub fn optimal_effective_bet(self, rebate: RebateRule) -> MainBet {
        // 先假设 Player 最优
        let mut optimal_bet = MainBet::Player;

        let mut optimal_ev = effective_ev(self, MainBet::Player, rebate).effective_ev();

        // 比较 Banker
        let banker_ev = effective_ev(self, MainBet::Banker, rebate).effective_ev();

        if banker_ev > optimal_ev {
            optimal_bet = MainBet::Banker;

            // 非常重要：同步更新当前最大 EV
            optimal_ev = banker_ev;
        }

        // 比较 Tie
        let tie_ev = effective_ev(self, MainBet::Tie, rebate).effective_ev();

        if tie_ev > optimal_ev {
            optimal_bet = MainBet::Tie;
        }

        optimal_bet
    }

    pub fn optimal_effective_ev(self, rebate: RebateRule) -> f64 {
        self.effective_metrics(self.optimal_effective_bet(rebate), rebate)
            .effective_ev()
    }
}

/// 根据当前牌靴和赔付规则计算完整主注分析。
///
/// 这是 CLI 和 Python 最适合调用的高层入口：
///
/// 1. 借用牌靴，不修改调用者持有的状态；
/// 2. 精确枚举庄、闲、和权重；
/// 3. 根据赔付规则计算 EV；
/// 4. 组合指标并选出最大 EV 的下注。
pub fn analyze_main_bets(
    shoe: &Shoe,
    rules: MainBetRules,
) -> Result<MainBetAnalysis, ProbabilityError> {
    // `?` 会把牌数不足、权重溢出等概率错误直接返回给上层；
    // 只有枚举成功时才继续组装分析结果。
    let weights = calculate_main_outcomes(shoe)?;
    Ok(MainBetAnalysis::from_weights(weights, rules))
}

#[cfg(test)]
mod tests {
    use super::{MainBetAnalysis, analyze_main_bets, effective_ev};
    use crate::{MainBet, MainBetRules, OutcomeWeights, RebateRule, Shoe};

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn complete_shoe_analysis_exposes_python_facing_metrics() {
        let analysis = analyze_main_bets(&Shoe::default(), MainBetRules::standard())
            .expect("完整八副牌应能够完成主注分析");

        assert_close(analysis.player().probability(), 0.446246609344);
        assert_close(analysis.banker().probability(), 0.458597422633);
        assert_close(analysis.tie().probability(), 0.095155968024);

        for bet in [MainBet::Player, MainBet::Banker, MainBet::Tie] {
            let metrics = analysis.metrics(bet);
            assert_close(metrics.house_edge(), -metrics.ev());
            assert_close(metrics.rtp(), 1.0 + metrics.ev());
        }

        assert_eq!(analysis.optimal_bet(), MainBet::Banker);
        assert_eq!(analysis.optimal_bet().as_str(), "banker");
        assert_close(analysis.optimal_ev(), analysis.banker().ev());
    }

    #[test]
    fn optimal_bet_uses_the_largest_ev_not_the_largest_probability() {
        let weights =
            OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应构成完整分布");
        let analysis = MainBetAnalysis::from_weights(weights, MainBetRules::standard());

        assert_eq!(analysis.optimal_bet(), MainBet::Tie);
        assert_close(analysis.optimal_ev(), 0.5);
    }

    #[test]
    fn effective_ev_weights_rebate_by_possible_outcome() {
        let weights =
            OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应构成完整分布");
        let analysis = MainBetAnalysis::from_weights(weights, MainBetRules::standard());
        let rebate = RebateRule::AllExceptMainBetTie { rate: 0.015 };

        // Player/Banker 遇到 Tie 没有返水，因此返水期望是：
        // 0.015 × (P(Player) + P(Banker))
        // = 0.015 × (1/2 + 1/3)
        // = 0.0125。
        let player = effective_ev(analysis, MainBet::Player, rebate);
        assert_close(player.probability(), 0.5);
        assert_close(player.base_ev(), 1.0 / 6.0);
        assert_close(player.rebate_ev(), 0.0125);
        assert_close(player.effective_ev(), 1.0 / 6.0 + 0.0125);

        let banker = effective_ev(analysis, MainBet::Banker, rebate);
        assert_close(banker.base_ev(), -11.0 / 60.0);
        assert_close(banker.rebate_ev(), 0.0125);
        assert_close(banker.effective_ev(), -11.0 / 60.0 + 0.0125);

        // 按当前规则，Tie 注三种结果都有返水，所以返水期望就是完整 rate。
        let tie = effective_ev(analysis, MainBet::Tie, rebate);
        assert_close(tie.base_ev(), 0.5);
        assert_close(tie.rebate_ev(), 0.015);
        assert_close(tie.effective_ev(), 0.515);
    }
}
