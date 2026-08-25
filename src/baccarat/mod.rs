//! 标准百家乐的手牌、补牌规则和牌局结果。

pub mod hand;
mod probability;
mod round;
mod rule;

pub use hand::BaccaratHand;
pub use probability::{OutcomeWeights, ProbabilityError};
pub use round::{RoundError, RoundOutcome, RoundResult, compare_hands, resolve_round};
pub use rule::{banker_should_draw, player_should_draw};
