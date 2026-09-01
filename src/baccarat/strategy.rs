//! 有效 EV 下注方向策略。
//!
//! 本模块只决定“是否下注”和“下注哪个方向”，不决定下注金额。
//! 下注金额由 `risk` 模块中的 `KellyPolicy` 根据这里的决策继续计算。
//!
//! 这里的“有效 EV”是方向策略唯一比较的数值：
//!
//! ```text
//! 基础概率 + 赔付规则 -> base EV
//! 可能结果 + 返水规则 -> rebate EV
//! base EV + rebate EV -> effective EV
//! effective EV >= 对应门槛 -> Place，否则 Skip
//! ```
//!
//! 因此“最优”与“值得下注”是两个不同问题：先在候选中找有效 EV 最大者，
//! 再检查它是否达到门槛；多注模式则返回所有达到各自门槛的候选，而不是只保留第一名。

use crate::baccarat::RebateRule;
use crate::{MainBet, MainBetAnalysis, SideBet, SideBetAnalysis};

/// 自动策略能够选择的完整下注目标。
///
/// 用一个枚举统一主注和边注后，策略、凯利金额、JSON 与 CSV 回放都可以传递
/// 同一个明确类型，避免依靠字符串猜测当前到底是哪一种玩法。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BetTarget {
    /// 闲、庄、和中的一种主注。
    Main(MainBet),
    /// 任意对子、庄对、闲对、完美对子、大、小、幸运 7 或超级幸运 7。
    Side(SideBet),
}

impl BetTarget {
    /// 返回供 JSON、日志和前端使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main(bet) => bet.as_str(),
            Self::Side(bet) => bet.as_str(),
        }
    }

    /// 判断当前目标是否为边注，资金层据此应用单独的边注金额上限。
    pub const fn is_side(self) -> bool {
        matches!(self, Self::Side(_))
    }
}

/// 方向选择配置。
///
/// `rebate` 会加入主注和边注的基础 EV；`minimum_effective_ev` 是主注门槛，
/// `minimum_side_bet_ev` 是边注门槛。即使某个方向在全部目标中最优，如果仍
/// 低于对应门槛，也会选择 Skip。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BettingPolicy {
    /// 当前玩家或渠道适用的返水规则。
    rebate: RebateRule,
    /// 允许下注所需达到的最低有效 EV。
    minimum_effective_ev: f64,
    /// 十一种边注各自必须达到的最低有效 EV，已经包含边注返水。
    minimum_side_bet_ev: f64,
}

/// 主注与边注共同参与比较时的方向动作。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CombinedBetAction {
    /// 至少一个目标达到自己的 EV 门槛，选择其中 EV 最大者。
    Place { bet: BetTarget },
    /// 十四个目标都没有达到各自门槛。
    Skip { reason: SkipReason },
}

/// 十四种下注目标共同比较后的可审计结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombinedBetDecision {
    /// 这次比较中有效 EV 最高的候选，若无目标达标也保留它用于解释 Skip。
    candidate: BetTarget,
    /// 候选目标不含返水的基础 EV。
    base_ev: f64,
    /// 候选目标由返水带来的期望贡献。
    rebate_ev: f64,
    /// `base_ev + rebate_ev`，实际用于排序和门槛比较。
    effective_ev: f64,
    /// 候选所属类别的最低有效 EV 门槛。
    minimum_ev: f64,
    /// 该候选最终是 Place 还是 Skip。
    action: CombinedBetAction,
}

/// 策略比较阶段的统一候选记录。
///
/// 主注和边注的原始指标结构不同，但进入策略后都可以抽象成这五个字段。
/// 先统一成该类型，后面的“过滤门槛、比较 EV、排序”就不需要分别写两套逻辑。
#[derive(Debug, Clone, Copy)]
struct CandidateMetrics {
    target: BetTarget,
    base_ev: f64,
    rebate_ev: f64,
    effective_ev: f64,
    minimum_ev: f64,
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
            minimum_side_bet_ev: minimum_effective_ev,
        }
    }

    /// 创建主注和边注使用不同 EV 门槛的统一策略。
    pub const fn with_side_bet_minimum(
        rebate: RebateRule,
        minimum_effective_ev: f64,
        minimum_side_bet_ev: f64,
    ) -> Self {
        Self {
            rebate,
            minimum_effective_ev,
            minimum_side_bet_ev,
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

    /// 返回十一种边注共同使用的最低 EV 门槛。
    pub const fn minimum_side_bet_ev(&self) -> f64 {
        self.minimum_side_bet_ev
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

    /// 同时比较三种主注和十一种边注，并选择达到各自门槛后的最高 EV。
    ///
    /// 这里先过滤门槛再比较，避免一个“EV 虽高但没有达到更严格边注门槛”的
    /// 候选挡住另一个已经满足主注门槛的可下注方向。
    pub fn decide_all(
        &self,
        main_analysis: MainBetAnalysis,
        side_analysis: SideBetAnalysis,
    ) -> CombinedBetDecision {
        self.decide_all_with_side_bet_filter(main_analysis, side_analysis, |_| true)
    }

    /// 在比较全部下注方向前，先用调用方规则过滤暂时不可下注的边注。
    ///
    /// `allows_side_bet` 只控制边注候选是否进入本局比较；庄、闲、和三种主注
    /// 始终保留。CSV 回放用它实现“超过指定局数后禁用幸运 6/7”，同时不会
    /// 因为幸运边注被禁用而错误跳过其他仍然合法的下注方向。
    pub fn decide_all_with_side_bet_filter<F>(
        &self,
        main_analysis: MainBetAnalysis,
        side_analysis: SideBetAnalysis,
        allows_side_bet: F,
    ) -> CombinedBetDecision
    where
        F: Fn(SideBet) -> bool,
    {
        // 主注和边注的门槛可能不同，所以不能先把所有 EV 排序再统一判断；
        // 每个候选都要携带自己的 minimum_ev，只有达标者才进入 best_eligible。
        let main_candidates = [MainBet::Player, MainBet::Banker, MainBet::Tie].map(|bet| {
            let metrics = main_analysis.effective_metrics(bet, self.rebate);
            CandidateMetrics {
                target: BetTarget::Main(bet),
                base_ev: metrics.base_ev(),
                rebate_ev: metrics.rebate_ev(),
                effective_ev: metrics.effective_ev(),
                minimum_ev: self.minimum_effective_ev,
            }
        });

        // best_overall 用于“全部不达标”时解释为什么跳过；它不代表一定可以下注。
        let mut best_overall = main_candidates[0];
        let mut best_eligible: Option<CandidateMetrics> = None;

        let side_candidates = SideBet::ALL
            .into_iter()
            .filter(|bet| allows_side_bet(*bet))
            .map(|bet| {
                let metrics = side_analysis.metrics(bet);
                let base_ev = metrics.ev();
                // 当前业务规则下所有边注结果都按下注额返水，所以它对边注 EV
                // 的贡献就是返水率本身，不需要再按输赢结果拆分加权。
                let rebate_ev = self.rebate.rate_for_side_bet();
                CandidateMetrics {
                    target: BetTarget::Side(bet),
                    base_ev,
                    rebate_ev,
                    effective_ev: base_ev + rebate_ev,
                    minimum_ev: self.minimum_side_bet_ev,
                }
            });

        // 这里一次遍历完成两件事：保存无门槛的最大值用于 Skip，
        // 保存达标候选的最大值用于 Place。这样被禁用的边注不会影响其他目标。
        for candidate in main_candidates.into_iter().chain(side_candidates) {
            if candidate.effective_ev > best_overall.effective_ev {
                best_overall = candidate;
            }
            if candidate.effective_ev >= candidate.minimum_ev
                && best_eligible.is_none_or(|current| candidate.effective_ev > current.effective_ev)
            {
                best_eligible = Some(candidate);
            }
        }

        // 有达标候选时返回它；没有时退回 best_overall，并把门槛差距写入原因。
        let (candidate, action) = if let Some(candidate) = best_eligible {
            (
                candidate,
                CombinedBetAction::Place {
                    bet: candidate.target,
                },
            )
        } else {
            (
                best_overall,
                CombinedBetAction::Skip {
                    reason: SkipReason::BelowMinimumEv {
                        effective_ev: best_overall.effective_ev,
                        minimum_ev: best_overall.minimum_ev,
                    },
                },
            )
        };

        CombinedBetDecision {
            candidate: candidate.target,
            base_ev: candidate.base_ev,
            rebate_ev: candidate.rebate_ev,
            effective_ev: candidate.effective_ev,
            minimum_ev: candidate.minimum_ev,
            action,
        }
    }

    /// 返回所有达到各自 EV 门槛的下注目标。
    ///
    /// 结果按有效 EV 从高到低排列，因此第一个目标仍然是原来单注模式下
    /// 的“最优目标”。开启多注模式时，调用方可以继续使用后面的合格目标。
    pub fn eligible_all(
        &self,
        main_analysis: MainBetAnalysis,
        side_analysis: SideBetAnalysis,
    ) -> Vec<CombinedBetDecision> {
        self.eligible_all_with_side_bet_filter(main_analysis, side_analysis, |_| true)
    }

    /// 返回通过 EV 门槛且没有被临时边注过滤器移除的全部目标。
    pub fn eligible_all_with_side_bet_filter<F>(
        &self,
        main_analysis: MainBetAnalysis,
        side_analysis: SideBetAnalysis,
        allows_side_bet: F,
    ) -> Vec<CombinedBetDecision>
    where
        F: Fn(SideBet) -> bool,
    {
        // 多注模式不能沿用 decide_all 的“只保存一个 best”逻辑，先把所有
        // 候选收集起来，再逐个过滤并按 effective EV 排序。
        let mut candidates = Vec::with_capacity(3 + SideBet::ALL.len());

        for bet in [MainBet::Player, MainBet::Banker, MainBet::Tie] {
            let metrics = main_analysis.effective_metrics(bet, self.rebate);
            candidates.push(CandidateMetrics {
                target: BetTarget::Main(bet),
                base_ev: metrics.base_ev(),
                rebate_ev: metrics.rebate_ev(),
                effective_ev: metrics.effective_ev(),
                minimum_ev: self.minimum_effective_ev,
            });
        }

        for bet in SideBet::ALL {
            if !allows_side_bet(bet) {
                continue;
            }
            let metrics = side_analysis.metrics(bet);
            let base_ev = metrics.ev();
            let rebate_ev = self.rebate.rate_for_side_bet();
            candidates.push(CandidateMetrics {
                target: BetTarget::Side(bet),
                base_ev,
                rebate_ev,
                effective_ev: base_ev + rebate_ev,
                minimum_ev: self.minimum_side_bet_ev,
            });
        }

        let mut decisions: Vec<_> = candidates
            .into_iter()
            .filter(|candidate| candidate.effective_ev >= candidate.minimum_ev)
            .map(|candidate| CombinedBetDecision {
                candidate: candidate.target,
                base_ev: candidate.base_ev,
                rebate_ev: candidate.rebate_ev,
                effective_ev: candidate.effective_ev,
                minimum_ev: candidate.minimum_ev,
                action: CombinedBetAction::Place {
                    bet: candidate.target,
                },
            })
            .collect();

        // `total_cmp` 对有限 EV 和异常浮点值都有确定顺序，避免排序过程中
        // 因 `partial_cmp(...).unwrap()` 发生崩溃。正常输入下 EV 都是有限值。
        decisions.sort_by(|left, right| right.effective_ev.total_cmp(&left.effective_ev));
        decisions
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

impl CombinedBetDecision {
    /// 返回比较结果中的候选下注目标。
    pub const fn candidate(self) -> BetTarget {
        self.candidate
    }

    /// 返回候选目标不含返水的基础 EV。
    pub const fn base_ev(self) -> f64 {
        self.base_ev
    }

    /// 返回返水对候选目标的 EV 贡献。
    pub const fn rebate_ev(self) -> f64 {
        self.rebate_ev
    }

    /// 返回包含返水、实际用于策略判断的有效 EV。
    pub const fn effective_ev(self) -> f64 {
        self.effective_ev
    }

    /// 返回本候选需要达到的最低有效 EV。
    pub const fn minimum_ev(self) -> f64 {
        self.minimum_ev
    }

    /// 借用最终动作，避免读取时移动整个决策对象。
    pub const fn action(&self) -> &CombinedBetAction {
        &self.action
    }
}

#[cfg(test)]
mod tests {
    use super::{BetAction, BetTarget, BettingPolicy, CombinedBetAction, SkipReason};
    use crate::{
        MainBet, MainBetAnalysis, MainBetRules, OutcomeWeights, RebateRule, SideBet,
        SideBetAnalysis, SideBetRules, SideBetWeights,
    };

    fn sample_analysis() -> MainBetAnalysis {
        let weights =
            OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应该构成完整概率分布");

        MainBetAnalysis::from_weights(weights, MainBetRules::standard())
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }

    fn side_analysis_with_positive_any_pair() -> SideBetAnalysis {
        let weights = SideBetWeights::new(
            100, 30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, [0; 6], 0, [0; 6], 0,
        );
        SideBetAnalysis::calculate(weights, SideBetRules::default())
    }

    fn side_analysis_with_positive_lucky_six() -> SideBetAnalysis {
        // 20% 命中幸运 6 两张档位，在 12:1 净赔付下 EV 为 1.6，
        // 足以超过 sample_analysis() 中 EV 为 0.5 的和注。
        let weights = SideBetWeights::new(
            100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 20, 0, [0; 6], 0, [0; 6], 0,
        );
        SideBetAnalysis::calculate(weights, SideBetRules::default())
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

    #[test]
    fn decide_all_can_select_a_side_bet_over_all_main_bets() {
        let policy = BettingPolicy::with_side_bet_minimum(RebateRule::None, 0.0, 0.0);
        let decision = policy.decide_all(sample_analysis(), side_analysis_with_positive_any_pair());

        assert_eq!(decision.candidate(), BetTarget::Side(SideBet::AnyPair));
        assert_close(decision.effective_ev(), 0.8);
        assert!(matches!(
            decision.action(),
            CombinedBetAction::Place {
                bet: BetTarget::Side(SideBet::AnyPair)
            }
        ));
    }

    #[test]
    fn side_bet_effective_ev_includes_rebate() {
        let rebate = RebateRule::AllExceptMainBetTie { rate: 0.015 };
        let policy = BettingPolicy::with_side_bet_minimum(rebate, 10.0, 0.0);
        let decision = policy.decide_all(sample_analysis(), side_analysis_with_positive_any_pair());

        assert_eq!(decision.candidate(), BetTarget::Side(SideBet::AnyPair));
        assert_close(decision.base_ev(), 0.8);
        assert_close(decision.rebate_ev(), 0.015);
        assert_close(decision.effective_ev(), 0.815);
    }

    #[test]
    fn decide_all_includes_lucky_six_in_the_same_strategy_comparison() {
        let policy = BettingPolicy::with_side_bet_minimum(RebateRule::None, 0.0, 0.0);
        let decision =
            policy.decide_all(sample_analysis(), side_analysis_with_positive_lucky_six());

        assert_eq!(decision.candidate(), BetTarget::Side(SideBet::LuckySix));
        assert_close(decision.effective_ev(), 1.6);
        assert!(matches!(
            decision.action(),
            CombinedBetAction::Place {
                bet: BetTarget::Side(SideBet::LuckySix)
            }
        ));
    }

    #[test]
    fn decide_all_filter_removes_lucky_bets_without_blocking_other_targets() {
        let policy = BettingPolicy::with_side_bet_minimum(RebateRule::None, 0.0, 0.0);
        let decision = policy.decide_all_with_side_bet_filter(
            sample_analysis(),
            side_analysis_with_positive_lucky_six(),
            |bet| {
                !matches!(
                    bet,
                    SideBet::LuckySix | SideBet::LuckySeven | SideBet::SuperLuckySeven
                )
            },
        );

        // 幸运 6 原本以 EV=1.6 胜出；过滤后策略继续比较其他方向，
        // 因此改选这组测试数据中 EV=0.5 的和注，而不是整局停止下注。
        assert_eq!(decision.candidate(), BetTarget::Main(MainBet::Tie));
        assert_close(decision.effective_ev(), 0.5);
        assert!(matches!(
            decision.action(),
            CombinedBetAction::Place {
                bet: BetTarget::Main(MainBet::Tie)
            }
        ));
    }

    #[test]
    fn eligible_all_returns_every_target_that_reaches_its_threshold() {
        let policy = BettingPolicy::with_side_bet_minimum(RebateRule::None, 0.2, 0.0);
        let decisions =
            policy.eligible_all(sample_analysis(), side_analysis_with_positive_any_pair());

        assert_eq!(decisions[0].candidate(), BetTarget::Side(SideBet::AnyPair));
        assert_eq!(decisions[1].candidate(), BetTarget::Main(MainBet::Tie));
        assert_eq!(decisions.len(), 2);
        assert!(
            decisions
                .iter()
                .all(|decision| matches!(decision.action(), CombinedBetAction::Place { .. }))
        );
    }

    #[test]
    fn stricter_side_threshold_does_not_block_an_eligible_main_bet() {
        let policy = BettingPolicy::with_side_bet_minimum(RebateRule::None, 0.5, 0.9);
        let decision = policy.decide_all(sample_analysis(), side_analysis_with_positive_any_pair());

        // 任意对子 EV=0.8 高于和注 EV=0.5，但没有通过边注 0.9 门槛；
        // 和注已经通过主注 0.5 门槛，所以策略应下注和，而不是整局跳过。
        assert_eq!(decision.candidate(), BetTarget::Main(MainBet::Tie));
        assert!(matches!(
            decision.action(),
            CombinedBetAction::Place {
                bet: BetTarget::Main(MainBet::Tie)
            }
        ));
    }
}
