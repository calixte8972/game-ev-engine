//! 面向 Python、JSON 和数据库的稳定决策快照。
//!
//! 核心领域类型主要为 Rust 内部计算服务。例如 `BetPlanAction` 以后可能增加
//! 新字段，`MainBet` 也可能增加更多行为。如果直接把这些内部类型序列化，
//! 一次普通重构就可能意外改变 Python 或数据库看到的 JSON。
//!
//! 本模块使用 DTO（Data Transfer Object，数据传输对象）隔离这条边界：
//!
//! ```text
//! Shoe + 规则 + 策略 + bankroll
//!   ↓ analyze_snapshot()
//! 内部概率、EV、BetDecision、BetPlan
//!   ↓ 只提取稳定字段
//! DecisionSnapshot
//!   ↓ serde_json
//! JSON / Python / 数据库
//! ```
//!
//! DTO 不重新计算任何概率或 EV。它只调用现有核心模块，再把结果整理成适合
//! 跨语言传输的普通数字、字符串和结构体。

use std::{error::Error, fmt};

use serde::Serialize;

use crate::Shoe;

use super::{
    BetPlanAction, BetPlanSkipReason, BettingPolicy, EffectiveBetMetrics, KellyError, KellyPolicy,
    KellyQuote, MainBet, MainBetAnalysis, MainBetRules, OutcomeWeights, ProbabilityError,
    SkipReason, calculate_main_outcomes,
};

/// 当前 JSON 快照结构版本。
///
/// 只有当字段含义或 JSON 结构发生不兼容变化时才增加这个版本。单纯修改内部
/// 枚举算法、注释或性能实现，不需要修改它。
pub const DECISION_SNAPSHOT_SCHEMA_VERSION: u16 = 1;

/// 当前引擎版本，直接读取 `Cargo.toml` 中的 package version。
///
/// `env!` 在编译期把版本写入程序，不需要运行时读取文件。
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 一个下注方向在生成快照时的完整有效指标。
///
/// 字段全部使用“每下注 1 单位”的口径，因此不会随着 bankroll 或最终下注
/// 金额改变。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BetSnapshot {
    /// 该方向对应的主结果发生概率，例如 Banker 字段保存 `P(Banker)`。
    pub probability: f64,
    /// 不考虑返水时，每下注 1 单位的基础净 EV。
    pub base_ev: f64,
    /// 按所有可能结果概率加权后的返水 EV。
    pub rebate_ev: f64,
    /// `base_ev + rebate_ev`，方向策略真正用于比较的 EV。
    pub effective_ev: f64,
}

impl BetSnapshot {
    /// 把内部强类型指标复制到只包含稳定基础字段的 DTO。
    fn from_metrics(metrics: EffectiveBetMetrics) -> Self {
        Self {
            probability: metrics.probability(),
            base_ev: metrics.base_ev(),
            rebate_ev: metrics.rebate_ev(),
            effective_ev: metrics.effective_ev(),
        }
    }
}

/// 快照对外暴露的最终动作。
///
/// `#[serde(tag = "type")]` 使用内部标签形式生成 JSON：
///
/// ```json
/// { "type": "place", "bet": "banker", "amount": 25.0 }
/// ```
///
/// `rename_all = "snake_case"` 保证 Rust 的 `Place`、`Skip` 在 JSON 中稳定写成
/// 小写 `place`、`skip`，Python 不需要了解 Rust 的命名习惯。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionSnapshot {
    /// EV 门槛和资金限制均已通过，允许下注。
    Place {
        /// 稳定小写下注方向：`player`、`banker` 或 `tie`。
        bet: String,
        /// 经过凯利比例、单局上限和桌台上限后的最终金额。
        amount: f64,
    },
    /// 本局最终不下注。
    Skip {
        /// 稳定机器可读原因，例如 `below_minimum_ev`。
        reason: String,
    },
}

/// 一次下注前分析的稳定、可序列化结果。
///
/// 字段设置为 `pub` 是因为 DTO 的用途就是让上层直接读取数据；核心领域对象
/// 仍然保持私有字段加 getter 的封装方式。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DecisionSnapshot {
    /// JSON 结构版本，当前固定为 [`DECISION_SNAPSHOT_SCHEMA_VERSION`]。
    pub schema_version: u16,
    /// 生成快照的 Rust crate 版本。
    pub engine_version: String,
    /// 闲注在当前牌靴和返水规则下的指标。
    pub player: BetSnapshot,
    /// 庄注在当前牌靴和返水规则下的指标。
    pub banker: BetSnapshot,
    /// 和注在当前牌靴和返水规则下的指标。
    pub tie: BetSnapshot,
    /// 三个方向中有效 EV 最大的候选方向；即使最终 Skip 也会保留。
    pub candidate_bet: String,
    /// 上层真正应该执行的最终动作。
    pub action: ActionSnapshot,
    /// 生成计划时交给凯利策略管理的资金。
    pub bankroll: f64,
    /// 原始完整凯利比例；方向策略直接 Skip 时为 `None`。
    pub kelly_fraction: Option<f64>,
    /// 经过所有资金上限后的实际下注比例；方向策略直接 Skip 时为 `None`。
    pub applied_fraction: Option<f64>,
    /// 最终建议下注金额；任何 Skip 场景都为 0。
    pub suggested_amount: f64,
    /// `suggested_amount × candidate effective EV`；任何 Skip 场景都为 0。
    pub expected_profit: f64,
}

impl DecisionSnapshot {
    /// 把快照格式化成带缩进的 JSON，适合日志、调试和固定 fixture。
    ///
    /// 生产网络传输如果更关心体积，可以直接调用 `serde_json::to_string(&snapshot)`
    /// 生成不带额外空白的紧凑 JSON。
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// 根据当前牌靴一步生成完整决策快照。
///
/// 这是未来 Python 绑定最适合暴露的高层入口。调用者只需要准备下注前牌靴、
/// 赔付、方向策略、凯利策略和 bankroll，不需要自己拼接中间对象。
pub fn analyze_snapshot(
    shoe: &Shoe,
    rules: MainBetRules,
    betting_policy: &BettingPolicy,
    kelly_policy: KellyPolicy,
    bankroll: f64,
) -> Result<DecisionSnapshot, SnapshotError> {
    // 第一步仍然复用生产点数枚举器。`?` 会把 ProbabilityError 通过下面的
    // From 实现自动包装成 SnapshotError::Probability。
    let weights = calculate_main_outcomes(shoe)?;

    decision_snapshot_from_weights(weights, rules, betting_policy, kelly_policy, bankroll)
}

/// 根据已经缓存的概率权重生成决策快照。
///
/// 如果同一牌靴需要针对多个返水政策或多个 bankroll 生成结果，可以先枚举一次
/// `OutcomeWeights`，再反复调用本函数，避免重复执行概率枚举。
pub fn decision_snapshot_from_weights(
    weights: OutcomeWeights,
    rules: MainBetRules,
    betting_policy: &BettingPolicy,
    kelly_policy: KellyPolicy,
    bankroll: f64,
) -> Result<DecisionSnapshot, SnapshotError> {
    // 先建立三个方向的基础分析，再用同一个 rebate 分别生成有效指标快照。
    let analysis = MainBetAnalysis::from_weights(weights, rules);
    let rebate = betting_policy.rebate();
    let player = bet_snapshot(analysis, MainBet::Player, rebate);
    let banker = bet_snapshot(analysis, MainBet::Banker, rebate);
    let tie = bet_snapshot(analysis, MainBet::Tie, rebate);

    // KellyPolicy::plan 内部会调用 BettingPolicy::decide，所以这里不会另外实现
    // 一份候选方向或最低 EV 判断逻辑。
    let plan = kelly_policy.plan(betting_policy, weights, rules, bankroll)?;
    let candidate_bet = plan.decision().candidate().as_str().to_owned();
    let quote = plan.quote();

    // Option::map 只在 quote 为 Some 时读取字段；策略门槛直接拒绝时保留 None，
    // JSON 会明确输出 null，让上层区分“没有计算金额”和“比例恰好为零”。
    let kelly_fraction = quote.map(KellyQuote::kelly_fraction);
    let applied_fraction = quote.map(KellyQuote::applied_fraction);

    // 无报价时使用 0；有报价但最终 Skip 时，KellyQuote 本身也会给出 0 金额。
    let suggested_amount = quote.map_or(0.0, KellyQuote::amount);
    let expected_profit = quote.map_or(0.0, KellyQuote::expected_profit);

    let action = action_snapshot(*plan.action());

    Ok(DecisionSnapshot {
        schema_version: DECISION_SNAPSHOT_SCHEMA_VERSION,
        engine_version: ENGINE_VERSION.to_owned(),
        player,
        banker,
        tie,
        candidate_bet,
        action,
        bankroll,
        kelly_fraction,
        applied_fraction,
        suggested_amount,
        expected_profit,
    })
}

/// 从同一个分析对象读取指定方向，并转换成 DTO。
fn bet_snapshot(analysis: MainBetAnalysis, bet: MainBet, rebate: super::RebateRule) -> BetSnapshot {
    BetSnapshot::from_metrics(analysis.effective_metrics(bet, rebate))
}

/// 把内部最终动作转换成不会泄露内部枚举布局的稳定 DTO。
fn action_snapshot(action: BetPlanAction) -> ActionSnapshot {
    match action {
        BetPlanAction::Place { bet, amount } => ActionSnapshot::Place {
            bet: bet.as_str().to_owned(),
            amount,
        },
        BetPlanAction::Skip { reason } => ActionSnapshot::Skip {
            reason: skip_reason_code(reason).to_owned(),
        },
    }
}

/// 为内部 Skip 变体分配稳定、机器可读的字符串代码。
///
/// 不使用 `format!("{reason:?}")`，因为 Debug 文本是给开发者看的，不承诺稳定；
/// 显式 match 才能让未来重命名内部枚举时保留原 JSON 协议。
fn skip_reason_code(reason: BetPlanSkipReason) -> &'static str {
    match reason {
        BetPlanSkipReason::Strategy(SkipReason::BelowMinimumEv { .. }) => "below_minimum_ev",
        BetPlanSkipReason::NonPositiveKelly => "non_positive_kelly",
        BetPlanSkipReason::RiskLimitIsZero => "risk_limit_is_zero",
    }
}

/// 生成决策快照时，概率层或凯利层返回的错误。
#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotError {
    /// 当前牌靴无法完成概率枚举。
    Probability(ProbabilityError),
    /// 凯利输入或资金配置无效。
    Kelly(KellyError),
}

impl From<ProbabilityError> for SnapshotError {
    /// 允许 `?` 自动把概率错误提升到快照错误。
    fn from(error: ProbabilityError) -> Self {
        Self::Probability(error)
    }
}

impl From<KellyError> for SnapshotError {
    /// 允许 `?` 自动把凯利错误提升到快照错误。
    fn from(error: KellyError) -> Self {
        Self::Kelly(error)
    }
}

impl fmt::Display for SnapshotError {
    /// 在保留结构化错误变体的同时，复用底层错误的人类可读文本。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Probability(error) => write!(formatter, "probability analysis failed: {error}"),
            Self::Kelly(error) => write!(formatter, "Kelly planning failed: {error}"),
        }
    }
}

impl Error for SnapshotError {
    /// 暴露原始底层错误，方便日志或上层错误库沿错误链继续检查原因。
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Probability(error) => Some(error),
            Self::Kelly(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{
        ActionSnapshot, DECISION_SNAPSHOT_SCHEMA_VERSION, ENGINE_VERSION, SnapshotError,
        analyze_snapshot, decision_snapshot_from_weights,
    };
    use crate::{
        BettingPolicy, KellyError, KellyPolicy, MainBetRules, OutcomeWeights, RebateRule, Shoe,
    };

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-10, "{actual} != {expected}");
    }

    fn sample_weights() -> OutcomeWeights {
        OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重应该构成完整分布")
    }

    #[test]
    fn place_snapshot_contains_direction_metrics_and_capped_amount() {
        let betting_policy = BettingPolicy::new(RebateRule::None, 0.0);
        let kelly_policy = KellyPolicy::full(40.0, 50.0).expect("金额上限应该合法");

        let snapshot = decision_snapshot_from_weights(
            sample_weights(),
            MainBetRules::standard(),
            &betting_policy,
            kelly_policy,
            1_000.0,
        )
        .expect("正 EV 测试分布应该生成快照");

        assert_eq!(snapshot.schema_version, DECISION_SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.engine_version, ENGINE_VERSION);
        assert_eq!(snapshot.candidate_bet, "tie");
        assert_close(snapshot.tie.probability, 1.0 / 6.0);
        assert_close(snapshot.tie.base_ev, 0.5);
        assert_close(snapshot.tie.rebate_ev, 0.0);
        assert_close(snapshot.tie.effective_ev, 0.5);
        assert_close(
            snapshot.kelly_fraction.expect("应该计算凯利比例"),
            1.0 / 16.0,
        );
        assert_close(snapshot.applied_fraction.expect("应该计算实际比例"), 0.04);
        assert_close(snapshot.suggested_amount, 40.0);
        assert_close(snapshot.expected_profit, 20.0);

        assert_eq!(
            snapshot.action,
            ActionSnapshot::Place {
                bet: "tie".to_owned(),
                amount: 40.0,
            }
        );
    }

    #[test]
    fn skip_snapshot_uses_null_for_uncomputed_kelly_fields() {
        let betting_policy = BettingPolicy::new(RebateRule::None, 0.6);
        let kelly_policy = KellyPolicy::full(100.0, 100.0).expect("金额上限应该合法");

        let snapshot = decision_snapshot_from_weights(
            sample_weights(),
            MainBetRules::standard(),
            &betting_policy,
            kelly_policy,
            1_000.0,
        )
        .expect("低于门槛应该生成 Skip 快照，而不是返回错误");

        assert_eq!(snapshot.candidate_bet, "tie");
        assert_eq!(snapshot.kelly_fraction, None);
        assert_eq!(snapshot.applied_fraction, None);
        assert_eq!(snapshot.suggested_amount, 0.0);
        assert_eq!(snapshot.expected_profit, 0.0);
        assert_eq!(
            snapshot.action,
            ActionSnapshot::Skip {
                reason: "below_minimum_ev".to_owned(),
            }
        );

        let json: Value =
            serde_json::from_str(&snapshot.to_json_pretty().expect("合法快照应该能够序列化"))
                .expect("快照 JSON 应该能够再次解析");

        assert!(json["kelly_fraction"].is_null());
        assert!(json["applied_fraction"].is_null());
        assert_eq!(json["action"]["type"], "skip");
        assert_eq!(json["action"]["reason"], "below_minimum_ev");
    }

    #[test]
    fn json_field_names_and_action_tag_are_stable() {
        let betting_policy = BettingPolicy::new(RebateRule::None, 0.0);
        let kelly_policy = KellyPolicy::full(40.0, 50.0).expect("金额上限应该合法");
        let snapshot = decision_snapshot_from_weights(
            sample_weights(),
            MainBetRules::standard(),
            &betting_policy,
            kelly_policy,
            1_000.0,
        )
        .expect("应该生成快照");

        let json = serde_json::to_value(snapshot).expect("合法快照应该能够序列化");

        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["engine_version"], ENGINE_VERSION);
        assert_eq!(json["candidate_bet"], "tie");
        assert_eq!(json["action"]["type"], "place");
        assert_eq!(json["action"]["bet"], "tie");
        assert_eq!(json["action"]["amount"], 40.0);
        assert!(json.get("player").is_some());
        assert!(json.get("banker").is_some());
        assert!(json.get("tie").is_some());
        assert!(json.get("suggested_amount").is_some());
        assert!(json.get("expected_profit").is_some());
    }

    #[test]
    fn high_level_snapshot_analyzes_a_real_eight_deck_shoe() {
        let betting_policy =
            BettingPolicy::new(RebateRule::AllExceptMainBetTie { rate: 0.015 }, 0.0);
        let kelly_policy = KellyPolicy::full(1_000.0, 1_000.0).expect("金额上限应该合法");

        let snapshot = analyze_snapshot(
            &Shoe::default(),
            MainBetRules::standard(),
            &betting_policy,
            kelly_policy,
            10_000.0,
        )
        .expect("完整八副牌应该能够生成快照");

        assert_eq!(snapshot.candidate_bet, "banker");
        assert!(snapshot.banker.effective_ev > 0.0);
        assert!(snapshot.suggested_amount > 0.0);
        assert!(matches!(snapshot.action, ActionSnapshot::Place { .. }));
    }

    #[test]
    fn invalid_bankroll_is_preserved_as_a_structured_snapshot_error() {
        let betting_policy = BettingPolicy::new(RebateRule::None, 0.0);
        let kelly_policy = KellyPolicy::full(100.0, 100.0).expect("金额上限应该合法");

        let result = decision_snapshot_from_weights(
            sample_weights(),
            MainBetRules::standard(),
            &betting_policy,
            kelly_policy,
            0.0,
        );

        assert_eq!(
            result,
            Err(SnapshotError::Kelly(KellyError::InvalidBankroll {
                value: 0.0
            }))
        );
    }
}
