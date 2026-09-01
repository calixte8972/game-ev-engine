//! 凯利公式下注金额与风险限制。
//!
//! 整个下注过程被拆成了两个互相独立的步骤：
//!
//! 1. [`BettingPolicy`](super::BettingPolicy) 查看三个方向的有效 EV，决定
//!    “这一局是否值得下注”和“下注 Player、Banker 还是 Tie”；
//! 2. [`KellyPolicy`] 接收已经选好的方向，根据每种结果的概率和盈亏，决定
//!    “应该拿当前资金的多少比例下注”。
//!
//! 最常用的完整数据流是：
//!
//! ```text
//! 当前牌靴
//!   ↓ 精确枚举
//! OutcomeWeights（Player、Banker、Tie 的概率权重）
//!   ↓ BettingPolicy
//! BetDecision（是否下注、下注方向、有效 EV）
//!   ↓ KellyPolicy
//! BetPlan（最终方向、凯利比例、最终下注金额）
//! ```
//!
//! 这里有一个重要边界：所有计算都发生在下注前。代码并不知道本局最后会开出
//! 什么结果，而是把所有可能结果分别计算，再按照各自概率加权。
//!
//! 把方向选择和资金管理拆开后，EV 门槛、返水、凯利公式和金额上限都可以
//! 单独测试。以后即使把完整凯利换成半凯利，也不需要重写概率枚举器。
//!
//! 资金层的一个容易混淆的点是：策略比例和安全上限不是同一个东西。
//! 例如半凯利先把数学凯利比例乘以 0.5，再与 `max_fraction`、单局金额、
//! 桌台上限和当前本金逐层取最小值。前者回答“数学上想下注多少”，后者回答
//! “运营上最多允许下注多少”。

use std::{error::Error, fmt};

use super::{
    BetAction, BetDecision, BetTarget, BettingPolicy, CombinedBetAction, CombinedBetDecision,
    MainBet, MainBetAnalysis, MainBetRules, OutcomeWeights, RebateRule, RoundOutcome, SideBet,
    SideBetAnalysis, SideBetRules, SideBetWeights, SkipReason,
};

/// 浮点概率求和时允许的舍入误差。
const PROBABILITY_TOLERANCE: f64 = 1e-12;
/// 二分 100 次已经远高于 `f64` 在 `0.0..=1.0` 区间内的有效精度。
const BISECTION_ITERATIONS: usize = 100;
/// 不在会令资金变成零的数学边界上取值，避免计算 `ln(0)` 或除以零。
const DOMAIN_MARGIN: f64 = 1e-12;

/// 通过 EV 门槛后，如何把当前本金转换成目标下注金额。
///
/// 方向选择仍由 [`BettingPolicy`] 负责；本枚举只负责金额。把它建模成枚举，
/// 可以保证一笔计划在同一时刻只使用一种互斥的金额算法。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StakeSizingStrategy {
    /// 使用公式算出的完整凯利比例。
    FullKelly,
    /// 使用完整凯利比例的一半，降低波动与模型误差风险。
    HalfKelly,
    /// 使用完整凯利比例的四分之一，进一步降低回撤。
    QuarterKelly,
    /// 使用调用者指定的完整凯利倍数，例如 `0.3` 表示三成凯利。
    CustomKelly {
        /// `0.0..=1.0` 内的完整凯利缩放系数。
        fraction: f64,
    },
    /// 每次通过 EV 门槛后尝试下注固定金额。
    Fixed {
        /// 风控上限生效前的目标金额。
        amount: f64,
    },
    /// 每次下注当前本金的固定比例，不读取凯利结果。
    FixedBankrollFraction {
        /// `0.0..=1.0` 内的本金比例。
        fraction: f64,
    },
    /// 反推达到指定“单笔期望盈利金额”所需的下注额。
    TargetExpectedProfit {
        /// 风控上限生效前希望获得的单笔期望盈利。
        amount: f64,
    },
    /// 根据单位收益分布的标准差控制单笔资金波动。
    TargetVolatility {
        /// 希望单笔收益标准差占当前本金的比例。
        fraction: f64,
    },
}

impl StakeSizingStrategy {
    /// 返回供 JSON、日志和前端使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FullKelly => "full_kelly",
            Self::HalfKelly => "half_kelly",
            Self::QuarterKelly => "quarter_kelly",
            Self::CustomKelly { .. } => "custom_kelly",
            Self::Fixed { .. } => "fixed",
            Self::FixedBankrollFraction { .. } => "bankroll_fraction",
            Self::TargetExpectedProfit { .. } => "target_expected_profit",
            Self::TargetVolatility { .. } => "target_volatility",
        }
    }

    /// 把所选金额策略转换为目标本金比例。
    fn target_fraction(
        self,
        full_kelly_fraction: f64,
        bankroll: f64,
        effective_ev: f64,
        outcomes: &[KellyOutcome],
    ) -> f64 {
        match self {
            // 这些策略都先从同一份收益分布得到完整凯利，再只改变目标比例。
            Self::FullKelly => full_kelly_fraction,
            Self::HalfKelly => full_kelly_fraction * 0.5,
            Self::QuarterKelly => full_kelly_fraction * 0.25,
            Self::CustomKelly { fraction } => full_kelly_fraction * fraction,
            Self::Fixed { amount } => amount / bankroll,
            Self::FixedBankrollFraction { fraction } => fraction,
            Self::TargetExpectedProfit { amount } => {
                // EV 是每下注 1 单位的期望盈利，所以 amount / EV 得到目标
                // 下注额，再除以 bankroll 转成统一的“本金比例”。
                if effective_ev > 0.0 {
                    amount / (bankroll * effective_ev)
                } else {
                    0.0
                }
            }
            Self::TargetVolatility { fraction } => {
                // 对单位下注收益计算方差。下注额是本金 × f，标准差为
                // bankroll × f × sqrt(variance)，令它占本金 fraction 后可得
                // f = fraction / sqrt(variance)。
                let variance = outcomes
                    .iter()
                    .map(|outcome| {
                        let deviation = outcome.net_profit - effective_ev;
                        outcome.probability * deviation * deviation
                    })
                    .sum::<f64>();
                if variance > 0.0 {
                    fraction / variance.sqrt()
                } else {
                    0.0
                }
            }
        }
    }

    /// 只有凯利类策略需要用“完整凯利必须为正”作为额外放行条件。
    const fn requires_positive_kelly(self) -> bool {
        matches!(
            self,
            Self::FullKelly | Self::HalfKelly | Self::QuarterKelly | Self::CustomKelly { .. }
        )
    }

    /// 固定金额策略返回配置金额；其他策略没有“固定下注额”。
    pub const fn fixed_amount(self) -> Option<f64> {
        match self {
            Self::Fixed { amount } => Some(amount),
            _ => None,
        }
    }

    /// 返回前端配置该策略时使用的单一数值参数。
    pub const fn parameter(self) -> Option<f64> {
        match self {
            Self::FullKelly | Self::HalfKelly | Self::QuarterKelly => None,
            Self::CustomKelly { fraction }
            | Self::FixedBankrollFraction { fraction }
            | Self::TargetVolatility { fraction } => Some(fraction),
            Self::Fixed { amount } | Self::TargetExpectedProfit { amount } => Some(amount),
        }
    }
}

/// 凯利计算中的一个互斥结果。
///
/// `net_profit` 表示下注 1 单位后，该结果发生时相对于本金的净盈利：
///
/// - `1.0`：净赢 1 单位；
/// - `-1.0`：输掉 1 单位本金；
/// - `0.0`：Push，本金不增不减；
/// - `0.015`：只获得 1.5% 返水。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KellyOutcome {
    /// 这个结果发生的概率。例如 `0.45` 表示 45%。
    probability: f64,
    /// 如果这个结果真的发生，每下注 1 单位最终净赚或净亏多少。
    /// 正数表示盈利，负数表示亏损，零表示本金没有变化。
    net_profit: f64,
}

impl KellyOutcome {
    /// 创建一个“可能结果”。
    ///
    /// 这里暂时只保存数据，不立刻检查概率是否合法。真正调用
    /// [`calculate_kelly_fraction`] 时会统一检查所有结果，这样还可以验证
    /// 所有概率相加是否等于 1。
    pub const fn new(probability: f64, net_profit: f64) -> Self {
        Self {
            probability,
            net_profit,
        }
    }

    /// 返回该互斥结果发生的概率。
    pub const fn probability(self) -> f64 {
        self.probability
    }

    /// 返回该结果下每下注 1 单位的净盈利。
    pub const fn net_profit(self) -> f64 {
        self.net_profit
    }
}

/// 凯利下注的资金限制。
///
/// `full()` 使用完整凯利比例，但完整凯利仍然要经过单局和桌台上限保护。
/// `new()` 可以进一步限制最大资金比例；例如 `0.25` 表示最多使用资金的 25%，
/// 它是风险上限，并不是“四分之一凯利”的乘数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KellyPolicy {
    /// 通过方向与 EV 门槛后使用的金额算法。
    strategy: StakeSizingStrategy,
    /// 最终允许使用的最大资金比例，`1.0` 表示最多可以使用全部 bankroll。
    ///
    /// 它只是安全上限，不是实际下注比例。实际比例通常由凯利公式算得更小。
    max_fraction: f64,
    /// 本系统自己规定的单局最大下注金额。
    max_round_stake: f64,
    /// 赌场或桌台允许的最大下注金额。
    table_limit: f64,
    /// 边注额外使用的单笔金额上限；主注不会读取这个字段。
    side_bet_limit: f64,
}

impl KellyPolicy {
    /// 创建完整凯利策略。
    ///
    /// 最终下注额仍取以下几项的最小值：
    ///
    /// `资金 × 完整凯利比例`、单局风控上限、桌台上限、当前资金。
    pub fn full(max_round_stake: f64, table_limit: f64) -> Result<Self, KellyError> {
        Self::new(1.0, max_round_stake, table_limit)
    }

    /// 创建带资金比例上限的凯利策略。
    ///
    /// 三个参数都允许为零，因此可以把零上限当成紧急停止开关；但它们必须是
    /// 有限的非负数，并且 `max_fraction` 不能超过 1。
    pub fn new(
        max_fraction: f64,
        max_round_stake: f64,
        table_limit: f64,
    ) -> Result<Self, KellyError> {
        Self::with_strategy(
            StakeSizingStrategy::FullKelly,
            max_fraction,
            max_round_stake,
            table_limit,
        )
    }

    /// 创建指定金额算法并应用统一风险上限。
    pub fn with_strategy(
        strategy: StakeSizingStrategy,
        max_fraction: f64,
        max_round_stake: f64,
        table_limit: f64,
    ) -> Result<Self, KellyError> {
        validate_max_fraction(max_fraction)?;
        if let StakeSizingStrategy::Fixed { amount } = strategy
            && (!amount.is_finite() || amount < 0.0)
        {
            return Err(KellyError::InvalidFixedStake { value: amount });
        }
        match strategy {
            StakeSizingStrategy::CustomKelly { fraction }
            | StakeSizingStrategy::FixedBankrollFraction { fraction }
            | StakeSizingStrategy::TargetVolatility { fraction }
                if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) =>
            {
                return Err(KellyError::InvalidStrategyParameter {
                    strategy: strategy.as_str(),
                    value: fraction,
                    expected: "a finite fraction in 0..=1",
                });
            }
            StakeSizingStrategy::TargetExpectedProfit { amount }
                if !amount.is_finite() || amount < 0.0 =>
            {
                return Err(KellyError::InvalidStrategyParameter {
                    strategy: strategy.as_str(),
                    value: amount,
                    expected: "a finite non-negative amount",
                });
            }
            _ => {}
        }
        validate_limit(
            max_round_stake,
            KellyError::InvalidMaxRoundStake {
                value: max_round_stake,
            },
        )?;
        validate_limit(
            table_limit,
            KellyError::InvalidTableLimit { value: table_limit },
        )?;

        Ok(Self {
            strategy,
            max_fraction,
            max_round_stake,
            table_limit,
            // 旧调用者没有边注参数时，默认不比系统单局上限更宽松。
            side_bet_limit: max_round_stake,
        })
    }

    /// 为十一种边注增加独立金额上限。
    pub fn with_side_bet_limit(mut self, side_bet_limit: f64) -> Result<Self, KellyError> {
        validate_limit(
            side_bet_limit,
            KellyError::InvalidSideBetLimit {
                value: side_bet_limit,
            },
        )?;
        self.side_bet_limit = side_bet_limit;
        Ok(self)
    }

    /// 返回当前使用的金额算法。
    pub const fn strategy(self) -> StakeSizingStrategy {
        self.strategy
    }

    /// 返回资金比例安全上限。
    pub const fn max_fraction(self) -> f64 {
        self.max_fraction
    }

    /// 返回系统单局金额上限。
    pub const fn max_round_stake(self) -> f64 {
        self.max_round_stake
    }

    /// 返回赌场桌台金额上限。
    pub const fn table_limit(self) -> f64 {
        self.table_limit
    }

    /// 返回边注单笔金额上限。
    pub const fn side_bet_limit(self) -> f64 {
        self.side_bet_limit
    }

    /// 为一个已经确定方向的百家乐主注计算完整凯利报价。
    ///
    /// 这个函数不负责比较 Player、Banker、Tie。调用者已经通过 `bet` 参数
    /// 指定了方向，本函数只回答“如果下这个方向，应当下多少钱”。
    ///
    /// 参数含义：
    ///
    /// - `weights`：下注前，根据当前剩余牌枚举出的结果权重；
    /// - `rules`：闲、庄、和的赔付规则；
    /// - `rebate`：每种下注和结果对应的返水规则；
    /// - `bet`：已经选定的下注方向；
    /// - `bankroll`：这一套策略当前允许管理的资金，不一定等于账户全部余额。
    ///
    /// 返回值同时保留原始完整凯利比例和经过全部资金上限后的实际比例，
    /// 方便真实数据回放时解释“理论建议多少”和“最终为什么只下这么多”。
    pub fn quote(
        self,
        weights: OutcomeWeights,
        rules: MainBetRules,
        rebate: RebateRule,
        bet: MainBet,
        bankroll: f64,
    ) -> Result<KellyQuote, KellyError> {
        let outcomes = main_bet_kelly_outcomes(weights, rules, rebate, bet);
        self.quote_outcomes(BetTarget::Main(bet), &outcomes, bankroll)
    }

    /// 为一个已经选定的边注计算多结果凯利报价。
    pub fn quote_side(
        self,
        weights: SideBetWeights,
        rules: SideBetRules,
        bet: SideBet,
        bankroll: f64,
    ) -> Result<KellyQuote, KellyError> {
        self.quote_side_with_rebate(weights, rules, RebateRule::None, bet, bankroll)
    }

    /// 为边注计算包含返水的多结果凯利报价。
    ///
    /// 保留 [`KellyPolicy::quote_side`] 作为“不返水”的兼容入口；策略和 CSV
    /// 回放调用本函数，把边注返水同时加入赢、输、Push 的单位净收益。这样
    /// 报价中的有效 EV、凯利比例和期望盈利会与最终结算保持一致。
    pub fn quote_side_with_rebate(
        self,
        weights: SideBetWeights,
        rules: SideBetRules,
        rebate: RebateRule,
        bet: SideBet,
        bankroll: f64,
    ) -> Result<KellyQuote, KellyError> {
        let outcomes = side_bet_kelly_outcomes_with_rebate(weights, rules, rebate, bet);
        self.quote_outcomes(BetTarget::Side(bet), &outcomes, bankroll)
    }

    /// 主注和边注最终都转换为相同的“概率 + 单位净收益”分布后，共用这一份
    /// 凯利与金额上限逻辑。这样幸运 7 的分档赔率也不需要另一套近似公式。
    fn quote_outcomes(
        self,
        target: BetTarget,
        outcomes: &[KellyOutcome],
        bankroll: f64,
    ) -> Result<KellyQuote, KellyError> {
        validate_bankroll(bankroll)?;

        // 第二步：重新从完整收益分布计算有效 EV。
        // 这里的结果应该与 analysis 层的 effective_ev 一致；测试会检查这一点。
        let effective_ev = validate_outcomes(outcomes)?;

        // 先计算不打折的完整凯利比例，再应用系统配置的资金比例上限。
        // 这样日志中可以同时看到数学结果和实际执行结果。
        let kelly_fraction = calculate_kelly_fraction(outcomes, 1.0)?;

        // 第三步：金额策略统一转换成“目标本金比例”。凯利类缩放完整凯利，
        // 固定金额用 amount / bankroll，固定本金比例直接使用配置比例；
        // 目标期望盈利和目标波动率则分别从 EV、收益标准差反推比例。
        // 后面的限额逻辑因此不需要知道具体使用了哪一种金额算法。
        let strategy_fraction =
            self.strategy
                .target_fraction(kelly_fraction, bankroll, effective_ev, outcomes);

        // 第四步再应用共同的本金比例上限。金额策略和安全上限是两个概念：
        // 前者回答“想下多少”，后者回答“最多允许下多少”。
        let fraction_after_policy = strategy_fraction.min(self.max_fraction);

        // 上限按从“理论金额”到“最终金额”的顺序逐项收紧。
        // 最后的 bankroll 保护保证任何配置都不会下注超过当前资金。
        let mut amount = (bankroll * fraction_after_policy)
            .min(self.max_round_stake)
            .min(self.table_limit)
            .min(bankroll);
        if target.is_side() {
            amount = amount.min(self.side_bet_limit);
        }

        // applied_fraction 使用“最终金额”反除 bankroll，所以它已经反映了
        // max_fraction、单局上限和桌台上限的共同影响。
        let applied_fraction = amount / bankroll;

        Ok(KellyQuote {
            bet: target,
            effective_ev,
            kelly_fraction,
            strategy_fraction,
            applied_fraction,
            amount,
            // EV 是“每下注 1 单位的期望净盈利”，所以乘以实际下注额
            // 就得到这一笔下注的期望净盈利金额。
            expected_profit: amount * effective_ev,
        })
    }

    /// 从“下注方向策略”一直生成到“最终金额”的完整计划。
    ///
    /// 数据流向如下：
    ///
    /// `OutcomeWeights -> MainBetAnalysis -> BettingPolicy -> KellyQuote -> BetPlan`
    ///
    /// 同一个 `BettingPolicy` 同时提供 EV 门槛和返水规则，因此下注方向使用的
    /// effective EV 与凯利金额使用的返水不会意外不一致。
    ///
    /// 上层程序通常应该调用这个函数，而不是自己分别调用 `decide()` 和
    /// `quote()`。它把两步串在一起，可以防止上层用 A 返水选择方向，
    /// 却误用 B 返水计算下注金额。
    pub fn plan(
        self,
        betting_policy: &BettingPolicy,
        weights: OutcomeWeights,
        rules: MainBetRules,
        bankroll: f64,
    ) -> Result<BetPlan, KellyError> {
        validate_bankroll(bankroll)?;

        // 第一步：把同一份权重和赔付组合成三个下注方向的概率与 EV。
        let analysis = MainBetAnalysis::from_weights(weights, rules);

        // 第二步：方向策略比较三个 effective EV，并应用最低 EV 门槛。
        let decision = betting_policy.decide(analysis);

        match *decision.action() {
            // 上一层已经因 EV 门槛拒绝下注时，凯利层不能重新放行。
            BetAction::Skip { reason } => Ok(BetPlan {
                decision,
                quote: None,
                action: BetPlanAction::Skip {
                    reason: BetPlanSkipReason::Strategy(reason),
                },
            }),
            BetAction::Place { bet } => {
                // 第三步：只有方向策略同意下注时，才需要计算资金比例与金额。
                let quote = self.quote(weights, rules, betting_policy.rebate(), bet, bankroll)?;

                let action =
                    if self.strategy.requires_positive_kelly() && quote.kelly_fraction() <= 0.0 {
                        // 即使 EV 门槛被配置为负数，凯利公式也不会为非正 EV 投入资金。
                        BetPlanAction::Skip {
                            reason: BetPlanSkipReason::NonPositiveKelly,
                        }
                    } else if quote.amount() <= 0.0 {
                        // 数学上应该下注，但某个资金上限为零；这可以作为运营停机开关。
                        BetPlanAction::Skip {
                            reason: BetPlanSkipReason::RiskLimitIsZero,
                        }
                    } else {
                        BetPlanAction::Place {
                            bet,
                            amount: quote.amount(),
                        }
                    };

                Ok(BetPlan {
                    decision,
                    quote: Some(quote),
                    action,
                })
            }
        }
    }

    /// 从十四种下注目标中选择方向，并生成受边注独立上限保护的最终计划。
    pub fn plan_all(
        self,
        betting_policy: &BettingPolicy,
        main_weights: OutcomeWeights,
        main_rules: MainBetRules,
        side_weights: SideBetWeights,
        side_rules: SideBetRules,
        bankroll: f64,
    ) -> Result<CombinedBetPlan, KellyError> {
        self.plan_all_with_side_bet_filter(
            betting_policy,
            main_weights,
            main_rules,
            side_weights,
            side_rules,
            bankroll,
            |_| true,
        )
    }

    /// 生成完整下注计划，但只允许通过调用方过滤规则的边注参与竞争。
    ///
    /// 过滤发生在 EV 候选比较之前；这比选出幸运 6/7 后再强制 Skip 更合理，
    /// 因为禁用的幸运边注不应挡住本来可以下注的庄、闲、和或其他边注。
    #[allow(clippy::too_many_arguments)]
    pub fn plan_all_with_side_bet_filter<F>(
        self,
        betting_policy: &BettingPolicy,
        main_weights: OutcomeWeights,
        main_rules: MainBetRules,
        side_weights: SideBetWeights,
        side_rules: SideBetRules,
        bankroll: f64,
        allows_side_bet: F,
    ) -> Result<CombinedBetPlan, KellyError>
    where
        F: Fn(SideBet) -> bool,
    {
        validate_bankroll(bankroll)?;

        let main_analysis = MainBetAnalysis::from_weights(main_weights, main_rules);
        let side_analysis = SideBetAnalysis::calculate(side_weights, side_rules);
        let decision = betting_policy.decide_all_with_side_bet_filter(
            main_analysis,
            side_analysis,
            allows_side_bet,
        );

        match *decision.action() {
            CombinedBetAction::Skip { reason } => Ok(CombinedBetPlan {
                decision,
                quote: None,
                action: CombinedBetPlanAction::Skip {
                    reason: BetPlanSkipReason::Strategy(reason),
                },
            }),
            CombinedBetAction::Place { bet } => {
                let quote = match bet {
                    BetTarget::Main(main_bet) => self.quote(
                        main_weights,
                        main_rules,
                        betting_policy.rebate(),
                        main_bet,
                        bankroll,
                    )?,
                    BetTarget::Side(side_bet) => self.quote_side_with_rebate(
                        side_weights,
                        side_rules,
                        betting_policy.rebate(),
                        side_bet,
                        bankroll,
                    )?,
                };

                let action =
                    if self.strategy.requires_positive_kelly() && quote.kelly_fraction() <= 0.0 {
                        CombinedBetPlanAction::Skip {
                            reason: BetPlanSkipReason::NonPositiveKelly,
                        }
                    } else if quote.amount() <= 0.0 {
                        CombinedBetPlanAction::Skip {
                            reason: BetPlanSkipReason::RiskLimitIsZero,
                        }
                    } else {
                        CombinedBetPlanAction::Place {
                            bet,
                            amount: quote.amount(),
                        }
                    };

                Ok(CombinedBetPlan {
                    decision,
                    quote: Some(quote),
                    action,
                })
            }
        }
    }

    /// 为所有达到 EV 门槛的目标生成下注计划。
    ///
    /// 每个目标先独立计算自己的凯利报价，然后把可下注金额放进同一个
    /// 本局总风险预算。如果各目标的金额加总超过本金比例、单局金额、桌台
    /// 金额或本金中的任意一个上限，就按比例同时缩小，避免多注模式把风险
    /// 上限重复使用多次。
    #[allow(clippy::too_many_arguments)]
    pub fn plan_all_multiple_with_side_bet_filter<F>(
        self,
        betting_policy: &BettingPolicy,
        main_weights: OutcomeWeights,
        main_rules: MainBetRules,
        side_weights: SideBetWeights,
        side_rules: SideBetRules,
        bankroll: f64,
        allows_side_bet: F,
    ) -> Result<Vec<CombinedBetPlan>, KellyError>
    where
        F: Fn(SideBet) -> bool,
    {
        validate_bankroll(bankroll)?;

        let main_analysis = MainBetAnalysis::from_weights(main_weights, main_rules);
        let side_analysis = SideBetAnalysis::calculate(side_weights, side_rules);
        let decisions = betting_policy.eligible_all_with_side_bet_filter(
            main_analysis,
            side_analysis,
            allows_side_bet,
        );

        // 每个达标目标先独立报价。此时 quote.amount() 只受到“单个目标”的
        // 边注上限/共同基础上限影响，暂时还没有把同局其他下注加进来。
        let mut plans = Vec::with_capacity(decisions.len());
        for decision in decisions {
            let bet = decision.candidate();
            let quote = match bet {
                BetTarget::Main(main_bet) => self.quote(
                    main_weights,
                    main_rules,
                    betting_policy.rebate(),
                    main_bet,
                    bankroll,
                )?,
                BetTarget::Side(side_bet) => self.quote_side_with_rebate(
                    side_weights,
                    side_rules,
                    betting_policy.rebate(),
                    side_bet,
                    bankroll,
                )?,
            };

            let action = if self.strategy.requires_positive_kelly() && quote.kelly_fraction() <= 0.0
            {
                CombinedBetPlanAction::Skip {
                    reason: BetPlanSkipReason::NonPositiveKelly,
                }
            } else if quote.amount() <= 0.0 {
                CombinedBetPlanAction::Skip {
                    reason: BetPlanSkipReason::RiskLimitIsZero,
                }
            } else {
                CombinedBetPlanAction::Place {
                    bet,
                    amount: quote.amount(),
                }
            };

            plans.push(CombinedBetPlan {
                decision,
                quote: Some(quote),
                action,
            });
        }

        let requested_total: f64 = plans
            .iter()
            .filter_map(|plan| match plan.action() {
                CombinedBetPlanAction::Place { amount, .. } => Some(*amount),
                CombinedBetPlanAction::Skip { .. } => None,
            })
            .sum();
        let common_limit = (bankroll * self.max_fraction)
            .min(self.max_round_stake)
            .min(self.table_limit)
            .min(bankroll);
        // 如果多个目标的独立报价合计超过同局预算，所有 Place 计划按同一比例
        // 缩小。按比例而不是按顺序截断，可以避免候选排列顺序决定谁拿到预算。
        let scale = if requested_total > common_limit && requested_total > 0.0 {
            common_limit / requested_total
        } else {
            1.0
        };

        for plan in &mut plans {
            plan.scale_amount(scale);
        }

        Ok(plans)
    }
}

/// 一次凯利金额计算的可审计结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KellyQuote {
    /// 凯利金额所对应的下注方向。
    bet: BetTarget,
    /// 该方向每下注 1 单位的期望净收益，已经包含返水。
    effective_ev: f64,
    /// 不打折的完整凯利比例。
    kelly_fraction: f64,
    /// 选定金额策略产生的目标比例，尚未经过共同风险上限。
    strategy_fraction: f64,
    /// 经过资金比例、单局金额和桌台金额上限后的实际资金比例。
    applied_fraction: f64,
    /// 最终建议下注额。
    amount: f64,
    /// `amount × effective_ev`，表示这一笔的期望净盈利金额。
    expected_profit: f64,
}

impl KellyQuote {
    /// 返回报价对应的下注方向。
    pub const fn bet(self) -> BetTarget {
        self.bet
    }

    /// 返回包含返水的有效 EV。
    pub const fn effective_ev(self) -> f64 {
        self.effective_ev
    }

    /// 返回公式算出的原始完整凯利比例。
    pub const fn kelly_fraction(self) -> f64 {
        self.kelly_fraction
    }

    /// 返回金额策略产生、但尚未经过上限裁剪的目标本金比例。
    pub const fn strategy_fraction(self) -> f64 {
        self.strategy_fraction
    }

    /// 返回经过所有资金上限后的实际下注比例。
    pub const fn applied_fraction(self) -> f64 {
        self.applied_fraction
    }

    /// 返回最终建议下注金额。
    pub const fn amount(self) -> f64 {
        self.amount
    }

    /// 返回这笔建议金额对应的期望净盈利。
    pub const fn expected_profit(self) -> f64 {
        self.expected_profit
    }

    /// 按本局组合风险上限的比例缩放金额。
    ///
    /// 单个下注先按自身收益分布得到报价；多下注模式再把所有报价一起按同一
    /// 比例收缩。凯利比例和金额策略目标比例保持原值，`applied_fraction`、
    /// 最终金额与期望盈利则同步更新，方便审计“为什么实际金额变小”。
    fn scaled_amount(self, scale: f64) -> Self {
        let amount = self.amount * scale;
        Self {
            applied_fraction: self.applied_fraction * scale,
            amount,
            expected_profit: amount * self.effective_ev,
            ..self
        }
    }
}

/// 完整下注计划的最终动作。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BetPlanAction {
    /// 所有策略和资金检查均已通过，可以按指定方向和金额下注。
    Place {
        /// 最终应该下注的方向。
        bet: MainBet,
        /// 经过全部风险上限后的最终下注金额。
        amount: f64,
    },
    /// 本局不下注，并保留具体原因供日志、Python 或数据库记录。
    Skip {
        /// 最终拒绝下注的具体原因。
        reason: BetPlanSkipReason,
    },
}

/// 凯利计划跳过下注的原因。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BetPlanSkipReason {
    /// EV 方向策略没有通过门槛。
    Strategy(SkipReason),
    /// 方向策略允许下注，但该收益分布的完整凯利比例为零。
    NonPositiveKelly,
    /// 凯利比例为正，但配置的资金上限把最终金额限制成了零。
    RiskLimitIsZero,
}

/// 把方向决策和金额报价组合在一起，作为未来 Python/数据库层的稳定输入。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BetPlan {
    /// EV 层给出的原始方向决策。即使最终因金额限制而 Skip，也保留它。
    decision: BetDecision,
    /// 凯利报价。方向策略直接 Skip 时为 `None`，进入过金额计算时为 `Some`。
    quote: Option<KellyQuote>,
    /// 上层真正应执行的最终动作。
    action: BetPlanAction,
}

impl BetPlan {
    /// 返回方向策略生成的原始决策。
    pub const fn decision(&self) -> &BetDecision {
        &self.decision
    }

    /// 策略门槛直接拒绝时没有金额报价；其他情况会保留报价用于审计。
    pub const fn quote(self) -> Option<KellyQuote> {
        self.quote
    }

    /// 返回上层最终应该执行的动作。
    pub const fn action(&self) -> &BetPlanAction {
        &self.action
    }
}

/// 主注和边注共同比较后的最终动作。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CombinedBetPlanAction {
    /// 该目标通过策略和资金检查后的实际下注。
    Place { bet: BetTarget, amount: f64 },
    /// 该目标最终没有执行，并保留跳过原因。
    Skip { reason: BetPlanSkipReason },
}

/// 十四种目标统一经过 EV 门槛、凯利公式和金额上限后的完整计划。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombinedBetPlan {
    /// 方向层生成的候选和 EV 门槛结果。
    decision: CombinedBetDecision,
    /// 该目标的凯利/金额报价；策略层直接 Skip 时为 `None`。
    quote: Option<KellyQuote>,
    /// 上层真正需要执行的动作。
    action: CombinedBetPlanAction,
}

impl CombinedBetPlan {
    /// 返回方向层决策，包含候选、基础 EV、返水 EV 和门槛。
    pub const fn decision(&self) -> &CombinedBetDecision {
        &self.decision
    }

    /// 返回金额报价；没有进入金额计算时为 `None`。
    pub const fn quote(self) -> Option<KellyQuote> {
        self.quote
    }

    /// 返回经过所有检查后的最终动作。
    pub const fn action(&self) -> &CombinedBetPlanAction {
        &self.action
    }

    /// 将本计划的实际金额按组合风险比例缩放。
    fn scale_amount(&mut self, scale: f64) {
        let Some(quote) = self.quote else {
            return;
        };

        // 多注模式先为每个目标独立算报价，再在这里按共同比例同步缩放。
        // 这样每个下注仍保留自己的 EV/凯利信息，同时总暴露不超过本局预算。
        let quote = quote.scaled_amount(scale);
        self.quote = Some(quote);
        self.action = match self.action {
            CombinedBetPlanAction::Place { bet, .. } if quote.amount() > 0.0 => {
                CombinedBetPlanAction::Place {
                    bet,
                    amount: quote.amount(),
                }
            }
            CombinedBetPlanAction::Place { .. } => CombinedBetPlanAction::Skip {
                reason: BetPlanSkipReason::RiskLimitIsZero,
            },
            action => action,
        };
    }
}

/// 把百家乐结果转成凯利公式真正需要的“概率 + 单位净收益”分布。
///
/// 下注庄时必须把庄赢拆成“非 6 点庄赢”和“6 点庄赢”。标准佣金庄的
/// 两项净收益相同，但免佣庄的两项分别可能是 `1.0` 和 `0.5`；保留拆分后，
/// 同一套凯利函数就能正确处理两种赔付。
///
/// 返水直接加到该结果的净收益上，因为两者的单位都是“每下注 1 单位能带来
/// 多少净收益”。例如输掉本金但获得 1.5% 返水时：
///
/// `net_profit = -1.0 + 0.015 = -0.985`
///
/// 当前规则下，各方向大致会形成下面的收益：
///
/// ```text
/// Player 注：Player 赢 = 闲赔付 + 返水；Banker 赢 = -1 + 返水；Tie = 0
/// Banker 注：Player 赢 = -1 + 返水；Banker 赢 = 庄赔付 + 返水；Tie = 0
/// Tie 注：   Tie = 和赔付 + 返水；Player/Banker 赢 = -1 + 返水
/// ```
pub fn main_bet_kelly_outcomes(
    weights: OutcomeWeights,
    rules: MainBetRules,
    rebate: RebateRule,
    bet: MainBet,
) -> Vec<KellyOutcome> {
    let player_probability = weights.player_probability();
    let banker_probability = weights.banker_probability();
    let tie_probability = weights.tie_probability();

    if bet == MainBet::Banker {
        // Banker 是唯一需要四个结果的方向，因为“庄赢”内部还要继续区分
        // 普通庄赢与庄 6 点赢。免佣庄只对庄 6 使用较低赔付。
        let banker_six_probability = weights.banker_win_on_six_probability();

        // banker_win_on_six 是 banker 的子集，所以普通庄赢权重等于两者相减。
        // 先用整数权重相减，再转成 f64，可以少一次浮点减法误差。
        let banker_non_six_probability = (weights.banker_weight()
            - weights.banker_win_on_six_weight()) as f64
            / weights.total_weight() as f64;

        // 当前返水只由“下注方向 + 最终主结果”决定，不区分庄是否 6 点，
        // 因此两种庄赢可以复用同一个返水率。
        let banker_rebate = rebate.rate_for(MainBet::Banker, RoundOutcome::Banker);

        vec![
            KellyOutcome::new(
                player_probability,
                rules.settle(MainBet::Banker, RoundOutcome::Player)
                    + rebate.rate_for(MainBet::Banker, RoundOutcome::Player),
            ),
            KellyOutcome::new(
                banker_non_six_probability,
                rules.banker_payout_for_total(5) + banker_rebate,
            ),
            KellyOutcome::new(
                banker_six_probability,
                rules.banker_payout_for_total(6) + banker_rebate,
            ),
            KellyOutcome::new(
                tie_probability,
                rules.settle(MainBet::Banker, RoundOutcome::Tie)
                    + rebate.rate_for(MainBet::Banker, RoundOutcome::Tie),
            ),
        ]
    } else {
        // Player 和 Tie 的赔付不需要查看庄家最终点数，所以只需要
        // Player、Banker、Tie 三个互斥结果。
        [
            (RoundOutcome::Player, player_probability),
            (RoundOutcome::Banker, banker_probability),
            (RoundOutcome::Tie, tie_probability),
        ]
        .into_iter()
        .map(|(outcome, probability)| {
            KellyOutcome::new(
                probability,
                rules.settle(bet, outcome) + rebate.rate_for(bet, outcome),
            )
        })
        .collect()
    }
}

/// 把边注的命中档位转换为凯利公式使用的完整互斥收益分布。
///
/// 单赔率边注只有“命中赔率”和“未命中 -1”两个结果；幸运 6/7 与龙宝必须
/// 保留每个赔率档位。龙宝还必须保留 Natural 的 `0` 收益 Push；不能用平均
/// 赔率代替这些互斥结果，否则对数增长最优点会产生偏差。
pub fn side_bet_kelly_outcomes(
    weights: SideBetWeights,
    rules: SideBetRules,
    bet: SideBet,
) -> Vec<KellyOutcome> {
    side_bet_kelly_outcomes_with_rebate(weights, rules, RebateRule::None, bet)
}

/// 把边注档位转换为包含返水的完整互斥收益分布。
///
/// 边注返水按实际下注额发放，不依赖最后是赢、输还是 Push，所以每个结果的
/// 单位净收益都加上相同的 `rebate_per_unit`。不能只把返水加到最终 EV 上：
/// 凯利公式需要完整收益分布，遗漏后会得到错误的下注比例。
pub fn side_bet_kelly_outcomes_with_rebate(
    weights: SideBetWeights,
    rules: SideBetRules,
    rebate: RebateRule,
    bet: SideBet,
) -> Vec<KellyOutcome> {
    let total = weights.total_weight() as f64;
    let rebate_per_unit = rebate.rate_for_side_bet();
    let (tier_weights, payouts, push_weight): (Vec<u64>, Vec<f64>, u64) = match bet {
        SideBet::AnyPair
        | SideBet::BankerPair
        | SideBet::PlayerPair
        | SideBet::PerfectPair
        | SideBet::Big
        | SideBet::Small => (
            vec![weights.win_weight(bet)],
            vec![rules.payout(bet).expect("对子边注必须有单一赔付")],
            0,
        ),
        SideBet::LuckySeven => (
            weights.lucky_seven_tier_weights().to_vec(),
            rules.lucky_seven_payouts().to_vec(),
            0,
        ),
        SideBet::SuperLuckySeven => (
            weights.super_lucky_seven_tier_weights().to_vec(),
            rules.super_lucky_seven_payouts().to_vec(),
            0,
        ),
        SideBet::LuckySix => (
            weights.lucky_six_tier_weights().to_vec(),
            rules.lucky_six_payouts().to_vec(),
            0,
        ),
        SideBet::BankerDragonBonus => (
            weights.banker_dragon_bonus_tier_weights().to_vec(),
            rules.dragon_bonus_payouts().to_vec(),
            weights.banker_dragon_bonus_push_weight(),
        ),
        SideBet::PlayerDragonBonus => (
            weights.player_dragon_bonus_tier_weights().to_vec(),
            rules.dragon_bonus_payouts().to_vec(),
            weights.player_dragon_bonus_push_weight(),
        ),
    };
    let win_weight = tier_weights.iter().sum::<u64>();
    let mut outcomes = tier_weights
        .into_iter()
        .zip(payouts)
        .map(|(weight, payout)| KellyOutcome::new(weight as f64 / total, payout + rebate_per_unit))
        .collect::<Vec<_>>();
    if push_weight > 0 {
        outcomes.push(KellyOutcome::new(
            push_weight as f64 / total,
            rebate_per_unit,
        ));
    }
    outcomes.push(KellyOutcome::new(
        (weights.total_weight() - win_weight - push_weight) as f64 / total,
        -1.0 + rebate_per_unit,
    ));
    outcomes
}

/// 对任意互斥收益分布计算凯利比例。
///
/// 如果把资金的 `f` 比例下注，结果 `i` 发生后的资金倍数是
/// `1 + f × net_profit[i]`。凯利公式最大化的是长期对数增长：
///
/// `G(f) = Σ probability[i] × ln(1 + f × net_profit[i])`
///
/// 最大点满足导数为零：
///
/// `G'(f) = Σ probability[i] × net_profit[i] / (1 + f × net_profit[i]) = 0`
///
/// 多结果百家乐没有统一的简单闭式公式，但 `G(f)` 是凹函数，导数单调下降，
/// 因此可以用二分法稳定地找到唯一最大点。
///
/// 可以把二分法理解成“不断试一个下注比例”：
///
/// - 导数大于 0：当前下注太少，继续向更大的比例寻找；
/// - 导数小于 0：当前下注太多，回到更小的比例寻找；
/// - 不断把搜索区间缩小，最终得到最优比例。
///
/// `max_fraction` 是本次计算允许搜索的最大比例。传 `1.0` 表示计算完整
/// 凯利；传 `0.2` 表示即使数学最优值更大，也最多返回 20%。
pub fn calculate_kelly_fraction(
    outcomes: &[KellyOutcome],
    max_fraction: f64,
) -> Result<f64, KellyError> {
    // outcomes 必须是互斥且穷尽的完整收益分布；不能只传“赢的概率”，
    // 因为多档赔率、Push 和返水都会改变最优比例。
    validate_max_fraction(max_fraction)?;
    let effective_ev = validate_outcomes(outcomes)?;

    // G'(0) 就是 EV。EV <= 0 表示从“不下注”开始增加下注比例只会让
    // 长期对数增长变差，因此最优比例直接是 0。
    if max_fraction == 0.0 || effective_ev <= 0.0 {
        return Ok(0.0);
    }

    // 对每个净收益为负的结果，都必须满足 1 + f*x > 0。
    // 例如 x = -1 时要求 f < 1；x = -0.985 时要求 f < 1/0.985。
    let loss_boundary = outcomes
        .iter()
        .filter(|outcome| outcome.probability > 0.0 && outcome.net_profit < 0.0)
        .map(|outcome| -1.0 / outcome.net_profit)
        .fold(f64::INFINITY, f64::min);

    // lower 和 upper 是二分搜索的左右边界。lower 从“不下注”的 0 开始，
    // upper 从允许的最大比例开始，再根据“不能把资金变成零”收紧。
    let mut upper = max_fraction;
    if loss_boundary.is_finite() {
        upper = upper.min(loss_boundary * (1.0 - DOMAIN_MARGIN));
    }

    if upper <= 0.0 {
        return Ok(0.0);
    }

    // 如果到达允许的最大比例时导数仍非负，说明真正最优点在上限之外，
    // 当前风险约束下就直接使用上限。
    if growth_derivative(outcomes, upper) >= 0.0 {
        return Ok(upper);
    }

    let mut lower = 0.0;
    for _ in 0..BISECTION_ITERATIONS {
        let middle = (lower + upper) / 2.0;

        if growth_derivative(outcomes, middle) > 0.0 {
            // 导数仍为正：增加下注比例还能提高增长率，根在右边。
            lower = middle;
        } else {
            // 导数已经为负或为零：下注比例过大或正好到顶，根在左边。
            upper = middle;
        }
    }

    Ok((lower + upper) / 2.0)
}

fn growth_derivative(outcomes: &[KellyOutcome], fraction: f64) -> f64 {
    // 这是 G'(f)。调用者只关心它的正负：正数表示比例还可以增大，
    // 负数表示比例已经过大。无需直接计算 ln，也能找到 G(f) 的最大点。
    // 每个结果的导数贡献都按该结果概率加权；对所有互斥结果求和后，
    // 只需观察总导数正负就能决定二分区间方向。
    outcomes
        .iter()
        .map(|outcome| {
            outcome.probability * outcome.net_profit / (1.0 + fraction * outcome.net_profit)
        })
        .sum()
}

/// 验证分布并返回 `Σ p*x`，也就是该分布的有效 EV。
fn validate_outcomes(outcomes: &[KellyOutcome]) -> Result<f64, KellyError> {
    if outcomes.is_empty() {
        return Err(KellyError::EmptyOutcomes);
    }

    let mut probability_sum = 0.0;
    let mut effective_ev = 0.0;

    for (index, outcome) in outcomes.iter().enumerate() {
        // enumerate() 同时给出数组下标和元素。错误中保存下标后，调用者能
        // 直接知道是第几个结果的数据有问题。
        if !outcome.probability.is_finite() || outcome.probability < 0.0 {
            return Err(KellyError::InvalidProbability {
                index,
                value: outcome.probability,
            });
        }
        if !outcome.net_profit.is_finite() {
            return Err(KellyError::InvalidNetProfit {
                index,
                value: outcome.net_profit,
            });
        }

        // 一次循环同时完成两件事：验证完整概率分布，并计算 Σ(p*x)。
        probability_sum += outcome.probability;
        effective_ev += outcome.probability * outcome.net_profit;
    }

    if (probability_sum - 1.0).abs() > PROBABILITY_TOLERANCE {
        return Err(KellyError::ProbabilitySumNotOne {
            actual: probability_sum,
        });
    }

    Ok(effective_ev)
}

fn validate_max_fraction(value: f64) -> Result<(), KellyError> {
    // is_finite 同时排除 NaN、正无穷和负无穷。比例还必须位于闭区间 0..=1。
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(KellyError::InvalidMaxFraction { value });
    }
    Ok(())
}

fn validate_limit(value: f64, error: KellyError) -> Result<(), KellyError> {
    // 单局上限和桌台上限使用相同验证规则，但需要返回不同错误变体，
    // 因此由调用者把对应的 error 传进来。
    if !value.is_finite() || value < 0.0 {
        return Err(error);
    }
    Ok(())
}

fn validate_bankroll(value: f64) -> Result<(), KellyError> {
    // 与金额上限不同，bankroll 不能为零，因为后面需要计算 amount / bankroll。
    if !value.is_finite() || value <= 0.0 {
        return Err(KellyError::InvalidBankroll { value });
    }
    Ok(())
}

/// 凯利输入或资金配置无效。
#[derive(Debug, Clone, PartialEq)]
pub enum KellyError {
    /// 没有提供任何可能结果，无法形成概率分布。
    EmptyOutcomes,
    /// 第 `index` 个结果的概率为负数、NaN 或无穷大。
    InvalidProbability {
        /// 非法结果在输入切片中的零基下标。
        index: usize,
        /// 调用者实际提供的非法概率。
        value: f64,
    },
    /// 第 `index` 个结果的单位净收益为 NaN 或无穷大。
    InvalidNetProfit {
        /// 非法结果在输入切片中的零基下标。
        index: usize,
        /// 调用者实际提供的非法单位净收益。
        value: f64,
    },
    /// 所有互斥结果的概率之和在允许误差外不等于 1。
    ProbabilitySumNotOne {
        /// 所有输入概率实际相加得到的值。
        actual: f64,
    },
    /// 最大资金比例不是 `0.0..=1.0` 内的有限值。
    InvalidMaxFraction {
        /// 调用者实际提供的最大资金比例。
        value: f64,
    },
    /// 固定下注金额不是有限非负数。
    InvalidFixedStake {
        /// 调用者提供的固定金额。
        value: f64,
    },
    /// 某个可配置金额策略收到不在允许范围内的参数。
    InvalidStrategyParameter {
        /// 稳定策略名称。
        strategy: &'static str,
        /// 调用者提供的非法数值。
        value: f64,
        /// 该策略要求的范围说明。
        expected: &'static str,
    },
    /// 系统单局金额上限不是有限非负数。
    InvalidMaxRoundStake {
        /// 调用者实际提供的单局金额上限。
        value: f64,
    },
    /// 赌场桌台上限不是有限非负数。
    InvalidTableLimit {
        /// 调用者实际提供的桌台金额上限。
        value: f64,
    },
    /// 边注单笔上限不是有限非负数。
    InvalidSideBetLimit {
        /// 调用者实际提供的边注金额上限。
        value: f64,
    },
    /// 可管理资金不是有限正数。
    InvalidBankroll {
        /// 调用者实际提供的可管理资金。
        value: f64,
    },
}

impl fmt::Display for KellyError {
    /// 把适合程序匹配的错误变体转换成人类可以阅读的错误文本。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyOutcomes => formatter.write_str("at least one Kelly outcome is required"),
            Self::InvalidProbability { index, value } => {
                write!(formatter, "outcome {index} has invalid probability {value}")
            }
            Self::InvalidNetProfit { index, value } => {
                write!(formatter, "outcome {index} has invalid net profit {value}")
            }
            Self::ProbabilitySumNotOne { actual } => {
                write!(
                    formatter,
                    "outcome probabilities sum to {actual}; expected 1"
                )
            }
            Self::InvalidMaxFraction { value } => {
                write!(
                    formatter,
                    "max Kelly fraction must be in 0..=1; got {value}"
                )
            }
            Self::InvalidFixedStake { value } => {
                write!(
                    formatter,
                    "fixed stake must be finite and non-negative; got {value}"
                )
            }
            Self::InvalidStrategyParameter {
                strategy,
                value,
                expected,
            } => write!(
                formatter,
                "strategy {strategy} requires {expected}; got {value}"
            ),
            Self::InvalidMaxRoundStake { value } => {
                write!(
                    formatter,
                    "max round stake must be finite and non-negative; got {value}"
                )
            }
            Self::InvalidTableLimit { value } => {
                write!(
                    formatter,
                    "table limit must be finite and non-negative; got {value}"
                )
            }
            Self::InvalidSideBetLimit { value } => {
                write!(
                    formatter,
                    "side bet limit must be finite and non-negative; got {value}"
                )
            }
            Self::InvalidBankroll { value } => {
                write!(
                    formatter,
                    "bankroll must be finite and positive; got {value}"
                )
            }
        }
    }
}

impl Error for KellyError {}

#[cfg(test)]
mod tests {
    use super::{
        BetPlanAction, BetPlanSkipReason, CombinedBetPlanAction, KellyOutcome, KellyPolicy,
        StakeSizingStrategy, calculate_kelly_fraction, main_bet_kelly_outcomes,
        side_bet_kelly_outcomes, side_bet_kelly_outcomes_with_rebate,
    };
    use crate::{
        BetTarget, BettingPolicy, MainBet, MainBetAnalysis, MainBetRules, OutcomeWeights,
        RebateRule, Shoe, SideBet, SideBetAnalysis, SideBetRules, SideBetWeights,
        calculate_main_outcomes,
    };

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
    }

    fn sample_weights() -> OutcomeWeights {
        OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应该构成完整分布")
    }

    #[test]
    fn binary_even_money_example_returns_ten_percent() {
        let outcomes = [KellyOutcome::new(0.55, 1.0), KellyOutcome::new(0.45, -1.0)];

        // 1:1 赔率下，简化公式为 p-q，因此 55% 对 45% 的完整凯利是 10%。
        let fraction = calculate_kelly_fraction(&outcomes, 1.0).expect("分布合法");
        assert_close(fraction, 0.10);
    }

    #[test]
    fn non_positive_ev_never_places_capital_at_risk() {
        let outcomes = [KellyOutcome::new(0.50, 1.0), KellyOutcome::new(0.50, -1.0)];

        assert_eq!(
            calculate_kelly_fraction(&outcomes, 1.0).expect("分布合法"),
            0.0
        );
    }

    #[test]
    fn push_probability_is_part_of_the_full_distribution() {
        let outcomes = [
            KellyOutcome::new(0.55, 1.0),
            KellyOutcome::new(0.40, -1.0),
            KellyOutcome::new(0.05, 0.0),
        ];

        let fraction = calculate_kelly_fraction(&outcomes, 1.0).expect("分布合法");
        assert_close(fraction, (0.55 - 0.40) / (0.55 + 0.40));
    }

    #[test]
    fn baccarat_outcome_ev_matches_the_existing_effective_ev_layer() {
        let weights = OutcomeWeights::from_detailed_weights(6, 360, 240, 120, 60)
            .expect("测试权重应该构成完整分布");
        let rebate = RebateRule::AllExceptMainBetTie { rate: 0.015 };

        for rules in [MainBetRules::standard(), MainBetRules::no_commission()] {
            let analysis = MainBetAnalysis::from_weights(weights, rules);

            for bet in [MainBet::Player, MainBet::Banker, MainBet::Tie] {
                let outcomes = main_bet_kelly_outcomes(weights, rules, rebate, bet);
                let outcome_ev: f64 = outcomes
                    .iter()
                    .map(|outcome| outcome.probability() * outcome.net_profit())
                    .sum();

                assert_close(
                    outcome_ev,
                    analysis.effective_metrics(bet, rebate).effective_ev(),
                );
            }
        }
    }

    #[test]
    fn banker_outcomes_keep_six_point_half_payout_separate() {
        let weights = OutcomeWeights::from_detailed_weights(6, 360, 240, 120, 60)
            .expect("测试权重应该构成完整分布");
        let outcomes = main_bet_kelly_outcomes(
            weights,
            MainBetRules::no_commission(),
            RebateRule::None,
            MainBet::Banker,
        );

        assert_eq!(outcomes.len(), 4);
        assert_close(outcomes[1].probability(), 0.25);
        assert_close(outcomes[1].net_profit(), 1.0);
        assert_close(outcomes[2].probability(), 1.0 / 12.0);
        assert_close(outcomes[2].net_profit(), 0.5);
    }

    #[test]
    fn plan_uses_direction_policy_then_applies_money_limits() {
        let weights = sample_weights();
        let betting_policy = BettingPolicy::new(RebateRule::None, 0.0);
        let kelly_policy = KellyPolicy::full(40.0, 50.0).expect("资金上限合法");

        let plan = kelly_policy
            .plan(&betting_policy, weights, MainBetRules::standard(), 1_000.0)
            .expect("应生成下注计划");
        let quote = plan.quote().expect("通过 EV 门槛后应该有凯利报价");

        // 此测试分布下 Tie 的 EV 为 0.5，完整凯利比例为 1/16。
        // 1000 × 1/16 = 62.5，但单局上限 40 更低，因此最终下 40。
        assert_eq!(quote.bet(), BetTarget::Main(MainBet::Tie));
        assert_close(quote.effective_ev(), 0.5);
        assert_close(quote.kelly_fraction(), 1.0 / 16.0);
        assert_close(quote.applied_fraction(), 0.04);
        assert_close(quote.amount(), 40.0);
        assert_close(quote.expected_profit(), 20.0);

        match plan.action() {
            BetPlanAction::Place { bet, amount } => {
                assert_eq!(*bet, MainBet::Tie);
                assert_close(*amount, 40.0);
            }
            BetPlanAction::Skip { .. } => panic!("正 EV 且金额上限大于零时应该下注"),
        }
    }

    #[test]
    fn plan_preserves_a_strategy_skip() {
        let betting_policy = BettingPolicy::new(RebateRule::None, 0.6);
        let kelly_policy = KellyPolicy::full(100.0, 100.0).expect("资金上限合法");

        let plan = kelly_policy
            .plan(
                &betting_policy,
                sample_weights(),
                MainBetRules::standard(),
                1_000.0,
            )
            .expect("应生成跳过计划");

        assert!(plan.quote().is_none());
        assert!(matches!(
            plan.action(),
            BetPlanAction::Skip {
                reason: BetPlanSkipReason::Strategy(_)
            }
        ));
    }

    #[test]
    fn complete_eight_deck_shoe_with_rebate_produces_a_positive_banker_plan() {
        let weights = calculate_main_outcomes(&Shoe::default()).expect("完整八副牌应该能够枚举");
        let betting_policy =
            BettingPolicy::new(RebateRule::AllExceptMainBetTie { rate: 0.015 }, 0.0);
        let kelly_policy = KellyPolicy::full(10_000.0, 10_000.0).expect("资金上限合法");

        let plan = kelly_policy
            .plan(&betting_policy, weights, MainBetRules::standard(), 10_000.0)
            .expect("完整牌靴应该生成下注计划");
        let quote = plan.quote().expect("1.5% 返水下应该通过零 EV 门槛");

        assert_eq!(quote.bet(), BetTarget::Main(MainBet::Banker));
        assert!(quote.effective_ev() > 0.0);
        assert!(quote.kelly_fraction() > 0.0);
        assert!(quote.amount() > 0.0);
        assert!(matches!(
            plan.action(),
            BetPlanAction::Place {
                bet: MainBet::Banker,
                amount
            } if *amount > 0.0
        ));
    }

    #[test]
    fn fractional_kelly_scales_the_target_before_limits_are_applied() {
        let weights = sample_weights();
        let full =
            KellyPolicy::with_strategy(StakeSizingStrategy::FullKelly, 1.0, 10_000.0, 10_000.0)
                .expect("完整凯利配置合法")
                .quote(
                    weights,
                    MainBetRules::standard(),
                    RebateRule::None,
                    MainBet::Tie,
                    1_000.0,
                )
                .expect("完整凯利报价合法");
        let half =
            KellyPolicy::with_strategy(StakeSizingStrategy::HalfKelly, 1.0, 10_000.0, 10_000.0)
                .expect("半凯利配置合法")
                .quote(
                    weights,
                    MainBetRules::standard(),
                    RebateRule::None,
                    MainBet::Tie,
                    1_000.0,
                )
                .expect("半凯利报价合法");

        assert_close(half.kelly_fraction(), full.kelly_fraction());
        assert_close(half.strategy_fraction(), full.kelly_fraction() * 0.5);
        assert_close(half.amount(), full.amount() * 0.5);
    }

    #[test]
    fn custom_kelly_accepts_an_arbitrary_safe_fraction() {
        let weights = sample_weights();
        let full =
            KellyPolicy::with_strategy(StakeSizingStrategy::FullKelly, 1.0, 10_000.0, 10_000.0)
                .expect("完整凯利配置合法")
                .quote(
                    weights,
                    MainBetRules::standard(),
                    RebateRule::None,
                    MainBet::Tie,
                    1_000.0,
                )
                .expect("完整凯利报价合法");
        let custom = KellyPolicy::with_strategy(
            StakeSizingStrategy::CustomKelly { fraction: 0.3 },
            1.0,
            10_000.0,
            10_000.0,
        )
        .expect("三成凯利配置合法")
        .quote(
            weights,
            MainBetRules::standard(),
            RebateRule::None,
            MainBet::Tie,
            1_000.0,
        )
        .expect("三成凯利报价合法");

        assert_close(custom.strategy_fraction(), full.kelly_fraction() * 0.3);
        assert_close(custom.amount(), full.amount() * 0.3);
    }

    #[test]
    fn non_kelly_strategies_convert_their_objective_into_a_stake() {
        let weights = sample_weights();
        let fixed_fraction = KellyPolicy::with_strategy(
            StakeSizingStrategy::FixedBankrollFraction { fraction: 0.02 },
            1.0,
            10_000.0,
            10_000.0,
        )
        .expect("固定本金比例配置合法")
        .quote(
            weights,
            MainBetRules::standard(),
            RebateRule::None,
            MainBet::Tie,
            1_000.0,
        )
        .expect("固定本金比例报价合法");
        assert_close(fixed_fraction.strategy_fraction(), 0.02);
        assert_close(fixed_fraction.amount(), 20.0);

        // 测试分布中的和注 EV 为 0.5。若目标期望盈利是 10，所需下注额为
        // 10 / 0.5 = 20，因此同样占 1000 本金的 2%。
        let target_profit = KellyPolicy::with_strategy(
            StakeSizingStrategy::TargetExpectedProfit { amount: 10.0 },
            1.0,
            10_000.0,
            10_000.0,
        )
        .expect("目标期望盈利配置合法")
        .quote(
            weights,
            MainBetRules::standard(),
            RebateRule::None,
            MainBet::Tie,
            1_000.0,
        )
        .expect("目标期望盈利报价合法");
        assert_close(target_profit.strategy_fraction(), 0.02);
        assert_close(target_profit.amount(), 20.0);

        let target_volatility = KellyPolicy::with_strategy(
            StakeSizingStrategy::TargetVolatility { fraction: 0.01 },
            1.0,
            10_000.0,
            10_000.0,
        )
        .expect("目标波动率配置合法")
        .quote(
            weights,
            MainBetRules::standard(),
            RebateRule::None,
            MainBet::Tie,
            1_000.0,
        )
        .expect("目标波动率报价合法");
        assert!(target_volatility.amount() > 0.0);
        assert!(target_volatility.amount() < 1_000.0);
    }

    #[test]
    fn percentage_strategy_parameters_must_be_between_zero_and_one() {
        assert!(
            KellyPolicy::with_strategy(
                StakeSizingStrategy::FixedBankrollFraction { fraction: 1.01 },
                1.0,
                100.0,
                100.0,
            )
            .is_err()
        );
        assert!(
            KellyPolicy::with_strategy(
                StakeSizingStrategy::CustomKelly { fraction: f64::NAN },
                1.0,
                100.0,
                100.0,
            )
            .is_err()
        );
    }

    #[test]
    fn fixed_stake_is_clipped_by_the_same_risk_limits() {
        let quote = KellyPolicy::with_strategy(
            StakeSizingStrategy::Fixed { amount: 100.0 },
            1.0,
            80.0,
            90.0,
        )
        .expect("固定金额配置合法")
        .quote(
            sample_weights(),
            MainBetRules::standard(),
            RebateRule::None,
            MainBet::Tie,
            1_000.0,
        )
        .expect("固定金额报价合法");

        assert_close(quote.strategy_fraction(), 0.1);
        assert_close(quote.applied_fraction(), 0.08);
        assert_close(quote.amount(), 80.0);
    }

    #[test]
    fn invalid_fixed_stake_is_rejected_at_configuration_time() {
        assert!(
            KellyPolicy::with_strategy(
                StakeSizingStrategy::Fixed { amount: -1.0 },
                1.0,
                100.0,
                100.0,
            )
            .is_err()
        );
    }

    #[test]
    fn side_bet_kelly_uses_tier_distribution_and_the_side_limit() {
        let weights = SideBetWeights::new(
            100, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, [0; 6], 0, [0; 6], 0,
        );
        let rules = SideBetRules::default();
        let outcomes = side_bet_kelly_outcomes(weights, rules, SideBet::AnyPair);
        let analysis = SideBetAnalysis::calculate(weights, rules);
        let outcome_ev = outcomes
            .iter()
            .map(|outcome| outcome.probability() * outcome.net_profit())
            .sum::<f64>();
        assert_close(outcome_ev, analysis.metrics(SideBet::AnyPair).ev());

        let quote = KellyPolicy::full(500.0, 1_000.0)
            .and_then(|policy| policy.with_side_bet_limit(25.0))
            .expect("边注限额应该合法")
            .quote_side(weights, rules, SideBet::AnyPair, 1_000.0)
            .expect("正 EV 边注应该得到凯利报价");

        assert_eq!(quote.bet(), BetTarget::Side(SideBet::AnyPair));
        assert_close(quote.amount(), 25.0);
    }

    #[test]
    fn side_bet_kelly_distribution_adds_rebate_to_every_outcome() {
        let weights = SideBetWeights::new(
            100, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, [0; 6], 0, [0; 6], 0,
        );
        let rules = SideBetRules::default();
        let rebate = RebateRule::AllExceptMainBetTie { rate: 0.02 };
        let base = side_bet_kelly_outcomes(weights, rules, SideBet::AnyPair);
        let with_rebate =
            side_bet_kelly_outcomes_with_rebate(weights, rules, rebate, SideBet::AnyPair);

        assert_eq!(base.len(), with_rebate.len());
        for (base_outcome, rebate_outcome) in base.iter().zip(&with_rebate) {
            assert_close(base_outcome.probability(), rebate_outcome.probability());
            assert_close(
                rebate_outcome.net_profit(),
                base_outcome.net_profit() + 0.02,
            );
        }

        let analysis = SideBetAnalysis::calculate(weights, rules);
        let effective_ev = with_rebate
            .iter()
            .map(|outcome| outcome.probability() * outcome.net_profit())
            .sum::<f64>();
        assert_close(effective_ev, analysis.metrics(SideBet::AnyPair).ev() + 0.02);
    }

    #[test]
    fn multiple_plans_share_the_common_round_risk_limit() {
        let side_weights = SideBetWeights::new(
            100, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, [0; 6], 0, [0; 6], 0,
        );
        let betting_policy = BettingPolicy::with_side_bet_minimum(RebateRule::None, 0.2, 0.0);
        let kelly_policy = KellyPolicy::with_strategy(
            StakeSizingStrategy::Fixed { amount: 100.0 },
            1.0,
            100.0,
            1_000.0,
        )
        .and_then(|policy| policy.with_side_bet_limit(100.0))
        .expect("多注测试的金额配置应该合法");

        let plans = kelly_policy
            .plan_all_multiple_with_side_bet_filter(
                &betting_policy,
                sample_weights(),
                MainBetRules::standard(),
                side_weights,
                SideBetRules::default(),
                1_000.0,
                |_| true,
            )
            .expect("应该为所有合格目标生成计划");

        assert_eq!(plans.len(), 2);
        let total: f64 = plans
            .iter()
            .map(|plan| match plan.action() {
                CombinedBetPlanAction::Place { amount, .. } => *amount,
                CombinedBetPlanAction::Skip { .. } => 0.0,
            })
            .sum();
        assert_close(total, 100.0);
        for plan in plans {
            assert!(matches!(
                plan.action(),
                CombinedBetPlanAction::Place { amount, .. } if (*amount - 50.0).abs() < 1e-10
            ));
        }
    }

    #[test]
    fn dragon_bonus_kelly_distribution_keeps_natural_push_and_matches_ev() {
        let weights = SideBetWeights::new(
            100,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            [10, 5, 3, 2, 1, 1],
            8,
            [0; 6],
            0,
        );
        let rules = SideBetRules::default();
        let outcomes = side_bet_kelly_outcomes(weights, rules, SideBet::BankerDragonBonus);
        let analysis = SideBetAnalysis::calculate(weights, rules);
        let outcome_ev = outcomes
            .iter()
            .map(|outcome| outcome.probability() * outcome.net_profit())
            .sum::<f64>();

        assert_close(
            outcome_ev,
            analysis.metrics(SideBet::BankerDragonBonus).ev(),
        );
        assert!(outcomes.iter().any(|outcome| {
            outcome.net_profit() == 0.0 && (outcome.probability() - 0.08).abs() < 1e-12
        }));
    }
}
