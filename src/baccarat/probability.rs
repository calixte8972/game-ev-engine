//! 标准百家乐主注概率的精确整数权重表示。

use std::{error::Error, fmt};

/// 一局百家乐最多使用的牌数。
const MAX_ROUND_CARDS: u8 = 6;

/// 庄、闲、和三种结果的精确整数权重。
///
/// 三种结果共用六张有序抽牌序列的总权重作为分母，只有权重和等于
/// 共同分母时才能成功构造该类型。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct OutcomeWeights {
    player: u64,
    banker: u64,
    tie: u64,
    total: u64,
}

impl OutcomeWeights {
    /// 根据牌靴总数与三种结果权重构造一个完整分布。
    pub fn from_weights(
        total_cards: u16,
        player: u64,
        banker: u64,
        tie: u64,
    ) -> Result<Self, ProbabilityError> {
        if total_cards < u16::from(MAX_ROUND_CARDS) {
            return Err(ProbabilityError::NotEnoughCards {
                remaining: total_cards,
            });
        }

        let total = falling_factorial(total_cards, MAX_ROUND_CARDS);
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

        Ok(Self {
            player,
            banker,
            tie,
            total,
        })
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
    pub fn player_probability(self) -> f64 {
        self.player as f64 / self.total as f64
    }

    /// 返回庄赢概率。
    pub fn banker_probability(self) -> f64 {
        self.banker as f64 / self.total as f64
    }

    /// 返回和局概率。
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
    NotEnoughCards { remaining: u16 },
    WeightOverflow,
    WeightSumMismatch { expected: u64, actual: u64 },
}

impl fmt::Display for ProbabilityError {
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
        }
    }
}

impl Error for ProbabilityError {}

/// 计算 `n × (n - 1) × ...`，共包含 `count` 个因子。
fn falling_factorial(mut n: u16, count: u8) -> u64 {
    debug_assert!(u16::from(count) <= n);

    let mut result = 1_u64;
    for _ in 0..count {
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
    }
}
