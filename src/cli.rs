//! 命令行参数解析与牌靴构造。
//!
//! CLI 层只做“输入适配”，不实现百家乐规则和 EV 算法。它把操作系统提供的
//! 字符串参数转换成核心库认识的 `Card` 和 `Shoe`：
//!
//! ```text
//! 命令行字符串 -> parse_args() -> AnalyzeInput -> build_shoe() -> Shoe
//! ```
//!
//! 当前支持的形式：
//!
//! ```text
//! game-ev-engine analyze consumed AS 10H KD
//! game-ev-engine analyze remaining AS 10H KD
//! ```
//!
//! `consumed` 表示列出已经发走的牌；`remaining` 表示列出当前仍在牌靴中的牌。

use crate::{Card, DEFAULT_DECKS, Shoe, ShoeError};

/// 用户提供的牌列表采用哪一种解释方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardSource {
    /// 列表中的牌已经消耗：先创建完整八副牌，再把它们扣除。
    Consumed,
    /// 列表中的牌就是全部剩余牌：直接从这些牌重建牌靴。
    Remaining,
}

/// `analyze` 命令解析完成后的结构化输入。
#[derive(Debug, PartialEq, Eq)]
pub struct AnalyzeInput {
    /// 指明 `cards` 是已消耗牌还是剩余牌。
    pub source: CardSource,
    /// 已经从文本成功解析出来的具体牌。
    pub cards: Vec<Card>,
}

/// CLI 支持的顶层命令。
///
/// 使用枚举后，未来可以继续加入 `Replay`、`Serve` 等命令，而不必用字符串
/// 在整个程序中到处判断。
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// 分析当前牌靴的庄、闲、和概率与 EV。
    Analyze(AnalyzeInput),
}

/// 参数解析或牌靴构造失败的原因。
#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    /// 没有提供 `analyze` 等命令名。
    MissingCommand,
    /// 第一项参数不是当前支持的命令。
    UnknownCommand(String),
    /// `analyze` 后没有提供 `consumed` 或 `remaining`。
    MissingCardSource,
    /// 提供了未知的牌列表解释方式。
    UnknownCardSource(String),
    /// 某一项牌面文本无法解析成 `Card`。
    InvalidCard(String),
    /// 参数本身能解析，但无法构造合法牌靴。
    Shoe(ShoeError),
    /// 预留错误：上层要求了当前版本尚未实现的来源。
    UnsupportedCardSource(CardSource),
}

/// 允许 `?` 自动把底层 `ShoeError` 包装成 `CliError::Shoe`。
impl From<ShoeError> for CliError {
    fn from(error: ShoeError) -> Self {
        Self::Shoe(error)
    }
}

/// 根据结构化 CLI 输入构造核心算法使用的牌靴。
pub fn build_shoe(input: &AnalyzeInput) -> Result<Shoe, CliError> {
    match input.source {
        CardSource::Consumed => {
            // 已消耗模式：以完整八副牌为起点，再原子扣除所有已知牌。
            let mut shoe = Shoe::default();

            // remove_many 返回 ShoeError；`?` 会通过上面的 From 实现把它
            // 自动转换成 CliError，并在失败时立即结束本函数。
            shoe.remove_many(&input.cards)?;

            Ok(shoe)
        }
        // 剩余模式：输入列表本身就是最终牌靴，不需要先创建完整牌靴。
        CardSource::Remaining => Ok(Shoe::from_remaining(DEFAULT_DECKS, &input.cards)?),
    }
}

/// 把去掉程序名后的命令行参数解析成一个 [`Command`]。
///
/// `args` 使用切片借用，函数只读取参数，不取得 `Vec<String>` 的所有权。
pub fn parse_args(args: &[String]) -> Result<Command, CliError> {
    // first() 返回 Option<&String>。没有第一项时，ok_or 把 None 转成领域错误；
    // `?` 再把这个错误直接返回。
    let command = args.first().ok_or(CliError::MissingCommand)?;

    if command != "analyze" {
        return Err(CliError::UnknownCommand(command.clone()));
    }

    // get(1) 是安全下标访问：参数不足时返回 None，不会发生越界 panic。
    let source_text = args.get(1).ok_or(CliError::MissingCardSource)?;

    let source = match source_text.as_str() {
        "consumed" => CardSource::Consumed,
        "remaining" => CardSource::Remaining,
        // other 绑定没有匹配前两项的 &str；to_owned() 复制成错误可持有的 String。
        other => return Err(CliError::UnknownCardSource(other.to_owned())),
    };

    // 参数数量运行时才知道，因此使用可增长的 Vec，而不是固定长度数组。
    let mut cards = Vec::new();

    // 前两项已经分别是 command 和 source，真正的牌从下标 2 开始。
    for card_text in &args[2..] {
        // parse::<Card>() 调用 Card 的 FromStr 实现。
        // map_err 把底层详细解析错误转换成 CLI 对外使用的错误，并保留原文本。
        let card = card_text
            .parse::<Card>()
            .map_err(|_| CliError::InvalidCard(card_text.clone()))?;

        cards.push(card);
    }

    // 所有牌都成功解析后，才组装最终命令；中途失败不会返回半成品。
    Ok(Command::Analyze(AnalyzeInput { source, cards }))
}
