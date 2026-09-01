//! 标准百家乐概率、EV、策略和资金管理模块的统一入口。
//!
//! 子模块按职责分层：
//!
//! ```text
//! hand / rule / round       手牌、补牌规则和单局解析
//! point_enumerate           从当前牌靴精确枚举结果权重
//! probability              保存并验证整数权重
//! bet / ev / rebate         赔付、基础 EV 和返水
//! analysis / strategy       应用层指标、是否下注和下注方向
//! risk                      凯利比例、金额上限和最终 BetPlan
//! snapshot                  面向 JSON、Python 和数据库的稳定 DTO
//! replay                    CSV 牌靴重建、策略回放和滚动本金结算
//! ```
//!
//! 外部调用者只需要使用本文件 `pub use` 导出的稳定类型，不需要了解每个类型
//! 实际放在哪个子文件中。没有 `pub` 的模块和函数属于内部实现细节。

// 具体牌穷举器只作为小牌靴测试基准，不进入生产构建。
#[cfg(test)]
mod enumerate;
/// 一方百家乐手牌的表示和点数计算。
mod hand;
/// 按百家乐点数聚合的生产概率枚举器。
mod point_enumerate;
// 以下模块保持私有，再通过本文件选择性导出公开类型，避免暴露内部辅助函数。
mod analysis;
mod bet;
mod ev;
mod probability;
mod rebate;
mod replay;
mod risk;
mod round;
mod rule;
mod side_bet;
mod snapshot;
mod strategy;

// 对外统一暴露稳定的百家乐 API，调用者不需要依赖内部文件布局。
pub use analysis::{
    BetMetrics, EffectiveBetMetrics, MainBetAnalysis, analyze_main_bets, effective_ev,
};
pub use bet::{BankerPayoutRule, MainBet, MainBetRules};
pub use ev::MainBetEv;
pub use hand::BaccaratHand;
pub use point_enumerate::{
    calculate_main_and_side_outcomes, calculate_main_outcomes, calculate_side_bet_outcomes,
};
pub use probability::{OutcomeWeights, ProbabilityError};
pub use rebate::RebateRule;
pub use replay::{
    CsvBetBreakdown, CsvBetCounts, CsvBetDetail, CsvBetPerformance, CsvDatasetReport,
    CsvQualityReport, CsvReplayConfig, CsvReplayConfigSnapshot, CsvReplayError, CsvReplayReport,
    CsvReplaySummary, SideBetRoundLimits, replay_csv_text,
};
pub use risk::{
    BetPlan, BetPlanAction, BetPlanSkipReason, CombinedBetPlan, CombinedBetPlanAction, KellyError,
    KellyOutcome, KellyPolicy, KellyQuote, StakeSizingStrategy, calculate_kelly_fraction,
    main_bet_kelly_outcomes, side_bet_kelly_outcomes, side_bet_kelly_outcomes_with_rebate,
};
pub(crate) use round::resolve_point_round;
pub use round::{RoundError, RoundOutcome, RoundResult, compare_hands, resolve_round};
pub use rule::{banker_should_draw, player_should_draw};
pub use side_bet::{
    SideBet, SideBetAnalysis, SideBetMetrics, SideBetRuleError, SideBetRules, SideBetWeights,
};
pub use snapshot::{
    ActionSnapshot, BetSnapshot, DECISION_SNAPSHOT_SCHEMA_VERSION, DecisionSnapshot,
    ENGINE_VERSION, SnapshotError, analyze_snapshot, decision_snapshot_from_weights,
};
pub use strategy::{
    BetAction, BetDecision, BetTarget, BettingPolicy, CombinedBetAction, CombinedBetDecision,
    SkipReason,
};

#[cfg(test)]
mod cross_algorithm_tests {
    // 这组测试用两种独立算法计算同一个小牌靴：
    // 1. point_enumerate 按 0～9 点聚合，是生产算法；
    // 2. enumerate 按 52 种具体牌枚举，是较慢但直观的参考算法。
    // 两者权重完全相等，说明点数压缩没有丢失主注所需信息。
    use crate::{Card, Rank, Shoe, Suit};

    use super::{calculate_main_outcomes, enumerate::enumerate_main_outcomes_by_card};

    fn card(input: &str) -> Card {
        input.parse().expect("测试使用的牌面必须合法")
    }

    #[test]
    fn point_aggregation_matches_concrete_card_enumeration() {
        let retained = [
            card("AS"),
            card("2C"),
            card("3D"),
            card("4H"),
            card("5S"),
            card("6C"),
        ];
        let mut removed = Vec::new();

        // 构造一个只有六张牌的小牌靴。原本是两副牌：retained 中每种牌保留
        // 一张，其他所有具体牌两张都扣掉。
        for rank in Rank::ALL {
            for suit in Suit::ALL {
                let candidate = Card::new(rank, suit);
                let copies_to_remove = if retained.contains(&candidate) { 1 } else { 2 };

                for _ in 0..copies_to_remove {
                    removed.push(candidate);
                }
            }
        }

        let mut shoe = Shoe::new(2).expect("两副牌必须是合法牌靴");
        shoe.remove_many(&removed).expect("测试牌必须能够扣除");
        // 保存初始状态，最后验证两套枚举算法都只借用输入，没有修改真实牌靴。
        let initial_shoe = shoe.clone();

        let point_weights = calculate_main_outcomes(&shoe).expect("点数聚合枚举应该成功");
        let card_weights = enumerate_main_outcomes_by_card(&shoe).expect("具体牌枚举应该成功");

        assert_eq!(point_weights, card_weights);
        assert_eq!(shoe, initial_shoe);
    }
}
