//! 生成可以直接交给网页或 `replay_csv_text()` 回放的随机百家乐 CSV。
//!
//! 这个程序生成的不是“每局独立随机结果”，而是完整牌靴：
//!
//! ```text
//! 创建 N 副完整扑克牌
//!     -> 使用种子随机洗牌
//!     -> 按 P1、B1、P2、B2 顺序发牌
//!     -> 复用项目中的闲家/庄家补牌规则
//!     -> 从同一牌靴移除已经发出的牌
//!     -> 写出 raw_cards 和一致的 result_code
//! ```
//!
//! 因此同一靴后面的牌确实受到前面耗牌影响，可以用来测试牌靴 EV、资金策略、
//! 边注结算和 CSV 回放流程。相同 `--seed` 与参数会得到完全相同的 CSV，便于
//! 比较两套策略，而不被两份不同随机样本干扰。

use std::{
    env,
    error::Error,
    fmt,
    fs::{self, File},
    io::Write,
    path::PathBuf,
};

use game_ev_engine::baccarat::{banker_should_draw, player_should_draw};
use game_ev_engine::{Card, Rank, RoundOutcome, Suit, resolve_round};
use serde::Serialize;

const DEFAULT_SHOES: u64 = 100;
const DEFAULT_DECKS: u8 = 8;
const DEFAULT_ROUNDS_PER_SHOE: u32 = 60;
const DEFAULT_SEED: u64 = 20_260_902;
const DEFAULT_TABLES: u64 = 10;
const DEFAULT_START_SESSION_ID: u64 = 1_000_000;
const DEFAULT_ROUND_SECONDS: u32 = 45;
const DEFAULT_START_DATE: &str = "2026-09-02";

/// 命令行配置。输出路径是第一个位置参数，其余参数使用 `--name=value`。
#[derive(Debug, Clone)]
struct GeneratorConfig {
    output: PathBuf,
    shoes: u64,
    decks: u8,
    rounds_per_shoe: u32,
    seed: u64,
    tables: u64,
    start_session_id: u64,
    round_seconds: u32,
    start_date: String,
}

impl GeneratorConfig {
    fn from_args() -> Result<Self, GeneratorError> {
        let mut arguments = env::args().skip(1);
        let output = arguments
            .next()
            .ok_or_else(|| GeneratorError::Arguments(usage("缺少输出 CSV 路径")))?;

        let mut config = Self {
            output: PathBuf::from(output),
            shoes: DEFAULT_SHOES,
            decks: DEFAULT_DECKS,
            rounds_per_shoe: DEFAULT_ROUNDS_PER_SHOE,
            seed: DEFAULT_SEED,
            tables: DEFAULT_TABLES,
            start_session_id: DEFAULT_START_SESSION_ID,
            round_seconds: DEFAULT_ROUND_SECONDS,
            start_date: DEFAULT_START_DATE.to_owned(),
        };

        for argument in arguments {
            let (name, value) = argument.split_once('=').ok_or_else(|| {
                GeneratorError::Arguments(usage(&format!(
                    "参数必须使用 --name=value 格式：{argument}"
                )))
            })?;

            match name {
                "--shoes" => config.shoes = parse_number(name, value)?,
                "--decks" => config.decks = parse_number(name, value)?,
                "--rounds-per-shoe" => {
                    config.rounds_per_shoe = parse_number(name, value)?;
                }
                "--seed" => config.seed = parse_number(name, value)?,
                "--tables" => config.tables = parse_number(name, value)?,
                "--start-session-id" => {
                    config.start_session_id = parse_number(name, value)?;
                }
                "--round-seconds" => config.round_seconds = parse_number(name, value)?,
                "--start-date" => config.start_date = value.to_owned(),
                _ => {
                    return Err(GeneratorError::Arguments(usage(&format!(
                        "未知参数：{name}"
                    ))));
                }
            }
        }

        config.validate()?;
        Ok(config)
    }

    /// 提前拒绝不可能生成完整数据的参数，避免写出一半文件后才报错。
    fn validate(&self) -> Result<(), GeneratorError> {
        if self.shoes == 0 {
            return Err(GeneratorError::Arguments("--shoes 必须大于 0".to_owned()));
        }
        if !(1..=8).contains(&self.decks) {
            return Err(GeneratorError::Arguments(
                "--decks 必须在 1..=8 之间".to_owned(),
            ));
        }
        if self.rounds_per_shoe == 0 {
            return Err(GeneratorError::Arguments(
                "--rounds-per-shoe 必须大于 0".to_owned(),
            ));
        }

        // 一局最多使用 6 张牌。使用最坏情况上限，保证任何随机洗牌结果都能
        // 完成指定局数，而不是某些种子能生成、某些种子会在最后一局断牌。
        let maximum_guaranteed_rounds = u32::from(self.decks) * 52 / 6;
        if self.rounds_per_shoe > maximum_guaranteed_rounds {
            return Err(GeneratorError::Arguments(format!(
                "{} 副牌最多保证生成 {maximum_guaranteed_rounds} 局，当前要求 {} 局",
                self.decks, self.rounds_per_shoe
            )));
        }
        if self.tables == 0 {
            return Err(GeneratorError::Arguments("--tables 必须大于 0".to_owned()));
        }
        if self.round_seconds == 0 {
            return Err(GeneratorError::Arguments(
                "--round-seconds 必须大于 0".to_owned(),
            ));
        }
        if !looks_like_iso_date(&self.start_date) {
            return Err(GeneratorError::Arguments(
                "--start-date 必须使用 YYYY-MM-DD 格式".to_owned(),
            ));
        }

        self.start_session_id
            .checked_add(self.shoes - 1)
            .ok_or_else(|| GeneratorError::Arguments("场次编号发生 u64 溢出".to_owned()))?;

        // 每个场次的时间必须在同一天内保持递增，否则全局时间排序可能把同一靴
        // 的后局排到前局之前。不同场次允许时间重叠，正好模拟多桌并行。
        let latest_session_offset = self.shoes.saturating_sub(1).min(3_599) as u32;
        let final_started_second = latest_session_offset
            .checked_add((self.rounds_per_shoe - 1).saturating_mul(self.round_seconds))
            .ok_or_else(|| GeneratorError::Arguments("局时间发生 u32 溢出".to_owned()))?;
        if final_started_second.saturating_add(30) >= 86_400 {
            return Err(GeneratorError::Arguments(
                "局数与局间隔会跨越第二天，请减小 --rounds-per-shoe 或 --round-seconds".to_owned(),
            ));
        }

        Ok(())
    }
}

fn parse_number<T>(name: &str, value: &str) -> Result<T, GeneratorError>
where
    T: std::str::FromStr,
{
    value
        .parse::<T>()
        .map_err(|_| GeneratorError::Arguments(format!("参数 {name} 不是有效整数：{value}")))
}

/// 输出字段与 `CsvRound` 完全一致；serde 会使用字段名生成第一行表头。
#[derive(Debug, Serialize)]
struct GeneratedRound {
    #[serde(rename = "__source_pk")]
    source_pk: String,
    table_id: u64,
    session_id: u64,
    round_no: u32,
    started_at: String,
    settled_at: String,
    raw_cards: String,
    result_code: u64,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct GenerationSummary {
    shoes: u64,
    rounds: u64,
    player_wins: u64,
    banker_wins: u64,
    ties: u64,
}

/// 一局已经按真实发牌流程得到的数字牌码。
struct DealtRound {
    player: Vec<u8>,
    banker: Vec<u8>,
    dealing_order: Vec<u8>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("随机百家乐 CSV 生成失败：{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let config = GeneratorConfig::from_args()?;
    if let Some(parent) = config
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let file = File::create(&config.output)?;
    let summary = generate_csv(file, &config)?;
    eprintln!(
        "生成完成：{} 靴，{} 局（闲胜 {}、庄胜 {}、和 {}）\n输出：{}",
        summary.shoes,
        summary.rounds,
        summary.player_wins,
        summary.banker_wins,
        summary.ties,
        config.output.display()
    );
    Ok(())
}

fn generate_csv<W: Write>(
    writer: W,
    config: &GeneratorConfig,
) -> Result<GenerationSummary, Box<dyn Error>> {
    let mut csv = csv::WriterBuilder::new().from_writer(writer);
    let mut random = DeterministicRng::new(config.seed);
    let mut summary = GenerationSummary::default();

    for shoe_index in 0..config.shoes {
        let table_id = shoe_index % config.tables + 1;
        let session_id = config.start_session_id + shoe_index;
        let session_start_second = shoe_index.min(3_599) as u32;
        let mut shoe = full_shoe_codes(config.decks);
        random.shuffle(&mut shoe);

        for round_no in 1..=config.rounds_per_shoe {
            let dealt = deal_round(&mut shoe)?;

            // 再交给正式 RoundResult 校验一次。生成器负责摸牌，核心规则负责判定；
            // 如果二者未来出现差异，生成过程会直接失败，不会产出自相矛盾的 CSV。
            let cards = dealt
                .dealing_order
                .iter()
                .copied()
                .map(provider_card)
                .collect::<Result<Vec<_>, _>>()?;
            let result = resolve_round(&cards).map_err(|error| {
                GeneratorError::GeneratedRound(format!(
                    "牌靴 {session_id} 第 {round_no} 局不符合补牌规则：{error}"
                ))
            })?;

            match result.outcome() {
                RoundOutcome::Player => summary.player_wins += 1,
                RoundOutcome::Banker => summary.banker_wins += 1,
                RoundOutcome::Tie => summary.ties += 1,
            }

            let started_second = session_start_second + (round_no - 1) * config.round_seconds;
            let settled_second = started_second + config.round_seconds.min(30);
            csv.serialize(GeneratedRound {
                source_pk: format!(
                    "sim-{:016x}-{table_id}-{session_id}-{round_no}",
                    config.seed
                ),
                table_id,
                session_id,
                round_no,
                started_at: format_timestamp(&config.start_date, started_second),
                settled_at: format_timestamp(&config.start_date, settled_second),
                raw_cards: format!(
                    "b:{};p:{}",
                    join_codes(&dealt.banker),
                    join_codes(&dealt.player)
                ),
                result_code: result_code(result.outcome()),
            })?;
            summary.rounds += 1;
        }
        summary.shoes += 1;
    }

    csv.flush()?;
    Ok(summary)
}

/// 构造具体牌码，而不是只构造 0～9 点数。这样对子、完美对子、龙宝等边注
/// 都能从同一份随机 CSV 中得到真实可结算的牌面。
fn full_shoe_codes(decks: u8) -> Vec<u8> {
    let mut cards = Vec::with_capacity(usize::from(decks) * 52);
    for _ in 0..decks {
        for suit_start in [0_u8, 20, 40, 60] {
            for rank in 1_u8..=13 {
                cards.push(suit_start + rank);
            }
        }
    }
    cards
}

fn deal_round(shoe: &mut Vec<u8>) -> Result<DealtRound, GeneratorError> {
    // 前四张永远交错发给闲、庄。Vec::pop() 从已经洗乱的牌靴尾部取下一张，
    // 每张牌只会被取出一次，因此天然满足“无放回抽样”。
    let player_first = draw(shoe)?;
    let banker_first = draw(shoe)?;
    let player_second = draw(shoe)?;
    let banker_second = draw(shoe)?;

    let mut player = vec![player_first, player_second];
    let mut banker = vec![banker_first, banker_second];
    let mut dealing_order = vec![player_first, banker_first, player_second, banker_second];
    let player_initial = hand_total(&player);
    let banker_initial = hand_total(&banker);

    // 任意一方起手 8/9 都是自然牌，双方都停止补牌。
    if !matches!(player_initial, 8 | 9) && !matches!(banker_initial, 8 | 9) {
        let player_third = if player_should_draw(player_initial) {
            let card = draw(shoe)?;
            player.push(card);
            dealing_order.push(card);
            Some(card)
        } else {
            None
        };

        if banker_should_draw(banker_initial, player_third.map(card_point)) {
            let card = draw(shoe)?;
            banker.push(card);
            dealing_order.push(card);
        }
    }

    Ok(DealtRound {
        player,
        banker,
        dealing_order,
    })
}

fn draw(shoe: &mut Vec<u8>) -> Result<u8, GeneratorError> {
    shoe.pop()
        .ok_or_else(|| GeneratorError::GeneratedRound("牌靴没有足够的牌完成指定局数".to_owned()))
}

fn hand_total(cards: &[u8]) -> u8 {
    cards.iter().copied().map(card_point).sum::<u8>() % 10
}

fn card_point(code: u8) -> u8 {
    match code % 20 {
        1..=9 => code % 20,
        _ => 0,
    }
}

fn provider_card(code: u8) -> Result<Card, GeneratorError> {
    let suit_index = code / 20;
    let rank_number = code % 20;
    if suit_index >= Suit::ALL.len() as u8 || !(1..=13).contains(&rank_number) {
        return Err(GeneratorError::GeneratedRound(format!(
            "生成了非法来源牌码：{code}"
        )));
    }
    Ok(Card::new(
        Rank::ALL[usize::from(rank_number - 1)],
        Suit::ALL[usize::from(suit_index)],
    ))
}

fn result_code(outcome: RoundOutcome) -> u64 {
    match outcome {
        RoundOutcome::Banker => 0b001,
        RoundOutcome::Player => 0b010,
        RoundOutcome::Tie => 0b100,
    }
}

fn join_codes(cards: &[u8]) -> String {
    cards
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn format_timestamp(date: &str, second_of_day: u32) -> String {
    let hour = second_of_day / 3_600;
    let minute = second_of_day % 3_600 / 60;
    let second = second_of_day % 60;
    format!("{date} {hour:02}:{minute:02}:{second:02}")
}

fn looks_like_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

/// 小型确定性 PRNG。这里不依赖操作系统随机源，是为了让种子成为可审计参数。
/// 它适合模拟与测试数据生成，不用于密码、密钥或真钱抽奖。
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        // xorshift 的全零状态不会前进，因此把 0 映射到固定的非零常量。
        Self {
            state: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.state = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn index(&mut self, upper_exclusive: usize) -> usize {
        debug_assert!(upper_exclusive > 0);
        let upper = upper_exclusive as u64;
        // 直接 `% upper` 在 2^64 不能被 upper 整除时会产生极小的模偏差。
        // 拒绝落在不完整区间内的随机数，使每个可选索引拥有完全相同的数量。
        let rejection_threshold = upper.wrapping_neg() % upper;
        loop {
            let value = self.next_u64();
            if value >= rejection_threshold {
                return (value % upper) as usize;
            }
        }
    }

    /// Fisher–Yates 洗牌：第 i 个位置从 0..=i 中等概率选择交换对象。
    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let other = self.index(index + 1);
            values.swap(index, other);
        }
    }
}

fn usage(message: &str) -> String {
    format!(
        "{message}\n\
用法：cargo run --release --bin generate_baccarat_csv -- <output.csv> \\\n+  [--shoes=100] [--decks=8] [--rounds-per-shoe=60] [--seed=20260902] \\\n+  [--tables=10] [--start-session-id=1000000] [--round-seconds=45] \\\n+  [--start-date=2026-09-02]"
    )
}

#[derive(Debug)]
enum GeneratorError {
    Arguments(String),
    GeneratedRound(String),
}

impl fmt::Display for GeneratorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Arguments(message) | Self::GeneratedRound(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl Error for GeneratorError {}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use game_ev_engine::{CsvReplayConfig, replay_csv_text};

    use super::{GeneratorConfig, generate_csv};

    fn config(seed: u64) -> GeneratorConfig {
        GeneratorConfig {
            output: PathBuf::from("unused.csv"),
            shoes: 2,
            decks: 8,
            rounds_per_shoe: 10,
            seed,
            tables: 2,
            start_session_id: 50_000,
            round_seconds: 45,
            start_date: "2026-09-02".to_owned(),
        }
    }

    #[test]
    fn same_seed_generates_identical_csv() {
        let mut first = Vec::new();
        let mut second = Vec::new();
        generate_csv(&mut first, &config(12345)).expect("第一次生成应该成功");
        generate_csv(&mut second, &config(12345)).expect("第二次生成应该成功");

        assert_eq!(first, second);
    }

    #[test]
    fn generated_csv_passes_the_real_replay_validator() {
        let mut bytes = Vec::new();
        let summary = generate_csv(&mut bytes, &config(98765)).expect("随机 CSV 应该生成成功");
        assert_eq!(summary.shoes, 2);
        assert_eq!(summary.rounds, 20);
        assert_eq!(
            summary.player_wins + summary.banker_wins + summary.ties,
            summary.rounds
        );

        let csv_text = String::from_utf8(bytes).expect("CSV 应该是 UTF-8");
        let replay_config = CsvReplayConfig::new(8, 0.009, 0.0, 10_000.0, 0.05, 500.0, 500.0)
            .expect("回放配置应该合法");
        let report = replay_csv_text(&csv_text, replay_config).expect("随机 CSV 应该通过真实回放");

        assert_eq!(report.dataset.total_rows, 20);
        assert_eq!(report.quality.fully_observable_sessions, 2);
        assert_eq!(report.quality.valid_card_rows, 20);
        assert_eq!(report.quality.invalid_card_rows, 0);
        assert_eq!(report.quality.outcome_mismatch_rows, 0);
        assert_eq!(report.quality.quarantined_rounds, 0);
    }
}
