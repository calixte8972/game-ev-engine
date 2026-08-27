//! 标准百家乐主注概率的精确整数权重表示。

use std::{error::Error, fmt};

/// 一局百家乐最多使用的牌数。
const MAX_ROUND_CARDS: u8 = 6;

/// 庄、闲、和三种结果以及庄六点获胜子集的精确整数权重。
///
/// 三种结果共用六张有序抽牌序列的总权重作为分母，只有权重和等于
/// 共同分母时才能成功构造该类型。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct OutcomeWeights {
    /// 所有闲赢终局对应的六张有序序列数量。
    player: u64,
    /// 所有庄赢终局对应的六张有序序列数量。
    banker: u64,
    /// 所有和局终局对应的六张有序序列数量。
    tie: u64,
    /// 从初始牌靴不放回抽六张的有序序列总数，即 `(N)₆`。
    total: u64,
    /// 庄家最终为 6 点且庄家获胜的路径数量；它是 `banker` 的子集。
    banker_win_on_six: u64,
}

impl OutcomeWeights {
    /// 根据牌靴总数与三种结果权重构造一个完整分布。
    ///
    /// 这是兼容构造函数，没有额外的庄六点子集信息，因此将
    /// `banker_win_on_six` 设为零。需要计算免佣庄时，应使用
    /// [`Self::from_detailed_weights`]。
    pub fn from_weights(
        total_cards: u16,
        player: u64,
        banker: u64,
        tie: u64,
    ) -> Result<Self, ProbabilityError> {
        Self::from_detailed_weights(total_cards, player, banker, tie, 0)
    }

    /// 根据三种主结果和庄六点获胜子集构造完整权重。
    ///
    /// 除了验证三种互斥结果之和等于 `(N)₆`，还会验证庄六点获胜权重
    /// 不超过庄家获胜总权重。
    pub fn from_detailed_weights(
        total_cards: u16,
        player: u64,
        banker: u64,
        tie: u64,
        banker_win_on_six: u64,
    ) -> Result<Self, ProbabilityError> {
        if total_cards < u16::from(MAX_ROUND_CARDS) {
            return Err(ProbabilityError::NotEnoughCards {
                remaining: total_cards,
            });
        }

        // 一局最多六张牌，因此所有 4、5、6 张终局统一映射到六张共同分母。
        let total = falling_factorial(total_cards, MAX_ROUND_CARDS);
        // 两次 checked_add 同时保护 player + banker 和再加 tie 的过程。
        let actual = player
            .checked_add(banker)
            .and_then(|value| value.checked_add(tie))
            .ok_or(ProbabilityError::WeightOverflow)?;

        if actual != total {
            return Err(ProbabilityError::WeightSumMismatch {
                expected: total,
                actual,
            });
        }

        if banker_win_on_six > banker {
            return Err(ProbabilityError::BankerWinOnSixExceedsBankerWeight {
                banker,
                banker_win_on_six,
            });
        }

        Ok(Self {
            player,
            banker,
            tie,
            total,
            banker_win_on_six,
        })
    }

    /// 返回庄家最终为 6 点且庄家获胜的路径权重。
    pub const fn banker_win_on_six_weight(self) -> u64 {
        self.banker_win_on_six
    }

    /// 返回闲赢路径的整数权重。
    pub const fn player_weight(self) -> u64 {
        self.player
    }

    /// 返回庄赢路径的整数权重。
    pub const fn banker_weight(self) -> u64 {
        self.banker
    }

    /// 返回和局路径的整数权重。
    pub const fn tie_weight(self) -> u64 {
        self.tie
    }

    /// 返回全部六张有序抽牌序列的共同分母。
    pub const fn total_weight(self) -> u64 {
        self.total
    }

    /// 返回闲赢概率。
    ///
    /// 精确枚举阶段始终累计整数，到展示或 EV 计算阶段才转换为浮点数，
    /// 避免递归过程中反复相加浮点概率产生累计误差。
    pub fn player_probability(self) -> f64 {
        self.player as f64 / self.total as f64
    }

    /// 返回庄赢概率；只在读取结果时执行一次浮点除法。
    pub fn banker_probability(self) -> f64 {
        self.banker as f64 / self.total as f64
    }

    /// 返回庄家最终为 6 点且庄家获胜的概率。
    pub fn banker_win_on_six_probability(self) -> f64 {
        self.banker_win_on_six as f64 / self.total as f64
    }

    /// 返回和局概率；只在读取结果时执行一次浮点除法。
    pub fn tie_probability(self) -> f64 {
        self.tie as f64 / self.total as f64
    }

    /// 检查庄、闲、和权重之和是否等于共同分母。
    pub fn weights_sum_to_total(self) -> bool {
        self.player
            .checked_add(self.banker)
            .and_then(|value| value.checked_add(self.tie))
            == Some(self.total)
    }
}

/// 概率权重无法构造成完整分布时返回的错误。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProbabilityError {
    /// 牌靴不足六张，无法建立统一的六张有序序列分母。
    NotEnoughCards { remaining: u16 },
    /// 权重乘法或加法超出了 `u64` 表示范围。
    WeightOverflow,
    /// 三种结果权重之和与理论共同分母不相等。
    WeightSumMismatch { expected: u64, actual: u64 },
    /// 庄六点获胜权重不能大于庄家获胜总权重。
    BankerWinOnSixExceedsBankerWeight { banker: u64, banker_win_on_six: u64 },
}

impl fmt::Display for ProbabilityError {
    /// 将结构化概率错误转换成便于上层展示的文本。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEnoughCards { remaining } => write!(
                formatter,
                "at least {MAX_ROUND_CARDS} cards are required; only {remaining} remain"
            ),
            Self::WeightOverflow => formatter.write_str("outcome weight sum overflowed u64"),
            Self::WeightSumMismatch { expected, actual } => write!(
                formatter,
                "outcome weights sum to {actual}; expected common denominator {expected}"
            ),
            Self::BankerWinOnSixExceedsBankerWeight {
                banker,
                banker_win_on_six,
            } => write!(
                formatter,
                "banker six-point win weight {banker_win_on_six} exceeds banker weight {banker}"
            ),
        }
    }
}

impl Error for ProbabilityError {}

/// 计算 `n × (n - 1) × ...`，共包含 `count` 个因子。
///
/// 这是下降阶乘，也就是从 `n` 个对象中依次、不放回地抽取 `count` 个对象的
/// 有序序列数量。例如 `(6)₄ = 6 × 5 × 4 × 3 = 360`。
pub(super) fn falling_factorial(mut n: u16, count: u8) -> u64 {
    // 调用者必须保证可抽数量不超过现有数量；只在调试构建中检查这条内部约束。
    debug_assert!(u16::from(count) <= n);

    let mut result = 1_u64;
    for _ in 0..count {
        // 每抽一张，可供下一位置选择的牌就减少一张。
        result *= u64::from(n);
        n -= 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{OutcomeWeights, ProbabilityError, falling_factorial};

    #[test]
    fn exact_weight_foundation_uses_a_shared_six_card_denominator() {
        assert_eq!(falling_factorial(5, 0), 1);
        assert_eq!(falling_factorial(5, 1), 5);
        assert_eq!(falling_factorial(5, 3), 60);
        assert_eq!(falling_factorial(416, 6), 4_998_398_275_503_360);

        let weights =
            OutcomeWeights::from_weights(6, 360, 240, 120).expect("测试权重之和应等于六张共同分母");
        assert_eq!(weights.total_weight(), 720);
        assert_eq!(weights.banker_win_on_six_weight(), 0);
        assert!(weights.weights_sum_to_total());
        assert!((weights.player_probability() - 0.5).abs() < f64::EPSILON);
        assert!(
            (weights.player_probability()
                + weights.banker_probability()
                + weights.tie_probability()
                - 1.0)
                .abs()
                < 1e-15
        );

        assert_eq!(
            OutcomeWeights::from_weights(5, 1, 1, 1),
            Err(ProbabilityError::NotEnoughCards { remaining: 5 })
        );
        assert_eq!(
            OutcomeWeights::from_weights(6, 1, 2, 3),
            Err(ProbabilityError::WeightSumMismatch {
                expected: 720,
                actual: 6,
            })
        );

        let detailed = OutcomeWeights::from_detailed_weights(6, 360, 240, 120, 60)
            .expect("庄六点获胜权重应当可以作为庄家权重的子集");
        assert_eq!(detailed.banker_win_on_six_weight(), 60);
        assert!((detailed.banker_win_on_six_probability() - 1.0 / 12.0).abs() < 1e-15);

        assert_eq!(
            OutcomeWeights::from_detailed_weights(6, 360, 240, 120, 241),
            Err(ProbabilityError::BankerWinOnSixExceedsBankerWeight {
                banker: 240,
                banker_win_on_six: 241,
            })
        );
    }
}
