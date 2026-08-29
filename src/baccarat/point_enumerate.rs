//! 按百家乐点数聚合的标准主注概率枚举器。
//!
//! 主注不关心花色，也不区分 10、J、Q、K，因为它们都是 0 点。因此牌靴可
//! 压缩成 `[0 点数量, 1 点数量, ..., 9 点数量]`。
//!
//! 最直观的算法会在每次计算时重新走完 `10^6` 条六张点数顺序。单次计算尚可，
//! 但一天近十万局的回放会重复做大量与当前牌靴无关的工作。本文件采用等价的
//! “组合系数表”算法：第一次调用时归并一百万种抽象顺序；六张牌的点数组合
//! 只有 `C(15, 6) = 5,005` 种；之后每局只把当前点数数量代入这 5,005 项。
//!
//! 例如某项是“两张 0、三张 1、一张 7”，当前数量为 `n0、n1、n7`，每种
//! 抽象排列代表的物理序列数就是 `(n0)₂ × (n1)₃ × (n7)₁`。`(n)ₖ` 是下降
//! 阶乘，准确表达不放回抽牌，所以本算法不是概率近似。

use std::{collections::HashMap, sync::OnceLock};

use crate::{OutcomeWeights, ProbabilityError, RoundError, RoundOutcome, Shoe};

use super::probability::falling_factorial;
use super::resolve_point_round;

/// 六张牌分配到十种点数的组合数量：C(6 + 10 - 1, 10 - 1)。
const COMPOSITION_COUNT: usize = 5_005;

/// 与具体牌靴无关的一项组合系数。
///
/// `multiplicities[point]` 是六个位置中该点数的张数；其余字段记录具有同一
/// 组成的抽象排列分别产生多少种结果。计数最大不超过 `6! = 720`。
#[derive(Debug)]
struct CompositionCoefficient {
    multiplicities: [u8; 10],
    player_permutations: u16,
    banker_permutations: u16,
    tie_permutations: u16,
    banker_win_on_six_permutations: u16,
}

/// 全进程共享系数表。同一 WASM 实例内的所有牌靴状态只初始化一次。
static COMPOSITION_TABLE: OnceLock<Vec<CompositionCoefficient>> = OnceLock::new();

/// 根据当前牌靴精确计算下一局庄、闲、和的概率权重。
pub fn calculate_main_outcomes(shoe: &Shoe) -> Result<OutcomeWeights, ProbabilityError> {
    let total_cards = shoe.total_remaining();
    if total_cards < 6 {
        return Err(ProbabilityError::NotEnoughCards {
            remaining: total_cards,
        });
    }

    let point_counts = shoe.baccarat_point_counts();

    // 一项组合最多取同一点数六次。先计算每个点数的 (n)₀ 到 (n)₆，
    // 避免在 5,005 项循环里反复计算相同下降阶乘。
    let mut falling = [[0_u64; 7]; 10];
    for (point, &available) in point_counts.iter().enumerate() {
        falling[point][0] = 1;
        for count in 1_u8..=6 {
            falling[point][usize::from(count)] = if u16::from(count) <= available {
                falling_factorial(available, count)
            } else {
                0
            };
        }
    }

    let mut player = 0_u64;
    let mut banker = 0_u64;
    let mut tie = 0_u64;
    let mut banker_win_on_six = 0_u64;

    for coefficient in composition_table() {
        // 同一点数组成的所有抽象排列，对应相同数量的物理发牌序列。
        // 数量不足时 falling 值为 0，整项自然不会贡献权重。
        let mut physical_sequences_per_permutation = 1_u64;
        for (point, &count) in coefficient.multiplicities.iter().enumerate() {
            if count != 0 {
                physical_sequences_per_permutation = physical_sequences_per_permutation
                    .checked_mul(falling[point][usize::from(count)])
                    .ok_or(ProbabilityError::WeightOverflow)?;
            }
        }

        if physical_sequences_per_permutation == 0 {
            continue;
        }

        player = add_weight(
            player,
            physical_sequences_per_permutation,
            coefficient.player_permutations,
        )?;
        banker = add_weight(
            banker,
            physical_sequences_per_permutation,
            coefficient.banker_permutations,
        )?;
        tie = add_weight(
            tie,
            physical_sequences_per_permutation,
            coefficient.tie_permutations,
        )?;
        banker_win_on_six = add_weight(
            banker_win_on_six,
            physical_sequences_per_permutation,
            coefficient.banker_win_on_six_permutations,
        )?;
    }

    // 构造器会再次验证三种互斥结果是否恰好等于 (总牌数)₆。
    OutcomeWeights::from_detailed_weights(total_cards, player, banker, tie, banker_win_on_six)
}

/// 把“一种排列的物理权重 × 该结果排列数”安全加入结果桶。
fn add_weight(
    current: u64,
    physical_weight: u64,
    permutations: u16,
) -> Result<u64, ProbabilityError> {
    let contribution = physical_weight
        .checked_mul(u64::from(permutations))
        .ok_or(ProbabilityError::WeightOverflow)?;
    current
        .checked_add(contribution)
        .ok_or(ProbabilityError::WeightOverflow)
}

fn composition_table() -> &'static [CompositionCoefficient] {
    COMPOSITION_TABLE.get_or_init(build_composition_table)
}

/// 一次性把 10^6 种六张点数顺序归并为 5,005 项。
fn build_composition_table() -> Vec<CompositionCoefficient> {
    let mut coefficients =
        HashMap::<[u8; 10], CompositionCoefficient>::with_capacity(COMPOSITION_COUNT);
    let mut points = [0_u8; 6];
    let mut multiplicities = [0_u8; 10];

    enumerate_abstract_sequences(0, &mut points, &mut multiplicities, &mut coefficients);

    let mut table: Vec<_> = coefficients.into_values().collect();
    // HashMap 迭代顺序不稳定；排序让不同平台的执行顺序与基准一致。
    table.sort_unstable_by_key(|coefficient| coefficient.multiplicities);
    debug_assert_eq!(table.len(), COMPOSITION_COUNT);
    table
}

/// 深度固定为六层的抽象点数枚举，只在构造系数表时运行一次。
fn enumerate_abstract_sequences(
    position: usize,
    points: &mut [u8; 6],
    multiplicities: &mut [u8; 10],
    coefficients: &mut HashMap<[u8; 10], CompositionCoefficient>,
) {
    if position == points.len() {
        let result = resolve_six_position_sequence(points);
        let coefficient =
            coefficients
                .entry(*multiplicities)
                .or_insert_with(|| CompositionCoefficient {
                    multiplicities: *multiplicities,
                    player_permutations: 0,
                    banker_permutations: 0,
                    tie_permutations: 0,
                    banker_win_on_six_permutations: 0,
                });

        match result.outcome() {
            RoundOutcome::Player => coefficient.player_permutations += 1,
            RoundOutcome::Banker => coefficient.banker_permutations += 1,
            RoundOutcome::Tie => coefficient.tie_permutations += 1,
        }
        if result.outcome() == RoundOutcome::Banker && result.banker_total() == 6 {
            coefficient.banker_win_on_six_permutations += 1;
        }
        return;
    }

    for point in 0_u8..=9 {
        points[position] = point;
        multiplicities[usize::from(point)] += 1;
        enumerate_abstract_sequences(position + 1, points, multiplicities, coefficients);
        multiplicities[usize::from(point)] -= 1;
    }
}

/// 六个候选位置中，真实牌局可能只使用前四、五或六个位置。
///
/// 逐个尝试合法前缀可复用生产补牌规则；自然牌或停牌后未使用的位置仍属于
/// 共同六张分母中的补全位置，与原递归枚举器语义相同。
fn resolve_six_position_sequence(points: &[u8; 6]) -> super::round::PointRoundResult {
    for used in 4..=6 {
        match resolve_point_round(&points[..used]) {
            Ok(result) => return result,
            Err(
                RoundError::NotEnoughInitialCards
                | RoundError::MissingPlayerThirdCard
                | RoundError::MissingBankerThirdCard,
            ) => continue,
            Err(RoundError::UnexpectedExtraCards) => {
                unreachable!("从四张开始递增前缀，不会越过已经完成的牌局")
            }
        }
    }

    unreachable!("标准百家乐最多使用六张牌")
}

#[cfg(test)]
mod tests {
    use crate::{Card, Rank, Shoe, Suit};

    use super::{COMPOSITION_COUNT, calculate_main_outcomes, composition_table};

    fn card(input: &str) -> Card {
        input.parse().expect("测试使用的牌面必须合法")
    }

    #[test]
    fn coefficient_table_contains_every_six_card_composition() {
        assert_eq!(composition_table().len(), COMPOSITION_COUNT);
        assert!(composition_table().iter().all(|coefficient| {
            coefficient.multiplicities.iter().sum::<u8>() == 6
                && coefficient.player_permutations
                    + coefficient.banker_permutations
                    + coefficient.tie_permutations
                    > 0
        }));
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
        assert_eq!(weights.banker_win_on_six_weight(), 0);
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
        assert!(weights.banker_win_on_six_weight() > 0);
        assert!(weights.banker_win_on_six_weight() <= weights.banker_weight());
    }
}
