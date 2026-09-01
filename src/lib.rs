//! 真人桌面游戏概率、EV 与下注风险计算引擎的库入口。
//!
//! `lib.rs` 本身不实现业务算法，它负责声明模块并整理对外 API。当前主要数据流：
//!
//! ```text
//! Card -> Shoe -> OutcomeWeights -> MainBetAnalysis
//!                                -> BettingPolicy -> KellyPolicy -> BetPlan
//!                                                                  -> DecisionSnapshot
//! ```
//!
//! `pub mod` 让模块对外可见，`pub use` 则把最常用类型重新导出到 crate 根路径。
//! 因此调用者可以写 `game_ev_engine::Shoe`，不必记住它实际位于 `shoe.rs`。

/// 扑克牌的基础类型、文本解析和百家乐点数。
pub mod card;
/// 多副牌牌靴的剩余数量和安全扣牌操作。
pub mod shoe;

/// 标准百家乐手牌、补牌规则、回合结果和概率计算。
pub mod baccarat;
/// 有限牌靴二十一点手牌、桌规和动作 EV。
pub mod blackjack;
/// 命令行字符串到核心领域类型的输入适配层。
pub mod cli;
/// 浏览器输入到核心牌靴和分析结果的 WebAssembly 适配层。
pub mod web_api;

// 把最常使用的百家乐领域类型提升到 crate 根路径。
// 这里只改变访问路径，不会复制类型，也不会产生运行时开销。
pub use baccarat::{
    ActionSnapshot, BaccaratHand, BankerPayoutRule, BetAction, BetDecision, BetMetrics, BetPlan,
    BetPlanAction, BetPlanSkipReason, BetSnapshot, BetTarget, BettingPolicy, CombinedBetAction,
    CombinedBetDecision, CombinedBetPlan, CombinedBetPlanAction, CsvBetBreakdown, CsvBetCounts,
    CsvBetDetail, CsvBetPerformance, CsvDatasetReport, CsvQualityReport, CsvReplayConfig,
    CsvReplayConfigSnapshot, CsvReplayError, CsvReplayReport, CsvReplaySummary,
    DECISION_SNAPSHOT_SCHEMA_VERSION, DecisionSnapshot, ENGINE_VERSION, EffectiveBetMetrics,
    KellyError, KellyOutcome, KellyPolicy, KellyQuote, MainBet, MainBetAnalysis, MainBetEv,
    MainBetRules, OutcomeWeights, ProbabilityError, RebateRule, RoundError, RoundOutcome,
    RoundResult, SideBet, SideBetAnalysis, SideBetMetrics, SideBetRoundLimits, SideBetRuleError,
    SideBetRules, SideBetWeights, SkipReason, SnapshotError, StakeSizingStrategy,
    analyze_main_bets, analyze_snapshot, calculate_kelly_fraction,
    calculate_main_and_side_outcomes, calculate_main_outcomes, calculate_side_bet_outcomes,
    decision_snapshot_from_weights, effective_ev, main_bet_kelly_outcomes, replay_csv_text,
    resolve_round, side_bet_kelly_outcomes, side_bet_kelly_outcomes_with_rebate,
};
pub use blackjack::{
    BlackjackAction, BlackjackActionEvs, BlackjackAnalysis, BlackjackError, BlackjackRules,
    analyze_blackjack_hand,
};
pub use card::{Card, CardParseError, Rank, Suit};
pub use shoe::{DEFAULT_DECKS, MAX_DECKS, MIN_DECKS, Shoe, ShoeError};

/// 应用在中文界面中显示的名称。
pub const APP_NAME: &str = "真人桌面游戏概率与 EV 计算引擎";
