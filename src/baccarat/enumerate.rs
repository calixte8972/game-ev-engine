//! 按具体牌枚举标准百家乐主注结果。
//!
//! 这是概率引擎的“参考答案算法”：它直接遍历仍可能发出的具体牌面和花色，
//! 每条路径都交给已经测试过的 `resolve_round` 判定。实现容易理解和审查，
//! 但完整牌靴的分支数量过大，因此只在小牌靴测试中运行。
//!
//! 后续生产算法会把牌压缩成 0～9 共十种点数来提速。保留本模块的原因是：
//! 高速算法即使算出一个概率和为 1 的结果，也可能把庄、闲路径分错；把两种
//! 独立实现放在同一个小牌靴上比较，才能更可靠地发现这种“数字合理但结果错误”。

use crate::{Card, OutcomeWeights, ProbabilityError, Rank, RoundError, RoundOutcome, Shoe, Suit};

use super::{probability::falling_factorial, resolve_round};

/// 按具体牌面与花色枚举下一局的庄、闲、和权重。
///
/// 输入只借用 `Shoe`，函数内部克隆工作副本，所以计算结束后调用者的牌靴不变。
/// 该函数的复杂度不适合完整牌靴，目前只在 `#[cfg(test)]` 构建中存在。
pub(crate) fn enumerate_main_outcomes_by_card(
    shoe: &Shoe,
) -> Result<OutcomeWeights, ProbabilityError> {
    // 共同分母必须始终使用递归开始前的总牌数，不能使用分支中不断减少的数量。
    let initial_total = shoe.total_remaining();
    if initial_total < 6 {
        return Err(ProbabilityError::NotEnoughCards {
            remaining: initial_total,
        });
    }

    // 每个分支都在工作副本上原地扣牌并回溯，避免改变调用者持有的真实状态。
    let mut working_shoe = shoe.clone();
    // 标准百家乐最多发六张，预留容量可避免递归时 Vec 重新分配。
    let mut cards = Vec::with_capacity(6);
    let mut accumulator = OutcomeAccumulator::default();

    enumerate_paths(&mut working_shoe, &mut cards, 1, &mut accumulator)?;

    // 递归结束后，两条断言共同检查所有 push/remove 都有对应的 pop/restore。
    debug_assert_eq!(&working_shoe, shoe);
    debug_assert!(cards.is_empty());

    // 最终构造器还会验证 player + banker + tie 是否等于 `(initial_total)₆`。
    OutcomeWeights::from_weights(
        initial_total,
        accumulator.player,
        accumulator.banker,
        accumulator.tie,
    )
}

#[derive(Debug, Default)]
/// 递归过程中使用的可变结果桶。
///
/// `OutcomeWeights` 只允许表示已经完整验证的最终分布，所以不能一边递归一边
/// 构造它；先用本类型累计，全部路径结束后再交给 `OutcomeWeights` 校验。
struct OutcomeAccumulator {
    /// 当前已经发现的闲赢路径权重。
    player: u64,
    /// 当前已经发现的庄赢路径权重。
    banker: u64,
    /// 当前已经发现的和局路径权重。
    tie: u64,
}

impl OutcomeAccumulator {
    /// 把一个终局权重加入其庄、闲或和结果桶，并检查加法溢出。
    fn add(&mut self, outcome: RoundOutcome, weight: u64) -> Result<(), ProbabilityError> {
        // `destination` 是三个字段之一的可变引用，让后面的安全加法只写一次。
        let destination = match outcome {
            RoundOutcome::Player => &mut self.player,
            RoundOutcome::Banker => &mut self.banker,
            RoundOutcome::Tie => &mut self.tie,
        };

        // `checked_add` 在溢出时返回 None，再转换成统一的概率错误。
        *destination = destination
            .checked_add(weight)
            .ok_or(ProbabilityError::WeightOverflow)?;
        Ok(())
    }
}

/// 判断当前牌序列是否已经结束；未结束时递归枚举下一张牌。
///
/// `path_weight` 表示到达当前节点的物理发牌序列数量。例如某位置选择的牌
/// 还有 8 张副本，则进入该子分支时权重需要乘 8。
fn enumerate_paths(
    shoe: &mut Shoe,
    cards: &mut Vec<Card>,
    path_weight: u64,
    accumulator: &mut OutcomeAccumulator,
) -> Result<(), ProbabilityError> {
    // 同一个规则解析器既用于解析真实输入，也用于判断枚举过程还缺哪张牌，
    // 从而避免在概率代码中复制一份容易产生差异的补牌规则。
    match resolve_round(cards) {
        Ok(result) => {
            // 回合可能在第 4、5 或 6 张结束。为使所有路径可以直接相加，
            // 4/5 张终局必须用“后续任意牌排列数”扩展到统一的六张分母。
            let missing_cards = 6 - result.card_count();
            let completion_weight = falling_factorial(shoe.total_remaining(), missing_cards);
            // `path_weight` 代表已经实际发出的部分，`completion_weight` 代表
            // 终局后不影响结果、但在共同分母中仍需计入的剩余位置。
            let terminal_weight = path_weight
                .checked_mul(completion_weight)
                .ok_or(ProbabilityError::WeightOverflow)?;

            accumulator.add(result.outcome(), terminal_weight)
        }
        Err(
            RoundError::NotEnoughInitialCards
            | RoundError::MissingPlayerThirdCard
            | RoundError::MissingBankerThirdCard,
        ) => {
            // 这些不是算法失败，而是“当前序列尚未完成”的三种正常状态。
            enumerate_next_card(shoe, cards, path_weight, accumulator)
        }
        Err(RoundError::UnexpectedExtraCards) => {
            // 本函数只在 resolve_round 明确表示缺牌时才继续递归，理论上不可能多发。
            unreachable!("枚举器不应在牌局结束后继续发牌")
        }
    }
}

/// 遍历牌靴中仍有剩余的全部具体牌类别，并逐一进入下一层递归。
///
/// 循环遍历的是 52 种“牌面 + 花色”类别。多副牌里同一种具体牌的多个物理
/// 副本不重复建立分支，而是通过 `copies` 乘入路径权重。
fn enumerate_next_card(
    shoe: &mut Shoe,
    cards: &mut Vec<Card>,
    path_weight: u64,
    accumulator: &mut OutcomeAccumulator,
) -> Result<(), ProbabilityError> {
    for rank in Rank::ALL {
        for suit in Suit::ALL {
            let card = Card::new(rank, suit);
            let copies = shoe.remaining(card);
            if copies == 0 {
                continue;
            }

            // 例如当前路径权重为 12，而下一张具体牌还有 3 个副本，那么
            // 新路径代表 12 × 3 = 36 条物理发牌序列。
            let next_weight = path_weight
                .checked_mul(u64::from(copies))
                .ok_or(ProbabilityError::WeightOverflow)?;

            // 进入分支：从局部牌靴扣牌，并把它追加到发牌序列。
            shoe.remove(card).expect("剩余数量大于零的牌必须能够扣除");
            cards.push(card);

            let branch_result = enumerate_paths(shoe, cards, next_weight, accumulator);

            // 离开分支：无论递归成功还是失败，都必须先恢复现场。
            // 如果直接写 `enumerate_paths(...)?`，错误会提前返回，下面两步不会执行。
            cards.pop();
            shoe.restore(card).expect("递归分支扣除的牌必须能够恢复");

            // 现场已经恢复安全，此时才把子分支错误向上传播。
            branch_result?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Card, Rank, Shoe, Suit};

    use super::enumerate_main_outcomes_by_card;

    fn card(input: &str) -> Card {
        input.parse().expect("测试使用的牌面必须合法")
    }

    #[test]
    fn four_card_naturals_are_extended_to_the_six_card_denominator() {
        // 两副牌中只保留三种 4 点牌，每种两个副本，共六张牌。
        // 任意前四张都会让庄闲各得到 4 + 4 = Natural 8，所以一定四张结束并和局。
        let retained = [card("4C"), card("4D"), card("4H")];
        let mut removed = Vec::with_capacity(98);

        for rank in Rank::ALL {
            for suit in Suit::ALL {
                let candidate = Card::new(rank, suit);
                if !retained.contains(&candidate) {
                    // 当前是两副牌，因此每一种不保留的具体牌都需要扣除两次。
                    removed.extend([candidate; 2]);
                }
            }
        }

        let mut shoe = Shoe::new(2).expect("两副牌必须是合法牌靴");
        shoe.remove_many(&removed).expect("测试牌必须能够扣除");
        let initial_shoe = shoe.clone();

        let weights = enumerate_main_outcomes_by_card(&shoe).expect("六张测试牌必须能够完成枚举");

        assert_eq!(shoe, initial_shoe);
        // 六张物理牌的共同分母为 6! = 720；四张自然牌终局通过剩余
        // 两张牌的 2! 种排列补齐后，720 条六张序列全部属于和局。
        assert_eq!(weights.player_weight(), 0);
        assert_eq!(weights.banker_weight(), 0);
        assert_eq!(weights.tie_weight(), 720);
        assert_eq!(weights.total_weight(), 720);
    }
}
