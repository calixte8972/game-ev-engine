//! 紧凑的多副牌牌靴状态：每一种具体牌使用一个计数器。

use std::{error::Error, fmt};

use crate::{Card, Rank, Suit};

/// 本项目百家乐规则默认使用的副牌数。
pub const DEFAULT_DECKS: u8 = 8;

/// 第一个版本支持的副牌数范围。
pub const MIN_DECKS: u8 = 1;
/// 第一个版本允许创建的最大副牌数。
pub const MAX_DECKS: u8 = 8;

/// 多副牌牌靴中的剩余牌。
///
/// `counts[card.index()]` 保存对应牌面与花色组合的剩余张数。
/// 因此，一个八副牌牌靴只需要 52 个计数字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shoe {
    /// 初始牌靴的副牌数，同时也是每一种具体牌允许恢复到的容量上限。
    decks: u8,
    /// 52 种“牌面 + 花色”的剩余数量，位置由 `Card::index` 唯一确定。
    counts: [u8; Card::DISTINCT_COUNT],
    /// 所有具体牌剩余数量之和；单独缓存可避免每次查询都遍历 52 个计数。
    total: u16,
}

impl Shoe {
    /// 创建一个包含 `decks` 副标准 52 张扑克牌的完整牌靴。
    pub fn new(decks: u8) -> Result<Self, ShoeError> {
        if !(MIN_DECKS..=MAX_DECKS).contains(&decks) {
            return Err(ShoeError::InvalidDeckCount { decks });
        }

        Ok(Self {
            decks,
            counts: [decks; Card::DISTINCT_COUNT],
            total: u16::from(decks) * Card::DISTINCT_COUNT as u16,
        })
    }

    /// 创建该牌靴时使用的副牌数。
    pub const fn decks(&self) -> u8 {
        self.decks
    }

    /// 当前剩余的总牌数。
    pub const fn total_remaining(&self) -> u16 {
        self.total
    }

    /// 某一种具体牌当前的剩余张数。
    pub fn remaining(&self, card: Card) -> u8 {
        self.counts[card.index()]
    }

    /// 从牌靴中扣除一张已知的牌。
    pub fn remove(&mut self, card: Card) -> Result<(), ShoeError> {
        // 直接取得目标计数的可变引用，后续修改不会扫描整个数组。
        let remaining = &mut self.counts[card.index()];
        if *remaining == 0 {
            return Err(ShoeError::CardUnavailable {
                card,
                requested: 1,
                remaining: 0,
            });
        }

        *remaining -= 1;
        self.total -= 1;
        self.debug_assert_invariants();
        Ok(())
    }

    /// 恢复一张之前扣除的牌。
    pub fn restore(&mut self, card: Card) -> Result<(), ShoeError> {
        let remaining = &mut self.counts[card.index()];
        // 每种具体牌最初只有 `decks` 张，超过这个值说明恢复了未扣除的牌。
        if *remaining == self.decks {
            return Err(ShoeError::CardAtCapacity {
                card,
                capacity: self.decks,
            });
        }

        *remaining += 1;
        self.total += 1;
        self.debug_assert_invariants();
        Ok(())
    }

    /// 按百家乐点数聚合当前剩余牌，返回 0～9 点各自的剩余张数。
    ///
    /// 底层仍保留 52 种具体牌，确保对子等边注以后能够区分牌面和花色；
    /// 主注只依赖点数时，再临时压缩为 10 类以减少枚举分支。
    pub fn baccarat_point_counts(&self) -> [u16; 10] {
        let mut point_counts = [0_u16; 10];

        for rank in Rank::ALL {
            // 10、J、Q、K 都会聚合到下标 0，其余牌面聚合到自身点数。
            let point_index = usize::from(rank.baccarat_value());

            for suit in Suit::ALL {
                let card = Card::new(rank, suit);
                point_counts[point_index] += u16::from(self.remaining(card));
            }
        }

        debug_assert_eq!(point_counts.iter().sum::<u16>(), self.total);
        point_counts
    }

    /// 以原子方式扣除所有传入的牌。
    ///
    /// 修改牌靴前会先验证全部请求数量。如果任何一种牌数量不足，
    /// 整个操作都会失败，并且牌靴保持不变。
    pub fn remove_many(&mut self, cards: &[Card]) -> Result<(), ShoeError> {
        // 第一遍只统计请求，不修改牌靴。重复输入同一张牌时会累加到同一下标。
        let mut requested = [0_usize; Card::DISTINCT_COUNT];
        // 保存一种代表牌，用于数量不足时构造包含具体牌面的错误。
        let mut examples = [None; Card::DISTINCT_COUNT];

        for &card in cards {
            let index = card.index();
            requested[index] += 1;
            examples[index] = Some(card);
        }

        // 第二遍先验证全部请求。只有全部可满足，才进入后面的实际扣牌阶段，
        // 因而不会出现“扣了一半后才发现另一张牌不足”的部分修改。
        for index in 0..Card::DISTINCT_COUNT {
            let available = self.counts[index];
            if requested[index] > usize::from(available) {
                let card = examples[index].expect("a requested count always has an example card");
                return Err(ShoeError::CardUnavailable {
                    card,
                    requested: requested[index],
                    remaining: available,
                });
            }
        }

        let mut removed = 0_u16;
        // `zip` 把每个剩余计数与相同下标的请求数成对处理。
        for (remaining, requested) in self.counts.iter_mut().zip(requested) {
            // 单类牌最多只有 8 张，且前面已经验证过数量，因此这里转换为 u8 安全。
            let requested = requested as u8;
            *remaining -= requested;
            removed += u16::from(requested);
        }
        self.total -= removed;

        self.debug_assert_invariants();
        Ok(())
    }

    /// 在调试和测试构建中检查牌靴内部状态是否自洽。
    ///
    /// `debug_assert!` 在优化后的 release 构建中会被移除，不影响正式计算性能。
    fn debug_assert_invariants(&self) {
        debug_assert!(self.counts.iter().all(|&count| count <= self.decks));
        debug_assert_eq!(
            self.counts
                .iter()
                .map(|&count| u16::from(count))
                .sum::<u16>(),
            self.total
        );
    }
}

impl Default for Shoe {
    /// 创建规则基线使用的完整八副牌牌靴。
    fn default() -> Self {
        Self {
            decks: DEFAULT_DECKS,
            counts: [DEFAULT_DECKS; Card::DISTINCT_COUNT],
            total: u16::from(DEFAULT_DECKS) * Card::DISTINCT_COUNT as u16,
        }
    }
}

/// 对牌靴执行非法操作时返回的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShoeError {
    /// 请求的副牌数不在当前支持的 1～8 范围内。
    InvalidDeckCount { decks: u8 },
    /// 要扣除的某种具体牌多于牌靴中的剩余数量。
    CardUnavailable {
        card: Card,
        requested: usize,
        remaining: u8,
    },
    /// 恢复后会超过该具体牌的初始容量。
    CardAtCapacity { card: Card, capacity: u8 },
}

impl fmt::Display for ShoeError {
    /// 将结构化牌靴错误转换为便于 CLI 展示的文本。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDeckCount { decks } => write!(
                formatter,
                "invalid deck count {decks}; expected {MIN_DECKS} through {MAX_DECKS}"
            ),
            Self::CardUnavailable {
                card,
                requested,
                remaining,
            } => write!(
                formatter,
                "cannot remove {requested} copies of {card}; only {remaining} remain"
            ),
            Self::CardAtCapacity { card, capacity } => write!(
                formatter,
                "cannot restore {card}; all {capacity} copies are already in the shoe"
            ),
        }
    }
}

impl Error for ShoeError {}

#[cfg(test)]
mod tests {
    use crate::{Card, Rank, Suit};

    use super::{DEFAULT_DECKS, Shoe, ShoeError};

    fn card(input: &str) -> Card {
        input.parse().expect("test card must be valid")
    }

    #[test]
    fn default_shoe_has_eight_decks_and_416_cards() {
        let shoe = Shoe::default();

        assert_eq!(shoe.decks(), DEFAULT_DECKS);
        assert_eq!(shoe.total_remaining(), 416);

        for rank in Rank::ALL {
            for suit in Suit::ALL {
                assert_eq!(shoe.remaining(Card::new(rank, suit)), DEFAULT_DECKS);
            }
        }
    }

    #[test]
    fn rejects_deck_counts_outside_one_through_eight() {
        assert_eq!(Shoe::new(0), Err(ShoeError::InvalidDeckCount { decks: 0 }));
        assert_eq!(Shoe::new(9), Err(ShoeError::InvalidDeckCount { decks: 9 }));
    }

    #[test]
    fn removing_a_card_updates_its_count_and_the_total() {
        let mut shoe = Shoe::default();
        let ace_of_spades = card("AS");

        shoe.remove(ace_of_spades).expect("AS should be available");

        assert_eq!(shoe.remaining(ace_of_spades), 7);
        assert_eq!(shoe.remaining(card("AH")), 8);
        assert_eq!(shoe.total_remaining(), 415);
    }

    #[test]
    fn cannot_remove_more_copies_than_the_shoe_contains() {
        let mut shoe = Shoe::default();
        let ace_of_spades = card("AS");

        for _ in 0..DEFAULT_DECKS {
            shoe.remove(ace_of_spades).expect("AS should be available");
        }
        let state_before_failure = shoe.clone();

        assert_eq!(
            shoe.remove(ace_of_spades),
            Err(ShoeError::CardUnavailable {
                card: ace_of_spades,
                requested: 1,
                remaining: 0,
            })
        );
        assert_eq!(shoe, state_before_failure);
    }

    #[test]
    fn restoring_a_removed_card_returns_to_the_previous_state() {
        let mut shoe = Shoe::default();
        let initial = shoe.clone();
        let ace_of_spades = card("AS");

        shoe.remove(ace_of_spades).expect("AS should be available");
        shoe.restore(ace_of_spades)
            .expect("AS should be restorable");

        assert_eq!(shoe, initial);
    }

    #[test]
    fn cannot_restore_a_card_beyond_the_initial_capacity() {
        let mut shoe = Shoe::default();
        let initial = shoe.clone();
        let ace_of_spades = card("AS");

        assert_eq!(
            shoe.restore(ace_of_spades),
            Err(ShoeError::CardAtCapacity {
                card: ace_of_spades,
                capacity: DEFAULT_DECKS,
            })
        );
        assert_eq!(shoe, initial);
    }

    #[test]
    fn batch_removal_supports_duplicate_cards() {
        let mut shoe = Shoe::default();
        let ace_of_spades = card("AS");
        let cards = [ace_of_spades, card("KD"), ace_of_spades];

        shoe.remove_many(&cards).expect("batch should be available");

        assert_eq!(shoe.remaining(ace_of_spades), 6);
        assert_eq!(shoe.remaining(card("KD")), 7);
        assert_eq!(shoe.total_remaining(), 413);
    }

    #[test]
    fn failed_batch_removal_is_atomic() {
        let mut shoe = Shoe::new(1).expect("one deck is valid");
        let initial = shoe.clone();
        let ace_of_spades = card("AS");
        let cards = [ace_of_spades, card("KD"), ace_of_spades];

        assert_eq!(
            shoe.remove_many(&cards),
            Err(ShoeError::CardUnavailable {
                card: ace_of_spades,
                requested: 2,
                remaining: 1,
            })
        );
        assert_eq!(shoe, initial);
    }

    #[test]
    fn empty_batch_is_a_successful_no_op() {
        let mut shoe = Shoe::default();
        let initial = shoe.clone();

        shoe.remove_many(&[]).expect("empty batch should succeed");

        assert_eq!(shoe, initial);
    }

    #[test]
    fn baccarat_point_counts_reflect_the_current_shoe() {
        let mut shoe = Shoe::default();

        assert_eq!(
            shoe.baccarat_point_counts(),
            [128, 32, 32, 32, 32, 32, 32, 32, 32, 32]
        );

        shoe.remove(card("KS")).expect("KS should be available");
        shoe.remove(card("7H")).expect("7H should be available");

        let point_counts = shoe.baccarat_point_counts();
        assert_eq!(point_counts, [127, 32, 32, 32, 32, 32, 32, 31, 32, 32]);
        assert_eq!(point_counts.iter().sum::<u16>(), shoe.total_remaining());
    }
}
