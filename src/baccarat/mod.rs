//! 标准百家乐的手牌、补牌规则和牌局结果。

// 具体牌穷举器只作为小牌靴测试基准，不进入生产构建。
#[cfg(test)]
mod enumerate;
/// 一方百家乐手牌的表示和点数计算。
pub mod hand;
// 以下模块通过本文件选择性导出，避免暴露内部辅助函数。
mod probability;
mod round;
mod rule;

// 对外统一暴露稳定的百家乐 API，调用者不需要依赖内部文件布局。
pub use hand::BaccaratHand;
pub use probability::{OutcomeWeights, ProbabilityError};
pub use round::{RoundError, RoundOutcome, RoundResult, compare_hands, resolve_round};
pub use rule::{banker_should_draw, player_should_draw};
