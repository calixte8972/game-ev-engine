//! 第一批百家乐边注的概率、赔率和 EV。
//!
//! 边注与主注分开建模，原因有两个：
//!
//! 1. 主注只需要最终庄、闲、和；普通对子必须查看 Rank，完美对子还要看花色；
//! 2. 幸运 6、幸运 7 和龙宝不是统一赔率，必须保留各自赔付档位。
//!
//! 当前默认赔付表如下，所有数字都是“净赔付”，本金另行返还：
//!
//! ```text
//! 任意对子 5:1；庄对/闲对 11:1；完美对子 25:1
//! 大（总牌数 5 或 6）0.5:1；小（总牌数 4）1.5:1
//! 幸运 6：庄两张/三张以 6 点获胜分别 12/18:1
//! 幸运 7：闲两张 7 点胜 6:1；闲三张 7 点胜 15:1
//! 超级幸运 7：闲 7 对庄 6，总牌数 4/5/6 时分别 30/40/100:1
//! 庄/闲龙宝：非 Natural 胜方按点差 4～9 分别 1/2/3/5/10/30:1
//! ```
//!
//! 赔率属于规则输入，不属于概率本身。将它们集中放在 [`SideBetRules`] 中，
//! 未来接入不同供应商时可以复用同一份精确概率，只替换赔付表。
//!
//! 边注结算统一遵守一个顺序：先判断是否命中，再选择命中的赔率档位，最后
//! 对未命中返回 `-1.0`。概率层记录的是“命中各档位的序列权重”，EV 层再把
//! 每个档位的概率与 `1 + payout` 的总返还相乘；因此多档边注不能只保留一个
//! 平均赔率，否则凯利层会丢失收益分布信息。

use std::{error::Error, fmt};

use super::{RoundOutcome, RoundResult};

/// 当前支持的十一种边注。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SideBet {
    /// 庄家或闲家至少一方的起手两张牌 Rank 相同。
    AnyPair,
    /// 庄家起手两张牌 Rank 相同。
    BankerPair,
    /// 闲家起手两张牌 Rank 相同。
    PlayerPair,
    /// 庄家或闲家至少一方的起手两张牌 Rank 和花色都相同。
    PerfectPair,
    /// 一局最终共发出 5 张或 6 张牌。
    Big,
    /// 一局最终只发出 4 张牌。
    Small,
    /// 闲家以 7 点获胜，并按闲家使用两张或三张牌分档。
    LuckySeven,
    /// 闲 7 点战胜庄 6 点，并按双方合计 4、5、6 张牌分档。
    SuperLuckySeven,
    /// 庄家以 6 点获胜，并按庄家使用两张或三张牌分档。
    LuckySix,
    /// 庄家非 Natural 获胜且点差至少为 4；Natural 庄赢按规则 Push。
    BankerDragonBonus,
    /// 闲家非 Natural 获胜且点差至少为 4；Natural 闲赢按规则 Push。
    PlayerDragonBonus,
}

impl SideBet {
    /// 策略比较时使用的稳定顺序。EV 完全相同时，排在前面的边注优先。
    pub const ALL: [Self; 11] = [
        Self::AnyPair,
        Self::BankerPair,
        Self::PlayerPair,
        Self::PerfectPair,
        Self::Big,
        Self::Small,
        Self::LuckySeven,
        Self::SuperLuckySeven,
        Self::LuckySix,
        Self::BankerDragonBonus,
        Self::PlayerDragonBonus,
    ];

    /// 返回供 JSON、日志和前端使用的稳定名称。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnyPair => "any_pair",
            Self::BankerPair => "banker_pair",
            Self::PlayerPair => "player_pair",
            Self::PerfectPair => "perfect_pair",
            Self::Big => "big",
            Self::Small => "small",
            Self::LuckySeven => "lucky_seven",
            Self::SuperLuckySeven => "super_lucky_seven",
            Self::LuckySix => "lucky_six",
            Self::BankerDragonBonus => "banker_dragon_bonus",
            Self::PlayerDragonBonus => "player_dragon_bonus",
        }
    }
}

/// 十一种边注使用的净赔付表。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideBetRules {
    any_pair: f64,
    banker_pair: f64,
    player_pair: f64,
    perfect_pair: f64,
    big: f64,
    small: f64,
    lucky_seven_two_cards: f64,
    lucky_seven_three_cards: f64,
    super_lucky_seven_four_cards: f64,
    super_lucky_seven_five_cards: f64,
    super_lucky_seven_six_cards: f64,
    lucky_six_two_cards: f64,
    lucky_six_three_cards: f64,
    dragon_bonus_by_margin: [f64; 6],
}

impl SideBetRules {
    /// 创建当前项目采用的澳门式幸运 7 赔付表。
    pub const fn macau_lucky_seven() -> Self {
        Self {
            any_pair: 5.0,
            banker_pair: 11.0,
            player_pair: 11.0,
            perfect_pair: 25.0,
            big: 0.5,
            small: 1.5,
            lucky_seven_two_cards: 6.0,
            lucky_seven_three_cards: 15.0,
            super_lucky_seven_four_cards: 30.0,
            super_lucky_seven_five_cards: 40.0,
            super_lucky_seven_six_cards: 100.0,
            lucky_six_two_cards: 12.0,
            lucky_six_three_cards: 18.0,
            dragon_bonus_by_margin: [1.0, 2.0, 3.0, 5.0, 10.0, 30.0],
        }
    }

    /// 创建自定义赔付表。每个净赔付都必须是有限非负数。
    #[allow(clippy::too_many_arguments)]
    pub fn with_payouts(
        any_pair: f64,
        banker_pair: f64,
        player_pair: f64,
        perfect_pair: f64,
        big: f64,
        small: f64,
        lucky_seven_two_cards: f64,
        lucky_seven_three_cards: f64,
        super_lucky_seven_four_cards: f64,
        super_lucky_seven_five_cards: f64,
        super_lucky_seven_six_cards: f64,
    ) -> Result<Self, SideBetRuleError> {
        let values = [
            ("any_pair", any_pair),
            ("banker_pair", banker_pair),
            ("player_pair", player_pair),
            ("perfect_pair", perfect_pair),
            ("big", big),
            ("small", small),
            ("lucky_seven_two_cards", lucky_seven_two_cards),
            ("lucky_seven_three_cards", lucky_seven_three_cards),
            ("super_lucky_seven_four_cards", super_lucky_seven_four_cards),
            ("super_lucky_seven_five_cards", super_lucky_seven_five_cards),
            ("super_lucky_seven_six_cards", super_lucky_seven_six_cards),
        ];

        for (field, value) in values {
            if !value.is_finite() || value < 0.0 {
                return Err(SideBetRuleError { field, value });
            }
        }

        Ok(Self {
            any_pair,
            banker_pair,
            player_pair,
            perfect_pair,
            big,
            small,
            lucky_seven_two_cards,
            lucky_seven_three_cards,
            super_lucky_seven_four_cards,
            super_lucky_seven_five_cards,
            super_lucky_seven_six_cards,
            lucky_six_two_cards: 12.0,
            lucky_six_three_cards: 18.0,
            dragon_bonus_by_margin: [1.0, 2.0, 3.0, 5.0, 10.0, 30.0],
        })
    }

    /// 返回单一赔率边注的净赔付。
    pub const fn payout(self, bet: SideBet) -> Option<f64> {
        match bet {
            SideBet::AnyPair => Some(self.any_pair),
            SideBet::BankerPair => Some(self.banker_pair),
            SideBet::PlayerPair => Some(self.player_pair),
            SideBet::PerfectPair => Some(self.perfect_pair),
            SideBet::Big => Some(self.big),
            SideBet::Small => Some(self.small),
            SideBet::LuckySeven
            | SideBet::SuperLuckySeven
            | SideBet::LuckySix
            | SideBet::BankerDragonBonus
            | SideBet::PlayerDragonBonus => None,
        }
    }

    /// 幸运 7 的两张、三张闲手净赔付。
    pub const fn lucky_seven_payouts(self) -> [f64; 2] {
        [self.lucky_seven_two_cards, self.lucky_seven_three_cards]
    }

    /// 超级幸运 7 在总牌数 4、5、6 时的净赔付。
    pub const fn super_lucky_seven_payouts(self) -> [f64; 3] {
        [
            self.super_lucky_seven_four_cards,
            self.super_lucky_seven_five_cards,
            self.super_lucky_seven_six_cards,
        ]
    }

    /// 幸运 6 的庄两张、庄三张净赔付。
    pub const fn lucky_six_payouts(self) -> [f64; 2] {
        [self.lucky_six_two_cards, self.lucky_six_three_cards]
    }

    /// 龙宝在非 Natural 点差 4、5、6、7、8、9 时的净赔付。
    pub const fn dragon_bonus_payouts(self) -> [f64; 6] {
        self.dragon_bonus_by_margin
    }

    /// 使用已经开奖的完整牌局结算一笔边注。
    ///
    /// 返回值统一使用“每下注 1 单位的净盈利”口径：命中返回对应档位净赔付，
    /// 未命中返回 `-1.0`。龙宝的 Natural 退回本金时返回 `0.0`；边注不会叠加主注返水。
    pub fn settle(self, bet: SideBet, round: RoundResult) -> f64 {
        // 先提取手牌和常用条件，后面的 match 只负责业务判断，不再重复访问
        // RoundResult。所有判断都发生在“真实结果已知”的结算阶段；下注前的
        // 概率/EV 计算使用的是同一规则对应的权重桶。
        let player = round.player_hand();
        let banker = round.banker_hand();
        let player_pair = player.first_card().rank() == player.second_card().rank();
        let banker_pair = banker.first_card().rank() == banker.second_card().rank();
        let player_perfect_pair = player.first_card() == player.second_card();
        let banker_perfect_pair = banker.first_card() == banker.second_card();
        let outcome = round.outcome();

        // match 的每个 guard 对应一个玩法命中条件。返回 Option 是为了把“命中
        // 某个赔率档位”和“没有命中”分开，最后统一把 None 映射成 -1.0。
        let payout = match bet {
            SideBet::AnyPair if player_pair || banker_pair => Some(self.any_pair),
            SideBet::BankerPair if banker_pair => Some(self.banker_pair),
            SideBet::PlayerPair if player_pair => Some(self.player_pair),
            SideBet::PerfectPair if player_perfect_pair || banker_perfect_pair => {
                Some(self.perfect_pair)
            }
            SideBet::Big if round.card_count() >= 5 => Some(self.big),
            SideBet::Small if round.card_count() == 4 => Some(self.small),
            SideBet::LuckySeven if player.total() == 7 && player.total() > banker.total() => {
                Some(if player.card_count() == 2 {
                    self.lucky_seven_two_cards
                } else {
                    self.lucky_seven_three_cards
                })
            }
            SideBet::SuperLuckySeven if player.total() == 7 && banker.total() == 6 => {
                Some(match round.card_count() {
                    4 => self.super_lucky_seven_four_cards,
                    5 => self.super_lucky_seven_five_cards,
                    6 => self.super_lucky_seven_six_cards,
                    _ => unreachable!("百家乐一局只能使用 4、5 或 6 张牌"),
                })
            }
            SideBet::LuckySix if outcome == RoundOutcome::Banker && banker.total() == 6 => {
                Some(if banker.card_count() == 2 {
                    self.lucky_six_two_cards
                } else {
                    self.lucky_six_three_cards
                })
            }
            SideBet::BankerDragonBonus
                if outcome == RoundOutcome::Banker && banker.is_natural() =>
            {
                Some(0.0)
            }
            SideBet::PlayerDragonBonus
                if outcome == RoundOutcome::Player && player.is_natural() =>
            {
                Some(0.0)
            }
            SideBet::BankerDragonBonus | SideBet::PlayerDragonBonus
                if outcome == RoundOutcome::Tie && player.is_natural() && banker.is_natural() =>
            {
                Some(0.0)
            }
            SideBet::BankerDragonBonus if outcome == RoundOutcome::Banker => {
                dragon_bonus_payout(self.dragon_bonus_by_margin, banker.total() - player.total())
            }
            SideBet::PlayerDragonBonus if outcome == RoundOutcome::Player => {
                dragon_bonus_payout(self.dragon_bonus_by_margin, player.total() - banker.total())
            }
            _ => None,
        };

        payout.unwrap_or(-1.0)
    }
}

fn dragon_bonus_payout(payouts: [f64; 6], margin: u8) -> Option<f64> {
    (margin >= 4).then(|| payouts[usize::from(margin - 4)])
}

impl Default for SideBetRules {
    fn default() -> Self {
        Self::macau_lucky_seven()
    }
}

/// 精确枚举得到的边注整数权重，所有字段共用 `(N)₆` 分母。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct SideBetWeights {
    /// 所有六张有序物理发牌序列的共同分母。
    total: u64,
    any_pair: u64,
    banker_pair: u64,
    player_pair: u64,
    perfect_pair: u64,
    big: u64,
    small: u64,
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
}

impl SideBetWeights {
    /// 枚举器完成所有溢出检查后创建权重快照。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        total: u64,
        any_pair: u64,
        banker_pair: u64,
        player_pair: u64,
        perfect_pair: u64,
        big: u64,
        small: u64,
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
    ) -> Self {
        debug_assert!(
            [
                any_pair,
                banker_pair,
                player_pair,
                perfect_pair,
                big,
                small,
                lucky_seven_two_cards,
                lucky_seven_three_cards,
                super_lucky_seven_four_cards,
                super_lucky_seven_five_cards,
                super_lucky_seven_six_cards,
                lucky_six_two_cards,
                lucky_six_three_cards,
                banker_dragon_bonus_push,
                player_dragon_bonus_push,
            ]
            .into_iter()
            .chain(banker_dragon_bonus_tiers)
            .chain(player_dragon_bonus_tiers)
            .all(|weight| weight <= total)
        );

        Self {
            total,
            any_pair,
            banker_pair,
            player_pair,
            perfect_pair,
            big,
            small,
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
        }
    }

    /// 返回共同的六张有序序列分母。
    pub const fn total_weight(self) -> u64 {
        self.total
    }

    /// 返回某个边注的总命中权重。
    pub const fn win_weight(self, bet: SideBet) -> u64 {
        match bet {
            SideBet::AnyPair => self.any_pair,
            SideBet::BankerPair => self.banker_pair,
            SideBet::PlayerPair => self.player_pair,
            SideBet::PerfectPair => self.perfect_pair,
            SideBet::Big => self.big,
            SideBet::Small => self.small,
            SideBet::LuckySeven => self.lucky_seven_two_cards + self.lucky_seven_three_cards,
            SideBet::SuperLuckySeven => {
                self.super_lucky_seven_four_cards
                    + self.super_lucky_seven_five_cards
                    + self.super_lucky_seven_six_cards
            }
            SideBet::LuckySix => self.lucky_six_two_cards + self.lucky_six_three_cards,
            // `win_weight` 是 const fn，当前稳定版 Rust 还不允许在这里用
            // `iter().sum()`，所以把点差 4～9 的六档权重显式相加。
            SideBet::BankerDragonBonus => {
                self.banker_dragon_bonus_tiers[0]
                    + self.banker_dragon_bonus_tiers[1]
                    + self.banker_dragon_bonus_tiers[2]
                    + self.banker_dragon_bonus_tiers[3]
                    + self.banker_dragon_bonus_tiers[4]
                    + self.banker_dragon_bonus_tiers[5]
            }
            SideBet::PlayerDragonBonus => {
                self.player_dragon_bonus_tiers[0]
                    + self.player_dragon_bonus_tiers[1]
                    + self.player_dragon_bonus_tiers[2]
                    + self.player_dragon_bonus_tiers[3]
                    + self.player_dragon_bonus_tiers[4]
                    + self.player_dragon_bonus_tiers[5]
            }
        }
    }

    /// 返回某个边注命中的总概率。
    pub fn probability(self, bet: SideBet) -> f64 {
        // 命中率只统计 win_weight；龙宝 Natural Push 单独保存，不能把 Push
        // 当作“命中赔率”混入这里。
        self.win_weight(bet) as f64 / self.total as f64
    }

    /// 返回幸运 7 的两张、三张命中权重。
    pub const fn lucky_seven_tier_weights(self) -> [u64; 2] {
        [self.lucky_seven_two_cards, self.lucky_seven_three_cards]
    }

    /// 返回超级幸运 7 在总牌数 4、5、6 时的命中权重。
    pub const fn super_lucky_seven_tier_weights(self) -> [u64; 3] {
        [
            self.super_lucky_seven_four_cards,
            self.super_lucky_seven_five_cards,
            self.super_lucky_seven_six_cards,
        ]
    }

    /// 返回幸运 6 的庄两张、庄三张命中权重。
    pub const fn lucky_six_tier_weights(self) -> [u64; 2] {
        [self.lucky_six_two_cards, self.lucky_six_three_cards]
    }

    /// 返回庄龙宝点差 4～9 的命中权重。
    pub const fn banker_dragon_bonus_tier_weights(self) -> [u64; 6] {
        self.banker_dragon_bonus_tiers
    }

    /// 返回闲龙宝点差 4～9 的命中权重。
    pub const fn player_dragon_bonus_tier_weights(self) -> [u64; 6] {
        self.player_dragon_bonus_tiers
    }

    /// 返回庄龙宝退回本金的 Natural 权重。
    pub const fn banker_dragon_bonus_push_weight(self) -> u64 {
        self.banker_dragon_bonus_push
    }

    /// 返回闲龙宝退回本金的 Natural 权重。
    pub const fn player_dragon_bonus_push_weight(self) -> u64 {
        self.player_dragon_bonus_push
    }
}

/// 一个边注在当前牌靴下的概率与收益指标。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideBetMetrics {
    probability: f64,
    ev: f64,
}

impl SideBetMetrics {
    /// 返回边注命中任一赔付档位的概率。
    pub const fn probability(self) -> f64 {
        self.probability
    }

    /// 返回每下注 1 单位的期望净盈利。
    pub const fn ev(self) -> f64 {
        self.ev
    }

    /// 返回庄家优势；正数表示长期对玩家不利。
    pub const fn house_edge(self) -> f64 {
        -self.ev
    }

    /// 返回理论返还率，即 `1 + EV`。
    pub const fn rtp(self) -> f64 {
        1.0 + self.ev
    }
}

/// 十一种边注在同一牌靴和赔付表下的分析结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideBetAnalysis {
    any_pair: SideBetMetrics,
    banker_pair: SideBetMetrics,
    player_pair: SideBetMetrics,
    perfect_pair: SideBetMetrics,
    big: SideBetMetrics,
    small: SideBetMetrics,
    lucky_seven: SideBetMetrics,
    super_lucky_seven: SideBetMetrics,
    lucky_six: SideBetMetrics,
    banker_dragon_bonus: SideBetMetrics,
    player_dragon_bonus: SideBetMetrics,
}

impl SideBetAnalysis {
    /// 把精确命中权重与赔付表组合成概率和 EV。
    pub fn calculate(weights: SideBetWeights, rules: SideBetRules) -> Self {
        // 单赔率边注可以统一走 single；多赔率边注必须保留完整档位数组，
        // 龙宝还要把 Natural Push 传给带 push 的公式。
        let total = weights.total_weight();
        let single = |bet: SideBet| {
            metrics_from_tiers(
                total,
                &[weights.win_weight(bet)],
                &[rules.payout(bet).expect("单赔率边注必须有净赔付")],
            )
        };

        Self {
            any_pair: single(SideBet::AnyPair),
            banker_pair: single(SideBet::BankerPair),
            player_pair: single(SideBet::PlayerPair),
            perfect_pair: single(SideBet::PerfectPair),
            big: single(SideBet::Big),
            small: single(SideBet::Small),
            lucky_seven: metrics_from_tiers(
                total,
                &weights.lucky_seven_tier_weights(),
                &rules.lucky_seven_payouts(),
            ),
            super_lucky_seven: metrics_from_tiers(
                total,
                &weights.super_lucky_seven_tier_weights(),
                &rules.super_lucky_seven_payouts(),
            ),
            lucky_six: metrics_from_tiers(
                total,
                &weights.lucky_six_tier_weights(),
                &rules.lucky_six_payouts(),
            ),
            banker_dragon_bonus: metrics_from_tiers_with_push(
                total,
                &weights.banker_dragon_bonus_tier_weights(),
                &rules.dragon_bonus_payouts(),
                weights.banker_dragon_bonus_push_weight(),
            ),
            player_dragon_bonus: metrics_from_tiers_with_push(
                total,
                &weights.player_dragon_bonus_tier_weights(),
                &rules.dragon_bonus_payouts(),
                weights.player_dragon_bonus_push_weight(),
            ),
        }
    }

    /// 返回指定边注的指标。
    pub const fn metrics(self, bet: SideBet) -> SideBetMetrics {
        match bet {
            SideBet::AnyPair => self.any_pair,
            SideBet::BankerPair => self.banker_pair,
            SideBet::PlayerPair => self.player_pair,
            SideBet::PerfectPair => self.perfect_pair,
            SideBet::Big => self.big,
            SideBet::Small => self.small,
            SideBet::LuckySeven => self.lucky_seven,
            SideBet::SuperLuckySeven => self.super_lucky_seven,
            SideBet::LuckySix => self.lucky_six,
            SideBet::BankerDragonBonus => self.banker_dragon_bonus,
            SideBet::PlayerDragonBonus => self.player_dragon_bonus,
        }
    }
}

/// 多档赔付的 EV 统一公式。
///
/// 某档命中时，玩家拿回本金并获得 `payout` 净盈利，因此该档总返还倍数是
/// `1 + payout`；未命中时返还为 0。RTP 是所有命中档位的概率加权返还，
/// `EV = RTP - 1`。
fn metrics_from_tiers(total: u64, tier_weights: &[u64], payouts: &[f64]) -> SideBetMetrics {
    metrics_from_tiers_with_push(total, tier_weights, payouts, 0)
}

/// 多档赔付加 Push 权重的 EV。Push 返回原始本金，因此只贡献 RTP，不计入命中率。
fn metrics_from_tiers_with_push(
    total: u64,
    tier_weights: &[u64],
    payouts: &[f64],
    push_weight: u64,
) -> SideBetMetrics {
    // tier_weights 与 payouts 一一对应：每个命中档位的总返还为 1 + payout；
    // 未命中不返还，Push 返还本金但不计入命中率。最后用 RTP - 1 转回净 EV。
    debug_assert_eq!(tier_weights.len(), payouts.len());
    let probability = tier_weights.iter().sum::<u64>() as f64 / total as f64;
    let winning_return = tier_weights
        .iter()
        .zip(payouts)
        .map(|(&weight, &payout)| weight as f64 / total as f64 * (1.0 + payout))
        .sum::<f64>();
    let rtp = winning_return + push_weight as f64 / total as f64;

    SideBetMetrics {
        probability,
        ev: rtp - 1.0,
    }
}

/// 自定义边注赔付表包含非法数值。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideBetRuleError {
    field: &'static str,
    value: f64,
}

impl fmt::Display for SideBetRuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "side-bet payout {} must be finite and non-negative; got {}",
            self.field, self.value
        )
    }
}

impl Error for SideBetRuleError {}

#[cfg(test)]
mod tests {
    use crate::{Card, Shoe, calculate_side_bet_outcomes, resolve_round};

    use super::{SideBet, SideBetAnalysis, SideBetRules};

    fn assert_close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} differs from {expected} by more than {tolerance}"
        );
    }

    fn round(input: &str) -> crate::RoundResult {
        let cards = input
            .split_whitespace()
            .map(|card| card.parse::<Card>().expect("测试牌面必须合法"))
            .collect::<Vec<_>>();
        resolve_round(&cards).expect("测试牌局必须符合补牌规则")
    }

    #[test]
    fn complete_eight_deck_shoe_matches_the_published_paytable_rtp() {
        let weights =
            calculate_side_bet_outcomes(&Shoe::default()).expect("完整八副牌应该能枚举边注");
        let analysis = SideBetAnalysis::calculate(weights, SideBetRules::default());

        // 这些四舍五入到百分比后分别对应规则表中的 89.64%、81.70%、85.16%。
        assert_close(
            analysis.metrics(SideBet::BankerPair).rtp(),
            0.8964,
            0.000_05,
        );
        assert_close(
            analysis.metrics(SideBet::PlayerPair).rtp(),
            0.8964,
            0.000_05,
        );
        assert_close(
            analysis.metrics(SideBet::PerfectPair).probability(),
            0.033_450_234_245_752_35,
            1e-15,
        );
        assert_close(
            analysis.metrics(SideBet::PerfectPair).rtp(),
            0.869_706_090_389_561_1,
            1e-12,
        );
        assert_close(
            analysis.metrics(SideBet::Big).probability(),
            0.621_131_509_119_718_4,
            1e-15,
        );
        assert_close(
            analysis.metrics(SideBet::Big).rtp(),
            0.931_697_263_679_577_6,
            1e-12,
        );
        assert_close(
            analysis.metrics(SideBet::Small).probability(),
            0.378_868_490_880_281_57,
            1e-15,
        );
        assert_close(
            analysis.metrics(SideBet::Small).rtp(),
            0.947_171_227_200_703_9,
            1e-12,
        );
        assert_close(
            analysis.metrics(SideBet::LuckySeven).rtp(),
            0.8170,
            0.000_05,
        );
        assert_close(
            analysis.metrics(SideBet::SuperLuckySeven).rtp(),
            0.8516,
            0.000_1,
        );
        assert!(analysis.metrics(SideBet::LuckySix).probability() > 0.0);
        assert!(analysis.metrics(SideBet::BankerDragonBonus).probability() > 0.0);
        assert!(analysis.metrics(SideBet::PlayerDragonBonus).probability() > 0.0);
    }

    #[test]
    fn any_pair_is_the_union_not_the_sum_of_both_pair_probabilities() {
        let weights =
            calculate_side_bet_outcomes(&Shoe::default()).expect("完整八副牌应该能枚举边注");
        let any = weights.probability(SideBet::AnyPair);
        let banker = weights.probability(SideBet::BankerPair);
        let player = weights.probability(SideBet::PlayerPair);

        assert_eq!(banker, player);
        assert!(any > banker);
        assert!(any < banker + player);
    }

    #[test]
    fn perfect_pair_uses_exact_card_identity_and_is_impossible_in_one_deck() {
        let eight_deck_weights =
            calculate_side_bet_outcomes(&Shoe::default()).expect("完整八副牌应该能枚举边注");
        let probability = eight_deck_weights.probability(SideBet::PerfectPair);

        // 单方完美对概率为 7/415；本边注同时覆盖庄、闲任一方，并用容斥法
        // 扣除双方同时命中的重叠，所以应低于简单的两倍。
        assert!(probability > 7.0 / 415.0);
        assert!(probability < 2.0 * 7.0 / 415.0);

        let one_deck = Shoe::new(1).expect("一副牌应该合法");
        let one_deck_weights =
            calculate_side_bet_outcomes(&one_deck).expect("完整一副牌应该能枚举边注");
        assert_eq!(one_deck_weights.probability(SideBet::PerfectPair), 0.0);
    }

    #[test]
    fn big_and_small_are_mutually_exclusive_and_cover_every_round() {
        let weights =
            calculate_side_bet_outcomes(&Shoe::default()).expect("完整八副牌应该能枚举边注");
        let big = weights.probability(SideBet::Big);
        let small = weights.probability(SideBet::Small);

        assert_close(big + small, 1.0, 1e-15);
        assert!(big > small);
    }

    #[test]
    fn payout_changes_ev_without_changing_probability() {
        let weights =
            calculate_side_bet_outcomes(&Shoe::default()).expect("完整八副牌应该能枚举边注");
        let default = SideBetAnalysis::calculate(weights, SideBetRules::default());
        let generous = SideBetAnalysis::calculate(
            weights,
            SideBetRules::with_payouts(
                6.0, 12.0, 12.0, 26.0, 0.55, 1.6, 7.0, 16.0, 31.0, 41.0, 101.0,
            )
            .expect("测试赔付应合法"),
        );

        assert_eq!(
            default.metrics(SideBet::AnyPair).probability(),
            generous.metrics(SideBet::AnyPair).probability()
        );
        assert!(generous.metrics(SideBet::AnyPair).ev() > default.metrics(SideBet::AnyPair).ev());
    }

    #[test]
    fn pair_side_bets_settle_from_the_two_initial_cards() {
        let rules = SideBetRules::default();
        let player_pair = round("4S AC 4H 2D");
        let perfect_pair = round("4S AC 4S 2D");

        assert_eq!(rules.settle(SideBet::AnyPair, player_pair), 5.0);
        assert_eq!(rules.settle(SideBet::PlayerPair, player_pair), 11.0);
        assert_eq!(rules.settle(SideBet::BankerPair, player_pair), -1.0);
        assert_eq!(rules.settle(SideBet::PerfectPair, player_pair), -1.0);
        assert_eq!(rules.settle(SideBet::PerfectPair, perfect_pair), 25.0);
    }

    #[test]
    fn lucky_seven_and_super_lucky_seven_use_the_correct_card_tier() {
        let rules = SideBetRules::default();
        let four_cards = round("3S 2C 4H 4D");
        let five_cards = round("2S 2C 3H 4D 2D");

        assert_eq!(rules.settle(SideBet::LuckySeven, four_cards), 6.0);
        assert_eq!(rules.settle(SideBet::SuperLuckySeven, four_cards), 30.0);
        assert_eq!(rules.settle(SideBet::LuckySeven, five_cards), 15.0);
        assert_eq!(rules.settle(SideBet::SuperLuckySeven, five_cards), 40.0);
    }

    #[test]
    fn lucky_six_uses_the_bankers_two_or_three_card_tier() {
        let rules = SideBetRules::default();
        // 庄家起手 3 + 3 = 6，闲家补牌后只有 4 点：庄以两张牌、6 点获胜。
        let banker_two_cards = round("AS 3C 2H 3D AH");
        // 庄家起手 2 + 2 = 4，补 2 后成为 6；闲家三张合计只有 2 点。
        let banker_three_cards = round("KS 2C QH 2D 2S 2H");

        assert_eq!(rules.settle(SideBet::LuckySix, banker_two_cards), 12.0);
        assert_eq!(rules.settle(SideBet::LuckySix, banker_three_cards), 18.0);
    }

    #[test]
    fn dragon_bonus_uses_margin_tiers_and_pushes_selected_side_naturals() {
        let rules = SideBetRules::default();
        // 双方都补牌后闲 8、庄 4，闲龙宝按点差 4 净赔 1:1。
        let player_margin_four = round("2S 2C 2H 2D 4S KH");
        // 双方都补牌后庄 9、闲 2，庄龙宝按点差 7 净赔 5:1。
        let banker_margin_seven = round("KS 2C QH 2D 2S 5H");
        // 闲天然 9 获胜：闲龙宝退回本金（净收益 0），庄龙宝仍然输。
        let player_natural = round("4S 2C 5H 3D");
        // 双方天然 9 和局：两个方向都退回本金。
        let both_natural_tie = round("4S 5C 5H 4D");

        assert_eq!(
            rules.settle(SideBet::PlayerDragonBonus, player_margin_four),
            1.0
        );
        assert_eq!(
            rules.settle(SideBet::BankerDragonBonus, banker_margin_seven),
            5.0
        );
        assert_eq!(
            rules.settle(SideBet::PlayerDragonBonus, player_natural),
            0.0
        );
        assert_eq!(
            rules.settle(SideBet::BankerDragonBonus, player_natural),
            -1.0
        );
        assert_eq!(
            rules.settle(SideBet::PlayerDragonBonus, both_natural_tie),
            0.0
        );
        assert_eq!(
            rules.settle(SideBet::BankerDragonBonus, both_natural_tie),
            0.0
        );
    }

    #[test]
    fn big_and_small_settle_from_the_final_card_count() {
        let rules = SideBetRules::default();
        let four_cards = round("AS 4C 7H 3D");
        let five_cards = round("2C 4H 3D 2S 5C");
        let six_cards = round("KC 2C QH KS 5D 4H");

        assert_eq!(rules.settle(SideBet::Small, four_cards), 1.5);
        assert_eq!(rules.settle(SideBet::Big, four_cards), -1.0);
        assert_eq!(rules.settle(SideBet::Big, five_cards), 0.5);
        assert_eq!(rules.settle(SideBet::Big, six_cards), 0.5);
        assert_eq!(rules.settle(SideBet::Small, six_cards), -1.0);
    }
}
