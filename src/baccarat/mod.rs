//! 标准百家乐的手牌、补牌规则和牌局结果。

// 具体牌穷举器只作为小牌靴测试基准，不进入生产构建。
#[cfg(test)]
mod enumerate;
/// 一方百家乐手牌的表示和点数计算。
pub mod hand;
/// 按百家乐点数聚合的生产概率枚举器。
mod point_enumerate;
// 以下模块通过本文件选择性导出，避免暴露内部辅助函数。
mod analysis;
mod bet;
mod ev;
mod probability;
mod round;
mod rule;

// 对外统一暴露稳定的百家乐 API，调用者不需要依赖内部文件布局。
pub use analysis::{BetMetrics, MainBetAnalysis, analyze_main_bets};
pub use bet::{BankerPayoutRule, MainBet, MainBetRules};
pub use ev::MainBetEv;
pub use hand::BaccaratHand;
pub use point_enumerate::calculate_main_outcomes;
pub use probability::{OutcomeWeights, ProbabilityError};
pub(crate) use round::resolve_point_round;
pub use round::{RoundError, RoundOutcome, RoundResult, compare_hands, resolve_round};
pub use rule::{banker_should_draw, player_should_draw};

#[cfg(test)]
mod cross_algorithm_tests {
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
        let initial_shoe = shoe.clone();

        let point_weights = calculate_main_outcomes(&shoe).expect("点数聚合枚举应该成功");
        let card_weights = enumerate_main_outcomes_by_card(&shoe).expect("具体牌枚举应该成功");

        assert_eq!(point_weights, card_weights);
        assert_eq!(shoe, initial_shoe);
    }
}
