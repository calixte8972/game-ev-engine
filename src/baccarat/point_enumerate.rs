//! 按百家乐点数聚合的标准主注概率枚举器。

use crate::{OutcomeWeights, ProbabilityError, RoundError, RoundOutcome, Shoe};

use super::probability::falling_factorial;
use super::resolve_point_round;

/// 根据当前牌靴精确计算下一局庄、闲、和的概率权重。
///
/// 主注只依赖百家乐点数，因此这里把 52 种具体牌聚合为 0～9 共十类，
/// 避免为花色和同点数牌面建立不必要的分支。具体牌面的枚举器仍保留在
/// 测试构建中，用于和本函数交叉验证。
pub fn calculate_main_outcomes(shoe: &Shoe) -> Result<OutcomeWeights, ProbabilityError> {
    let initial_total = shoe.total_remaining();
    if initial_total < 6 {
        return Err(ProbabilityError::NotEnoughCards {
            remaining: initial_total,
        });
    }

    let mut point_counts = shoe.baccarat_point_counts();
    let mut points = Vec::with_capacity(6);
    let mut accumulator = OutcomeAccumulator::default();

    enumerate_point_paths(
        &mut point_counts,
        initial_total,
        &mut points,
        1,
        &mut accumulator,
    )?;

    debug_assert!(points.is_empty());
    debug_assert_eq!(point_counts.iter().sum::<u16>(), initial_total);

    OutcomeWeights::from_weights(
        initial_total,
        accumulator.player,
        accumulator.banker,
        accumulator.tie,
    )
}

/// 递归枚举点数类别，并把每条终局路径转换为六张牌共同分母下的权重。
fn enumerate_point_paths(
    point_counts: &mut [u16; 10],
    remaining: u16,
    points: &mut Vec<u8>,
    path_weight: u64,
    accumulator: &mut OutcomeAccumulator,
) -> Result<(), ProbabilityError> {
    match resolve_point_round(points) {
        Ok(result) => {
            let missing_cards = 6 - result.card_count();
            let completion_weight = falling_factorial(remaining, missing_cards);
            let terminal_weight = path_weight
                .checked_mul(completion_weight)
                .ok_or(ProbabilityError::WeightOverflow)?;

            accumulator.add(result.outcome(), terminal_weight)
        }
        Err(
            RoundError::NotEnoughInitialCards
            | RoundError::MissingPlayerThirdCard
            | RoundError::MissingBankerThirdCard,
        ) => enumerate_next_point(point_counts, remaining, points, path_weight, accumulator),
        Err(RoundError::UnexpectedExtraCards) => {
            unreachable!("点数枚举器不应在牌局结束后继续发牌")
        }
    }
}

/// 遍历仍有剩余牌的十种百家乐点数，并递归处理每个分支。
fn enumerate_next_point(
    point_counts: &mut [u16; 10],
    remaining: u16,
    points: &mut Vec<u8>,
    path_weight: u64,
    accumulator: &mut OutcomeAccumulator,
) -> Result<(), ProbabilityError> {
    for point in 0_u8..=9 {
        let index = usize::from(point);
        let copies = point_counts[index];
        if copies == 0 {
            continue;
        }

        let next_weight = path_weight
            .checked_mul(u64::from(copies))
            .ok_or(ProbabilityError::WeightOverflow)?;

        point_counts[index] -= 1;
        points.push(point);

        let branch_result = enumerate_point_paths(
            point_counts,
            remaining - 1,
            points,
            next_weight,
            accumulator,
        );

        points.pop();
        point_counts[index] += 1;

        branch_result?;
    }

    Ok(())
}

#[derive(Debug, Default)]
struct OutcomeAccumulator {
    player: u64,
    banker: u64,
    tie: u64,
}

impl OutcomeAccumulator {
    fn add(&mut self, outcome: RoundOutcome, weight: u64) -> Result<(), ProbabilityError> {
        let destination = match outcome {
            RoundOutcome::Player => &mut self.player,
            RoundOutcome::Banker => &mut self.banker,
            RoundOutcome::Tie => &mut self.tie,
        };

        *destination = destination
            .checked_add(weight)
            .ok_or(ProbabilityError::WeightOverflow)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Card, Rank, Shoe, Suit};

    use super::calculate_main_outcomes;

    fn card(input: &str) -> Card {
        input.parse().expect("测试使用的牌面必须合法")
    }

    #[test]
    fn all_zero_point_cards_produce_only_ties() {
        let retained = [
            card("10C"),
            card("10D"),
            card("10H"),
            card("10S"),
            card("JC"),
            card("JD"),
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

        let weights = calculate_main_outcomes(&shoe).expect("六张 0 点牌应能够完成枚举");

        assert_eq!(shoe, initial_shoe);
        assert_eq!(weights.player_weight(), 0);
        assert_eq!(weights.banker_weight(), 0);
        assert_eq!(weights.tie_weight(), 720);
        assert_eq!(weights.total_weight(), 720);
    }

    #[test]
    fn full_shoe_produces_a_complete_probability_distribution() {
        let weights =
            calculate_main_outcomes(&Shoe::default()).expect("完整八副牌应能够完成主注枚举");

        assert!(weights.weights_sum_to_total());
        assert!(
            (weights.player_probability()
                + weights.banker_probability()
                + weights.tie_probability()
                - 1.0)
                .abs()
                < 1e-15
        );
        assert!(weights.banker_probability() > weights.player_probability());
    }
}
