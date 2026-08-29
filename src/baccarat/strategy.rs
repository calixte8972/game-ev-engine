//! 有效 EV 下注方向策略。
//!
//! 本模块只决定“是否下注”和“下注哪个方向”，不决定下注金额。
//! 下注金额由 `risk` 模块中的 `KellyPolicy` 根据这里的决策继续计算。

use crate::baccarat::RebateRule;
use crate::{MainBet, MainBetAnalysis};

/// 方向选择配置。
///
/// `rebate` 会先加入三个方向的基础 EV；`minimum_effective_ev` 是允许下注的
/// 最低门槛。即使某个方向是三者中最优，如果仍低于门槛，也会选择 Skip。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BettingPolicy {
    /// 当前玩家或渠道适用的返水规则。
    rebate: RebateRule,
    /// 允许下注所需达到的最低有效 EV。
    minimum_effective_ev: f64,
}

/// 方向策略给出的动作。这里只包含方向，不包含金额。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BetAction {
    /// 有效 EV 已达到门槛，允许继续计算这个方向的下注金额。
    Place {
        /// 通过有效 EV 门槛的候选下注方向。
        bet: MainBet,
    },
    /// 有效 EV 未达到门槛，本局停止，不进入金额计算。
    Skip {
        /// 方向策略拒绝下注的具体原因。
        reason: SkipReason,
    },
}

/// 方向策略拒绝下注的原因。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SkipReason {
    /// 三个方向中的最大有效 EV 仍然低于系统配置的门槛。
    BelowMinimumEv {
        /// 三个方向中已经最大的有效 EV。
        effective_ev: f64,
        /// 策略配置要求达到的最低有效 EV。
        minimum_ev: f64,
    },
}

/// 一次方向判断的完整、可审计结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BetDecision {
    /// 三个方向中有效 EV 最大的候选方向，即使最后 Skip 也会保留。
    candidate: MainBet,
    /// 候选方向不包含返水时的基础 EV。
    base_ev: f64,
    /// 返水单独贡献的 EV。
    rebate_ev: f64,
    /// `base_ev + rebate_ev`。
    effective_ev: f64,
    /// 根据 effective_ev 与门槛比较后得到的最终方向动作。
    action: BetAction,
}
impl BettingPolicy {
    /// 创建方向策略。
    ///
    /// 例如 `minimum_effective_ev = 0.0` 表示只允许非负有效 EV；`0.002`
    /// 表示至少达到每下注 1 单位期望盈利 0.002 才允许进入金额计算。
    pub const fn new(rebate: RebateRule, minimum_effective_ev: f64) -> Self {
        Self {
            rebate,
            minimum_effective_ev,
        }
    }

    /// 返回方向决策和金额计算共同使用的返水规则。
    pub const fn rebate(&self) -> RebateRule {
        self.rebate
    }

    /// 返回允许下注所需达到的最低有效 EV。
    pub const fn minimum_effective_ev(&self) -> f64 {
        self.minimum_effective_ev
    }

    /// 比较三个方向的有效 EV，并根据最低门槛生成方向决策。
    ///
    /// 输入的 `analysis` 已包含基础概率和赔付 EV；本函数加入当前策略的返水，
    /// 找到有效 EV 最大的方向，再返回 Place 或 Skip。它不会计算下注金额。
    pub fn decide(&self, analysis: MainBetAnalysis) -> BetDecision {
        // 先比较 Player、Banker、Tie 加入返水后的 effective EV，得到最大者。
        let candidate = analysis.optimal_effective_bet(self.rebate);

        // 再读取这个候选方向的详细指标，后面既用于比较门槛，也用于保存日志。
        let metrics = analysis.effective_metrics(candidate, self.rebate);
        let effective_ev = metrics.effective_ev();

        // “最优”只表示三个方向中最大，不代表一定值得下注。
        // 还必须达到最低有效 EV 门槛，等于门槛时也允许下注。
        let action: BetAction = if effective_ev >= self.minimum_effective_ev {
            BetAction::Place { bet: candidate }
        } else {
            BetAction::Skip {
                reason: SkipReason::BelowMinimumEv {
                    effective_ev,
                    minimum_ev: self.minimum_effective_ev,
                },
            }
        };
        BetDecision {
            candidate,
            base_ev: metrics.base_ev(),
            rebate_ev: metrics.rebate_ev(),
            effective_ev,
            action,
        }
    }
}
impl BetDecision {
    /// 返回有效 EV 最大的候选下注方向。
    pub const fn candidate(self) -> MainBet {
        self.candidate
    }

    /// 返回候选方向不含返水的基础 EV。
    pub const fn base_ev(self) -> f64 {
        self.base_ev
    }

    /// 返回候选方向由返水贡献的 EV。
    pub const fn rebate_ev(self) -> f64 {
        self.rebate_ev
    }

    /// 返回候选方向最终用于门槛比较的有效 EV。
    pub const fn effective_ev(self) -> f64 {
        self.effective_ev
    }

    /// 借用最终动作，不移动 BetDecision 中的枚举值。
    pub const fn action(&self) -> &BetAction {
        &self.action
    }
}

#[cfg(test)]
mod tests {
    use super::{BetAction, BettingPolicy, SkipReason};
    use crate::{MainBet, MainBetAnalysis, MainBetRules, OutcomeWeights, RebateRule};

    fn sample_analysis() -> MainBetAnalysis {
        let weights =
            OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应该构成完整概率分布");

        MainBetAnalysis::from_weights(weights, MainBetRules::standard())
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    #[test]
    fn decide_places_the_best_effective_ev_when_it_reaches_threshold() {
        let analysis = sample_analysis();
        let rebate = RebateRule::AllExceptMainBetTie { rate: 0.015 };
        let policy = BettingPolicy::new(rebate, 0.5);

        let decision = policy.decide(analysis);

        // 这个测试数据下，Tie 的有效 EV 为 0.515，是三个下注中最大的。
        assert_eq!(decision.candidate, MainBet::Tie);
        assert_close(decision.base_ev, 0.5);
        assert_close(decision.rebate_ev, 0.015);
        assert_close(decision.effective_ev, 0.515);

        match &decision.action {
            BetAction::Place { bet } => assert_eq!(*bet, MainBet::Tie),
            BetAction::Skip { .. } => panic!("有效 EV 已达到门槛，应该允许下注"),
        }
    }

    #[test]
    fn decide_skips_when_best_effective_ev_is_below_threshold() {
        let analysis = sample_analysis();
        let rebate = RebateRule::AllExceptMainBetTie { rate: 0.015 };
        let policy = BettingPolicy::new(rebate, 0.6);

        let decision = policy.decide(analysis);

        // 候选下注仍然是 Tie，但 0.515 小于 0.6，所以必须跳过。
        assert_eq!(decision.candidate, MainBet::Tie);

        match &decision.action {
            BetAction::Skip {
                reason:
                    SkipReason::BelowMinimumEv {
                        effective_ev,
                        minimum_ev,
                    },
            } => {
                assert_close(*effective_ev, 0.515);
                assert_close(*minimum_ev, 0.6);
            }
            BetAction::Place { .. } => panic!("有效 EV 低于门槛，不应该下注"),
        }
    }

    #[test]
    fn decide_places_when_effective_ev_equals_threshold() {
        let analysis = sample_analysis();
        let rebate = RebateRule::AllExceptMainBetTie { rate: 0.015 };
        let threshold = analysis
            .effective_metrics(MainBet::Tie, rebate)
            .effective_ev();
        let policy = BettingPolicy::new(rebate, threshold);

        let decision = policy.decide(analysis);

        // 用同一个计算结果作为门槛，验证当前策略采用 >= 语义。
        match &decision.action {
            BetAction::Place { bet } => assert_eq!(*bet, MainBet::Tie),
            BetAction::Skip { .. } => panic!("有效 EV 等于门槛时应该允许下注"),
        }
    }
}
