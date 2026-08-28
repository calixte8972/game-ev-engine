//! 真人桌面游戏概率与 EV 计算引擎的核心库。
//!
//! 游戏规则、牌靴状态、概率计算和 EV 计算会逐步实现在这个库中。

/// 扑克牌的基础类型、文本解析和百家乐点数。
pub mod card;
/// 多副牌牌靴的剩余数量和安全扣牌操作。
pub mod shoe;

/// 标准百家乐手牌、补牌规则、回合结果和概率计算。
pub mod baccarat;
pub mod cli;

// 把最常使用的领域类型提升到 crate 根路径，调用者无需记住内部文件结构。
pub use baccarat::{
    BaccaratHand, BankerPayoutRule, BetMetrics, MainBet, MainBetAnalysis, MainBetEv, MainBetRules,
    OutcomeWeights, ProbabilityError, RoundError, RoundOutcome, RoundResult, analyze_main_bets,
    calculate_main_outcomes, resolve_round,
};
pub use card::{Card, CardParseError, Rank, Suit};
pub use shoe::{DEFAULT_DECKS, MAX_DECKS, MIN_DECKS, Shoe, ShoeError};

/// 应用在中文界面中显示的名称。
pub const APP_NAME: &str = "真人桌面游戏概率与 EV 计算引擎";
