//! 有限牌靴二十一点动作 EV 分析。
//!
//! 本模块分析的是玩家已经拿到起手牌、庄家已经亮出明牌之后的动作选择。
//! 美式 Peek 规则下，如果庄家明牌为 A 或 10 点牌，结果以“庄家已经确认
//! 没有 Blackjack”为条件。未知暗牌仍然真实占用牌靴，并通过后验权重参与
//! 后续补牌概率，计算过程不会偷看暗牌。

use std::{collections::HashMap, error::Error, fmt};

use serde::Serialize;

use crate::{Card, Rank, Shoe};

const VALUE_CLASS_COUNT: usize = 10;
const ACE_INDEX: usize = 0;
const TEN_INDEX: usize = 9;
const PROBABILITY_EPSILON: f64 = 1e-12;

/// 一套常见美式二十一点桌规。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BlackjackRules {
    /// `true` 表示庄家软 17 补牌，`false` 表示软 17 停牌。
    pub dealer_hits_soft_17: bool,
    /// 玩家天然 Blackjack 的净赔付，例如 `1.5` 表示 3:2。
    pub blackjack_payout: f64,
    /// 是否允许起手两张牌晚投降。
    pub late_surrender: bool,
    /// 分牌后是否允许加倍。
    pub double_after_split: bool,
    /// 最多同时拥有多少手牌。
    pub max_split_hands: u8,
    /// 是否允许再次分 A。
    pub resplit_aces: bool,
    /// 分 A 后是否允许继续补牌。
    pub hit_split_aces: bool,
}

impl BlackjackRules {
    /// 项目默认规则：8 副牌、3:2、S17、DAS、晚投降。
    pub const fn standard() -> Self {
        Self {
            dealer_hits_soft_17: false,
            blackjack_payout: 1.5,
            late_surrender: true,
            double_after_split: true,
            max_split_hands: 4,
            resplit_aces: true,
            hit_split_aces: false,
        }
    }

    fn validate(self) -> Result<Self, BlackjackError> {
        if !self.blackjack_payout.is_finite() || self.blackjack_payout <= 0.0 {
            return Err(BlackjackError::InvalidBlackjackPayout(
                self.blackjack_payout,
            ));
        }
        if !(2..=4).contains(&self.max_split_hands) {
            return Err(BlackjackError::InvalidMaxSplitHands(self.max_split_hands));
        }
        Ok(self)
    }
}

impl Default for BlackjackRules {
    fn default() -> Self {
        Self::standard()
    }
}

/// 玩家在起手决策点可以选择的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlackjackAction {
    Blackjack,
    Stand,
    Hit,
    Double,
    Split,
    Surrender,
}

impl BlackjackAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blackjack => "blackjack",
            Self::Stand => "stand",
            Self::Hit => "hit",
            Self::Double => "double",
            Self::Split => "split",
            Self::Surrender => "surrender",
        }
    }
}

/// 每个可用动作按“原始下注 1 单位”计量的期望净盈利。
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BlackjackActionEvs {
    pub stand: Option<f64>,
    pub hit: Option<f64>,
    pub double: Option<f64>,
    pub split: Option<f64>,
    pub surrender: Option<f64>,
}

/// 一次已知手牌分析的完整结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlackjackAnalysis {
    pub player_total: u8,
    pub player_soft: bool,
    pub player_blackjack: bool,
    pub pair: bool,
    pub dealer_upcard: &'static str,
    pub dealer_blackjack_probability_before_peek: f64,
    pub conditional_on_no_dealer_blackjack: bool,
    /// 保险按保险下注自身 1 单位计算：庄家 Blackjack 净赢 2，否则净输 1。
    pub insurance_ev: Option<f64>,
    pub actions: BlackjackActionEvs,
    pub optimal_action: BlackjackAction,
    pub optimal_ev: f64,
    /// 当前分牌实现使用有限牌靴的一手边际 EV，再乘以两手。
    /// 它考虑暗牌后验和牌靴组成，但尚未联合枚举两手之间的互相耗牌。
    pub split_ev_is_independent_hand_estimate: bool,
    pub rules: BlackjackRules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HandState {
    /// 所有 A 都先按 1 计入的总点数。
    hard_total: u8,
    aces: u8,
    cards: u8,
}

impl HandState {
    fn from_cards(cards: &[Card]) -> Self {
        let mut hand = Self {
            hard_total: 0,
            aces: 0,
            cards: 0,
        };
        for card in cards {
            hand = hand.add_value(value_index(card.rank()));
        }
        hand
    }

    fn from_value(value: usize) -> Self {
        Self {
            hard_total: 0,
            aces: 0,
            cards: 0,
        }
        .add_value(value)
    }

    fn add_value(mut self, value: usize) -> Self {
        self.cards += 1;
        if value == ACE_INDEX {
            self.hard_total += 1;
            self.aces += 1;
        } else {
            self.hard_total += value as u8 + 1;
        }
        self
    }

    fn total(self) -> u8 {
        if self.aces > 0 && self.hard_total + 10 <= 21 {
            self.hard_total + 10
        } else {
            self.hard_total
        }
    }

    fn is_soft(self) -> bool {
        self.aces > 0 && self.hard_total + 10 <= 21
    }

    fn is_bust(self) -> bool {
        self.hard_total > 21
    }

    fn is_blackjack(self) -> bool {
        self.cards == 2 && self.total() == 21
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PlayerMemoKey {
    counts: [u16; VALUE_CLASS_COUNT],
    hand: HandState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DealerMemoKey {
    counts: [u16; VALUE_CLASS_COUNT],
    hand: HandState,
}

/// 固定起手可见信息后，用于所有动作共享的暗牌后验和庄家缓存。
struct Solver {
    rules: BlackjackRules,
    dealer_upcard: usize,
    root_counts: [u16; VALUE_CLASS_COUNT],
    allowed_holes: [bool; VALUE_CLASS_COUNT],
    dealer_memo: HashMap<DealerMemoKey, [f64; 6]>,
    player_memo: HashMap<PlayerMemoKey, f64>,
}

impl Solver {
    fn new(
        rules: BlackjackRules,
        dealer_upcard: usize,
        root_counts: [u16; VALUE_CLASS_COUNT],
    ) -> Result<Self, BlackjackError> {
        let mut allowed_holes = [true; VALUE_CLASS_COUNT];
        if dealer_upcard == ACE_INDEX {
            allowed_holes[TEN_INDEX] = false;
        } else if dealer_upcard == TEN_INDEX {
            allowed_holes[ACE_INDEX] = false;
        }

        let has_hole = root_counts
            .iter()
            .enumerate()
            .any(|(index, count)| allowed_holes[index] && *count > 0);
        if !has_hole {
            return Err(BlackjackError::NoPossibleDealerHoleCard);
        }

        Ok(Self {
            rules,
            dealer_upcard,
            root_counts,
            allowed_holes,
            dealer_memo: HashMap::new(),
            player_memo: HashMap::new(),
        })
    }

    /// 给定已经观察到的后续明牌，重新计算未知暗牌属于每一类牌的后验权重。
    fn hole_weights(&self, counts: &[u16; VALUE_CLASS_COUNT]) -> [f64; VALUE_CLASS_COUNT] {
        let mut weights = [0.0; VALUE_CLASS_COUNT];
        for (hole, hole_weight) in weights.iter_mut().enumerate() {
            if !self.allowed_holes[hole] || self.root_counts[hole] == 0 {
                continue;
            }

            let mut weight = f64::from(self.root_counts[hole]);
            for (value, count) in counts.iter().copied().enumerate() {
                let removed = self.root_counts[value].saturating_sub(count);
                let mut available = self.root_counts[value] - u16::from(value == hole);
                for _ in 0..removed {
                    if available == 0 {
                        weight = 0.0;
                        break;
                    }
                    weight *= f64::from(available);
                    available -= 1;
                }
                if weight == 0.0 {
                    break;
                }
            }
            *hole_weight = weight;
        }
        weights
    }

    fn visible_draw_probabilities(
        &self,
        counts: &[u16; VALUE_CLASS_COUNT],
    ) -> [f64; VALUE_CLASS_COUNT] {
        let weights = self.hole_weights(counts);
        let weight_sum: f64 = weights.iter().sum();
        let drawable = counts.iter().sum::<u16>().saturating_sub(1);
        let mut probabilities = [0.0; VALUE_CLASS_COUNT];
        if weight_sum <= 0.0 || drawable == 0 {
            return probabilities;
        }

        for value in 0..VALUE_CLASS_COUNT {
            let numerator = weights
                .iter()
                .enumerate()
                .map(|(hole, weight)| {
                    *weight * f64::from(counts[value].saturating_sub(u16::from(value == hole)))
                })
                .sum::<f64>();
            probabilities[value] = numerator / (weight_sum * f64::from(drawable));
        }
        probabilities
    }

    fn stand_ev(&mut self, counts: [u16; VALUE_CLASS_COUNT], player: HandState) -> f64 {
        if player.is_bust() {
            return -1.0;
        }

        let weights = self.hole_weights(&counts);
        let weight_sum: f64 = weights.iter().sum();
        if weight_sum <= 0.0 {
            return -1.0;
        }

        let mut expected = 0.0;
        for hole in 0..VALUE_CLASS_COUNT {
            if weights[hole] == 0.0 || counts[hole] == 0 {
                continue;
            }
            let mut dealer_counts = counts;
            dealer_counts[hole] -= 1;
            let dealer = HandState::from_value(self.dealer_upcard).add_value(hole);
            let distribution = self.dealer_distribution(dealer_counts, dealer);
            expected +=
                weights[hole] / weight_sum * settle_against_dealer(player.total(), distribution);
        }
        expected
    }

    fn dealer_distribution(
        &mut self,
        counts: [u16; VALUE_CLASS_COUNT],
        hand: HandState,
    ) -> [f64; 6] {
        if hand.is_bust() {
            return [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        }

        let total = hand.total();
        let should_hit =
            total < 17 || (total == 17 && hand.is_soft() && self.rules.dealer_hits_soft_17);
        if !should_hit {
            let mut distribution = [0.0; 6];
            distribution[usize::from(total.saturating_sub(16))] = 1.0;
            return distribution;
        }

        let key = DealerMemoKey { counts, hand };
        if let Some(cached) = self.dealer_memo.get(&key) {
            return *cached;
        }

        let remaining = counts.iter().sum::<u16>();
        if remaining == 0 {
            return [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        }
        let mut distribution = [0.0; 6];
        for value in 0..VALUE_CLASS_COUNT {
            if counts[value] == 0 {
                continue;
            }
            let probability = f64::from(counts[value]) / f64::from(remaining);
            let mut next_counts = counts;
            next_counts[value] -= 1;
            let branch = self.dealer_distribution(next_counts, hand.add_value(value));
            for (target, branch_probability) in distribution.iter_mut().zip(branch) {
                *target += probability * branch_probability;
            }
        }
        self.dealer_memo.insert(key, distribution);
        distribution
    }

    fn hit_ev(&mut self, counts: [u16; VALUE_CLASS_COUNT], hand: HandState) -> f64 {
        let probabilities = self.visible_draw_probabilities(&counts);
        let mut expected = 0.0;
        for value in 0..VALUE_CLASS_COUNT {
            let probability = probabilities[value];
            if probability == 0.0 || counts[value] == 0 {
                continue;
            }
            let mut next_counts = counts;
            next_counts[value] -= 1;
            let next_hand = hand.add_value(value);
            expected += probability
                * if next_hand.is_bust() {
                    -1.0
                } else {
                    self.optimal_hit_or_stand(next_counts, next_hand)
                };
        }
        expected
    }

    fn optimal_hit_or_stand(&mut self, counts: [u16; VALUE_CLASS_COUNT], hand: HandState) -> f64 {
        let key = PlayerMemoKey { counts, hand };
        if let Some(cached) = self.player_memo.get(&key) {
            return *cached;
        }
        let stand = self.stand_ev(counts, hand);
        let best = if hand.total() >= 21 {
            stand
        } else {
            stand.max(self.hit_ev(counts, hand))
        };
        self.player_memo.insert(key, best);
        best
    }

    fn double_ev(&mut self, counts: [u16; VALUE_CLASS_COUNT], hand: HandState) -> f64 {
        let probabilities = self.visible_draw_probabilities(&counts);
        let mut expected = 0.0;
        for value in 0..VALUE_CLASS_COUNT {
            let probability = probabilities[value];
            if probability == 0.0 || counts[value] == 0 {
                continue;
            }
            let mut next_counts = counts;
            next_counts[value] -= 1;
            let next_hand = hand.add_value(value);
            expected += probability
                * if next_hand.is_bust() {
                    -2.0
                } else {
                    2.0 * self.stand_ev(next_counts, next_hand)
                };
        }
        expected
    }

    fn split_estimate(&mut self, counts: [u16; VALUE_CLASS_COUNT], pair_value: usize) -> f64 {
        let probabilities = self.visible_draw_probabilities(&counts);
        let mut one_hand_ev = 0.0;
        for value in 0..VALUE_CLASS_COUNT {
            let probability = probabilities[value];
            if probability == 0.0 || counts[value] == 0 {
                continue;
            }
            let mut next_counts = counts;
            next_counts[value] -= 1;
            let hand = HandState::from_value(pair_value).add_value(value);
            let stand = self.stand_ev(next_counts, hand);
            let mut best = stand;
            if pair_value != ACE_INDEX || self.rules.hit_split_aces {
                best = best.max(self.hit_ev(next_counts, hand));
                if self.rules.double_after_split {
                    best = best.max(self.double_ev(next_counts, hand));
                }
            }
            one_hand_ev += probability * best;
        }
        2.0 * one_hand_ev
    }
}

/// 分析已扣除玩家牌和庄家明牌后的当前牌靴。
pub fn analyze_blackjack_hand(
    shoe_after_visible_cards: &Shoe,
    player_cards: &[Card],
    dealer_upcard: Card,
    rules: BlackjackRules,
) -> Result<BlackjackAnalysis, BlackjackError> {
    let rules = rules.validate()?;
    if player_cards.len() != 2 {
        return Err(BlackjackError::ExpectedTwoPlayerCards(player_cards.len()));
    }
    if shoe_after_visible_cards.total_remaining() < 2 {
        return Err(BlackjackError::NotEnoughCards(
            shoe_after_visible_cards.total_remaining(),
        ));
    }

    let counts = blackjack_value_counts(shoe_after_visible_cards);
    let upcard = value_index(dealer_upcard.rank());
    let player = HandState::from_cards(player_cards);
    let pair_value = value_index(player_cards[0].rank());
    let pair = pair_value == value_index(player_cards[1].rank());
    let total_unseen = counts.iter().sum::<u16>();
    let dealer_blackjack_probability_before_peek = if upcard == ACE_INDEX {
        f64::from(counts[TEN_INDEX]) / f64::from(total_unseen)
    } else if upcard == TEN_INDEX {
        f64::from(counts[ACE_INDEX]) / f64::from(total_unseen)
    } else {
        0.0
    };
    let insurance_ev =
        (upcard == ACE_INDEX).then_some(3.0 * dealer_blackjack_probability_before_peek - 1.0);
    let conditional_on_no_dealer_blackjack = upcard == ACE_INDEX || upcard == TEN_INDEX;
    let mut solver = Solver::new(rules, upcard, counts)?;

    if player.is_blackjack() {
        return Ok(BlackjackAnalysis {
            player_total: 21,
            player_soft: true,
            player_blackjack: true,
            pair,
            dealer_upcard: value_name(upcard),
            dealer_blackjack_probability_before_peek,
            conditional_on_no_dealer_blackjack,
            insurance_ev,
            actions: BlackjackActionEvs {
                stand: Some(rules.blackjack_payout),
                hit: None,
                double: None,
                split: None,
                surrender: None,
            },
            optimal_action: BlackjackAction::Blackjack,
            optimal_ev: rules.blackjack_payout,
            split_ev_is_independent_hand_estimate: false,
            rules,
        });
    }

    let stand = solver.stand_ev(counts, player);
    let hit = solver.hit_ev(counts, player);
    let double = solver.double_ev(counts, player);
    let split = pair.then(|| solver.split_estimate(counts, pair_value));
    let surrender = rules.late_surrender.then_some(-0.5);
    let mut best = (BlackjackAction::Stand, stand);
    for candidate in [
        (BlackjackAction::Hit, Some(hit)),
        (BlackjackAction::Double, Some(double)),
        (BlackjackAction::Split, split),
        (BlackjackAction::Surrender, surrender),
    ] {
        if let (action, Some(ev)) = candidate
            && ev > best.1 + PROBABILITY_EPSILON
        {
            best = (action, ev);
        }
    }

    Ok(BlackjackAnalysis {
        player_total: player.total(),
        player_soft: player.is_soft(),
        player_blackjack: false,
        pair,
        dealer_upcard: value_name(upcard),
        dealer_blackjack_probability_before_peek,
        conditional_on_no_dealer_blackjack,
        insurance_ev,
        actions: BlackjackActionEvs {
            stand: Some(stand),
            hit: Some(hit),
            double: Some(double),
            split,
            surrender,
        },
        optimal_action: best.0,
        optimal_ev: best.1,
        split_ev_is_independent_hand_estimate: split.is_some(),
        rules,
    })
}

fn blackjack_value_counts(shoe: &Shoe) -> [u16; VALUE_CLASS_COUNT] {
    let ranks = shoe.rank_counts();
    let mut counts = [0; VALUE_CLASS_COUNT];
    counts[..9].copy_from_slice(&ranks[..9]);
    counts[TEN_INDEX] = ranks[9..].iter().sum();
    counts
}

fn value_index(rank: Rank) -> usize {
    match rank {
        Rank::Ace => ACE_INDEX,
        Rank::Two => 1,
        Rank::Three => 2,
        Rank::Four => 3,
        Rank::Five => 4,
        Rank::Six => 5,
        Rank::Seven => 6,
        Rank::Eight => 7,
        Rank::Nine => 8,
        Rank::Ten | Rank::Jack | Rank::Queen | Rank::King => TEN_INDEX,
    }
}

fn value_name(value: usize) -> &'static str {
    ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10"][value]
}

/// `distribution[0]` 是庄家爆牌，`1..=5` 分别是 17..=21。
fn settle_against_dealer(player_total: u8, distribution: [f64; 6]) -> f64 {
    let mut expected = distribution[0];
    for dealer_total in 17..=21_u8 {
        let probability = distribution[usize::from(dealer_total - 16)];
        expected += probability
            * match player_total.cmp(&dealer_total) {
                std::cmp::Ordering::Greater => 1.0,
                std::cmp::Ordering::Equal => 0.0,
                std::cmp::Ordering::Less => -1.0,
            };
    }
    expected
}

/// 二十一点输入或规则无法构成合法分析时返回的错误。
#[derive(Debug, Clone, PartialEq)]
pub enum BlackjackError {
    ExpectedTwoPlayerCards(usize),
    NotEnoughCards(u16),
    NoPossibleDealerHoleCard,
    InvalidBlackjackPayout(f64),
    InvalidMaxSplitHands(u8),
}

impl fmt::Display for BlackjackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpectedTwoPlayerCards(actual) => {
                write!(formatter, "expected exactly two player cards; got {actual}")
            }
            Self::NotEnoughCards(actual) => {
                write!(
                    formatter,
                    "at least two unseen cards are required; got {actual}"
                )
            }
            Self::NoPossibleDealerHoleCard => {
                formatter.write_str("no possible dealer hole card remains after peek")
            }
            Self::InvalidBlackjackPayout(value) => {
                write!(
                    formatter,
                    "blackjack payout must be finite and positive; got {value}"
                )
            }
            Self::InvalidMaxSplitHands(value) => {
                write!(
                    formatter,
                    "maximum split hands must be between 2 and 4; got {value}"
                )
            }
        }
    }
}

impl Error for BlackjackError {}

#[cfg(test)]
mod tests {
    use super::{
        BlackjackAction, BlackjackRules, HandState, analyze_blackjack_hand, blackjack_value_counts,
    };
    use crate::{Card, Rank, Shoe, Suit};

    fn card(rank: Rank, suit: Suit) -> Card {
        Card::new(rank, suit)
    }

    fn prepared_shoe(player: &[Card], dealer: Card) -> Shoe {
        let mut shoe = Shoe::new(8).expect("八副牌合法");
        shoe.remove_many(player).expect("玩家牌应该可以扣除");
        shoe.remove(dealer).expect("庄家明牌应该可以扣除");
        shoe
    }

    #[test]
    fn hand_totals_treat_one_ace_as_eleven_when_safe() {
        let soft =
            HandState::from_cards(&[card(Rank::Ace, Suit::Spades), card(Rank::Six, Suit::Hearts)]);
        assert_eq!(soft.total(), 17);
        assert!(soft.is_soft());

        let hard = soft.add_value(9);
        assert_eq!(hard.total(), 17);
        assert!(!hard.is_soft());
    }

    #[test]
    fn ten_jack_queen_and_king_share_one_probability_class() {
        let shoe = Shoe::new(8).expect("八副牌合法");
        let counts = blackjack_value_counts(&shoe);
        assert_eq!(counts[0], 32);
        assert_eq!(counts[9], 128);
        assert_eq!(counts.iter().sum::<u16>(), 416);
    }

    #[test]
    fn natural_blackjack_pays_three_to_two_after_a_clear_peek() {
        let player = [
            card(Rank::Ace, Suit::Spades),
            card(Rank::King, Suit::Hearts),
        ];
        let dealer = card(Rank::Ace, Suit::Clubs);
        let shoe = prepared_shoe(&player, dealer);
        let analysis = analyze_blackjack_hand(&shoe, &player, dealer, BlackjackRules::standard())
            .expect("天然 Blackjack 应可分析");

        assert_eq!(analysis.optimal_action, BlackjackAction::Blackjack);
        assert_eq!(analysis.optimal_ev, 1.5);
        assert!(analysis.dealer_blackjack_probability_before_peek > 0.0);
        assert!(analysis.conditional_on_no_dealer_blackjack);
    }

    #[test]
    fn six_to_five_rule_changes_only_the_natural_payout() {
        let player = [
            card(Rank::Ace, Suit::Spades),
            card(Rank::King, Suit::Hearts),
        ];
        let dealer = card(Rank::Six, Suit::Clubs);
        let shoe = prepared_shoe(&player, dealer);
        let rules = BlackjackRules {
            blackjack_payout: 1.2,
            ..BlackjackRules::standard()
        };
        let analysis = analyze_blackjack_hand(&shoe, &player, dealer, rules)
            .expect("6:5 天然 Blackjack 应可分析");

        assert_eq!(analysis.optimal_action, BlackjackAction::Blackjack);
        assert_eq!(analysis.optimal_ev, 1.2);
    }

    #[test]
    fn ace_upcard_exposes_insurance_ev_from_the_pre_peek_probability() {
        let player = [
            card(Rank::Five, Suit::Spades),
            card(Rank::Six, Suit::Hearts),
        ];
        let dealer = card(Rank::Ace, Suit::Clubs);
        let shoe = prepared_shoe(&player, dealer);
        let analysis = analyze_blackjack_hand(&shoe, &player, dealer, BlackjackRules::standard())
            .expect("庄家 A 明牌应可计算保险");
        let probability = analysis.dealer_blackjack_probability_before_peek;

        assert!(analysis.conditional_on_no_dealer_blackjack);
        assert!((probability - 128.0 / 413.0).abs() < 1e-12);
        assert!(
            (analysis.insurance_ev.expect("A 明牌应有保险 EV") - (3.0 * probability - 1.0)).abs()
                < 1e-12
        );
    }

    #[test]
    fn obvious_hard_totals_choose_expected_actions() {
        let rules = BlackjackRules::standard();
        let twenty = [
            card(Rank::King, Suit::Spades),
            card(Rank::Queen, Suit::Hearts),
        ];
        let dealer_six = card(Rank::Six, Suit::Clubs);
        let shoe = prepared_shoe(&twenty, dealer_six);
        let analysis =
            analyze_blackjack_hand(&shoe, &twenty, dealer_six, rules).expect("硬 20 应可分析");
        assert_eq!(analysis.optimal_action, BlackjackAction::Stand);

        let eleven = [
            card(Rank::Five, Suit::Spades),
            card(Rank::Six, Suit::Hearts),
        ];
        let dealer_six = card(Rank::Six, Suit::Diamonds);
        let shoe = prepared_shoe(&eleven, dealer_six);
        let analysis =
            analyze_blackjack_hand(&shoe, &eleven, dealer_six, rules).expect("硬 11 应可分析");
        assert_eq!(analysis.optimal_action, BlackjackAction::Double);
    }

    #[test]
    fn pair_exposes_a_split_candidate() {
        let player = [
            card(Rank::Eight, Suit::Spades),
            card(Rank::Eight, Suit::Hearts),
        ];
        let dealer = card(Rank::Six, Suit::Clubs);
        let shoe = prepared_shoe(&player, dealer);
        let analysis = analyze_blackjack_hand(&shoe, &player, dealer, BlackjackRules::standard())
            .expect("对子应可分析");

        assert!(analysis.actions.split.is_some());
        assert!(analysis.split_ev_is_independent_hand_estimate);
    }

    #[test]
    fn late_surrender_can_be_the_best_action_for_hard_sixteen() {
        let player = [card(Rank::Ten, Suit::Spades), card(Rank::Six, Suit::Hearts)];
        let dealer = card(Rank::Ten, Suit::Clubs);
        let shoe = prepared_shoe(&player, dealer);
        let analysis = analyze_blackjack_hand(&shoe, &player, dealer, BlackjackRules::standard())
            .expect("硬 16 对十点牌应可分析");

        assert_eq!(analysis.optimal_action, BlackjackAction::Surrender);
        assert_eq!(analysis.actions.surrender, Some(-0.5));
    }
}
