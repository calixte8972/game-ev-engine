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
//!
//! 可以把本文件理解成两个阶段：
//!
//! 1. `build_composition_table` 只根据百家乐规则，把每一种六点数序列归类，
//!    这一步与当前牌靴无关，并通过 `OnceLock` 在进程内只做一次；
//! 2. `point_outcomes` 读取当前牌靴的点数数量，为每个组合计算物理权重，
//!    再把系数表中的各个结果桶加权汇总。
//!
//! 因此“规则判定”与“牌靴组成”是分离的：规则表只需要预计算一次，牌靴每局
//! 变化时只重新计算下降阶乘和加权求和。这也是回放大量局数时的主要性能来源。

use std::{collections::HashMap, sync::OnceLock};

use crate::{OutcomeWeights, ProbabilityError, RoundError, RoundOutcome, Shoe, SideBetWeights};

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
    /// 该项组合在六个发牌位置中各点数出现的次数。
    multiplicities: [u8; 10],
    // 下列字段统计同一个点数组成的所有有序排列中，分别有多少排列会落入
    // 对应结果或赔率档位。它们只统计“点数排列”，还没有乘入当前牌靴的
    // 具体物理牌数量；真正的物理权重在 point_outcomes() 中再计算。
    player_permutations: u16,
    banker_permutations: u16,
    tie_permutations: u16,
    banker_win_on_six_permutations: u16,
    lucky_seven_two_cards_permutations: u16,
    lucky_seven_three_cards_permutations: u16,
    super_lucky_seven_four_cards_permutations: u16,
    super_lucky_seven_five_cards_permutations: u16,
    super_lucky_seven_six_cards_permutations: u16,
    lucky_six_two_cards_permutations: u16,
    lucky_six_three_cards_permutations: u16,
    banker_dragon_bonus_tier_permutations: [u16; 6],
    banker_dragon_bonus_push_permutations: u16,
    player_dragon_bonus_tier_permutations: [u16; 6],
    player_dragon_bonus_push_permutations: u16,
    small_permutations: u16,
    big_permutations: u16,
}

/// 全进程共享系数表。同一 WASM 实例内的所有牌靴状态只初始化一次。
static COMPOSITION_TABLE: OnceLock<Vec<CompositionCoefficient>> = OnceLock::new();

/// 根据当前牌靴精确计算下一局庄、闲、和的概率权重。
pub fn calculate_main_outcomes(shoe: &Shoe) -> Result<OutcomeWeights, ProbabilityError> {
    point_outcomes(shoe)?.main_weights()
}

/// 根据当前牌靴计算对子、完美对子、幸运 7 和超级幸运 7 权重。
pub fn calculate_side_bet_outcomes(shoe: &Shoe) -> Result<SideBetWeights, ProbabilityError> {
    let point = point_outcomes(shoe)?;
    let pairs = pair_weights(shoe)?;
    Ok(point.side_bet_weights(pairs))
}

/// 一次点数枚举同时返回主注和边注权重，供浏览器避免重复遍历系数表。
pub fn calculate_main_and_side_outcomes(
    shoe: &Shoe,
) -> Result<(OutcomeWeights, SideBetWeights), ProbabilityError> {
    let point = point_outcomes(shoe)?;
    let main = point.main_weights()?;
    let sides = point.side_bet_weights(pair_weights(shoe)?);
    Ok((main, sides))
}

/// 一次点数聚合枚举的内部累计结果。
#[derive(Debug, Clone, Copy)]
struct PointOutcomeAccumulator {
    /// 当前牌靴剩余的具体牌总数，用来生成统一的六张有序序列分母。
    total_cards: u16,
    player: u64,
    banker: u64,
    tie: u64,
    banker_win_on_six: u64,
    lucky_seven_two_cards: u64,
    lucky_seven_three_cards: u64,
    super_lucky_seven_four_cards: u64,
    super_lucky_seven_five_cards: u64,
    super_lucky_seven_six_cards: u64,
    lucky_six_two_cards: u64,
    lucky_six_three_cards: u64,
    banker_dragon_bonus_tiers: [u64; 6],
    banker_dragon_bonus_push: u64,
    player_dragon_bonus_tiers: [u64; 6],
    player_dragon_bonus_push: u64,
    small: u64,
    big: u64,
}

impl PointOutcomeAccumulator {
    /// 把内部累计桶转换成带完整性校验的主注权重。
    ///
    /// 在枚举过程中不能直接构造 [`OutcomeWeights`]，因为它要求庄、闲、和
    /// 三个桶已经覆盖完整分母。等所有组合都累计完成后再转换，能把校验集中
    /// 在一个地方，并让中途的可变累加状态保持简单。
    fn main_weights(self) -> Result<OutcomeWeights, ProbabilityError> {
        OutcomeWeights::from_detailed_weights(
            self.total_cards,
            self.player,
            self.banker,
            self.tie,
            self.banker_win_on_six,
        )
    }

    /// 把点数枚举结果和具体牌 Rank/花色枚举结果合并成边注权重。
    ///
    /// 大小、幸运 6/7、龙宝只依赖点数与牌局结构，所以可以直接复用本对象；
    /// 只有对子和完美对子需要额外查看前四张具体牌，故通过 `pairs` 参数补入。
    fn side_bet_weights(self, pairs: PairWeights) -> SideBetWeights {
        let total = falling_factorial(self.total_cards, 6);
        SideBetWeights::new(
            total,
            pairs.any,
            pairs.banker,
            pairs.player,
            pairs.perfect,
            self.big,
            self.small,
            self.lucky_seven_two_cards,
            self.lucky_seven_three_cards,
            self.super_lucky_seven_four_cards,
            self.super_lucky_seven_five_cards,
            self.super_lucky_seven_six_cards,
            self.lucky_six_two_cards,
            self.lucky_six_three_cards,
            self.banker_dragon_bonus_tiers,
            self.banker_dragon_bonus_push,
            self.player_dragon_bonus_tiers,
            self.player_dragon_bonus_push,
        )
    }
}

fn point_outcomes(shoe: &Shoe) -> Result<PointOutcomeAccumulator, ProbabilityError> {
    let total_cards = shoe.total_remaining();
    // 所有终局都统一扩展到六张有序序列。少于六张时，即使某些规则只用
    // 四张牌也没有足够的“补全位置”来建立共同分母，所以直接拒绝。
    if total_cards < 6 {
        return Err(ProbabilityError::NotEnoughCards {
            remaining: total_cards,
        });
    }

    let point_counts = shoe.baccarat_point_counts();
    // 从这里开始不再区分具体牌面。例如 A、2、3、4 会分别落在 1、2、3、4
    // 点；10/J/Q/K 都落在点数 0。对子所需的 Rank 信息由 pair_weights 单独处理。

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
    let mut lucky_seven_two_cards = 0_u64;
    let mut lucky_seven_three_cards = 0_u64;
    let mut super_lucky_seven_four_cards = 0_u64;
    let mut super_lucky_seven_five_cards = 0_u64;
    let mut super_lucky_seven_six_cards = 0_u64;
    let mut lucky_six_two_cards = 0_u64;
    let mut lucky_six_three_cards = 0_u64;
    let mut banker_dragon_bonus_tiers = [0_u64; 6];
    let mut banker_dragon_bonus_push = 0_u64;
    let mut player_dragon_bonus_tiers = [0_u64; 6];
    let mut player_dragon_bonus_push = 0_u64;
    let mut small = 0_u64;
    let mut big = 0_u64;

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

        // 一项的总贡献 = 当前牌靴能实现这项点数组合的物理序列数 ×
        // 该组合中属于某个结果的点数排列数。每个结果桶都使用同一项物理权重，
        // 因为它们只是同一批抽牌序列按规则划分后的不同集合。
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
        lucky_seven_two_cards = add_weight(
            lucky_seven_two_cards,
            physical_sequences_per_permutation,
            coefficient.lucky_seven_two_cards_permutations,
        )?;
        lucky_seven_three_cards = add_weight(
            lucky_seven_three_cards,
            physical_sequences_per_permutation,
            coefficient.lucky_seven_three_cards_permutations,
        )?;
        super_lucky_seven_four_cards = add_weight(
            super_lucky_seven_four_cards,
            physical_sequences_per_permutation,
            coefficient.super_lucky_seven_four_cards_permutations,
        )?;
        super_lucky_seven_five_cards = add_weight(
            super_lucky_seven_five_cards,
            physical_sequences_per_permutation,
            coefficient.super_lucky_seven_five_cards_permutations,
        )?;
        super_lucky_seven_six_cards = add_weight(
            super_lucky_seven_six_cards,
            physical_sequences_per_permutation,
            coefficient.super_lucky_seven_six_cards_permutations,
        )?;
        lucky_six_two_cards = add_weight(
            lucky_six_two_cards,
            physical_sequences_per_permutation,
            coefficient.lucky_six_two_cards_permutations,
        )?;
        lucky_six_three_cards = add_weight(
            lucky_six_three_cards,
            physical_sequences_per_permutation,
            coefficient.lucky_six_three_cards_permutations,
        )?;
        for tier in 0..6 {
            banker_dragon_bonus_tiers[tier] = add_weight(
                banker_dragon_bonus_tiers[tier],
                physical_sequences_per_permutation,
                coefficient.banker_dragon_bonus_tier_permutations[tier],
            )?;
            player_dragon_bonus_tiers[tier] = add_weight(
                player_dragon_bonus_tiers[tier],
                physical_sequences_per_permutation,
                coefficient.player_dragon_bonus_tier_permutations[tier],
            )?;
        }
        banker_dragon_bonus_push = add_weight(
            banker_dragon_bonus_push,
            physical_sequences_per_permutation,
            coefficient.banker_dragon_bonus_push_permutations,
        )?;
        player_dragon_bonus_push = add_weight(
            player_dragon_bonus_push,
            physical_sequences_per_permutation,
            coefficient.player_dragon_bonus_push_permutations,
        )?;
        small = add_weight(
            small,
            physical_sequences_per_permutation,
            coefficient.small_permutations,
        )?;
        big = add_weight(
            big,
            physical_sequences_per_permutation,
            coefficient.big_permutations,
        )?;
    }

    // 此时每个可达的六点数组合都已处理完。主注转换器会继续验证三种结果
    // 是否恰好覆盖 falling_factorial(total_cards, 6)。
    Ok(PointOutcomeAccumulator {
        total_cards,
        player,
        banker,
        tie,
        banker_win_on_six,
        lucky_seven_two_cards,
        lucky_seven_three_cards,
        super_lucky_seven_four_cards,
        super_lucky_seven_five_cards,
        super_lucky_seven_six_cards,
        lucky_six_two_cards,
        lucky_six_three_cards,
        banker_dragon_bonus_tiers,
        banker_dragon_bonus_push,
        player_dragon_bonus_tiers,
        player_dragon_bonus_push,
        small,
        big,
    })
}

/// 对子只依赖前四张牌的 Rank；终局后再用任意两张补齐统一六张分母。
#[derive(Debug, Default, Clone, Copy)]
struct PairWeights {
    any: u64,
    banker: u64,
    player: u64,
    perfect: u64,
}

fn pair_weights(shoe: &Shoe) -> Result<PairWeights, ProbabilityError> {
    let total_cards = shoe.total_remaining();
    // 前四张决定庄对/闲对；统一六张分母还需要为后两张保留任意排列。
    if total_cards < 6 {
        return Err(ProbabilityError::NotEnoughCards {
            remaining: total_cards,
        });
    }

    let mut counts = shoe.rank_counts();
    // 这里是 13 个 Rank 的计数，而不是 52 个具体牌的计数。普通对子只问
    // “两张 Rank 是否相同”，因此不同花色、不同副本应作为同一 Rank 的可选项。
    let completion_weight = falling_factorial(total_cards - 4, 2);
    let mut result = PairWeights {
        // 完美对子必须保留花色和具体牌身份，所以这部分不能从 13 类 counts
        // 推导，而要使用 52 类 card_counts 单独做一次容斥计算。
        perfect: perfect_pair_first_four_weight(shoe)?
            .checked_mul(completion_weight)
            .ok_or(ProbabilityError::WeightOverflow)?,
        ..PairWeights::default()
    };

    // 发牌顺序仍是 P1、B1、P2、B2。循环中的 copies 让同 Rank 下不同花色、
    // 不同副牌的物理牌都被正确计入，同时扣减 counts 表达不放回抽牌。
    for player_first in 0..counts.len() {
        let player_first_copies = counts[player_first];
        if player_first_copies == 0 {
            continue;
        }
        counts[player_first] -= 1;

        for banker_first in 0..counts.len() {
            let banker_first_copies = counts[banker_first];
            if banker_first_copies == 0 {
                continue;
            }
            counts[banker_first] -= 1;

            for player_second in 0..counts.len() {
                let player_second_copies = counts[player_second];
                if player_second_copies == 0 {
                    continue;
                }
                counts[player_second] -= 1;

                for (banker_second, &banker_second_copies) in counts.iter().enumerate() {
                    if banker_second_copies == 0 {
                        continue;
                    }

                    let first_four_weight = u64::from(player_first_copies)
                        .checked_mul(u64::from(banker_first_copies))
                        .and_then(|weight| weight.checked_mul(u64::from(player_second_copies)))
                        .and_then(|weight| weight.checked_mul(u64::from(banker_second_copies)))
                        .ok_or(ProbabilityError::WeightOverflow)?;
                    let weight = first_four_weight
                        .checked_mul(completion_weight)
                        .ok_or(ProbabilityError::WeightOverflow)?;
                    let player_pair = player_first == player_second;
                    let banker_pair = banker_first == banker_second;

                    // first_four_weight 是按四个发牌位置依次选择具体 Rank 的
                    // 有序数量；completion_weight 再把已经结束后的两个无关位置
                    // 补进共同六张分母。any_pair 使用“或”，故不会重复计算双方
                    // 同时成对的同一条序列。
                    if player_pair {
                        result.player = result
                            .player
                            .checked_add(weight)
                            .ok_or(ProbabilityError::WeightOverflow)?;
                    }
                    if banker_pair {
                        result.banker = result
                            .banker
                            .checked_add(weight)
                            .ok_or(ProbabilityError::WeightOverflow)?;
                    }
                    if player_pair || banker_pair {
                        result.any = result
                            .any
                            .checked_add(weight)
                            .ok_or(ProbabilityError::WeightOverflow)?;
                    }
                }

                // 离开 player_second 分支前恢复它，保证下一种第二张牌看到的是
                // 同一个父节点状态。下面两层恢复逻辑与具体牌回溯器相同。
                counts[player_second] += 1;
            }

            counts[banker_first] += 1;
        }

        counts[player_first] += 1;
    }

    Ok(result)
}

/// 计算前四张牌中“闲完美对子或庄完美对子”的有序物理序列数。
///
/// 完美对子要求两张牌是完全相同的具体牌，例如两张黑桃 A。八副牌里每种
/// 具体牌最多有 8 张，所以不能只看 Rank 聚合数量。
///
/// 设事件 P 为闲家 P1/P2 完美成对，事件 B 为庄家 B1/B2 完美成对：
///
/// ```text
/// |P ∪ B| = |P| + |B| - |P ∩ B|
/// ```
///
/// `single_hand` 计算一方完美成对后，另一方任取两张的序列数；两方对称，
/// 所以乘 2。`both_hands` 再扣除同时命中的重复部分。这样只需遍历 52² 种
/// 具体牌组合，不需要枚举 52⁴ 条前四张序列。
fn perfect_pair_first_four_weight(shoe: &Shoe) -> Result<u64, ProbabilityError> {
    let counts = shoe.card_counts();
    let total = shoe.total_remaining();
    let other_hand_weight = falling_factorial(total - 2, 2);

    let mut single_hand = 0_u64;
    for &count in &counts {
        let pair_weight = if count >= 2 {
            falling_factorial(u16::from(count), 2)
        } else {
            0
        };
        single_hand = single_hand
            .checked_add(
                pair_weight
                    .checked_mul(other_hand_weight)
                    .ok_or(ProbabilityError::WeightOverflow)?,
            )
            .ok_or(ProbabilityError::WeightOverflow)?;
    }

    let mut both_hands = 0_u64;
    for (player_card, &player_count) in counts.iter().enumerate() {
        for (banker_card, &banker_count) in counts.iter().enumerate() {
            let weight = if player_card == banker_card {
                if player_count >= 4 {
                    falling_factorial(u16::from(player_count), 4)
                } else {
                    0
                }
            } else {
                let player_pair = if player_count >= 2 {
                    falling_factorial(u16::from(player_count), 2)
                } else {
                    0
                };
                let banker_pair = if banker_count >= 2 {
                    falling_factorial(u16::from(banker_count), 2)
                } else {
                    0
                };
                player_pair
                    .checked_mul(banker_pair)
                    .ok_or(ProbabilityError::WeightOverflow)?
            };
            both_hands = both_hands
                .checked_add(weight)
                .ok_or(ProbabilityError::WeightOverflow)?;
        }
    }

    single_hand
        .checked_mul(2)
        .and_then(|both_sides| both_sides.checked_sub(both_hands))
        .ok_or(ProbabilityError::WeightOverflow)
}

/// 把“一种排列的物理权重 × 该结果排列数”安全加入结果桶。
fn add_weight(
    current: u64,
    physical_weight: u64,
    permutations: u16,
) -> Result<u64, ProbabilityError> {
    // permutations 来自静态规则表，physical_weight 来自当前牌靴；两者相乘
    // 后才是这一结果桶实际覆盖的有序物理发牌序列数。
    let contribution = physical_weight
        .checked_mul(u64::from(permutations))
        .ok_or(ProbabilityError::WeightOverflow)?;
    current
        .checked_add(contribution)
        .ok_or(ProbabilityError::WeightOverflow)
}

/// 返回进程内共享的组合系数表。
///
/// `OnceLock` 保证第一次调用时完成初始化，之后只读复用；这对 CLI、WASM 页面
/// 和 CSV 回放都有效，但只限于同一个进程/WASM 实例。它不会把不同牌靴的状态
/// 混在一起，因为表中没有任何牌靴数量，牌靴数据始终由调用者单独传入。
fn composition_table() -> &'static [CompositionCoefficient] {
    COMPOSITION_TABLE.get_or_init(build_composition_table)
}

/// 一次性把 10^6 种六张点数顺序归并为 5,005 项。
///
/// `HashMap` 的键是十种点数的出现次数，而不是六个位置的顺序。这样所有
/// 具有相同多重集合的序列共享一个条目；值中的各类 permutation 字段再记录
/// 这些序列分别属于哪些结果和边注档位。
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
        // 叶节点代表一条完整的六点数有序序列。先用统一回合解析器确定
        // 实际只用 4/5/6 张中的哪一种，再把该叶节点归入对应组合桶。
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
                    lucky_seven_two_cards_permutations: 0,
                    lucky_seven_three_cards_permutations: 0,
                    super_lucky_seven_four_cards_permutations: 0,
                    super_lucky_seven_five_cards_permutations: 0,
                    super_lucky_seven_six_cards_permutations: 0,
                    lucky_six_two_cards_permutations: 0,
                    lucky_six_three_cards_permutations: 0,
                    banker_dragon_bonus_tier_permutations: [0; 6],
                    banker_dragon_bonus_push_permutations: 0,
                    player_dragon_bonus_tier_permutations: [0; 6],
                    player_dragon_bonus_push_permutations: 0,
                    small_permutations: 0,
                    big_permutations: 0,
                });

        // 主注结果三选一；这三项对每一条完整六点数序列都是互斥且穷尽的。
        match result.outcome() {
            RoundOutcome::Player => coefficient.player_permutations += 1,
            RoundOutcome::Banker => coefficient.banker_permutations += 1,
            RoundOutcome::Tie => coefficient.tie_permutations += 1,
        }
        if result.outcome() == RoundOutcome::Banker && result.banker_total() == 6 {
            // 庄六是庄赢的子集，同时还是幸运六的命中集合；这里分别记桶，
            // 后面才能用不同用途的赔率读取相同的底层路径。
            coefficient.banker_win_on_six_permutations += 1;
            if result.banker_card_count() == 2 {
                coefficient.lucky_six_two_cards_permutations += 1;
            } else {
                coefficient.lucky_six_three_cards_permutations += 1;
            }
        }
        if result.card_count() == 4 {
            // 大小只看最终实际发牌张数。四张是 Small，五/六张是 Big，
            // 所以两个桶在所有可达序列上应当互斥并覆盖全部序列。
            coefficient.small_permutations += 1;
        } else {
            coefficient.big_permutations += 1;
        }
        if result.outcome() == RoundOutcome::Player && result.player_total() == 7 {
            // Lucky Seven 先筛选“闲赢且闲为 7”，再按闲家用了两张还是三张
            // 分档；Super Lucky Seven 在同一筛选内再要求庄为 6。
            if result.player_card_count() == 2 {
                coefficient.lucky_seven_two_cards_permutations += 1;
            } else {
                coefficient.lucky_seven_three_cards_permutations += 1;
            }

            if result.banker_total() == 6 {
                match result.card_count() {
                    4 => coefficient.super_lucky_seven_four_cards_permutations += 1,
                    5 => coefficient.super_lucky_seven_five_cards_permutations += 1,
                    6 => coefficient.super_lucky_seven_six_cards_permutations += 1,
                    _ => unreachable!("百家乐终局只能使用四至六张牌"),
                }
            }
        }

        let player_natural =
            result.player_card_count() == 2 && matches!(result.player_total(), 8 | 9);
        let banker_natural =
            result.banker_card_count() == 2 && matches!(result.banker_total(), 8 | 9);
        match result.outcome() {
            // 龙宝的 Natural 不是普通点差赔率，而是退回本金的 Push；
            // 非 Natural 的胜方才按点差 4～9 进入赔率数组。
            RoundOutcome::Player if player_natural => {
                coefficient.player_dragon_bonus_push_permutations += 1;
            }
            RoundOutcome::Banker if banker_natural => {
                coefficient.banker_dragon_bonus_push_permutations += 1;
            }
            RoundOutcome::Tie if player_natural && banker_natural => {
                coefficient.player_dragon_bonus_push_permutations += 1;
                coefficient.banker_dragon_bonus_push_permutations += 1;
            }
            RoundOutcome::Player => {
                let margin = result.player_total() - result.banker_total();
                if margin >= 4 {
                    coefficient.player_dragon_bonus_tier_permutations[usize::from(margin - 4)] += 1;
                }
            }
            RoundOutcome::Banker => {
                let margin = result.banker_total() - result.player_total();
                if margin >= 4 {
                    coefficient.banker_dragon_bonus_tier_permutations[usize::from(margin - 4)] += 1;
                }
            }
            RoundOutcome::Tie => {}
        }
        return;
    }

    // 非叶节点：给当前位置依次放入 0～9 点，并在递归返回后撤销计数。
    // `multiplicities` 的加一/减一是回溯不变量；如果忘记减一，后续叶节点
    // 就会把父分支的点数数量带进来，最终不会得到正确的 5,005 个组合。
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
    // 同一条六点数序列先尝试四张，再尝试五张，最后尝试六张。
    // `resolve_point_round` 返回“缺牌”时，表示前缀还没有走完，而不是规则
    // 错误；因此继续扩大前缀。第一次成功的前缀就是实际终局长度。
    for used in 4..=6 {
        match resolve_point_round(&points[..used]) {
            // 即使牌局在四或五张已经结束，后面的点数仍作为统一六张分母的
            // 补全位置存在，但不再影响本局结果；这里直接返回实际终局结果。
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
                && coefficient.small_permutations + coefficient.big_permutations
                    == coefficient.player_permutations
                        + coefficient.banker_permutations
                        + coefficient.tie_permutations
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
