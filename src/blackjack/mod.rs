//! 有限牌靴二十一点动作 EV 分析。
//!
//! 本模块分析的是玩家已经拿到起手牌、庄家已经亮出明牌之后的动作选择。
//! 美式 Peek 规则下，如果庄家明牌为 A 或 10 点牌，结果以“庄家已经确认
//! 没有 Blackjack”为条件。未知暗牌仍然真实占用牌靴，并通过后验权重参与
//! 后续补牌概率，计算过程不会偷看暗牌。
//!
//! 计算分成三层：
//!
//! ```text
//! Shoe + 玩家两张牌 + 庄家明牌
//!   ↓ 扣除可见牌、建立暗牌后验
//! Solver：递归计算庄家终局分布与玩家动作 EV
//!   ↓ 比较 Stand / Hit / Double / Split / Surrender
//! BlackjackAnalysis
//! ```
//!
//! 这里的动作 EV 都按原始底注 1 单位计量。加倍会把最终输赢乘 2；分牌当前
//! 采用两手边际 EV 的透明估算，并在结果字段中明确标出它不是联合耗牌精确值。

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
    /// 返回供 JSON 和前端使用的稳定动作名称。
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
    /// 停牌的期望净盈利；合法输入下始终存在。
    pub stand: Option<f64>,
    /// 补牌的期望净盈利；爆牌或其他边界仍由核心返回数值。
    pub hit: Option<f64>,
    /// 加倍的期望净盈利；起手决策点可用时才存在。
    pub double: Option<f64>,
    /// 分牌的期望净盈利；只有两张牌 Rank 相同才存在。
    pub split: Option<f64>,
    /// 投降的期望净盈利；桌规关闭晚投降时为 `None`。
    pub surrender: Option<f64>,
}

/// 一次已知手牌分析的完整结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlackjackAnalysis {
    /// 玩家起手牌按二十一点规则计算的最终点数。
    pub player_total: u8,
    /// 是否为软牌：至少有一张 A 按 11 计分仍不爆牌。
    pub player_soft: bool,
    /// 是否为两张牌组成的天然 Blackjack。
    pub player_blackjack: bool,
    /// 两张起手牌的 Rank 是否相同，决定是否存在分牌动作。
    pub pair: bool,
    /// 庄家明牌的稳定文本，例如 `A` 或 `10`。
    pub dealer_upcard: &'static str,
    /// Peek 前庄家暗牌组成 Blackjack 的概率。
    pub dealer_blackjack_probability_before_peek: f64,
    /// 庄家明牌为 A/10 时，动作 EV 是否建立在“Peek 已确认无 Blackjack”条件下。
    pub conditional_on_no_dealer_blackjack: bool,
    /// 保险按保险下注自身 1 单位计算：庄家 Blackjack 净赢 2，否则净输 1。
    pub insurance_ev: Option<f64>,
    /// 所有动作的可用 EV；不可用动作用 `None` 表示，而不是伪造一个数值。
    pub actions: BlackjackActionEvs,
    /// 按动作 EV 选出的最优动作。
    pub optimal_action: BlackjackAction,
    /// 最优动作对应的期望净盈利。
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
    /// 当前手牌中 A 的数量；用来判断是否可以把其中一张按 11 计。
    aces: u8,
    /// 当前手牌张数；天然 Blackjack 需要恰好两张。
    cards: u8,
}

impl HandState {
    /// 从具体牌转换为递归求解使用的紧凑状态。
    ///
    /// A 先按 1 计入 `hard_total`，最后由 `total` 在不爆牌时补上 10；
    /// 这样不用枚举 A 的“软/硬”两种表示，状态也更容易作为缓存键。
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
        // 递归抽牌已经被压缩成 0..9 的价值类，因此不需要重新构造 Card。
        Self {
            hard_total: 0,
            aces: 0,
            cards: 0,
        }
        .add_value(value)
    }

    fn add_value(mut self, value: usize) -> Self {
        // 返回新的 Self 而不是修改外部引用，便于在递归分支中保留父节点状态。
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
        // 只有“硬点数 + 10”仍不超过 21 时，才把一张 A 从 1 提升为 11。
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
        // 暗牌没有被观察到，不能简单按当前 counts 直接均匀处理：
        // 每个候选暗牌都会导致一套不同的后续剩余牌数量。这里返回每种暗牌
        // 对应的“可观察历史权重”，visible_draw_probabilities 再用它混合。
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
        // 对每个可能暗牌分别计算“下一张可见牌”的数量，再按暗牌后验加权。
        // 分母是所有暗牌假设权重 × 暗牌之后可抽取的牌数。
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
        // 停牌时只需要枚举庄家暗牌和庄家后续自动补牌，不再消耗玩家牌。
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
        // 返回 [爆牌, 17, 18, 19, 20, 21] 六个互斥结果的概率。
        // 这是庄家自动补牌的递归子问题，使用 memo 避免不同玩家动作重复计算。
        if hand.is_bust() {
            return [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        }

        let total = hand.total();
        let should_hit =
            total < 17 || (total == 17 && hand.is_soft() && self.rules.dealer_hits_soft_17);
        if !should_hit {
            // 庄家已经停牌，只有当前点数对应的桶为 1。
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
        // 补一张后，如果爆牌立即是 -1；否则玩家还可以继续在每个后续状态
        // 选择 Hit 或 Stand，所以递归调用 optimal_hit_or_stand。
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
        // 相同“剩余价值类计数 + 玩家手牌状态”具有相同的最优后续价值，
        // 因此先查缓存，再在当前状态比较停牌与继续补牌。
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
        // 加倍只允许再发一张牌，之后必须停牌；所以这里不会递归到 hit。
        // 因为下注额翻倍，赢/输/庄家结算结果也整体乘 2。
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
        // 当前版本把两手分牌拆成两个独立的一手边际问题，各手使用同一套
        // 后验与策略规则，最后相加。它没有联合模拟两手依次耗牌的相关性。
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
    // 入口约定 shoe 已经扣除了玩家两张牌和庄家明牌；这里不重复扣除，
    // 只把可见牌转换成状态并把它们与未知牌靴交给 Solver。
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
    // 点数 10/J/Q/K 对二十一点动作来说完全等价，因此从 13 个 Rank
    // 压缩成 10 个价值类；具体牌靴仍由上层保留，避免影响百家乐功能。
    let upcard = value_index(dealer_upcard.rank());
    let player = HandState::from_cards(player_cards);
    let pair_value = value_index(player_cards[0].rank());
    let pair = pair_value == value_index(player_cards[1].rank());
    let total_unseen = counts.iter().sum::<u16>();
    // 保险与 Peek 前概率只看庄家暗牌是否能补成 21；动作 EV 则在 Solver 内
    // 根据明牌条件重新混合暗牌后验。
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
        // 天然 Blackjack 不再进入普通 Hit/Stand 状态树；它直接按天然赔付
        // 结算，同时保留保险和 Peek 相关信息给调用者。
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

    // 所有动作从同一个“看到起手牌之后”的 counts 开始。Solver 内部缓存庄家
    // 分布和玩家后续状态，使比较动作时不会重复展开相同子树。
    let stand = solver.stand_ev(counts, player);
    let hit = solver.hit_ev(counts, player);
    let double = solver.double_ev(counts, player);
    let split = pair.then(|| solver.split_estimate(counts, pair_value));
    let surrender = rules.late_surrender.then_some(-0.5);
    // 用 Stand 作为固定初始值，随后只在严格更优且超过浮点容差时替换，
    // 保证完全相等或微小舍入差异不会让动作在不同平台来回变化。
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
    // 前 9 类保留 A..9；最后一类把 10/J/Q/K 的 Rank 数量合并。
    let ranks = shoe.rank_counts();
    let mut counts = [0; VALUE_CLASS_COUNT];
    counts[..9].copy_from_slice(&ranks[..9]);
    counts[TEN_INDEX] = ranks[9..].iter().sum();
    counts
}

fn value_index(rank: Rank) -> usize {
    // 这里的下标是二十一点价值类，不是 Card/Ranks 的底层数组下标。
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
    // 与 VALUE_CLASS_COUNT 对齐，最后一个类统一显示为“10”。
    ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10"][value]
}

/// `distribution[0]` 是庄家爆牌，`1..=5` 分别是 17..=21。
fn settle_against_dealer(player_total: u8, distribution: [f64; 6]) -> f64 {
    // 爆牌对玩家下注的净收益固定为 +1；庄家 17..21 时再按点数比较，
    // 相等是 Push，贡献 0。
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
    /// 玩家起手牌不是恰好两张。
    ExpectedTwoPlayerCards(usize),
    /// 扣除可见牌后剩余牌不足以建立庄家暗牌/后续抽牌分布。
    NotEnoughCards(u16),
    /// Peek 条件下没有任何合法的庄家暗牌价值仍可发生。
    NoPossibleDealerHoleCard,
    /// 天然 Blackjack 赔付不是有限正数。
    InvalidBlackjackPayout(f64),
    /// 当前实现要求最大分牌手数在 2..=4 内。
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
