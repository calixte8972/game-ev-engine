//! 浏览器和其他上层程序可复用的百家乐 CSV 策略回放。
//!
//! 回放严格保持“下注前决策”的时间顺序：
//!
//! ```text
//! 当前牌靴 -> 枚举概率 -> EV 门槛 -> 凯利金额 -> 用真实结果结算 -> 扣除本局牌
//! ```
//!
//! 真实结果只用于结算已经生成的下注计划，绝不会参与本局下注方向或金额计算。
//! 本模块还会隔离无法从第 1 局连续重建的牌靴，防止使用不完整牌靴得到伪概率。

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    fmt,
};

use serde::{Deserialize, Serialize};

use crate::{Card, Rank, Shoe, Suit};

use super::{
    BaccaratHand, BetTarget, BettingPolicy, CombinedBetPlanAction, KellyPolicy, MainBet,
    MainBetRules, OutcomeWeights, RebateRule, RoundOutcome, SideBet, SideBetRules, SideBetWeights,
    StakeSizingStrategy, calculate_main_and_side_outcomes, resolve_round,
};

/// 一次 CSV 回放使用的完整策略配置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CsvReplayConfig {
    decks: u8,
    rules: MainBetRules,
    stake_strategy: StakeSizingStrategy,
    rebate_rate: f64,
    minimum_effective_ev: f64,
    minimum_side_bet_ev: f64,
    initial_bankroll: f64,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
    side_bet_limit: f64,
    /// 幸运 6、幸运 7、超级幸运 7 允许下注的最后局数；None 表示不限制。
    lucky_bet_max_round: Option<u32>,
    /// 是否允许同一局同时下注多个达到门槛的目标。
    allow_multiple_bets: bool,
}

impl CsvReplayConfig {
    /// 创建并验证回放配置。
    ///
    /// `max_fraction` 是单局最多使用的本金比例上限，不是“几分之几凯利”的
    /// 乘数。实际比例仍由完整凯利公式计算，再与这个上限取较小值。
    pub fn new(
        decks: u8,
        rebate_rate: f64,
        minimum_effective_ev: f64,
        initial_bankroll: f64,
        max_fraction: f64,
        max_round_stake: f64,
        table_limit: f64,
    ) -> Result<Self, CsvReplayError> {
        Self::with_strategy(
            decks,
            MainBetRules::standard(),
            StakeSizingStrategy::FullKelly,
            rebate_rate,
            minimum_effective_ev,
            initial_bankroll,
            max_fraction,
            max_round_stake,
            table_limit,
        )
    }

    /// 创建带赔付规则和金额策略的完整回放配置。
    #[allow(clippy::too_many_arguments)]
    pub fn with_strategy(
        decks: u8,
        rules: MainBetRules,
        stake_strategy: StakeSizingStrategy,
        rebate_rate: f64,
        minimum_effective_ev: f64,
        initial_bankroll: f64,
        max_fraction: f64,
        max_round_stake: f64,
        table_limit: f64,
    ) -> Result<Self, CsvReplayError> {
        Self::with_side_bets(
            decks,
            rules,
            stake_strategy,
            rebate_rate,
            minimum_effective_ev,
            minimum_effective_ev,
            initial_bankroll,
            max_fraction,
            max_round_stake,
            table_limit,
            max_round_stake,
        )
    }

    /// 创建允许十一种边注参与方向选择的完整回放配置。
    #[allow(clippy::too_many_arguments)]
    pub fn with_side_bets(
        decks: u8,
        rules: MainBetRules,
        stake_strategy: StakeSizingStrategy,
        rebate_rate: f64,
        minimum_effective_ev: f64,
        minimum_side_bet_ev: f64,
        initial_bankroll: f64,
        max_fraction: f64,
        max_round_stake: f64,
        table_limit: f64,
        side_bet_limit: f64,
    ) -> Result<Self, CsvReplayError> {
        Shoe::new(decks).map_err(|error| CsvReplayError::Configuration(error.to_string()))?;

        if !rebate_rate.is_finite() || !(0.0..=1.0).contains(&rebate_rate) {
            return Err(CsvReplayError::Configuration(
                "返水比例必须是 0..=1 内的有限小数".to_owned(),
            ));
        }
        if !minimum_effective_ev.is_finite() {
            return Err(CsvReplayError::Configuration(
                "最低有效 EV 必须是有限数字".to_owned(),
            ));
        }
        if !minimum_side_bet_ev.is_finite() {
            return Err(CsvReplayError::Configuration(
                "边注最低 EV 必须是有限数字".to_owned(),
            ));
        }
        if !initial_bankroll.is_finite() || initial_bankroll <= 0.0 {
            return Err(CsvReplayError::Configuration(
                "初始本金必须是有限正数".to_owned(),
            ));
        }

        // 资金上限的边界统一交给生产 KellyPolicy 验证，避免回放复制另一套规则。
        KellyPolicy::with_strategy(stake_strategy, max_fraction, max_round_stake, table_limit)
            .and_then(|policy| policy.with_side_bet_limit(side_bet_limit))
            .map_err(|error| CsvReplayError::Configuration(error.to_string()))?;

        Ok(Self {
            decks,
            rules,
            stake_strategy,
            rebate_rate,
            minimum_effective_ev,
            minimum_side_bet_ev,
            initial_bankroll,
            max_fraction,
            max_round_stake,
            table_limit,
            side_bet_limit,
            lucky_bet_max_round: None,
            allow_multiple_bets: false,
        })
    }

    /// 设置幸运 6/7 可以参与策略的最后一局。
    ///
    /// `0` 表示不限制；`N > 0` 表示第 1..=N 局允许，从第 N+1 局起禁用。
    /// 该限制只移除三种幸运边注，其他主注和边注仍照常比较 EV。
    pub fn with_lucky_bet_max_round(mut self, max_round: u32) -> Self {
        self.lucky_bet_max_round = (max_round > 0).then_some(max_round);
        self
    }

    /// 设置是否允许同一局同时下注多个合格目标。
    ///
    /// 关闭时保留旧行为，只选择有效 EV 最高的一项；开启时会把所有通过
    /// 各自 EV 门槛的目标都生成计划，并共享本局总风险上限。
    pub fn with_multiple_bets(mut self, enabled: bool) -> Self {
        self.allow_multiple_bets = enabled;
        self
    }

    fn rebate(self) -> RebateRule {
        if self.rebate_rate == 0.0 {
            RebateRule::None
        } else {
            RebateRule::AllExceptMainBetTie {
                rate: self.rebate_rate,
            }
        }
    }
}

/// 浏览器上传 CSV 后得到的完整机器可读报告。
#[derive(Debug, Serialize)]
pub struct CsvReplayReport {
    pub config: CsvReplayConfigSnapshot,
    pub dataset: CsvDatasetReport,
    pub quality: CsvQualityReport,
    pub summary: CsvReplaySummary,
    /// 只保存真正下注的局，直接回答“什么时候可以下注”。
    pub bets: Vec<CsvBetDetail>,
    /// 为兼容旧版 JSON 保留的字段。当前版本保留全部下注明细，因此恒为 0。
    pub omitted_bet_details: u64,
}

/// 报告携带的配置快照，避免结果脱离参数后被误读。
#[derive(Debug, Serialize)]
pub struct CsvReplayConfigSnapshot {
    pub decks: u8,
    pub payout_rule: &'static str,
    pub stake_strategy: &'static str,
    pub strategy_parameter: Option<f64>,
    pub fixed_stake: Option<f64>,
    pub rebate_rule: &'static str,
    pub rebate_rate: f64,
    pub minimum_effective_ev: f64,
    pub minimum_side_bet_ev: f64,
    pub initial_bankroll: f64,
    pub bankroll_mode: &'static str,
    pub max_fraction: f64,
    pub max_round_stake: f64,
    pub table_limit: f64,
    pub side_bet_limit: f64,
    /// null 表示不限制；正整数 N 表示仅前 N 局允许幸运 6/7。
    pub lucky_bet_max_round: Option<u32>,
    /// 是否允许一局同时保存多笔下注明细。
    pub allow_multiple_bets: bool,
}

/// CSV 文件与时间范围的基础画像。
#[derive(Debug, Default, Serialize)]
pub struct CsvDatasetReport {
    pub total_rows: u64,
    pub table_count: usize,
    pub session_count: usize,
    pub business_date_count: usize,
    pub business_date_min: String,
    pub business_date_max: String,
    pub started_at_min: String,
    pub started_at_max: String,
    pub settled_at_min: String,
    pub settled_at_max: String,
    pub duplicate_source_pk_rows: u64,
    pub duplicate_round_keys: u64,
}

/// 判断哪些牌靴可以安全回放的数据质量指标。
#[derive(Debug, Default, Serialize)]
pub struct CsvQualityReport {
    pub valid_card_rows: u64,
    pub empty_card_rows: u64,
    pub invalid_card_rows: u64,
    pub outcome_mismatch_rows: u64,
    pub sessions_starting_at_one: u64,
    pub sessions_starting_mid_shoe: u64,
    pub sessions_with_round_gaps: u64,
    pub sessions_with_empty_cards: u64,
    pub sessions_with_invalid_rows: u64,
    pub fully_observable_sessions: u64,
    pub quarantined_rounds: u64,
}

/// 按方向保存计数，字段名直接使用稳定业务名称。
#[derive(Debug, Default, Serialize)]
pub struct CsvBetCounts {
    pub player: u64,
    pub banker: u64,
    pub tie: u64,
    pub any_pair: u64,
    pub banker_pair: u64,
    pub player_pair: u64,
    pub perfect_pair: u64,
    pub big: u64,
    pub small: u64,
    pub lucky_seven: u64,
    pub super_lucky_seven: u64,
    pub lucky_six: u64,
    pub banker_dragon_bonus: u64,
    pub player_dragon_bonus: u64,
}

impl CsvBetCounts {
    fn increment(&mut self, bet: BetTarget) {
        match bet {
            BetTarget::Main(MainBet::Player) => self.player += 1,
            BetTarget::Main(MainBet::Banker) => self.banker += 1,
            BetTarget::Main(MainBet::Tie) => self.tie += 1,
            BetTarget::Side(SideBet::AnyPair) => self.any_pair += 1,
            BetTarget::Side(SideBet::BankerPair) => self.banker_pair += 1,
            BetTarget::Side(SideBet::PlayerPair) => self.player_pair += 1,
            BetTarget::Side(SideBet::PerfectPair) => self.perfect_pair += 1,
            BetTarget::Side(SideBet::Big) => self.big += 1,
            BetTarget::Side(SideBet::Small) => self.small += 1,
            BetTarget::Side(SideBet::LuckySeven) => self.lucky_seven += 1,
            BetTarget::Side(SideBet::SuperLuckySeven) => self.super_lucky_seven += 1,
            BetTarget::Side(SideBet::LuckySix) => self.lucky_six += 1,
            BetTarget::Side(SideBet::BankerDragonBonus) => self.banker_dragon_bonus += 1,
            BetTarget::Side(SideBet::PlayerDragonBonus) => self.player_dragon_bonus += 1,
        }
    }
}

/// 全部可回放局的策略和盈亏汇总。
#[derive(Debug, Default, Serialize)]
pub struct CsvReplaySummary {
    pub replayed_sessions: u64,
    pub replayed_rounds: u64,
    pub probability_cache_hits: u64,
    pub probability_cache_misses: u64,
    pub candidate_bets: CsvBetCounts,
    pub placed_bets: CsvBetCounts,
    pub placed_bet_count: u64,
    pub skipped_bets: u64,
    pub wins: u64,
    pub losses: u64,
    pub pushes: u64,
    pub hit_rate: Option<f64>,
    pub average_candidate_effective_ev: Option<f64>,
    pub minimum_candidate_effective_ev: Option<f64>,
    pub maximum_candidate_effective_ev: Option<f64>,
    pub total_stake: f64,
    pub total_expected_profit: f64,
    pub base_game_profit: f64,
    pub rebate_income: f64,
    pub total_profit: f64,
    pub initial_bankroll: f64,
    pub final_bankroll: f64,
    /// 回放期间每局结算后出现过的最高本金，初始本金也作为第一个基准点参与比较。
    pub maximum_bankroll: f64,
    /// 回放期间的最大累计盈利：最高本金减去初始本金，而不是某一笔下注的盈利。
    pub maximum_profit: f64,
    /// 回放期间每局结算后出现过的最低本金；它不把下注尚未开奖时的临时占用算作余额。
    pub minimum_bankroll: f64,
    pub return_on_initial: f64,
    pub maximum_drawdown: f64,
    pub maximum_drawdown_rate: f64,
    /// 所有真实下注中，单笔下注金额的最大值。
    pub maximum_single_stake: f64,
    /// 同一局所有下注金额之和的最大值，用来观察同局多注的最大风险敞口。
    pub maximum_round_stake: f64,
}

/// 一笔真实下注明细。
#[derive(Debug, Serialize)]
pub struct CsvBetDetail {
    pub started_at: String,
    pub table_id: u64,
    pub session_id: u64,
    pub round_no: u32,
    pub bet: &'static str,
    pub outcome: &'static str,
    /// 闲家最终手牌，例如 `JD 2H 7H`。
    pub player_cards: String,
    /// 庄家最终手牌，例如 `4D JD 5H`。
    pub banker_cards: String,
    /// 闲家最终点数，供前端与具体牌面一起显示。
    pub player_total: u8,
    /// 庄家最终点数，供前端与具体牌面一起显示。
    pub banker_total: u8,
    pub result: &'static str,
    pub effective_ev: f64,
    pub kelly_fraction: f64,
    pub strategy_fraction: f64,
    pub applied_fraction: f64,
    pub amount: f64,
    pub expected_profit: f64,
    pub base_game_profit: f64,
    pub rebate_income: f64,
    pub actual_profit: f64,
    pub bankroll_after: f64,
}

/// 数据库导出的原始一局。额外列会被 serde 自动忽略。
#[derive(Debug, Deserialize)]
struct CsvRound {
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

/// 单局完成格式、发牌规则和结果一致性校验后的状态。
#[derive(Debug)]
struct LoadedRound {
    table_id: u64,
    session_id: u64,
    round_no: u32,
    started_at: String,
    cards: Option<Vec<Card>>,
    outcome: Option<RoundOutcome>,
    banker_total: Option<u8>,
    validation_error: Option<String>,
}

/// 在内存中读取并回放一个或多个业务日期的 CSV。
///
/// 所有可观测牌靴中的局按 `started_at` 排序，并共享一份滚动本金。因此下一局
/// 凯利金额使用的是上一笔真实结算后的余额，而不是每局重复使用初始本金。
pub fn replay_csv_text(
    csv_text: &str,
    config: CsvReplayConfig,
) -> Result<CsvReplayReport, CsvReplayError> {
    let (rounds, dataset, mut quality) = load_rounds(csv_text)?;
    let (summary, bets) = replay_rounds(&rounds, config, &mut quality)?;
    let omitted_bet_details = summary.placed_bet_count.saturating_sub(bets.len() as u64);

    Ok(CsvReplayReport {
        config: CsvReplayConfigSnapshot {
            decks: config.decks,
            payout_rule: if config.rules == MainBetRules::no_commission() {
                "no_commission_banker_six_half_payout"
            } else {
                "standard_banker_commission_5_percent"
            },
            stake_strategy: config.stake_strategy.as_str(),
            strategy_parameter: config.stake_strategy.parameter(),
            fixed_stake: config.stake_strategy.fixed_amount(),
            rebate_rule: if config.rebate_rate == 0.0 {
                "none"
            } else {
                "all_except_player_or_banker_push"
            },
            rebate_rate: config.rebate_rate,
            minimum_effective_ev: config.minimum_effective_ev,
            minimum_side_bet_ev: config.minimum_side_bet_ev,
            initial_bankroll: config.initial_bankroll,
            bankroll_mode: "shared_running_bankroll_chronological",
            max_fraction: config.max_fraction,
            max_round_stake: config.max_round_stake,
            table_limit: config.table_limit,
            side_bet_limit: config.side_bet_limit,
            lucky_bet_max_round: config.lucky_bet_max_round,
            allow_multiple_bets: config.allow_multiple_bets,
        },
        dataset,
        quality,
        summary,
        bets,
        omitted_bet_details,
    })
}

/// 读取 CSV、检查重复键并提前验证每一局的牌面和结果。
fn load_rounds(
    csv_text: &str,
) -> Result<(Vec<LoadedRound>, CsvDatasetReport, CsvQualityReport), CsvReplayError> {
    let normalized = csv_text.strip_prefix('\u{feff}').unwrap_or(csv_text);
    let mut reader = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(normalized.as_bytes());
    let mut rounds = Vec::new();
    let mut dataset = CsvDatasetReport::default();
    let mut quality = CsvQualityReport::default();
    let mut source_keys = HashSet::new();
    let mut round_keys = HashSet::new();
    let mut tables = HashSet::new();
    let mut sessions = HashSet::new();
    let mut dates = HashSet::new();

    for row in reader.deserialize::<CsvRound>() {
        let source = row.map_err(|error| CsvReplayError::Csv(error.to_string()))?;
        dataset.total_rows += 1;

        if !source_keys.insert(source.source_pk.clone()) {
            dataset.duplicate_source_pk_rows += 1;
        }
        if !round_keys.insert((source.table_id, source.session_id, source.round_no)) {
            dataset.duplicate_round_keys += 1;
        }
        tables.insert(source.table_id);
        sessions.insert((source.table_id, source.session_id));

        update_min_max(
            &mut dataset.started_at_min,
            &mut dataset.started_at_max,
            &source.started_at,
        );
        update_min_max(
            &mut dataset.settled_at_min,
            &mut dataset.settled_at_max,
            &source.settled_at,
        );

        let date = source
            .started_at
            .get(..10)
            .ok_or_else(|| CsvReplayError::InvalidTimestamp(source.started_at.clone()))?
            .to_owned();
        dates.insert(date.clone());
        update_min_max(
            &mut dataset.business_date_min,
            &mut dataset.business_date_max,
            &date,
        );

        let (cards, outcome, banker_total, validation_error) =
            validate_source_round(&source, &mut quality);
        rounds.push(LoadedRound {
            table_id: source.table_id,
            session_id: source.session_id,
            round_no: source.round_no,
            started_at: source.started_at,
            cards,
            outcome,
            banker_total,
            validation_error,
        });
    }

    if dataset.total_rows == 0 {
        return Err(CsvReplayError::EmptyDataset);
    }

    dataset.table_count = tables.len();
    dataset.session_count = sessions.len();
    dataset.business_date_count = dates.len();

    if dataset.duplicate_source_pk_rows > 0 || dataset.duplicate_round_keys > 0 {
        return Err(CsvReplayError::DuplicateKeys {
            source_pk_rows: dataset.duplicate_source_pk_rows,
            round_keys: dataset.duplicate_round_keys,
        });
    }

    Ok((rounds, dataset, quality))
}

/// 校验单局本身，但此时绝不修改牌靴。
fn validate_source_round(
    source: &CsvRound,
    quality: &mut CsvQualityReport,
) -> (
    Option<Vec<Card>>,
    Option<RoundOutcome>,
    Option<u8>,
    Option<String>,
) {
    let parsed = match parse_raw_cards(&source.raw_cards) {
        Ok(Some(cards)) => cards,
        Ok(None) => {
            quality.empty_card_rows += 1;
            return (None, None, None, None);
        }
        Err(error) => {
            quality.invalid_card_rows += 1;
            return (None, None, None, Some(error.to_string()));
        }
    };

    let result = match resolve_round(&parsed) {
        Ok(result) => result,
        Err(error) => {
            quality.invalid_card_rows += 1;
            return (
                Some(parsed),
                None,
                None,
                Some(format!("牌序不符合百家乐补牌规则：{error}")),
            );
        }
    };
    let calculated = result.outcome();
    let banker_total = result.banker_hand().total();

    let recorded = match decode_recorded_outcome(source.result_code) {
        Ok(outcome) => outcome,
        Err(error) => {
            quality.invalid_card_rows += 1;
            return (
                Some(parsed),
                Some(calculated),
                Some(banker_total),
                Some(error.to_string()),
            );
        }
    };

    if recorded != calculated {
        quality.outcome_mismatch_rows += 1;
        return (
            Some(parsed),
            Some(calculated),
            Some(banker_total),
            Some(format!(
                "数据库结果 {recorded:?} 与 Rust 结果 {calculated:?} 不一致"
            )),
        );
    }

    quality.valid_card_rows += 1;
    (Some(parsed), Some(calculated), Some(banker_total), None)
}

/// 隔离不完整牌靴，再按真实时间顺序使用共享本金运行策略。
fn replay_rounds(
    rounds: &[LoadedRound],
    config: CsvReplayConfig,
    quality: &mut CsvQualityReport,
) -> Result<(CsvReplaySummary, Vec<CsvBetDetail>), CsvReplayError> {
    let mut groups = BTreeMap::<(u64, u64), Vec<usize>>::new();
    for (index, round) in rounds.iter().enumerate() {
        groups
            .entry((round.table_id, round.session_id))
            .or_default()
            .push(index);
    }

    let mut eligible_indices = Vec::new();
    let mut eligible_sessions = Vec::new();

    for (key, mut indices) in groups {
        indices.sort_by_key(|index| rounds[*index].round_no);
        let starts_at_one = rounds[indices[0]].round_no == 1;
        let has_gap = indices
            .windows(2)
            .any(|pair| rounds[pair[1]].round_no != rounds[pair[0]].round_no.saturating_add(1));
        let has_empty = indices.iter().any(|index| rounds[*index].cards.is_none());
        let has_invalid = indices
            .iter()
            .any(|index| rounds[*index].validation_error.is_some());

        if starts_at_one {
            quality.sessions_starting_at_one += 1;
        } else {
            quality.sessions_starting_mid_shoe += 1;
        }
        if has_gap {
            quality.sessions_with_round_gaps += 1;
        }
        if has_empty {
            quality.sessions_with_empty_cards += 1;
        }
        if has_invalid {
            quality.sessions_with_invalid_rows += 1;
        }

        if !starts_at_one || has_gap || has_empty || has_invalid {
            quality.quarantined_rounds += indices.len() as u64;
            continue;
        }

        quality.fully_observable_sessions += 1;
        eligible_sessions.push(key);
        eligible_indices.extend(indices);
    }

    // 不同桌台可能同时存在；这里使用开局时间建立单一、可重复的资金时间线。
    eligible_indices.sort_by(|left, right| {
        let left_round = &rounds[*left];
        let right_round = &rounds[*right];
        (
            &left_round.started_at,
            left_round.table_id,
            left_round.session_id,
            left_round.round_no,
        )
            .cmp(&(
                &right_round.started_at,
                right_round.table_id,
                right_round.session_id,
                right_round.round_no,
            ))
    });

    let rules = config.rules;
    let side_rules = SideBetRules::default();
    let rebate = config.rebate();
    let betting_policy = BettingPolicy::with_side_bet_minimum(
        rebate,
        config.minimum_effective_ev,
        config.minimum_side_bet_ev,
    );
    let kelly_policy = KellyPolicy::with_strategy(
        config.stake_strategy,
        config.max_fraction,
        config.max_round_stake,
        config.table_limit,
    )
    .and_then(|policy| policy.with_side_bet_limit(config.side_bet_limit))
    .map_err(|error| CsvReplayError::Configuration(error.to_string()))?;

    let mut shoes = HashMap::new();
    for key in eligible_sessions.iter().copied() {
        let shoe = Shoe::new(config.decks)
            .map_err(|error| CsvReplayError::Configuration(error.to_string()))?;
        shoes.insert(key, shoe);
    }

    // 完美对子概率必须区分 52 种“Rank + 花色”。如果仍用 13 种 Rank 数量
    // 作为键，不同花色耗牌状态会错误命中同一个缓存条目。52 类具体牌快照也
    // 足以推导普通对子和主注点数，因此三类概率可以安全共用同一个缓存。
    let mut probability_cache =
        HashMap::<[u8; Card::DISTINCT_COUNT], (OutcomeWeights, SideBetWeights)>::new();
    let mut summary = CsvReplaySummary {
        replayed_sessions: eligible_sessions.len() as u64,
        initial_bankroll: config.initial_bankroll,
        final_bankroll: config.initial_bankroll,
        maximum_bankroll: config.initial_bankroll,
        minimum_bankroll: config.initial_bankroll,
        minimum_candidate_effective_ev: None,
        maximum_candidate_effective_ev: None,
        ..CsvReplaySummary::default()
    };
    let mut details = Vec::new();
    let mut current_bankroll = config.initial_bankroll;
    let mut peak_bankroll = current_bankroll;
    let mut effective_ev_sum = 0.0;

    for index in eligible_indices {
        let round = &rounds[index];
        let key = (round.table_id, round.session_id);
        let shoe = shoes.get_mut(&key).expect("可回放场次应该已经创建牌靴");

        // 先使用本局发牌前的牌靴计算，随后才允许查看真实结果并扣牌。
        let card_counts = shoe.card_counts();
        let (weights, side_weights) = if let Some(weights) = probability_cache.get(&card_counts) {
            summary.probability_cache_hits += 1;
            *weights
        } else {
            let weights = calculate_main_and_side_outcomes(shoe).map_err(|error| {
                CsvReplayError::Probability {
                    table_id: round.table_id,
                    session_id: round.session_id,
                    round_no: round.round_no,
                    message: error.to_string(),
                }
            })?;
            summary.probability_cache_misses += 1;
            probability_cache.insert(card_counts, weights);
            weights
        };

        let lucky_bets_allowed = |side_bet| {
            side_bet_allowed_for_round(side_bet, round.round_no, config.lucky_bet_max_round)
        };
        let mut plans = if config.allow_multiple_bets {
            kelly_policy
                .plan_all_multiple_with_side_bet_filter(
                    &betting_policy,
                    weights,
                    rules,
                    side_weights,
                    side_rules,
                    current_bankroll,
                    lucky_bets_allowed,
                )
                .map_err(|error| CsvReplayError::Strategy(error.to_string()))?
        } else {
            Vec::new()
        };

        // 多注模式在“没有任何目标达到门槛”时返回空列表。此时仍保留单注
        // 模式的最佳候选和跳过原因，保证汇总字段及页面提示不会丢失。
        if plans.is_empty() {
            plans.push(
                kelly_policy
                    .plan_all_with_side_bet_filter(
                        &betting_policy,
                        weights,
                        rules,
                        side_weights,
                        side_rules,
                        current_bankroll,
                        lucky_bets_allowed,
                    )
                    .map_err(|error| CsvReplayError::Strategy(error.to_string()))?,
            );
        }

        // 多注计划按有效 EV 从高到低排列，因此第一项继续作为旧版汇总中的
        // “本局最优候选”；其余项通过 placed_bets 按目标分别累计。
        let primary_plan = plans.first().expect("回放至少应该保留一个决策计划");
        let decision = *primary_plan.decision();
        let candidate = decision.candidate();
        let effective_ev = decision.effective_ev();
        summary.candidate_bets.increment(candidate);
        effective_ev_sum += effective_ev;
        summary.minimum_candidate_effective_ev = Some(
            summary
                .minimum_candidate_effective_ev
                .map_or(effective_ev, |current| current.min(effective_ev)),
        );
        summary.maximum_candidate_effective_ev = Some(
            summary
                .maximum_candidate_effective_ev
                .map_or(effective_ev, |current| current.max(effective_ev)),
        );

        let has_placed_plan = plans
            .iter()
            .any(|plan| matches!(plan.action(), CombinedBetPlanAction::Place { .. }));
        let outcome = round.outcome.expect("可回放局应该有经过验证的结果");
        let banker_total = round.banker_total.expect("可回放局应该有庄家最终点数");
        let round_result = has_placed_plan.then(|| {
            let cards = round.cards.as_deref().expect("可回放局应该有牌面");
            resolve_round(cards).expect("可回放牌局已通过规则验证")
        });
        let mut round_profit = 0.0;
        let mut round_stake = 0.0;
        let mut placed_details = Vec::new();

        for plan in &plans {
            match *plan.action() {
                CombinedBetPlanAction::Place { bet, amount } => {
                    let quote = plan.quote().expect("Place 动作应该保留凯利报价");
                    // 单笔下注在这里登记；同一局的多笔下注则继续累加到 round_stake，
                    // 这样两个指标分别回答“最大一注是多少”和“单局最多暴露多少资金”。
                    round_stake += amount;
                    summary.maximum_single_stake = summary.maximum_single_stake.max(amount);
                    // 基础输赢和返水分别结算，既方便审计，也能在报告中看出利润来源。
                    let (base_profit_per_unit, rebate_per_unit) = match bet {
                        BetTarget::Main(main_bet) => (
                            rules.settle_with_banker_total(main_bet, outcome, banker_total),
                            rebate.rate_for(main_bet, outcome),
                        ),
                        BetTarget::Side(side_bet) => (
                            side_rules.settle(
                                side_bet,
                                round_result.expect("边注结算应该有已解析的牌局结果"),
                            ),
                            0.0,
                        ),
                    };
                    let base_game_profit = amount * base_profit_per_unit;
                    let rebate_income = amount * rebate_per_unit;
                    let actual_profit = base_game_profit + rebate_income;
                    round_profit += actual_profit;

                    summary.placed_bets.increment(bet);
                    summary.placed_bet_count += 1;
                    summary.total_stake += amount;
                    summary.total_expected_profit += quote.expected_profit();
                    summary.base_game_profit += base_game_profit;
                    summary.rebate_income += rebate_income;

                    let result = if base_profit_per_unit > 0.0 {
                        summary.wins += 1;
                        "win"
                    } else if base_profit_per_unit < 0.0 {
                        summary.losses += 1;
                        "loss"
                    } else {
                        summary.pushes += 1;
                        "push"
                    };

                    let round_result = round_result.expect("已下注局应该有已解析的牌局结果");
                    placed_details.push((
                        bet,
                        quote,
                        plan.decision().effective_ev(),
                        round_result,
                        result,
                        base_game_profit,
                        rebate_income,
                        actual_profit,
                        amount,
                    ));
                }
                CombinedBetPlanAction::Skip { .. } => {
                    summary.skipped_bets += 1;
                }
            }
        }

        // 同一局的多笔下注必须同时结算，然后再更新一次本金和回撤；否则后面的
        // 边注会错误地把同局前一笔输赢当成下一局资金变化。
        summary.maximum_round_stake = summary.maximum_round_stake.max(round_stake);
        current_bankroll += round_profit;
        // 最高/最低本金按每局真实结算后的余额统计，初始本金已经在循环前作为基准写入。
        summary.maximum_bankroll = summary.maximum_bankroll.max(current_bankroll);
        summary.minimum_bankroll = summary.minimum_bankroll.min(current_bankroll);
        peak_bankroll = peak_bankroll.max(current_bankroll);
        let drawdown = (peak_bankroll - current_bankroll).max(0.0);
        let drawdown_rate = if peak_bankroll > 0.0 {
            drawdown / peak_bankroll
        } else {
            0.0
        };
        if drawdown > summary.maximum_drawdown {
            summary.maximum_drawdown = drawdown;
        }
        if drawdown_rate > summary.maximum_drawdown_rate {
            summary.maximum_drawdown_rate = drawdown_rate;
        }

        // 报告保留每一笔真实下注。浏览器端通过分页控制一次创建的表格行数，
        // 因此这里不能再为了 DOM 性能截断业务数据。
        for (
            bet,
            quote,
            effective_ev,
            round_result,
            result,
            base_game_profit,
            rebate_income,
            actual_profit,
            amount,
        ) in placed_details
        {
            details.push(CsvBetDetail {
                started_at: round.started_at.clone(),
                table_id: round.table_id,
                session_id: round.session_id,
                round_no: round.round_no,
                bet: bet.as_str(),
                outcome: outcome_name(outcome),
                player_cards: format_hand_cards(round_result.player_hand()),
                banker_cards: format_hand_cards(round_result.banker_hand()),
                player_total: round_result.player_hand().total(),
                banker_total: round_result.banker_hand().total(),
                result,
                effective_ev,
                kelly_fraction: quote.kelly_fraction(),
                strategy_fraction: quote.strategy_fraction(),
                applied_fraction: quote.applied_fraction(),
                amount,
                expected_profit: quote.expected_profit(),
                base_game_profit,
                rebate_income,
                actual_profit,
                bankroll_after: current_bankroll,
            });
        }

        // 本局决策和真实结算完成后才扣牌，禁止未来牌泄漏到当前决策。
        let cards = round.cards.as_deref().expect("可回放局应该有牌面");
        shoe.remove_many(cards)
            .map_err(|error| CsvReplayError::ShoeState {
                table_id: round.table_id,
                session_id: round.session_id,
                round_no: round.round_no,
                message: error.to_string(),
            })?;
        summary.replayed_rounds += 1;
    }

    if summary.replayed_rounds > 0 {
        summary.average_candidate_effective_ev =
            Some(effective_ev_sum / summary.replayed_rounds as f64);
    }
    if summary.placed_bet_count > 0 {
        summary.hit_rate = Some(summary.wins as f64 / summary.placed_bet_count as f64);
    }

    summary.final_bankroll = current_bankroll;
    summary.total_profit = current_bankroll - config.initial_bankroll;
    summary.maximum_profit = summary.maximum_bankroll - config.initial_bankroll;
    summary.return_on_initial = summary.total_profit / config.initial_bankroll;

    Ok((summary, details))
}

/// 判断某种边注在当前牌靴局号是否仍可参与策略比较。
///
/// 限制只适用于幸运 6、幸运 7 和超级幸运 7。其他边注无论局号多少都返回
/// true。边界采用包含语义：配置 20 时，第 20 局可下，第 21 局不可下。
fn side_bet_allowed_for_round(
    side_bet: SideBet,
    round_no: u32,
    lucky_bet_max_round: Option<u32>,
) -> bool {
    let is_lucky_bet = matches!(
        side_bet,
        SideBet::LuckySix | SideBet::LuckySeven | SideBet::SuperLuckySeven
    );

    !is_lucky_bet || lucky_bet_max_round.is_none_or(|max_round| round_no <= max_round)
}

/// 把一方最终实际使用的两张或三张牌转换成稳定、易读的牌面字符串。
///
/// 回放输入使用供应商数字牌码，而输出统一使用手工分析也接受的 `AS`、`10H`
/// 格式。这样前端展示和人工复核不需要了解供应商牌码规则。
fn format_hand_cards(hand: BaccaratHand) -> String {
    [
        Some(hand.first_card()),
        Some(hand.second_card()),
        hand.third_card(),
    ]
    .into_iter()
    .flatten()
    .map(|card| card.to_string())
    .collect::<Vec<_>>()
    .join(" ")
}

/// 把 `b:庄牌;p:闲牌` 转换为 P1、B1、P2、B2、可选 P3、可选 B3。
fn parse_raw_cards(raw: &str) -> Result<Option<Vec<Card>>, ProviderDataError> {
    let body = raw
        .strip_prefix("b:")
        .ok_or_else(|| ProviderDataError::CardPayload(raw.to_owned()))?;
    let (banker_text, player_text) = body
        .split_once(";p:")
        .ok_or_else(|| ProviderDataError::CardPayload(raw.to_owned()))?;
    let banker = parse_side_cards(banker_text)?;
    let player = parse_side_cards(player_text)?;

    if banker.is_empty() && player.is_empty() {
        return Ok(None);
    }
    if !(2..=3).contains(&banker.len()) || !(2..=3).contains(&player.len()) {
        return Err(ProviderDataError::HandShape {
            banker_cards: banker.len(),
            player_cards: player.len(),
        });
    }

    let mut dealing_order = vec![player[0], banker[0], player[1], banker[1]];
    if player.len() == 3 {
        dealing_order.push(player[2]);
    }
    if banker.len() == 3 {
        dealing_order.push(banker[2]);
    }
    Ok(Some(dealing_order))
}

fn parse_side_cards(input: &str) -> Result<Vec<Card>, ProviderDataError> {
    input
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let code = item
                .parse::<u8>()
                .map_err(|_| ProviderDataError::CardCodeText(item.to_owned()))?;
            provider_card(code)
        })
        .collect()
}

fn provider_card(code: u8) -> Result<Card, ProviderDataError> {
    let suit_index = code / 20;
    let rank_number = code % 20;
    if suit_index >= Suit::ALL.len() as u8 || !(1..=13).contains(&rank_number) {
        return Err(ProviderDataError::CardCode(code));
    }

    let suit = Suit::ALL[usize::from(suit_index)];
    let rank = Rank::ALL[usize::from(rank_number - 1)];
    Ok(Card::new(rank, suit))
}

fn decode_recorded_outcome(result_code: u64) -> Result<RoundOutcome, ProviderDataError> {
    match result_code & 0b111 {
        0b001 => Ok(RoundOutcome::Banker),
        0b010 => Ok(RoundOutcome::Player),
        0b100 => Ok(RoundOutcome::Tie),
        bits => Err(ProviderDataError::MainOutcomeBits { result_code, bits }),
    }
}

fn outcome_name(outcome: RoundOutcome) -> &'static str {
    match outcome {
        RoundOutcome::Player => "player",
        RoundOutcome::Banker => "banker",
        RoundOutcome::Tie => "tie",
    }
}

fn update_min_max(minimum: &mut String, maximum: &mut String, value: &str) {
    if minimum.is_empty() || value < minimum.as_str() {
        *minimum = value.to_owned();
    }
    if maximum.is_empty() || value > maximum.as_str() {
        *maximum = value.to_owned();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ProviderDataError {
    CardPayload(String),
    CardCodeText(String),
    CardCode(u8),
    HandShape {
        banker_cards: usize,
        player_cards: usize,
    },
    MainOutcomeBits {
        result_code: u64,
        bits: u64,
    },
}

impl fmt::Display for ProviderDataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CardPayload(raw) => write!(formatter, "无法解析开奖内容：{raw}"),
            Self::CardCodeText(value) => write!(formatter, "牌码不是 u8：{value}"),
            Self::CardCode(code) => write!(formatter, "未知来源牌码：{code}"),
            Self::HandShape {
                banker_cards,
                player_cards,
            } => write!(
                formatter,
                "非法手牌数量：庄 {banker_cards} 张，闲 {player_cards} 张"
            ),
            Self::MainOutcomeBits { result_code, bits } => write!(
                formatter,
                "result_code={result_code} 的主结果位 {bits} 不是庄 1、闲 2 或和 4"
            ),
        }
    }
}

impl Error for ProviderDataError {}

/// CSV 回放失败时返回的可读错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsvReplayError {
    Configuration(String),
    Csv(String),
    EmptyDataset,
    InvalidTimestamp(String),
    DuplicateKeys {
        source_pk_rows: u64,
        round_keys: u64,
    },
    Probability {
        table_id: u64,
        session_id: u64,
        round_no: u32,
        message: String,
    },
    Strategy(String),
    ShoeState {
        table_id: u64,
        session_id: u64,
        round_no: u32,
        message: String,
    },
}

impl fmt::Display for CsvReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message) => formatter.write_str(message),
            Self::Csv(message) => write!(formatter, "CSV 读取失败：{message}"),
            Self::EmptyDataset => formatter.write_str("CSV 没有任何数据行"),
            Self::InvalidTimestamp(value) => write!(formatter, "非法 started_at：{value}"),
            Self::DuplicateKeys {
                source_pk_rows,
                round_keys,
            } => write!(
                formatter,
                "发现重复键：source_pk 重复 {source_pk_rows} 行，局键重复 {round_keys} 行"
            ),
            Self::Probability {
                table_id,
                session_id,
                round_no,
                message,
            } => write!(
                formatter,
                "桌 {table_id} 牌靴 {session_id} 第 {round_no} 局概率计算失败：{message}"
            ),
            Self::Strategy(message) => write!(formatter, "下注策略计算失败：{message}"),
            Self::ShoeState {
                table_id,
                session_id,
                round_no,
                message,
            } => write!(
                formatter,
                "桌 {table_id} 牌靴 {session_id} 第 {round_no} 局扣牌失败：{message}"
            ),
        }
    }
}

impl Error for CsvReplayError {}

#[cfg(test)]
mod tests {
    use super::{
        CsvReplayConfig, parse_raw_cards, provider_card, replay_csv_text,
        side_bet_allowed_for_round,
    };
    use crate::{MainBetRules, SideBet, StakeSizingStrategy};

    const HEADER: &str =
        "__source_pk,table_id,session_id,round_no,started_at,settled_at,raw_cards,result_code\n";

    fn strategy() -> CsvReplayConfig {
        // 2% 返水足以让完整牌靴的庄注有效 EV 为正，测试可以稳定进入下注分支。
        CsvReplayConfig::new(8, 0.02, 0.0, 10_000.0, 0.05, 1_000.0, 1_000.0)
            .expect("测试策略应该合法")
    }

    #[test]
    fn provider_codes_and_dealing_order_match_the_database_contract() {
        assert_eq!(provider_card(1).expect("合法牌码").to_string(), "AC");
        assert_eq!(provider_card(73).expect("合法牌码").to_string(), "KS");

        let cards = parse_raw_cards("b:24,31,45;p:31,42,47")
            .expect("开奖内容应该合法")
            .expect("本局应该有牌");
        let text: Vec<String> = cards.into_iter().map(|card| card.to_string()).collect();
        assert_eq!(text, ["JD", "4D", "2H", "JD", "7H", "5H"]);
    }

    #[test]
    fn lucky_bet_round_limit_includes_the_configured_boundary() {
        let limit = Some(20);

        for side_bet in [
            SideBet::LuckySix,
            SideBet::LuckySeven,
            SideBet::SuperLuckySeven,
        ] {
            assert!(side_bet_allowed_for_round(side_bet, 20, limit));
            assert!(!side_bet_allowed_for_round(side_bet, 21, limit));
            assert!(side_bet_allowed_for_round(side_bet, 999, None));
        }

        // 局数限制不影响普通对子、大小、龙宝等其他边注。
        assert!(side_bet_allowed_for_round(SideBet::BankerPair, 21, limit));
        assert!(side_bet_allowed_for_round(
            SideBet::BankerDragonBonus,
            21,
            limit
        ));
    }

    #[test]
    fn replay_uses_running_bankroll_and_reconciles_profit_components() {
        let csv = format!(
            "{HEADER}a,1,9001,1,2026-08-20 00:00:12,2026-08-20 00:00:44,\"b:24,31,45;p:31,42,47\",36\n\
             b,1,9001,2,2026-08-20 00:00:54,2026-08-20 00:01:17,\"b:73,62,;p:53,8,\",322\n"
        );

        let report = replay_csv_text(&csv, strategy()).expect("两局完整牌靴应该可以回放");

        assert_eq!(report.quality.fully_observable_sessions, 1);
        assert_eq!(report.summary.replayed_rounds, 2);
        assert!(report.summary.placed_bet_count > 0);
        assert_eq!(report.bets.len() as u64, report.summary.placed_bet_count);
        assert!(report.summary.maximum_bankroll >= report.summary.initial_bankroll);
        assert!(report.summary.minimum_bankroll <= report.summary.initial_bankroll);
        assert!(report.summary.maximum_single_stake > 0.0);
        assert!(report.summary.maximum_round_stake >= report.summary.maximum_single_stake);
        assert!(
            (report.summary.maximum_profit
                - (report.summary.maximum_bankroll - report.summary.initial_bankroll))
                .abs()
                < 1e-9
        );
        assert_eq!(report.bets[0].player_cards, "JD 2H 7H");
        assert_eq!(report.bets[0].banker_cards, "4D JD 5H");
        assert_eq!(report.bets[0].player_total, 9);
        assert_eq!(report.bets[0].banker_total, 9);

        let component_profit = report.summary.base_game_profit + report.summary.rebate_income;
        assert!((report.summary.total_profit - component_profit).abs() < 1e-9);
        assert!(
            (report.summary.final_bankroll
                - (report.summary.initial_bankroll + report.summary.total_profit))
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn replay_keeps_more_than_two_thousand_bet_details() {
        let mut csv = String::from(HEADER);
        for index in 0..2_001 {
            csv.push_str(&format!(
                "row-{index},1,{},1,2026-08-20 00:00:12,2026-08-20 00:00:44,\"b:24,31,45;p:31,42,47\",36\n",
                10_000 + index
            ));
        }
        let config = CsvReplayConfig::with_strategy(
            8,
            MainBetRules::standard(),
            StakeSizingStrategy::Fixed { amount: 1.0 },
            0.02,
            0.0,
            10_000.0,
            1.0,
            1.0,
            1.0,
        )
        .expect("固定 1 元的测试策略应该合法");

        let report = replay_csv_text(&csv, config).expect("大量独立牌靴应该可以回放");

        assert_eq!(report.summary.placed_bet_count, 2_001);
        assert_eq!(report.bets.len(), 2_001);
        assert_eq!(report.omitted_bet_details, 0);
    }

    #[test]
    fn session_starting_mid_shoe_is_quarantined_instead_of_guessed() {
        let csv = format!(
            "{HEADER}a,1,9002,28,2026-08-20 00:00:12,2026-08-20 00:00:44,\"b:24,31,45;p:31,42,47\",36\n"
        );

        let report = replay_csv_text(&csv, strategy()).expect("不完整牌靴应进入隔离报告");

        assert_eq!(report.quality.sessions_starting_mid_shoe, 1);
        assert_eq!(report.quality.quarantined_rounds, 1);
        assert_eq!(report.summary.replayed_rounds, 0);
        assert_eq!(report.summary.final_bankroll, 10_000.0);
        assert_eq!(report.summary.maximum_bankroll, 10_000.0);
        assert_eq!(report.summary.minimum_bankroll, 10_000.0);
        assert_eq!(report.summary.maximum_profit, 0.0);
        assert_eq!(report.summary.maximum_single_stake, 0.0);
        assert_eq!(report.summary.maximum_round_stake, 0.0);
    }
}
