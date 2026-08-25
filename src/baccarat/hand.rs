//! 百家乐手牌的数据结构与点数计算。

use crate::Card;

/// 一方的百家乐手牌，由两张起手牌和一张可选的第三张牌组成。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BaccaratHand {
    /// 按发牌顺序记录的第一张起手牌。
    first_card: Card,
    /// 按发牌顺序记录的第二张起手牌。
    second_card: Card,
    /// 只有补牌规则要求时才存在；`None` 表示该方停牌。
    third_card: Option<Card>,
}

impl BaccaratHand {
    /// 创建一手没有第三张牌的两张牌手牌。
    pub const fn new(first_card: Card, second_card: Card) -> Self {
        Self {
            first_card,
            second_card,
            third_card: None,
        }
    }

    /// 创建一手包含第三张牌的三张牌手牌。
    pub const fn with_third(first_card: Card, second_card: Card, third_card: Card) -> Self {
        Self {
            first_card,
            second_card,
            third_card: Some(third_card),
        }
    }

    /// 返回起手第一张牌。
    pub const fn first_card(self) -> Card {
        self.first_card
    }

    /// 返回起手第二张牌。
    pub const fn second_card(self) -> Card {
        self.second_card
    }

    /// 返回可选的第三张牌。
    pub const fn third_card(self) -> Option<Card> {
        self.third_card
    }

    /// 计算起手两张牌的点数，只保留个位数。
    pub const fn initial_total(self) -> u8 {
        // `% 10` 对应百家乐“只取总点数个位”的规则。
        (self.first_card.baccarat_value() + self.second_card.baccarat_value()) % 10
    }

    /// 计算包含可选第三张牌在内的最终点数，只保留个位数。
    pub const fn total(self) -> u8 {
        // 初始点数必须保留下来，因为庄家补牌规则关心的是起手点数；
        // 最终点数才把可选第三张牌加进去。
        match self.third_card {
            Some(third_card) => (self.initial_total() + third_card.baccarat_value()) % 10,
            None => self.initial_total(),
        }
    }

    /// 返回当前手牌的牌数。
    pub const fn card_count(self) -> u8 {
        match self.third_card {
            Some(_) => 3,
            None => 2,
        }
    }

    /// 判断该手牌是否为起手两张组成的 Natural 8 或 Natural 9。
    ///
    /// 三张牌凑成的 8 或 9 不属于 Natural，所以还必须确认第三张牌不存在。
    pub const fn is_natural(self) -> bool {
        self.third_card.is_none() && matches!(self.initial_total(), 8 | 9)
    }
}

#[cfg(test)]
mod tests {
    use super::BaccaratHand;
    use crate::Card;

    fn card(input: &str) -> Card {
        input.parse().expect("测试使用的牌面必须合法")
    }

    #[test]
    fn two_card_hand_keeps_cards_and_has_no_third_card() {
        let first = card("AS");
        let second = card("3H");
        let hand = BaccaratHand::new(first, second);

        assert_eq!(hand.first_card(), first);
        assert_eq!(hand.second_card(), second);
        assert_eq!(hand.third_card(), None);
        assert_eq!(hand.card_count(), 2);
    }

    #[test]
    fn two_card_total_uses_the_ones_digit() {
        let hand = BaccaratHand::new(card("7S"), card("8H"));

        assert_eq!(hand.initial_total(), 5);
        assert_eq!(hand.total(), 5);
        assert!(!hand.is_natural());
    }

    #[test]
    fn ten_and_face_cards_are_zero_points() {
        let hand = BaccaratHand::new(card("KS"), card("QH"));

        assert_eq!(hand.initial_total(), 0);
        assert_eq!(hand.total(), 0);
        assert!(!hand.is_natural());
    }

    #[test]
    fn two_card_eight_is_natural() {
        let hand = BaccaratHand::new(card("AS"), card("7H"));

        assert_eq!(hand.initial_total(), 8);
        assert_eq!(hand.total(), 8);
        assert_eq!(hand.card_count(), 2);
        assert!(hand.is_natural());
    }

    #[test]
    fn two_card_nine_is_natural() {
        let hand = BaccaratHand::new(card("AS"), card("8H"));

        assert_eq!(hand.initial_total(), 9);
        assert_eq!(hand.total(), 9);
        assert_eq!(hand.card_count(), 2);
        assert!(hand.is_natural());
    }

    #[test]
    fn third_card_changes_only_the_final_total() {
        let third = card("8D");
        let hand = BaccaratHand::with_third(card("9S"), card("7H"), third);

        assert_eq!(hand.initial_total(), 6);
        assert_eq!(hand.total(), 4);
        assert_eq!(hand.card_count(), 3);
        assert_eq!(hand.third_card(), Some(third));
        assert!(!hand.is_natural());
    }

    #[test]
    fn three_card_eight_is_not_natural() {
        let hand = BaccaratHand::with_third(card("2S"), card("3H"), card("3D"));

        assert_eq!(hand.initial_total(), 5);
        assert_eq!(hand.total(), 8);
        assert_eq!(hand.card_count(), 3);
        assert!(!hand.is_natural());
    }

    #[test]
    fn hand_with_a_third_card_is_not_natural_even_if_initial_total_is_eight() {
        let hand = BaccaratHand::with_third(card("AS"), card("7H"), card("3D"));

        assert_eq!(hand.initial_total(), 8);
        assert_eq!(hand.card_count(), 3);
        assert!(!hand.is_natural());
    }
}
