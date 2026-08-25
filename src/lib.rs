//! 真人桌面游戏概率与 EV 计算引擎的核心库。
//!
//! 游戏规则、牌靴状态、概率计算和 EV 计算会逐步实现在这个库中。

pub mod card;
pub mod shoe;

pub use card::{Card, CardParseError, Rank, Suit};
pub use shoe::{DEFAULT_DECKS, MAX_DECKS, MIN_DECKS, Shoe, ShoeError};

/// 应用在中文界面中显示的名称。
pub const APP_NAME: &str = "真人桌面游戏概率与 EV 计算引擎";
