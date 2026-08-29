//! 对一天百家乐牌局 CSV 进行只读离线回放。
//!
//! 这个程序是数据库导出格式与 Rust 核心之间的适配层。它严格遵守下面的顺序：
//!
//! ```text
//! 用当前牌靴计算下注前概率和 EV
//!     -> 根据策略生成方向和凯利金额
//!     -> 校验本局真实牌面与数据库结果
//!     -> 最后才从牌靴扣除本局牌面
//! ```
//!
//! 当前 CSV 的 `raw_cards` 形如 `b:24,31,45;p:31,42,47`。数据库把四种
//! 花色分别编码为 `1..13`、`21..33`、`41..53`、`61..73`，每个区间内的
//! `1..13` 对应 A 到 K。`result_code` 是位标志，最低三位中的 1、2、4
//! 分别表示庄胜、闲胜、和局。

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::Instant,
};

use game_ev_engine::{
    BetPlanAction, BettingPolicy, Card, KellyPolicy, MainBet, MainBetRules, OutcomeWeights, Rank,
    RebateRule, RoundOutcome, Shoe, Suit, calculate_main_outcomes, resolve_round,
};
use serde::{Deserialize, Serialize};

/// 数据库导出的原始一局。金额等当前回放不需要的列不进入这个结构。
#[derive(Debug, Deserialize)]
struct CsvRound {
    /// 来源记录唯一键，用于验证导出数据没有重复行。
    #[serde(rename = "__source_pk")]
    source_pk: String,
    /// 桌台编号。
    table_id: u64,
    /// 场次编号；当前数据中它承担牌靴 ID 的作用。
    session_id: u64,
    /// 场次内连续递增的局号。
    round_no: u32,
    /// 开局时间。格式为 `YYYY-MM-DD HH:MM:SS`，可按文本排序。
    started_at: String,
    /// 开奖时间。
    settled_at: String,
    /// 庄、闲各自收到的牌。
    raw_cards: String,
    /// 数据库组合结果位标志。
    result_code: u64,
}

/// 一行经过牌面与结果校验后的状态。
#[derive(Debug)]
struct LoadedRound {
    source: CsvRound,
    /// `None` 表示原始内容是 `b:,,;p:,,`，没有可用于回放的牌。
    cards: Option<Vec<Card>>,
    /// 有牌时由 Rust 根据牌面算出的庄、闲或和。
    outcome: Option<RoundOutcome>,
    /// 解析、发牌规则或记录结果不一致时保存原因。
    validation_error: Option<String>,
}

/// 命令行可调整的回放策略。当前默认使用业务确认的 0.9% 返水。
#[derive(Debug, Clone)]
struct ReplayConfig {
    input: PathBuf,
    rebate_rate: f64,
    minimum_ev: f64,
    bankroll: f64,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
}

impl ReplayConfig {
    /// 读取简单的 `--name=value` 参数，避免为一次性回放入口引入大型 CLI 框架。
    fn from_args() -> Result<Self, String> {
        let mut arguments = env::args().skip(1);
        let input = arguments.next().ok_or_else(|| usage("缺少 CSV 文件路径"))?;

        let mut config = Self {
            input: PathBuf::from(input),
            rebate_rate: 0.009,
            minimum_ev: 0.0,
            bankroll: 10_000.0,
            max_fraction: 1.0,
            max_round_stake: 10_000.0,
            table_limit: 10_000.0,
        };

        for argument in arguments {
            let (name, value) = argument
                .split_once('=')
                .ok_or_else(|| usage(&format!("参数必须使用 --name=value：{argument}")))?;

            let parsed = value
                .parse::<f64>()
                .map_err(|_| usage(&format!("参数 {name} 不是有效数字：{value}")))?;

            match name {
                "--rebate" => config.rebate_rate = parsed,
                "--minimum-ev" => config.minimum_ev = parsed,
                "--bankroll" => config.bankroll = parsed,
                "--max-fraction" => config.max_fraction = parsed,
                "--max-round-stake" => config.max_round_stake = parsed,
                "--table-limit" => config.table_limit = parsed,
                _ => return Err(usage(&format!("未知参数：{name}"))),
            }
        }

        if !config.rebate_rate.is_finite() || !(0.0..=1.0).contains(&config.rebate_rate) {
            return Err("--rebate 必须是 0..=1 内的有限小数".to_owned());
        }
        if !config.minimum_ev.is_finite() {
            return Err("--minimum-ev 必须是有限数字".to_owned());
        }
        if !config.bankroll.is_finite() || config.bankroll <= 0.0 {
            return Err("--bankroll 必须是有限正数".to_owned());
        }

        // 其余三个资金限制交给正式领域构造器再次验证。这里不复制同一套规则。
        KellyPolicy::new(
            config.max_fraction,
            config.max_round_stake,
            config.table_limit,
        )
        .map_err(|error| error.to_string())?;

        Ok(config)
    }

    /// 把数值返水率转换成核心层已有的业务规则。
    fn rebate(&self) -> RebateRule {
        if self.rebate_rate == 0.0 {
            RebateRule::None
        } else {
            RebateRule::AllExceptMainBetTie {
                rate: self.rebate_rate,
            }
        }
    }
}

/// 最终输出中的策略配置，防止报告脱离计算参数后被误读。
#[derive(Debug, Serialize)]
struct ConfigReport {
    payout_rule: &'static str,
    rebate_rule: &'static str,
    rebate_rate: f64,
    minimum_effective_ev: f64,
    bankroll: f64,
    bankroll_mode: &'static str,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
}

/// CSV 基础画像。
#[derive(Debug, Default, Serialize)]
struct DatasetReport {
    input_path: String,
    business_date: String,
    total_rows: u64,
    table_count: usize,
    session_count: usize,
    started_at_min: String,
    started_at_max: String,
    settled_at_min: String,
    settled_at_max: String,
    duplicate_source_pk_rows: u64,
    duplicate_round_keys: u64,
}

/// 决定数据是否能安全用于概率回放的质量指标。
#[derive(Debug, Default, Serialize)]
struct QualityReport {
    valid_card_rows: u64,
    empty_card_rows: u64,
    invalid_card_rows: u64,
    outcome_mismatch_rows: u64,
    sessions_starting_at_one: u64,
    sessions_starting_mid_shoe: u64,
    sessions_with_round_gaps: u64,
    sessions_with_empty_cards: u64,
    sessions_with_invalid_rows: u64,
    fully_observable_sessions: u64,
    quarantined_rounds: u64,
}

/// 按下注方向保存计数，JSON 字段直接对应业务术语。
#[derive(Debug, Default, Serialize)]
struct BetCounts {
    player: u64,
    banker: u64,
    tie: u64,
}

impl BetCounts {
    fn increment(&mut self, bet: MainBet) {
        match bet {
            MainBet::Player => self.player += 1,
            MainBet::Banker => self.banker += 1,
            MainBet::Tie => self.tie += 1,
        }
    }
}

/// 对所有可观测牌靴真正运行 Rust 核心后得到的统计。
#[derive(Debug, Default, Serialize)]
struct ReplayReport {
    replayed_sessions: u64,
    replayed_rounds: u64,
    probability_cache_hits: u64,
    probability_cache_misses: u64,
    candidate_bets: BetCounts,
    placed_bets: BetCounts,
    skipped_bets: u64,
    placed_bet_wins: u64,
    placed_bet_hit_rate: Option<f64>,
    average_candidate_effective_ev: Option<f64>,
    minimum_candidate_effective_ev: Option<f64>,
    maximum_candidate_effective_ev: Option<f64>,
    total_suggested_stake: f64,
    total_expected_profit: f64,
    hypothetical_actual_profit: f64,
}

/// stdout 中的完整机器可读报告。
#[derive(Debug, Serialize)]
struct DayReplayReport {
    config: ConfigReport,
    dataset: DatasetReport,
    quality: QualityReport,
    replay: ReplayReport,
    elapsed_seconds: f64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("单日回放失败：{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let config = ReplayConfig::from_args().map_err(ReplayError::Configuration)?;
    let started = Instant::now();

    eprintln!("读取 CSV：{}", config.input.display());
    let (rounds, mut dataset, mut quality) = load_rounds(&config.input)?;
    eprintln!(
        "读取完成：{} 行，{} 张桌，{} 个场次",
        dataset.total_rows, dataset.table_count, dataset.session_count
    );

    let replay = replay_rounds(&rounds, &config, &mut quality)?;
    let report = DayReplayReport {
        config: ConfigReport {
            payout_rule: "standard_banker_commission_5_percent",
            rebate_rule: if config.rebate_rate == 0.0 {
                "none"
            } else {
                "all_except_player_or_banker_push"
            },
            rebate_rate: config.rebate_rate,
            minimum_effective_ev: config.minimum_ev,
            bankroll: config.bankroll,
            bankroll_mode: "fixed_per_round_no_compounding",
            max_fraction: config.max_fraction,
            max_round_stake: config.max_round_stake,
            table_limit: config.table_limit,
        },
        dataset: {
            // 输出规范化后的绝对路径，方便以后确认报告究竟来自哪个文件。
            dataset.input_path = config
                .input
                .canonicalize()
                .unwrap_or_else(|_| config.input.clone())
                .display()
                .to_string();
            dataset
        },
        quality,
        replay,
        elapsed_seconds: started.elapsed().as_secs_f64(),
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// 一次读取完成三件事：CSV 反序列化、单局规则校验、数据集基础画像。
fn load_rounds(
    path: &Path,
) -> Result<(Vec<LoadedRound>, DatasetReport, QualityReport), Box<dyn Error>> {
    let mut reader = csv::ReaderBuilder::new().flexible(false).from_path(path)?;
    let mut rounds = Vec::new();
    let mut dataset = DatasetReport::default();
    let mut quality = QualityReport::default();
    let mut source_keys = HashSet::new();
    let mut round_keys = HashSet::new();
    let mut tables = HashSet::new();
    let mut sessions = HashSet::new();
    let mut dates = BTreeMap::<String, u64>::new();

    for row in reader.deserialize::<CsvRound>() {
        let source = row?;
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
            .ok_or_else(|| ReplayError::InvalidTimestamp(source.started_at.clone()))?
            .to_owned();
        *dates.entry(date).or_default() += 1;

        let (cards, outcome, validation_error) = validate_source_round(&source, &mut quality);
        rounds.push(LoadedRound {
            source,
            cards,
            outcome,
            validation_error,
        });
    }

    dataset.table_count = tables.len();
    dataset.session_count = sessions.len();
    if dates.len() != 1 {
        return Err(Box::new(ReplayError::MultipleBusinessDates(
            dates.into_iter().collect(),
        )));
    }
    dataset.business_date = dates
        .into_keys()
        .next()
        .expect("非空 CSV 在前面已经记录日期");

    if dataset.duplicate_source_pk_rows > 0 || dataset.duplicate_round_keys > 0 {
        return Err(Box::new(ReplayError::DuplicateKeys {
            source_pk_rows: dataset.duplicate_source_pk_rows,
            round_keys: dataset.duplicate_round_keys,
        }));
    }

    Ok((rounds, dataset, quality))
}

/// 校验单局本身，但此时不修改任何牌靴。
fn validate_source_round(
    source: &CsvRound,
    quality: &mut QualityReport,
) -> (Option<Vec<Card>>, Option<RoundOutcome>, Option<String>) {
    let parsed = match parse_raw_cards(&source.raw_cards) {
        Ok(Some(cards)) => cards,
        Ok(None) => {
            quality.empty_card_rows += 1;
            return (None, None, None);
        }
        Err(error) => {
            quality.invalid_card_rows += 1;
            return (None, None, Some(error.to_string()));
        }
    };

    let result = match resolve_round(&parsed) {
        Ok(result) => result,
        Err(error) => {
            quality.invalid_card_rows += 1;
            return (
                Some(parsed),
                None,
                Some(format!("牌序不符合百家乐补牌规则：{error}")),
            );
        }
    };

    let recorded = match decode_recorded_outcome(source.result_code) {
        Ok(outcome) => outcome,
        Err(error) => {
            quality.invalid_card_rows += 1;
            return (
                Some(parsed),
                Some(result.outcome()),
                Some(error.to_string()),
            );
        }
    };

    if recorded != result.outcome() {
        quality.outcome_mismatch_rows += 1;
        return (
            Some(parsed),
            Some(result.outcome()),
            Some(format!(
                "数据库结果 {recorded:?} 与 Rust 结果 {:?} 不一致",
                result.outcome()
            )),
        );
    }

    quality.valid_card_rows += 1;
    (Some(parsed), Some(result.outcome()), None)
}

/// 按桌台和牌靴分组，只回放从第 1 局开始且所有局都可验证的场次。
fn replay_rounds(
    rounds: &[LoadedRound],
    config: &ReplayConfig,
    quality: &mut QualityReport,
) -> Result<ReplayReport, Box<dyn Error>> {
    let mut groups = BTreeMap::<(u64, u64), Vec<&LoadedRound>>::new();
    for round in rounds {
        groups
            .entry((round.source.table_id, round.source.session_id))
            .or_default()
            .push(round);
    }

    let rules = MainBetRules::standard();
    let rebate = config.rebate();
    let betting_policy = BettingPolicy::new(rebate, config.minimum_ev);
    let kelly_policy = KellyPolicy::new(
        config.max_fraction,
        config.max_round_stake,
        config.table_limit,
    )?;
    let mut probability_cache = HashMap::<[u16; 10], OutcomeWeights>::new();
    let mut report = ReplayReport {
        minimum_candidate_effective_ev: None,
        maximum_candidate_effective_ev: None,
        ..ReplayReport::default()
    };
    let mut effective_ev_sum = 0.0;

    for ((table_id, session_id), mut group) in groups {
        group.sort_by_key(|round| round.source.round_no);
        let first_round = group[0].source.round_no;
        let starts_at_one = first_round == 1;
        let has_gap = group
            .windows(2)
            .any(|pair| pair[1].source.round_no != pair[0].source.round_no.saturating_add(1));
        let has_empty = group.iter().any(|round| round.cards.is_none());
        // 空牌已经由 has_empty 单独表示；这里只统计真正的格式、规则或结果错误。
        let has_invalid = group.iter().any(|round| round.validation_error.is_some());

        // 这四项是彼此独立的数据质量标签，必须先全部统计。例如一个场次既可能
        // 从第 28 局开始，也可能完全没有牌；不能因为第一项失败就漏记第二项。
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

        // 只要任何一项破坏了“从完整牌靴按连续有效牌面回放”的前提，就隔离
        // 整个场次。一个场次即使同时命中多个标签，也只累计一次隔离局数。
        if !starts_at_one || has_gap || has_empty || has_invalid {
            quality.quarantined_rounds += group.len() as u64;
            continue;
        }

        quality.fully_observable_sessions += 1;
        report.replayed_sessions += 1;
        let mut shoe = Shoe::default();

        for round in group {
            // 这一步发生在扣除本局牌之前。缓存键只包含 0～9 点的剩余数量，
            // 因为主注概率与具体花色无关；相同点数组成可安全复用同一结果。
            let point_counts = shoe.baccarat_point_counts();
            let weights = if let Some(weights) = probability_cache.get(&point_counts) {
                report.probability_cache_hits += 1;
                *weights
            } else {
                match calculate_main_outcomes(&shoe) {
                    Ok(weights) => {
                        report.probability_cache_misses += 1;
                        probability_cache.insert(point_counts, weights);
                        weights
                    }
                    Err(error) => {
                        return Err(Box::new(ReplayError::Probability {
                            table_id,
                            session_id,
                            round_no: round.source.round_no,
                            message: error.to_string(),
                        }));
                    }
                }
            };

            let plan = kelly_policy.plan(&betting_policy, weights, rules, config.bankroll)?;
            let decision = *plan.decision();
            let candidate = decision.candidate();
            let effective_ev = decision.effective_ev();
            report.candidate_bets.increment(candidate);
            effective_ev_sum += effective_ev;
            report.minimum_candidate_effective_ev = Some(
                report
                    .minimum_candidate_effective_ev
                    .map_or(effective_ev, |current| current.min(effective_ev)),
            );
            report.maximum_candidate_effective_ev = Some(
                report
                    .maximum_candidate_effective_ev
                    .map_or(effective_ev, |current| current.max(effective_ev)),
            );

            if let BetPlanAction::Place { bet, amount } = *plan.action() {
                report.placed_bets.increment(bet);
                let outcome = round.outcome.expect("场次进入回放前已验证 outcome");
                if bet_matches_outcome(bet, outcome) {
                    report.placed_bet_wins += 1;
                }
                report.total_suggested_stake += amount;
                report.total_expected_profit += amount * effective_ev;

                // 使用同一赔付和返水规则结算已经发生的结果。这里的 bankroll
                // 每局固定为配置值，因此只是独立报价回测，不是资金复利模拟。
                let net_profit_per_unit =
                    rules.settle(bet, outcome) + rebate.rate_for(bet, outcome);
                report.hypothetical_actual_profit += amount * net_profit_per_unit;
            } else {
                report.skipped_bets += 1;
            }

            // 决策和结果统计完成后，才允许扣除本局牌。这条顺序防止未来泄漏。
            let cards = round.cards.as_deref().expect("场次进入回放前已验证 cards");
            if let Err(error) = shoe.remove_many(cards) {
                return Err(Box::new(ReplayError::ShoeState {
                    table_id,
                    session_id,
                    round_no: round.source.round_no,
                    message: error.to_string(),
                }));
            }

            report.replayed_rounds += 1;
            if report.replayed_rounds.is_multiple_of(10_000) {
                eprintln!("已回放 {} 局……", report.replayed_rounds);
            }
        }
    }

    if report.replayed_rounds > 0 {
        report.average_candidate_effective_ev =
            Some(effective_ev_sum / report.replayed_rounds as f64);
    }
    let placed_total =
        report.placed_bets.player + report.placed_bets.banker + report.placed_bets.tie;
    if placed_total > 0 {
        report.placed_bet_hit_rate = Some(report.placed_bet_wins as f64 / placed_total as f64);
    }

    Ok(report)
}

/// 把 `b:庄牌;p:闲牌` 转换成核心解析器要求的真实发牌顺序。
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

    // 数据库存储是“按一方聚合”，核心规则使用“真实发牌顺序”。前四张固定
    // 交错，若双方都有第三张，则闲第三张一定先于庄第三张。
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

/// 将来源系统的数字牌码映射为项目中的 `Card`。
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

/// `result_code` 还可能包含对子等边注标志，这里只读取最低三位的主结果。
fn decode_recorded_outcome(result_code: u64) -> Result<RoundOutcome, ProviderDataError> {
    match result_code & 0b111 {
        0b001 => Ok(RoundOutcome::Banker),
        0b010 => Ok(RoundOutcome::Player),
        0b100 => Ok(RoundOutcome::Tie),
        bits => Err(ProviderDataError::MainOutcomeBits { result_code, bits }),
    }
}

fn bet_matches_outcome(bet: MainBet, outcome: RoundOutcome) -> bool {
    matches!(
        (bet, outcome),
        (MainBet::Player, RoundOutcome::Player)
            | (MainBet::Banker, RoundOutcome::Banker)
            | (MainBet::Tie, RoundOutcome::Tie)
    )
}

fn update_min_max(minimum: &mut String, maximum: &mut String, value: &str) {
    if minimum.is_empty() || value < minimum.as_str() {
        *minimum = value.to_owned();
    }
    if maximum.is_empty() || value > maximum.as_str() {
        *maximum = value.to_owned();
    }
}

fn usage(message: &str) -> String {
    format!(
        "{message}\n用法：replay_csv <csv-path> [--rebate=0.015] [--minimum-ev=0] \
         [--bankroll=10000] [--max-fraction=1] [--max-round-stake=10000] \
         [--table-limit=10000]"
    )
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

#[derive(Debug)]
enum ReplayError {
    Configuration(String),
    InvalidTimestamp(String),
    MultipleBusinessDates(Vec<(String, u64)>),
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
    ShoeState {
        table_id: u64,
        session_id: u64,
        round_no: u32,
        message: String,
    },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(message) => formatter.write_str(message),
            Self::InvalidTimestamp(value) => write!(formatter, "非法 started_at：{value}"),
            Self::MultipleBusinessDates(dates) => {
                write!(formatter, "单日回放文件包含多个开局日期：{dates:?}")
            }
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

impl Error for ReplayError {}

#[cfg(test)]
mod tests {
    use super::{decode_recorded_outcome, parse_raw_cards, provider_card};
    use game_ev_engine::{RoundOutcome, resolve_round};

    #[test]
    fn provider_card_ranges_map_to_four_suits_and_thirteen_ranks() {
        assert_eq!(provider_card(1).expect("1 是合法牌码").to_string(), "AC");
        assert_eq!(provider_card(13).expect("13 是合法牌码").to_string(), "KC");
        assert_eq!(provider_card(21).expect("21 是合法牌码").to_string(), "AD");
        assert_eq!(provider_card(33).expect("33 是合法牌码").to_string(), "KD");
        assert_eq!(provider_card(41).expect("41 是合法牌码").to_string(), "AH");
        assert_eq!(provider_card(53).expect("53 是合法牌码").to_string(), "KH");
        assert_eq!(provider_card(61).expect("61 是合法牌码").to_string(), "AS");
        assert_eq!(provider_card(73).expect("73 是合法牌码").to_string(), "KS");
        assert!(provider_card(20).is_err());
        assert!(provider_card(74).is_err());
    }

    #[test]
    fn raw_cards_are_reordered_from_sides_to_real_dealing_order() {
        let cards = parse_raw_cards("b:24,31,45;p:31,42,47")
            .expect("牌面格式合法")
            .expect("本局有牌");
        let text: Vec<String> = cards.into_iter().map(|card| card.to_string()).collect();

        assert_eq!(text, ["JD", "4D", "2H", "JD", "7H", "5H"]);
    }

    #[test]
    fn empty_provider_payload_is_not_a_playable_round() {
        assert_eq!(parse_raw_cards("b:,,;p:,,").expect("空牌格式合法"), None);
    }

    #[test]
    fn result_code_uses_low_bits_for_main_outcome() {
        assert_eq!(decode_recorded_outcome(33), Ok(RoundOutcome::Banker));
        assert_eq!(decode_recorded_outcome(322), Ok(RoundOutcome::Player));
        assert_eq!(decode_recorded_outcome(36), Ok(RoundOutcome::Tie));
        assert!(decode_recorded_outcome(7).is_err());
    }

    #[test]
    fn parsed_sample_obeys_the_same_round_rules_as_the_core() {
        let cards = parse_raw_cards("b:24,31,45;p:31,42,47")
            .expect("牌面格式合法")
            .expect("本局有牌");
        let result = resolve_round(&cards).expect("真实样例应符合补牌规则");

        assert_eq!(result.outcome(), RoundOutcome::Tie);
    }
}
