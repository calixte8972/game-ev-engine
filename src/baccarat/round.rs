//! 标准百家乐一局的发牌流程与最终结果。
//!
//! 输入必须按真实发牌顺序排列：
//!
//! ```text
//! P1 -> B1 -> P2 -> B2 -> 可选 P3 -> 可选 B3
//! ```
//!
//! 本文件有两个解析器，二者复用完全相同的补牌规则：
//!
//! - [`resolve_round`] 处理真正的 `Card`，用于 CLI、回放和参考枚举器；
//! - `resolve_point_round` 只处理 0～9 点，用于高速概率枚举器。
//!
//! 两个函数都不主动摸牌，只消费调用者给出的序列。如果规则要求补牌而
//! 输入还没有下一张，就返回“缺少补牌”错误；枚举器会把它解释成
//! “当前路径还没走完，继续枚举下一张”。

use std::{error::Error, fmt};

use crate::Card;

use super::{BaccaratHand, banker_should_draw, player_should_draw};

/// 标准百家乐主注的最终结果。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RoundOutcome {
    /// 闲家最终点数较高。
    Player,
    /// 庄家最终点数较高。
    Banker,
    /// 双方最终点数相同。
    Tie,
}

/// 一局结束后的双方手牌与主注结果。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct RoundResult {
    /// 闲家最终的两张或三张手牌。
    player_hand: BaccaratHand,
    /// 庄家最终的两张或三张手牌。
    banker_hand: BaccaratHand,
    /// 根据双方最终点数提前计算并保存的主注结果。
    outcome: RoundOutcome,
}

impl RoundResult {
    /// 根据双方最终手牌创建结果，并保证 `outcome` 与手牌点数一致。
    ///
    /// 构造函数保持私有，避免外部传入一个与双方手牌矛盾的结果字段。
    fn new(player_hand: BaccaratHand, banker_hand: BaccaratHand) -> Self {
        Self {
            player_hand,
            banker_hand,
            outcome: compare_hands(player_hand, banker_hand),
        }
    }

    /// 返回闲家最终手牌。
    pub const fn player_hand(self) -> BaccaratHand {
        self.player_hand
    }

    /// 返回庄家最终手牌。
    pub const fn banker_hand(self) -> BaccaratHand {
        self.banker_hand
    }

    /// 返回庄、闲或和的最终结果。
    pub const fn outcome(self) -> RoundOutcome {
        self.outcome
    }

    /// 返回本局实际开出的总牌数。
    pub const fn card_count(self) -> u8 {
        self.player_hand.card_count() + self.banker_hand.card_count()
    }
}

/// 比较双方最终点数，返回庄、闲或和。
pub const fn compare_hands(player_hand: BaccaratHand, banker_hand: BaccaratHand) -> RoundOutcome {
    let player_total = player_hand.total();
    let banker_total = banker_hand.total();

    if player_total > banker_total {
        RoundOutcome::Player
    } else if player_total < banker_total {
        RoundOutcome::Banker
    } else {
        RoundOutcome::Tie
    }
}

/// 解析具体牌序列失败时返回的错误。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RoundError {
    /// 当前序列不足四张起手牌，回合还不能判定。
    NotEnoughInitialCards,
    /// 按闲家规则必须补牌，但序列没有提供下一张。
    MissingPlayerThirdCard,
    /// 按庄家规则必须补牌，但序列没有提供下一张。
    MissingBankerThirdCard,
    /// 回合已经结束，但输入序列还有未被规则使用的牌。
    UnexpectedExtraCards,
}

impl fmt::Display for RoundError {
    /// 将结构化回合错误转换成便于上层展示的文本。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEnoughInitialCards => {
                formatter.write_str("a baccarat round requires four initial cards")
            }
            Self::MissingPlayerThirdCard => {
                formatter.write_str("the drawing rules require a player third card")
            }
            Self::MissingBankerThirdCard => {
                formatter.write_str("the drawing rules require a banker third card")
            }
            Self::UnexpectedExtraCards => {
                formatter.write_str("the round contains cards that the drawing rules do not use")
            }
        }
    }
}

impl Error for RoundError {}

/// 点数枚举器使用的精简回合结果。
///
/// 主注概率只关心庄闲最终点数和本局用牌数，不关心花色或具体牌面，
/// 所以不需要构造完整的 `BaccaratHand`。`pub(crate)` 表示只在当前
/// crate 内可见，不作为对外 API。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PointRoundResult {
    /// 闲家最终点数。
    player_total: u8,
    /// 庄家最终点数；概率层用它统计庄 6 点获胜子集。
    banker_total: u8,
    /// 回合实际消费的点数数量，可能是 4、5 或 6。
    // 生产枚举器优化后不再读取它，但保留该状态用于规则单元测试与教学验证。
    #[cfg_attr(not(test), allow(dead_code))]
    card_count: u8,
}

/// 按标准发牌顺序解析一个只含百家乐点数的回合。
///
/// 例如 `[1, 4, 7, 3]` 表示：闲第一张 1 点、庄第一张 4 点、
/// 闲第二张 7 点、庄第二张 3 点。闲起手 8 点是 Natural，因此四张结束。
pub(crate) fn resolve_point_round(points: &[u8]) -> Result<PointRoundResult, RoundError> {
    // 切片模式按顺序借用前四个点数。`..` 表示允许后面还有补牌；
    // let-else 在不足四项时立即走 else 错误分支。
    let [
        player_first_point,
        banker_first_point,
        player_second_point,
        banker_second_point,
        ..,
    ] = points
    else {
        return Err(RoundError::NotEnoughInitialCards);
    };

    // 模式匹配得到的是 &u8，前面的 * 解引用取出 u8 值。
    let player_initial = (*player_first_point + *player_second_point) % 10;

    let banker_initial = (*banker_first_point + *banker_second_point) % 10;

    // 先假设双方都不补牌，因此最终点数暂时等于起手点数。
    // 如果后面确定补牌，再覆盖对应 total。
    let mut player_total = player_initial;
    let mut banker_total = banker_initial;
    // 庄家补牌表需要知道闲家有没有补牌，以及补的是几点。
    let mut player_third_point: Option<u8> = None;
    // 前四张已经被起手牌消费，下一张可选补牌的下标是 4。
    let mut consumed: u8 = 4;

    let player_is_natural = matches!(player_initial, 8 | 9);

    let banker_is_natural = matches!(banker_initial, 8 | 9);

    // 任意一方是 Natural 8/9，整局必须立即结束，不查补牌表。
    if !player_is_natural && !banker_is_natural {
        if player_should_draw(player_initial) {
            // get() 返回 Option<&u8>，copied() 把它变成 Option<u8>，
            // ok_or() 在缺少这张牌时变成领域错误，? 再向上返回。
            let third_point = points
                .get(usize::from(consumed))
                .copied()
                .ok_or(RoundError::MissingPlayerThirdCard)?;

            player_third_point = Some(third_point);
            player_total = (player_initial + third_point) % 10;
            consumed += 1;
        }

        // 闲家如果补过牌，player_third_point 是 Some(点数)；否则是 None。
        if banker_should_draw(banker_initial, player_third_point) {
            let third_point = points
                .get(usize::from(consumed))
                .copied()
                .ok_or(RoundError::MissingBankerThirdCard)?;

            banker_total = (banker_initial + third_point) % 10;
            consumed += 1;
        }
    }

    // 规则需要的牌数必须与输入长度完全一致。例如 Natural 只用四张，
    // 如果调用者还给了第五张，这张就是非法多余输入。
    if usize::from(consumed) != points.len() {
        return Err(RoundError::UnexpectedExtraCards);
    }

    Ok(PointRoundResult {
        player_total,
        banker_total,
        card_count: consumed,
    })
}
impl PointRoundResult {
    /// 返回庄家最终点数，用于区分普通庄赢与庄 6 点赢。
    pub(crate) const fn banker_total(self) -> u8 {
        self.banker_total
    }

    /// 直接比较双方最终点数，得到庄、闲或和。
    pub(crate) const fn outcome(self) -> RoundOutcome {
        if self.player_total > self.banker_total {
            RoundOutcome::Player
        } else if self.player_total < self.banker_total {
            RoundOutcome::Banker
        } else {
            RoundOutcome::Tie
        }
    }
}

/// 按 `P1 → B1 → P2 → B2 → 可选 P3 → 可选 B3` 的顺序解析一局。
///
/// 该函数不会摸牌或修改牌靴，只验证调用者给出的牌序列是否恰好符合补牌规则。
/// “缺少下一张牌”使用 `RoundError` 表示，具体牌枚举器会把这些错误当作
/// “当前回合尚未结束，请继续枚举”的状态信号。
pub fn resolve_round(cards: &[Card]) -> Result<RoundResult, RoundError> {
    // 切片模式一次取出固定的四张起手牌；不足四张时直接返回对应状态。
    let [player_first, banker_first, player_second, banker_second, ..] = cards else {
        return Err(RoundError::NotEnoughInitialCards);
    };

    // `consumed` 记录规则实际使用了几张牌，最后用于拒绝多余输入。
    let mut consumed = 4;
    let mut player_hand = BaccaratHand::new(*player_first, *player_second);
    let mut banker_hand = BaccaratHand::new(*banker_first, *banker_second);

    // 任意一方为自然 8/9 时双方都不得补牌，整局在四张牌处结束。
    if !player_hand.is_natural() && !banker_hand.is_natural() {
        if player_should_draw(player_hand.initial_total()) {
            // `.get` 不会因下标越界而 panic；缺牌被转换成可匹配的领域错误。
            let third_card = cards
                .get(consumed)
                .copied()
                .ok_or(RoundError::MissingPlayerThirdCard)?;
            player_hand = BaccaratHand::with_third(*player_first, *player_second, third_card);
            consumed += 1;
        }

        // `map` 把 `Option<Card>` 转成 `Option<u8>`：闲未补牌仍为 None，
        // 闲已补牌则只把第三张牌的百家乐点数交给庄家规则。
        let player_third_value = player_hand.third_card().map(Card::baccarat_value);
        if banker_should_draw(banker_hand.initial_total(), player_third_value) {
            let third_card = cards
                .get(consumed)
                .copied()
                .ok_or(RoundError::MissingBankerThirdCard)?;
            banker_hand = BaccaratHand::with_third(*banker_first, *banker_second, third_card);
            consumed += 1;
        }
    }

    // 输入长度必须和规则实际消费的长度完全一致；否则调用者多提供了牌。
    if consumed != cards.len() {
        return Err(RoundError::UnexpectedExtraCards);
    }

    // 只要长度和规则都合法，就用最终手牌构造自洽的 RoundResult。
    Ok(RoundResult::new(player_hand, banker_hand))
}

#[cfg(test)]
mod tests {
    use crate::Card;

    use super::{RoundError, RoundOutcome, resolve_point_round, resolve_round};

    fn cards(input: &str) -> Vec<Card> {
        // split_whitespace 按任意数量的空白分隔；map 把每段文本变成 Card；
        // collect 最终收集为 Vec<Card>。
        input
            .split_whitespace()
            .map(|card| card.parse().expect("测试使用的牌面必须合法"))
            .collect()
    }

    #[test]
    fn resolves_all_necessary_dealing_paths() {
        let cases = [
            ("AS 4C 7H 3D", RoundOutcome::Player, 8, 7, 2, 2),
            ("KC 2C QH KS 5D 4H", RoundOutcome::Banker, 5, 6, 3, 3),
            ("2C 4H 3D 2S 5C", RoundOutcome::Banker, 0, 6, 3, 2),
            ("2C 2H 4D 3S 4C", RoundOutcome::Banker, 6, 9, 2, 3),
            ("2C 3H 4D 4S", RoundOutcome::Banker, 6, 7, 2, 2),
            ("2C 3H 4D 3S", RoundOutcome::Tie, 6, 6, 2, 2),
        ];

        for (input, outcome, player_total, banker_total, player_cards, banker_cards) in cases {
            let result = resolve_round(&cards(input)).expect("测试牌局应符合补牌规则");

            assert_eq!(result.outcome(), outcome, "发牌序列为 {input}");
            assert_eq!(
                result.player_hand().total(),
                player_total,
                "发牌序列为 {input}"
            );
            assert_eq!(
                result.banker_hand().total(),
                banker_total,
                "发牌序列为 {input}"
            );
            assert_eq!(
                result.player_hand().card_count(),
                player_cards,
                "发牌序列为 {input}"
            );
            assert_eq!(
                result.banker_hand().card_count(),
                banker_cards,
                "发牌序列为 {input}"
            );
            assert_eq!(
                result.card_count(),
                player_cards + banker_cards,
                "发牌序列为 {input}"
            );
        }
    }

    #[test]
    fn rejects_incomplete_or_extra_dealing_sequences() {
        let cases = [
            ("AS 2H 3D", RoundError::NotEnoughInitialCards),
            ("KC 2C QH 4S", RoundError::MissingPlayerThirdCard),
            ("2C 2H 4D 3S", RoundError::MissingBankerThirdCard),
            ("AS 4C 7H 3D 5S", RoundError::UnexpectedExtraCards),
        ];

        for (input, expected) in cases {
            assert_eq!(
                resolve_round(&cards(input)),
                Err(expected),
                "发牌序列为 {input}"
            );
        }
    }

    #[test]
    fn resolves_point_round_natural_after_four_cards() {
        let result = resolve_point_round(&[1, 4, 7, 3]).expect("Natural 牌局应在四张牌后结束");

        assert_eq!(result.player_total, 8);
        assert_eq!(result.banker_total, 7);
        assert_eq!(result.card_count, 4);
        assert_eq!(result.outcome(), RoundOutcome::Player);
    }

    #[test]
    fn resolves_point_round_when_both_sides_draw() {
        let result = resolve_point_round(&[0, 2, 0, 0, 5, 4]).expect("双方补牌的牌局应正常结束");

        assert_eq!(result.player_total, 5);
        assert_eq!(result.banker_total, 6);
        assert_eq!(result.card_count, 6);
        assert_eq!(result.outcome(), RoundOutcome::Banker);
    }

    #[test]
    fn resolves_point_round_when_player_stands_and_banker_draws() {
        let result =
            resolve_point_round(&[3, 0, 3, 1, 2]).expect("闲家停牌、庄家补牌的牌局应正常结束");

        assert_eq!(result.player_total, 6);
        assert_eq!(result.banker_total, 3);
        assert_eq!(result.card_count, 5);
        assert_eq!(result.outcome(), RoundOutcome::Player);
    }

    #[test]
    fn resolves_point_round_tie_after_four_cards() {
        let result = resolve_point_round(&[2, 3, 4, 3]).expect("四张牌和局应正常结束");

        assert_eq!(result.player_total, 6);
        assert_eq!(result.banker_total, 6);
        assert_eq!(result.card_count, 4);
        assert_eq!(result.outcome(), RoundOutcome::Tie);
    }

    #[test]
    fn point_round_reports_missing_cards_and_extra_cards() {
        assert_eq!(
            resolve_point_round(&[0, 2, 0, 0]),
            Err(RoundError::MissingPlayerThirdCard)
        );
        assert_eq!(
            resolve_point_round(&[3, 0, 3, 1]),
            Err(RoundError::MissingBankerThirdCard)
        );
        assert_eq!(
            resolve_point_round(&[1, 4, 7, 3, 5]),
            Err(RoundError::UnexpectedExtraCards)
        );
    }
}
