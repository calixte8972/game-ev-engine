//! 可重复的随机百家乐牌靴生成器。
//!
//! 生成器只负责创建真实牌靴、洗牌并按百家乐规则发牌，不计算下注策略。
//! 生成出的三列 CSV 可以继续交给 [`super::replay_csv_text`]，因此命令行、
//! 浏览器随机回测和用户上传 CSV 最终都复用同一条回放与结算路径。

use std::{error::Error, fmt, io::Write};

use serde::{Deserialize, Serialize};

use crate::{Card, Rank, RoundOutcome, Suit};

use super::{banker_should_draw, player_should_draw, resolve_round};

#[cfg(target_arch = "wasm32")]
use crate::Shoe;

#[cfg(target_arch = "wasm32")]
use super::{SideBetRoundLimits, calculate_main_and_side_outcomes_with_mask};

#[cfg(target_arch = "wasm32")]
use super::replay::parse_raw_cards;
#[cfg(target_arch = "wasm32")]
use super::{PreparedReplayWeight, StreamPacket, StreamSourceRound, outcome_code};

/// 随机回测的样本参数。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BaccaratSimulationConfig {
    /// 生成多少个相互独立、分别重新洗牌的牌靴。
    pub shoes: u64,
    /// 每個牌靴由几副标准扑克牌组成。
    pub decks: u8,
    /// 每靴生成的最大子局数；当前生成器会生成恰好这么多局。
    pub max_rounds_per_shoe: u32,
    /// 确定性随机种子；参数和种子相同，生成数据也完全相同。
    pub seed: u64,
    /// 第一靴使用的编号，后续牌靴逐一加一。
    pub start_session_id: u64,
}

impl BaccaratSimulationConfig {
    /// 构造并验证一组可以完整生成的参数。
    pub fn new(
        shoes: u64,
        decks: u8,
        max_rounds_per_shoe: u32,
        seed: u64,
        start_session_id: u64,
    ) -> Result<Self, BaccaratSimulationError> {
        if shoes == 0 {
            return Err(BaccaratSimulationError::InvalidConfig(
                "牌靴数必须大于 0".to_owned(),
            ));
        }
        if !(1..=8).contains(&decks) {
            return Err(BaccaratSimulationError::InvalidConfig(
                "副牌数必须在 1..=8 之间".to_owned(),
            ));
        }
        if max_rounds_per_shoe == 0 {
            return Err(BaccaratSimulationError::InvalidConfig(
                "每靴最大子局数必须大于 0".to_owned(),
            ));
        }

        // 一局最多会发六张牌。按最坏情况验证后，任意随机洗牌都能完整生成，
        // 不会出现某些种子成功、另一些种子在最后一局耗尽牌靴的情况。
        let guaranteed_rounds = u32::from(decks) * 52 / 6;
        if max_rounds_per_shoe > guaranteed_rounds {
            return Err(BaccaratSimulationError::InvalidConfig(format!(
                "{decks} 副牌最多保证生成 {guaranteed_rounds} 局，当前填写了 {max_rounds_per_shoe} 局"
            )));
        }
        start_session_id.checked_add(shoes - 1).ok_or_else(|| {
            BaccaratSimulationError::InvalidConfig("牌靴编号发生 u64 溢出".to_owned())
        })?;

        Ok(Self {
            shoes,
            decks,
            max_rounds_per_shoe,
            seed,
            start_session_id,
        })
    }
}

/// 生成完成后的基础样本统计。
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct BaccaratGenerationSummary {
    pub shoes: u64,
    pub rounds: u64,
    pub player_wins: u64,
    pub banker_wins: u64,
    pub ties: u64,
}

/// 随机生成过程可能返回的明确错误。
#[derive(Debug)]
pub enum BaccaratSimulationError {
    InvalidConfig(String),
    GeneratedRound(String),
    Csv(csv::Error),
    Utf8(std::string::FromUtf8Error),
}

impl fmt::Display for BaccaratSimulationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) | Self::GeneratedRound(message) => {
                formatter.write_str(message)
            }
            Self::Csv(error) => write!(formatter, "CSV 生成失败：{error}"),
            Self::Utf8(error) => write!(formatter, "CSV 不是有效 UTF-8：{error}"),
        }
    }
}

impl Error for BaccaratSimulationError {}

impl From<csv::Error> for BaccaratSimulationError {
    fn from(error: csv::Error) -> Self {
        Self::Csv(error)
    }
}

/// 写出只包含 `session_id,round_no,raw_cards` 的随机牌靴 CSV。
pub fn write_baccarat_csv<W: Write>(
    writer: W,
    config: BaccaratSimulationConfig,
) -> Result<BaccaratGenerationSummary, BaccaratSimulationError> {
    let mut csv = csv::WriterBuilder::new().from_writer(writer);
    let mut random = DeterministicRng::new(config.seed);
    let mut summary = BaccaratGenerationSummary::default();

    for shoe_index in 0..config.shoes {
        let session_id = config.start_session_id + shoe_index;
        let mut shoe = full_shoe_codes(config.decks);
        random.shuffle(&mut shoe);

        for round_no in 1..=config.max_rounds_per_shoe {
            let dealt = deal_round(&mut shoe)?;
            let cards = dealt
                .dealing_order
                .iter()
                .copied()
                .map(provider_card)
                .collect::<Result<Vec<_>, _>>()?;
            let result = resolve_round(&cards).map_err(|error| {
                BaccaratSimulationError::GeneratedRound(format!(
                    "牌靴 {session_id} 第 {round_no} 局不符合补牌规则：{error}"
                ))
            })?;

            match result.outcome() {
                RoundOutcome::Player => summary.player_wins += 1,
                RoundOutcome::Banker => summary.banker_wins += 1,
                RoundOutcome::Tie => summary.ties += 1,
            }

            csv.serialize(GeneratedRound {
                session_id,
                round_no,
                raw_cards: format!(
                    "b:{};p:{}",
                    join_codes(&dealt.banker),
                    join_codes(&dealt.player)
                ),
            })?;
            summary.rounds += 1;
        }
        summary.shoes += 1;
    }

    csv.flush().map_err(csv::Error::from)?;
    Ok(summary)
}

/// 在内存中生成三列 CSV，供浏览器直接衔接回放引擎。
pub fn generate_baccarat_csv_text(
    config: BaccaratSimulationConfig,
) -> Result<String, BaccaratSimulationError> {
    let mut bytes = Vec::new();
    write_baccarat_csv(&mut bytes, config)?;
    String::from_utf8(bytes).map_err(BaccaratSimulationError::Utf8)
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct GeneratedRound {
    session_id: u64,
    round_no: u32,
    raw_cards: String,
}

/// RNG 留在生成器中，不随批次或 Worker 数改变，且任何时刻只生成一靴。
#[allow(dead_code)]
pub(crate) struct BaccaratShoeGenerator {
    config: BaccaratSimulationConfig,
    random: DeterministicRng,
    next_index: u64,
}

#[allow(dead_code)]
impl BaccaratShoeGenerator {
    pub(crate) fn new(config: BaccaratSimulationConfig) -> Self {
        Self {
            config,
            random: DeterministicRng::new(config.seed),
            next_index: 0,
        }
    }

    pub(crate) fn next_json(&mut self) -> Result<String, String> {
        if self.next_index >= self.config.shoes {
            return Ok("null".into());
        }
        let mut shoe = full_shoe_codes(self.config.decks);
        self.random.shuffle(&mut shoe);
        let mut rows = Vec::with_capacity(self.config.max_rounds_per_shoe as usize);
        for round_no in 1..=self.config.max_rounds_per_shoe {
            let dealt = deal_round(&mut shoe).map_err(|e| e.to_string())?;
            rows.push(GeneratedRound {
                session_id: self.config.start_session_id + self.next_index,
                round_no,
                raw_cards: format!(
                    "b:{};p:{}",
                    join_codes(&dealt.banker),
                    join_codes(&dealt.player)
                ),
            });
        }
        self.next_index += 1;
        serde_json::to_string(&rows).map_err(|e| e.to_string())
    }
}

/// 把随机生成器产生的一靴三列数据直接转换为概率预计算包。
///
/// 这是随机回测的无 CSV 路径：生成器仍然只负责确定性洗牌和发牌，子 Worker
/// 收到 JSON 牌局后在 WASM 内重建 `Shoe`、计算概率并返回和 CSV 回放相同的
/// `StreamPacket`。这样不会再经历“随机数据 -> CSV 文本 -> CSV 解析器”的中间层。
#[cfg(target_arch = "wasm32")]
pub(crate) fn prepare_generated_shoe_json(
    rows_json: &str,
    decks: u8,
    source_base: usize,
    side_bet_round_limits: SideBetRoundLimits,
) -> Result<String, String> {
    let rows: Vec<GeneratedRound> = serde_json::from_str(rows_json).map_err(|e| e.to_string())?;
    let mut shoe = Shoe::new(decks).map_err(|e| e.to_string())?;
    let mut output = Vec::with_capacity(rows.len());

    for (index, row) in rows.into_iter().enumerate() {
        let cards = parse_raw_cards(&row.raw_cards)
            .map_err(|e| e.to_string())?
            .ok_or("随机牌局不应为空")?;
        let result = resolve_round(&cards).map_err(|e| e.to_string())?;
        let mask = side_bet_round_limits.calculation_mask(row.round_no);
        let (weights, side_weights) =
            calculate_main_and_side_outcomes_with_mask(&shoe, mask).map_err(|e| e.to_string())?;
        let source_order = source_base
            .checked_add(index)
            .ok_or("随机牌局来源行号溢出")?;
        let packet = StreamPacket {
            source: StreamSourceRound {
                table_id: 1,
                session_id: row.session_id,
                round_no: row.round_no,
                started_at: format!("CSV 第 {} 行", source_order + 2),
                source_order,
                has_started_at: false,
                cards: cards.iter().map(|card| card.index() as u8).collect(),
                outcome: outcome_code(result.outcome()),
                banker_total: result.banker_hand().total(),
            },
            prepared: PreparedReplayWeight {
                table_id: 1,
                session_id: row.session_id,
                round_no: row.round_no,
                weights,
                side_weights,
            },
            last_in_shoe: false,
        };
        output.push(packet);
        shoe.remove_many(&cards).map_err(|e| e.to_string())?;
    }

    // `last_in_shoe` 需要依据本次 rows 的长度设置。先统一生成后再标记，避免
    // 在生成循环里为了判断末尾重复保存全部牌面或额外传一个结束包。
    if let Some(last) = output.last_mut() {
        last.last_in_shoe = true;
    }
    serde_json::to_string(&output).map_err(|e| e.to_string())
}

struct DealtRound {
    player: Vec<u8>,
    banker: Vec<u8>,
    dealing_order: Vec<u8>,
}

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

fn deal_round(shoe: &mut Vec<u8>) -> Result<DealtRound, BaccaratSimulationError> {
    // 前四张永远按照闲 1、庄 1、闲 2、庄 2 的真实顺序发出。
    let player_first = draw(shoe)?;
    let banker_first = draw(shoe)?;
    let player_second = draw(shoe)?;
    let banker_second = draw(shoe)?;

    let mut player = vec![player_first, player_second];
    let mut banker = vec![banker_first, banker_second];
    let mut dealing_order = vec![player_first, banker_first, player_second, banker_second];
    let player_initial = hand_total(&player);
    let banker_initial = hand_total(&banker);

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

fn draw(shoe: &mut Vec<u8>) -> Result<u8, BaccaratSimulationError> {
    shoe.pop().ok_or_else(|| {
        BaccaratSimulationError::GeneratedRound("牌靴没有足够的牌完成指定局数".to_owned())
    })
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

fn provider_card(code: u8) -> Result<Card, BaccaratSimulationError> {
    let suit_index = code / 20;
    let rank_number = code % 20;
    if suit_index >= Suit::ALL.len() as u8 || !(1..=13).contains(&rank_number) {
        return Err(BaccaratSimulationError::GeneratedRound(format!(
            "生成了非法来源牌码：{code}"
        )));
    }
    Ok(Card::new(
        Rank::ALL[usize::from(rank_number - 1)],
        Suit::ALL[usize::from(suit_index)],
    ))
}

fn join_codes(cards: &[u8]) -> String {
    cards
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// 小型确定性 PRNG，仅用于可重复模拟，不用于密码或真钱抽奖。
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
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
        let upper = upper_exclusive as u64;
        let rejection_threshold = upper.wrapping_neg() % upper;
        loop {
            let value = self.next_u64();
            if value >= rejection_threshold {
                return (value % upper) as usize;
            }
        }
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let other = self.index(index + 1);
            values.swap(index, other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BaccaratShoeGenerator, BaccaratSimulationConfig, GeneratedRound, generate_baccarat_csv_text,
    };
    use crate::{CsvReplayConfig, replay_csv_text};

    fn config(seed: u64) -> BaccaratSimulationConfig {
        BaccaratSimulationConfig::new(2, 8, 10, seed, 50_000).expect("测试生成参数应该合法")
    }

    #[test]
    fn same_seed_generates_identical_three_column_csv() {
        let first = generate_baccarat_csv_text(config(12345)).expect("第一次生成应该成功");
        let second = generate_baccarat_csv_text(config(12345)).expect("第二次生成应该成功");

        assert_eq!(first, second);
        assert_eq!(first.lines().next(), Some("session_id,round_no,raw_cards"));
    }

    #[test]
    fn generated_csv_passes_the_real_replay_validator() {
        let csv = generate_baccarat_csv_text(config(98765)).expect("随机 CSV 应该生成成功");
        let replay_config = CsvReplayConfig::new(8, 0.009, 0.0, 10_000.0, 0.05, 500.0, 500.0)
            .expect("回放配置应该合法");
        let report = replay_csv_text(&csv, replay_config).expect("随机 CSV 应该通过真实回放");

        assert_eq!(report.dataset.total_rows, 20);
        assert_eq!(report.quality.fully_observable_sessions, 2);
        assert_eq!(report.quality.valid_card_rows, 20);
        assert_eq!(report.quality.invalid_card_rows, 0);
        assert_eq!(report.quality.outcome_mismatch_rows, 0);
    }

    #[test]
    fn incremental_generator_keeps_the_same_seed_stream_as_csv_writer() {
        let expected = generate_baccarat_csv_text(config(12345)).expect("整批生成应该成功");
        let expected_cards: Vec<String> = csv::Reader::from_reader(expected.as_bytes())
            .records()
            .map(|record| record.expect("生成的 CSV 应该可读取")[2].to_owned())
            .collect();

        let mut generator = BaccaratShoeGenerator::new(config(12345));
        let mut actual_cards = Vec::new();
        for _ in 0..2 {
            let rows: Vec<GeneratedRound> =
                serde_json::from_str(&generator.next_json().expect("增量牌靴应该生成"))
                    .expect("增量牌靴 JSON 应该有效");
            assert_eq!(rows.len(), 10);
            actual_cards.extend(rows.into_iter().map(|row| row.raw_cards));
        }
        assert_eq!(actual_cards, expected_cards);
        assert_eq!(generator.next_json().expect("生成完毕应该正常返回"), "null");
    }
}
