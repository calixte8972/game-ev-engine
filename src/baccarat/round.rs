//! 标准百家乐一局的发牌流程与最终结果。

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

    Ok(RoundResult::new(player_hand, banker_hand))
}

#[cfg(test)]
mod tests {
    use crate::Card;

    use super::{RoundError, RoundOutcome, resolve_round};

    fn cards(input: &str) -> Vec<Card> {
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
}
