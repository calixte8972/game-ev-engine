//! 扑克牌的花色、牌面、解析、显示和百家乐点数。
//!
//! 这是整个项目最底层的领域模块。上层牌靴、百家乐手牌和枚举器最终都依赖
//! 这里的三个核心类型：
//!
//! ```text
//! Suit（花色） + Rank（牌面） -> Card（一张具体牌）
//! ```
//!
//! 本模块还负责两种边界转换：
//!
//! - 文本 `"AS"`、`"10H"` -> `Card`，供 CLI 和以后 Python 输入使用；
//! - `Card` -> `0..52` 的稳定下标，供 `Shoe` 的固定数组存储使用。

use std::{error::Error, fmt, str::FromStr};

/// 扑克牌花色。
///
/// 显式指定 `u8` 表示和 0～3 判别值，使花色能够稳定参与牌靴下标计算。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Suit {
    /// 梅花，CLI 代码为 `C`。
    Clubs = 0,
    /// 方块，CLI 代码为 `D`。
    Diamonds = 1,
    /// 红桃，CLI 代码为 `H`。
    Hearts = 2,
    /// 黑桃，CLI 代码为 `S`。
    Spades = 3,
}

impl Suit {
    /// 一副牌中的全部花色。
    pub const ALL: [Self; 4] = [Self::Clubs, Self::Diamonds, Self::Hearts, Self::Spades];

    /// 在牌靴扁平计数数组中使用的稳定零基下标。
    pub const fn index(self) -> usize {
        self as usize
    }

    /// 用于 `AS`、`10H` 等牌面记法的 ASCII 花色代码。
    pub const fn ascii_code(self) -> char {
        match self {
            Self::Clubs => 'C',
            Self::Diamonds => 'D',
            Self::Hearts => 'H',
            Self::Spades => 'S',
        }
    }
}

impl fmt::Display for Suit {
    /// 使用 CLI 接受的单字符 ASCII 代码显示花色。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.ascii_code())
    }
}

impl FromStr for Suit {
    /// 实现 `FromStr` 后，字符串就可以调用 `"S".parse::<Suit>()`。
    type Err = CardParseError;

    /// 从 `C`、`D`、`H`、`S` 解析花色，忽略大小写和两端空白。
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        // trim() 返回去掉两端空白的 &str，不会复制原字符串。
        let input = input.trim();

        // `_ if 条件` 是 match 的守卫写法。这里用忽略 ASCII 大小写的比较，
        // 因此 C/c、D/d、H/h、S/s 都能解析。
        match input {
            _ if input.eq_ignore_ascii_case("C") => Ok(Self::Clubs),
            _ if input.eq_ignore_ascii_case("D") => Ok(Self::Diamonds),
            _ if input.eq_ignore_ascii_case("H") => Ok(Self::Hearts),
            _ if input.eq_ignore_ascii_case("S") => Ok(Self::Spades),
            _ => Err(CardParseError::InvalidSuit(input.to_owned())),
        }
    }
}

/// 扑克牌牌面。
///
/// 判别值按 A、2、……、K 顺序连续排列，供 `Card::index` 直接计算下标。
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rank {
    /// A，在百家乐中计 1 点。
    Ace = 0,
    /// 2，在百家乐中计 2 点。
    Two = 1,
    /// 3，在百家乐中计 3 点。
    Three = 2,
    /// 4，在百家乐中计 4 点。
    Four = 3,
    /// 5，在百家乐中计 5 点。
    Five = 4,
    /// 6，在百家乐中计 6 点。
    Six = 5,
    /// 7，在百家乐中计 7 点。
    Seven = 6,
    /// 8，在百家乐中计 8 点。
    Eight = 7,
    /// 9，在百家乐中计 9 点。
    Nine = 8,
    /// 10，在百家乐中计 0 点。
    Ten = 9,
    /// J，在百家乐中计 0 点。
    Jack = 10,
    /// Q，在百家乐中计 0 点。
    Queen = 11,
    /// K，在百家乐中计 0 点。
    King = 12,
}

impl Rank {
    /// 一副牌中的全部牌面。
    pub const ALL: [Self; 13] = [
        Self::Ace,
        Self::Two,
        Self::Three,
        Self::Four,
        Self::Five,
        Self::Six,
        Self::Seven,
        Self::Eight,
        Self::Nine,
        Self::Ten,
        Self::Jack,
        Self::Queen,
        Self::King,
    ];

    /// 在牌靴扁平计数数组中使用的稳定零基下标。
    pub const fn index(self) -> usize {
        self as usize
    }

    /// 牌面简写。
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Ace => "A",
            Self::Two => "2",
            Self::Three => "3",
            Self::Four => "4",
            Self::Five => "5",
            Self::Six => "6",
            Self::Seven => "7",
            Self::Eight => "8",
            Self::Nine => "9",
            Self::Ten => "10",
            Self::Jack => "J",
            Self::Queen => "Q",
            Self::King => "K",
        }
    }

    /// 该牌面在百家乐中的点数。
    pub const fn baccarat_value(self) -> u8 {
        match self {
            Self::Ace => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
            Self::Five => 5,
            Self::Six => 6,
            Self::Seven => 7,
            Self::Eight => 8,
            Self::Nine => 9,
            Self::Ten | Self::Jack | Self::Queen | Self::King => 0,
        }
    }
}

impl fmt::Display for Rank {
    /// 使用 `A`、`2`～`10`、`J`、`Q`、`K` 显示牌面。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.short_name())
    }
}

impl FromStr for Rank {
    /// Rank 和 Card 共用一套解析错误，便于上层统一处理。
    type Err = CardParseError;

    /// 从 ASCII 简写解析牌面；十既接受 `10`，也接受常见简写 `T`。
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();

        // A、J、Q、K、T 不能通过普通数字区间匹配，所以先单独处理。
        if input.eq_ignore_ascii_case("A") {
            return Ok(Self::Ace);
        }
        if input.eq_ignore_ascii_case("J") {
            return Ok(Self::Jack);
        }
        if input.eq_ignore_ascii_case("Q") {
            return Ok(Self::Queen);
        }
        if input.eq_ignore_ascii_case("K") {
            return Ok(Self::King);
        }
        if input.eq_ignore_ascii_case("T") || input == "10" {
            return Ok(Self::Ten);
        }

        match input {
            "2" => Ok(Self::Two),
            "3" => Ok(Self::Three),
            "4" => Ok(Self::Four),
            "5" => Ok(Self::Five),
            "6" => Ok(Self::Six),
            "7" => Ok(Self::Seven),
            "8" => Ok(Self::Eight),
            "9" => Ok(Self::Nine),
            _ => Err(CardParseError::InvalidRank(input.to_owned())),
        }
    }
}

/// 一张具体的扑克牌。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Card {
    /// 牌面，例如 Ace、Ten、King。
    rank: Rank,
    /// 花色，例如 Clubs、Spades。
    suit: Suit,
}

impl Card {
    /// 一副标准扑克牌中不同牌面与花色组合的数量。
    pub const DISTINCT_COUNT: usize = 52;

    /// 使用牌面和花色创建一张具体牌。
    pub const fn new(rank: Rank, suit: Suit) -> Self {
        Self { rank, suit }
    }

    /// 返回这张牌的牌面。
    pub const fn rank(self) -> Rank {
        self.rank
    }

    /// 返回这张牌的花色。
    pub const fn suit(self) -> Suit {
        self.suit
    }

    /// 以牌面为主序的扁平数组下标：`rank * 4 + suit`。
    pub const fn index(self) -> usize {
        self.rank.index() * Suit::ALL.len() + self.suit.index()
    }

    /// 这张牌在百家乐中的点数。
    pub const fn baccarat_value(self) -> u8 {
        self.rank.baccarat_value()
    }
}

impl fmt::Display for Card {
    /// 把具体牌显示为 `AS`、`10H` 之类的 CLI 牌面记法。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.rank, self.suit)
    }
}

impl FromStr for Card {
    /// `parse::<Card>()` 失败时返回可匹配的结构化错误，而不是 panic。
    type Err = CardParseError;

    /// 解析由“牌面 + 单字符花色”组成的具体牌文本。
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let input = input.trim();
        if input.is_empty() {
            return Err(CardParseError::Empty);
        }

        // ASCII 牌面记法以单字符花色结尾，例如 AS、10H 或 kd。
        let Some((suit_start, _)) = input.char_indices().next_back() else {
            return Err(CardParseError::Empty);
        };

        if suit_start == 0 {
            return Err(CardParseError::InvalidFormat(input.to_owned()));
        }

        // 使用最后一个字符的位置切分，才能同时兼容一字符牌面 `A` 和两字符牌面 `10`。
        let (rank_text, suit_text) = input.split_at(suit_start);
        // 这里没有重复实现 Rank/Suit 的解析规则，而是复用它们各自的 FromStr。
        // `?` 表示：解析成功就取出值；解析失败就立刻把错误返回给调用者。
        let rank = rank_text.parse()?;
        let suit = suit_text.parse()?;
        Ok(Self::new(rank, suit))
    }
}

/// 牌面文本无法转换成扑克牌时返回的错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardParseError {
    /// 输入去除两端空白后为空。
    Empty,
    /// 输入无法拆成牌面和花色两部分。
    InvalidFormat(String),
    /// 花色部分不是 C、D、H、S。
    InvalidSuit(String),
    /// 牌面部分不是 A、2～10、J、Q、K。
    InvalidRank(String),
}

impl fmt::Display for CardParseError {
    /// 将结构化解析错误转换为适合上层展示的文本。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("card input cannot be empty"),
            Self::InvalidFormat(input) => {
                write!(
                    formatter,
                    "invalid card format `{input}`; expected `AS` or `10H`"
                )
            }
            Self::InvalidSuit(suit) => write!(formatter, "invalid suit `{suit}`"),
            Self::InvalidRank(rank) => write!(formatter, "invalid rank `{rank}`"),
        }
    }
}

// Error 是一个标记性质的标准库 trait。实现后，该错误可以被通用错误处理库接收。
impl Error for CardParseError {}

#[cfg(test)]
mod tests {
    // 单元测试放在同一文件中，因此能测试公开接口，也能帮助初学者从输入输出
    // 反向理解每个类型的约束。
    use super::{Card, CardParseError, Rank, Suit};

    #[test]
    fn all_52_cards_round_trip_through_ascii_display() {
        let mut tested = 0;

        for suit in Suit::ALL {
            for rank in Rank::ALL {
                let card = Card::new(rank, suit);
                let displayed = card.to_string();

                assert_eq!(
                    displayed.parse::<Card>(),
                    Ok(card),
                    "无法往返解析 {displayed}"
                );
                tested += 1;
            }
        }

        assert_eq!(tested, 52);
    }

    #[test]
    fn card_indexes_are_unique_and_cover_zero_through_51() {
        let mut seen = [false; Card::DISTINCT_COUNT];

        for rank in Rank::ALL {
            for suit in Suit::ALL {
                let card = Card::new(rank, suit);
                let index = card.index();

                assert!(
                    index < Card::DISTINCT_COUNT,
                    "index out of range for {card}"
                );
                assert!(!seen[index], "duplicate index {index} for {card}");
                seen[index] = true;
            }
        }

        assert!(seen.into_iter().all(|value| value));
        assert_eq!(Card::new(Rank::Ace, Suit::Clubs).index(), 0);
        assert_eq!(Card::new(Rank::Ace, Suit::Spades).index(), 3);
        assert_eq!(Card::new(Rank::Two, Suit::Clubs).index(), 4);
        assert_eq!(Card::new(Rank::King, Suit::Spades).index(), 51);
    }

    #[test]
    fn ascii_input_is_case_insensitive_and_trimmed() {
        assert_eq!(
            "  as  ".parse::<Card>(),
            Ok(Card::new(Rank::Ace, Suit::Spades))
        );
        assert_eq!(
            "kd".parse::<Card>(),
            Ok(Card::new(Rank::King, Suit::Diamonds))
        );
        assert_eq!("th".parse::<Card>(), Ok(Card::new(Rank::Ten, Suit::Hearts)));
    }

    #[test]
    fn card_exposes_rank_suit_and_baccarat_value() {
        let card = Card::new(Rank::Nine, Suit::Hearts);

        assert_eq!(card.rank(), Rank::Nine);
        assert_eq!(card.suit(), Suit::Hearts);
        assert_eq!(card.baccarat_value(), 9);
        assert_eq!(card.to_string(), "9H");
    }

    #[test]
    fn baccarat_values_match_the_rules() {
        let expected = [1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0, 0];

        for (rank, expected_value) in Rank::ALL.into_iter().zip(expected) {
            assert_eq!(rank.baccarat_value(), expected_value, "牌面为 {rank}");
        }
    }

    #[test]
    fn invalid_inputs_return_structured_errors() {
        assert_eq!("".parse::<Card>(), Err(CardParseError::Empty));
        assert_eq!(
            "A".parse::<Card>(),
            Err(CardParseError::InvalidFormat("A".to_owned()))
        );
        assert_eq!(
            "11S".parse::<Card>(),
            Err(CardParseError::InvalidRank("11".to_owned()))
        );
        assert_eq!(
            "AX".parse::<Card>(),
            Err(CardParseError::InvalidSuit("X".to_owned()))
        );
    }

    #[test]
    fn errors_have_readable_messages() {
        assert_eq!(
            CardParseError::Empty.to_string(),
            "card input cannot be empty"
        );
        assert_eq!(
            CardParseError::InvalidRank("11".to_owned()).to_string(),
            "invalid rank `11`"
        );
    }
}
