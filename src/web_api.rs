//! 浏览器与 Rust 概率核心之间的轻量适配层。
//!
//! JavaScript 只传入字符串和数字，本模块负责：
//!
//! ```text
//! 牌面文本 -> Vec<Card> -> Shoe -> MainBetAnalysis -> JSON
//! ```
//!
//! 真正的发牌规则、概率枚举、EV 和返水计算仍然全部复用 Rust 核心。这样本地
//! 回放、未来 Python 调用和浏览器页面不会各自维护一套容易分叉的算法。

use serde::Serialize;

use crate::{
    BetPlanSkipReason, BettingPolicy, Card, CombinedBetPlanAction, CsvReplayConfig,
    EffectiveBetMetrics, KellyPolicy, MainBet, MainBetAnalysis, MainBetRules, RebateRule, Shoe,
    SideBet, SideBetAnalysis, SideBetMetrics, SideBetRules, SkipReason, StakeSizingStrategy,
    calculate_main_and_side_outcomes, replay_csv_text,
};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// WebAssembly 导出给 JavaScript 的入口。
///
/// `source_mode` 支持：
///
/// - `consumed`：输入牌已经从完整牌靴中发走；
/// - `remaining`：输入牌就是牌靴当前剩余的全部牌。
///
/// 成功时返回 JSON 字符串，失败时返回可以直接展示给用户的中文错误。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = analyzeBaccarat)]
pub fn analyze_baccarat(
    source_mode: &str,
    decks: u8,
    cards_text: &str,
    rebate_rate: f64,
) -> Result<String, JsValue> {
    analyze_baccarat_json(source_mode, decks, cards_text, rebate_rate)
        .map_err(|message| JsValue::from_str(&message))
}

/// 带完整方向策略和资金管理参数的浏览器分析入口。
///
/// 页面把百分比先转换成小数再传入：例如 0.9% 返水传 `0.009`，最多使用
/// 本金 5% 传 `0.05`。Rust 同时返回是否下注、凯利比例和最终建议金额。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = analyzeBaccaratStrategy)]
pub fn analyze_baccarat_strategy(
    source_mode: &str,
    decks: u8,
    cards_text: &str,
    rebate_rate: f64,
    minimum_effective_ev: f64,
    bankroll: f64,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
    payout_rule: &str,
    stake_strategy: &str,
    fixed_stake: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
) -> Result<String, JsValue> {
    analyze_baccarat_strategy_json_with_side_bets(
        source_mode,
        decks,
        cards_text,
        rebate_rate,
        minimum_effective_ev,
        bankroll,
        max_fraction,
        max_round_stake,
        table_limit,
        payout_rule,
        stake_strategy,
        fixed_stake,
        minimum_side_bet_ev,
        side_bet_limit,
    )
    .map_err(|message| JsValue::from_str(&message))
}

/// 在 Web Worker 中运行的大型 CSV 回放入口。
///
/// CSV 文本不会发送到服务器；JavaScript 读取本地文件后直接交给同一份 WASM
/// 内存。回放结果使用共享滚动本金，并只返回真正下注的局作为明细。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = replayBaccaratCsv)]
pub fn replay_baccarat_csv(
    csv_text: &str,
    decks: u8,
    rebate_rate: f64,
    minimum_effective_ev: f64,
    initial_bankroll: f64,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
    payout_rule: &str,
    stake_strategy: &str,
    fixed_stake: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
) -> Result<String, JsValue> {
    replay_baccarat_csv_json_with_side_bets(
        csv_text,
        decks,
        rebate_rate,
        minimum_effective_ev,
        initial_bankroll,
        max_fraction,
        max_round_stake,
        table_limit,
        payout_rule,
        stake_strategy,
        fixed_stake,
        minimum_side_bet_ev,
        side_bet_limit,
    )
    .map_err(|message| JsValue::from_str(&message))
}

/// 使用与 WASM 入口相同的规则生成浏览器 JSON。
///
/// 这个纯 Rust 函数在普通测试目标中也能运行，因此不需要启动浏览器就能测试
/// 字符串解析、牌靴构造和输出协议。WASM 函数只在最外层把错误转成 `JsValue`。
pub fn analyze_baccarat_json(
    source_mode: &str,
    decks: u8,
    cards_text: &str,
    rebate_rate: f64,
) -> Result<String, String> {
    analyze_baccarat_strategy_json(
        source_mode,
        decks,
        cards_text,
        rebate_rate,
        0.0,
        10_000.0,
        1.0,
        10_000.0,
        10_000.0,
        "standard",
        "full_kelly",
        0.0,
    )
}

/// 普通 Rust 测试也能调用的完整策略分析函数。
#[allow(clippy::too_many_arguments)]
pub fn analyze_baccarat_strategy_json(
    source_mode: &str,
    decks: u8,
    cards_text: &str,
    rebate_rate: f64,
    minimum_effective_ev: f64,
    bankroll: f64,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
    payout_rule: &str,
    stake_strategy: &str,
    fixed_stake: f64,
) -> Result<String, String> {
    analyze_baccarat_strategy_json_with_side_bets(
        source_mode,
        decks,
        cards_text,
        rebate_rate,
        minimum_effective_ev,
        bankroll,
        max_fraction,
        max_round_stake,
        table_limit,
        payout_rule,
        stake_strategy,
        fixed_stake,
        minimum_effective_ev,
        max_round_stake,
    )
}

/// 主注和边注共同参与策略时使用的完整浏览器分析函数。
#[allow(clippy::too_many_arguments)]
pub fn analyze_baccarat_strategy_json_with_side_bets(
    source_mode: &str,
    decks: u8,
    cards_text: &str,
    rebate_rate: f64,
    minimum_effective_ev: f64,
    bankroll: f64,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
    payout_rule: &str,
    stake_strategy: &str,
    fixed_stake: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
) -> Result<String, String> {
    if !rebate_rate.is_finite() || !(0.0..=1.0).contains(&rebate_rate) {
        return Err("返水比例必须是 0% 到 100% 之间的有限数字".to_owned());
    }
    if !minimum_effective_ev.is_finite() {
        return Err("最低有效 EV 必须是有限数字".to_owned());
    }
    if !minimum_side_bet_ev.is_finite() {
        return Err("边注最低 EV 必须是有限数字".to_owned());
    }
    if !bankroll.is_finite() || bankroll <= 0.0 {
        return Err("本金必须是有限正数".to_owned());
    }

    let cards = parse_cards(cards_text)?;
    let normalized_mode = source_mode.trim().to_ascii_lowercase();
    let shoe = match normalized_mode.as_str() {
        "consumed" => {
            let mut shoe = Shoe::new(decks).map_err(|error| format!("副牌数不合法：{error}"))?;
            shoe.remove_many(&cards)
                .map_err(|error| format!("已消耗牌无法从牌靴扣除：{error}"))?;
            shoe
        }
        "remaining" => Shoe::from_remaining(decks, &cards)
            .map_err(|error| format!("剩余牌无法构成合法牌靴：{error}"))?,
        _ => return Err("输入模式必须是 consumed 或 remaining".to_owned()),
    };

    if shoe.total_remaining() < 6 {
        return Err(format!(
            "当前只剩 {} 张牌，至少需要 6 张才能计算下一局完整概率",
            shoe.total_remaining()
        ));
    }

    let (rules, payout_rule_code) = parse_payout_rule(payout_rule)?;
    let stake_strategy = parse_stake_strategy(stake_strategy, fixed_stake)?;
    let rebate = if rebate_rate == 0.0 {
        RebateRule::None
    } else {
        RebateRule::AllExceptMainBetTie { rate: rebate_rate }
    };
    let policy =
        BettingPolicy::with_side_bet_minimum(rebate, minimum_effective_ev, minimum_side_bet_ev);
    let kelly_policy =
        KellyPolicy::with_strategy(stake_strategy, max_fraction, max_round_stake, table_limit)
            .and_then(|policy| policy.with_side_bet_limit(side_bet_limit))
            .map_err(|error| format!("资金策略不合法：{error}"))?;
    let (weights, side_weights) = calculate_main_and_side_outcomes(&shoe)
        .map_err(|error| format!("概率与 EV 计算失败：{error}"))?;
    let analysis = MainBetAnalysis::from_weights(weights, rules);
    let side_analysis = SideBetAnalysis::calculate(side_weights, SideBetRules::default());
    let plan = kelly_policy
        .plan_all(
            &policy,
            weights,
            rules,
            side_weights,
            SideBetRules::default(),
            bankroll,
        )
        .map_err(|error| format!("下注策略计算失败：{error}"))?;
    let decision = *plan.decision();
    let quote = plan.quote();
    let (action, reason) = match *plan.action() {
        CombinedBetPlanAction::Place { .. } => ("place", None),
        CombinedBetPlanAction::Skip { reason } => ("skip", Some(skip_reason_code(reason))),
    };

    let response = BrowserAnalysis {
        source_mode: normalized_mode,
        decks,
        input_card_count: cards.len(),
        remaining_card_count: shoe.total_remaining(),
        rebate_rate,
        payout_rule: payout_rule_code,
        stake_strategy: stake_strategy.as_str(),
        fixed_stake: stake_strategy.fixed_amount(),
        minimum_main_bet_ev: minimum_effective_ev,
        minimum_side_bet_ev,
        side_bet_limit,
        bets: BrowserBets {
            player: BrowserBetMetrics::from_analysis(analysis, MainBet::Player, rebate),
            banker: BrowserBetMetrics::from_analysis(analysis, MainBet::Banker, rebate),
            tie: BrowserBetMetrics::from_analysis(analysis, MainBet::Tie, rebate),
        },
        side_bet_rules: "macau_lucky_seven_6_15_super_30_40_100",
        side_bets: BrowserSideBets {
            any_pair: BrowserSideBetMetrics::new(side_analysis.metrics(SideBet::AnyPair), "5:1"),
            banker_pair: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::BankerPair),
                "11:1",
            ),
            player_pair: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::PlayerPair),
                "11:1",
            ),
            lucky_seven: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::LuckySeven),
                "闲2张 6:1 / 闲3张 15:1",
            ),
            super_lucky_seven: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::SuperLuckySeven),
                "总4张 30:1 / 5张 40:1 / 6张 100:1",
            ),
        },
        recommendation: BrowserRecommendation {
            candidate_bet: decision.candidate().as_str(),
            bet_category: if decision.candidate().is_side() {
                "side"
            } else {
                "main"
            },
            base_ev: decision.base_ev(),
            rebate_ev: decision.rebate_ev(),
            effective_ev: decision.effective_ev(),
            action,
            reason,
            bankroll,
            kelly_fraction: quote.map(|value| value.kelly_fraction()),
            strategy_fraction: quote.map(|value| value.strategy_fraction()),
            applied_fraction: quote.map(|value| value.applied_fraction()),
            suggested_amount: quote.map_or(0.0, |value| value.amount()),
            expected_profit: quote.map_or(0.0, |value| value.expected_profit()),
        },
    };

    serde_json::to_string(&response).map_err(|error| format!("结果序列化失败：{error}"))
}

/// 普通 Rust 测试与 WASM Worker 共用的 CSV JSON 入口。
#[allow(clippy::too_many_arguments)]
pub fn replay_baccarat_csv_json(
    csv_text: &str,
    decks: u8,
    rebate_rate: f64,
    minimum_effective_ev: f64,
    initial_bankroll: f64,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
    payout_rule: &str,
    stake_strategy: &str,
    fixed_stake: f64,
) -> Result<String, String> {
    replay_baccarat_csv_json_with_side_bets(
        csv_text,
        decks,
        rebate_rate,
        minimum_effective_ev,
        initial_bankroll,
        max_fraction,
        max_round_stake,
        table_limit,
        payout_rule,
        stake_strategy,
        fixed_stake,
        minimum_effective_ev,
        max_round_stake,
    )
}

/// 主注与边注使用独立门槛和金额上限的 CSV 回放入口。
#[allow(clippy::too_many_arguments)]
pub fn replay_baccarat_csv_json_with_side_bets(
    csv_text: &str,
    decks: u8,
    rebate_rate: f64,
    minimum_effective_ev: f64,
    initial_bankroll: f64,
    max_fraction: f64,
    max_round_stake: f64,
    table_limit: f64,
    payout_rule: &str,
    stake_strategy: &str,
    fixed_stake: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
) -> Result<String, String> {
    let (rules, _) = parse_payout_rule(payout_rule)?;
    let stake_strategy = parse_stake_strategy(stake_strategy, fixed_stake)?;
    let config = CsvReplayConfig::with_side_bets(
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
    )
    .map_err(|error| format!("回放配置不合法：{error}"))?;
    let report = replay_csv_text(csv_text, config).map_err(|error| error.to_string())?;

    serde_json::to_string(&report).map_err(|error| format!("回放结果序列化失败：{error}"))
}

/// 把浏览器稳定字符串转换成核心赔付规则。
fn parse_payout_rule(input: &str) -> Result<(MainBetRules, &'static str), String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "standard" => Ok((MainBetRules::standard(), "standard")),
        "no_commission" => Ok((MainBetRules::no_commission(), "no_commission")),
        _ => Err("庄赔付规则必须是 standard 或 no_commission".to_owned()),
    }
}

/// 把金额策略字符串转换成互斥的领域枚举。
fn parse_stake_strategy(input: &str, fixed_stake: f64) -> Result<StakeSizingStrategy, String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "full_kelly" => Ok(StakeSizingStrategy::FullKelly),
        "half_kelly" => Ok(StakeSizingStrategy::HalfKelly),
        "quarter_kelly" => Ok(StakeSizingStrategy::QuarterKelly),
        "fixed" => Ok(StakeSizingStrategy::Fixed {
            amount: fixed_stake,
        }),
        _ => Err("金额策略必须是 full_kelly、half_kelly、quarter_kelly 或 fixed".to_owned()),
    }
}

/// 把内部跳过原因转换成稳定的浏览器字符串。
fn skip_reason_code(reason: BetPlanSkipReason) -> &'static str {
    match reason {
        BetPlanSkipReason::Strategy(SkipReason::BelowMinimumEv { .. }) => "below_minimum_ev",
        BetPlanSkipReason::NonPositiveKelly => "non_positive_kelly",
        BetPlanSkipReason::RiskLimitIsZero => "risk_limit_is_zero",
    }
}

/// 把空格、逗号、分号或中文顿号分隔的牌面文本解析成牌列表。
fn parse_cards(input: &str) -> Result<Vec<Card>, String> {
    input
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ',' | '，' | ';' | '；' | '、')
        })
        .filter(|token| !token.is_empty())
        .map(|token| {
            token
                .parse::<Card>()
                .map_err(|error| format!("无法识别牌面“{token}”：{error}"))
        })
        .collect()
}

/// 浏览器需要的一次完整分析结果。
#[derive(Debug, Serialize)]
struct BrowserAnalysis {
    source_mode: String,
    decks: u8,
    input_card_count: usize,
    remaining_card_count: u16,
    rebate_rate: f64,
    payout_rule: &'static str,
    stake_strategy: &'static str,
    fixed_stake: Option<f64>,
    minimum_main_bet_ev: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    bets: BrowserBets,
    side_bet_rules: &'static str,
    side_bets: BrowserSideBets,
    recommendation: BrowserRecommendation,
}

/// 三个主注方向的指标。
#[derive(Debug, Serialize)]
struct BrowserBets {
    player: BrowserBetMetrics,
    banker: BrowserBetMetrics,
    tie: BrowserBetMetrics,
}

/// 第一批边注的浏览器展示结果。
#[derive(Debug, Serialize)]
struct BrowserSideBets {
    any_pair: BrowserSideBetMetrics,
    banker_pair: BrowserSideBetMetrics,
    player_pair: BrowserSideBetMetrics,
    lucky_seven: BrowserSideBetMetrics,
    super_lucky_seven: BrowserSideBetMetrics,
}

/// 边注的一行概率、基础 EV 与赔付说明。
#[derive(Debug, Serialize)]
struct BrowserSideBetMetrics {
    probability: f64,
    ev: f64,
    house_edge: f64,
    rtp: f64,
    payout: &'static str,
}

impl BrowserSideBetMetrics {
    fn new(metrics: SideBetMetrics, payout: &'static str) -> Self {
        Self {
            probability: metrics.probability(),
            ev: metrics.ev(),
            house_edge: metrics.house_edge(),
            rtp: metrics.rtp(),
            payout,
        }
    }
}

/// 页面表格中一行需要显示的概率和 EV 指标。
#[derive(Debug, Serialize)]
struct BrowserBetMetrics {
    probability: f64,
    base_ev: f64,
    rebate_ev: f64,
    effective_ev: f64,
    house_edge: f64,
    rtp: f64,
}

impl BrowserBetMetrics {
    /// 从已有有效指标复制稳定字段，避免 JavaScript 重新推导任何数学结果。
    fn from_analysis(analysis: MainBetAnalysis, bet: MainBet, rebate: RebateRule) -> Self {
        let metrics: EffectiveBetMetrics = analysis.effective_metrics(bet, rebate);
        Self {
            probability: metrics.probability(),
            base_ev: metrics.base_ev(),
            rebate_ev: metrics.rebate_ev(),
            effective_ev: metrics.effective_ev(),
            house_edge: metrics.house_edge(),
            rtp: metrics.rtp(),
        }
    }
}

/// 有效 EV 方向策略的最终结果。
#[derive(Debug, Serialize)]
struct BrowserRecommendation {
    candidate_bet: &'static str,
    bet_category: &'static str,
    base_ev: f64,
    rebate_ev: f64,
    effective_ev: f64,
    action: &'static str,
    reason: Option<&'static str>,
    bankroll: f64,
    kelly_fraction: Option<f64>,
    strategy_fraction: Option<f64>,
    applied_fraction: Option<f64>,
    suggested_amount: f64,
    expected_profit: f64,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{
        analyze_baccarat_json, analyze_baccarat_strategy_json,
        analyze_baccarat_strategy_json_with_side_bets, replay_baccarat_csv_json,
    };

    #[test]
    fn empty_consumed_input_analyzes_a_full_eight_deck_shoe() {
        let json = analyze_baccarat_json("consumed", 8, "", 0.009)
            .expect("完整八副牌应能在浏览器接口中计算");
        let value: Value = serde_json::from_str(&json).expect("接口应返回合法 JSON");

        assert_eq!(value["source_mode"], "consumed");
        assert_eq!(value["input_card_count"], 0);
        assert_eq!(value["remaining_card_count"], 416);
        assert_eq!(value["rebate_rate"], 0.009);
        assert_eq!(value["recommendation"]["candidate_bet"], "banker");
        assert_eq!(
            value["side_bet_rules"],
            "macau_lucky_seven_6_15_super_30_40_100"
        );
        assert_eq!(value["side_bets"]["banker_pair"]["payout"], "11:1");
        assert!(
            value["side_bets"]["lucky_seven"]["probability"]
                .as_f64()
                .expect("幸运 7 概率应为数字")
                > 0.0
        );

        let probability_sum = value["bets"]["player"]["probability"]
            .as_f64()
            .expect("闲概率应为数字")
            + value["bets"]["banker"]["probability"]
                .as_f64()
                .expect("庄概率应为数字")
            + value["bets"]["tie"]["probability"]
                .as_f64()
                .expect("和概率应为数字");
        assert!((probability_sum - 1.0).abs() < 1e-12);
    }

    #[test]
    fn consumed_input_accepts_mixed_chinese_and_ascii_separators() {
        let value: Value = serde_json::from_str(
            &analyze_baccarat_json("consumed", 8, "AS，10H KD、7C", 0.009)
                .expect("四张合法牌应成功扣除"),
        )
        .expect("接口应返回合法 JSON");

        assert_eq!(value["input_card_count"], 4);
        assert_eq!(value["remaining_card_count"], 412);
    }

    #[test]
    fn remaining_mode_rejects_too_few_cards_for_probability_enumeration() {
        let error = analyze_baccarat_json("remaining", 8, "AS 2H 3D", 0.009)
            .expect_err("三张剩余牌无法计算完整下一局");

        assert!(error.contains("至少需要 6 张"));
    }

    #[test]
    fn invalid_mode_card_and_rebate_return_readable_errors() {
        assert!(analyze_baccarat_json("unknown", 8, "", 0.009).is_err());
        assert!(analyze_baccarat_json("consumed", 8, "1X", 0.009).is_err());
        assert!(analyze_baccarat_json("consumed", 8, "", 1.01).is_err());
    }

    #[test]
    fn strategy_response_includes_kelly_amount_and_threshold_reason() {
        let placed: Value = serde_json::from_str(
            &analyze_baccarat_strategy_json(
                "consumed",
                8,
                "",
                0.02,
                0.0,
                10_000.0,
                0.05,
                1_000.0,
                1_000.0,
                "standard",
                "full_kelly",
                0.0,
            )
            .expect("2% 返水应让完整牌靴产生正有效 EV"),
        )
        .expect("接口应返回合法 JSON");
        assert_eq!(placed["recommendation"]["action"], "place");
        assert!(
            placed["recommendation"]["suggested_amount"]
                .as_f64()
                .expect("金额应为数字")
                > 0.0
        );

        let skipped: Value = serde_json::from_str(
            &analyze_baccarat_strategy_json(
                "consumed",
                8,
                "",
                0.02,
                0.50,
                10_000.0,
                0.05,
                1_000.0,
                1_000.0,
                "standard",
                "full_kelly",
                0.0,
            )
            .expect("高 EV 门槛应返回 Skip 而不是接口错误"),
        )
        .expect("接口应返回合法 JSON");
        assert_eq!(skipped["recommendation"]["action"], "skip");
        assert_eq!(skipped["recommendation"]["reason"], "below_minimum_ev");
    }

    #[test]
    fn no_commission_and_fractional_kelly_are_exposed_in_the_json_contract() {
        let full: Value = serde_json::from_str(
            &analyze_baccarat_strategy_json(
                "consumed",
                8,
                "",
                0.02,
                0.0,
                10_000.0,
                1.0,
                10_000.0,
                10_000.0,
                "no_commission",
                "full_kelly",
                0.0,
            )
            .expect("免佣完整凯利应该可以计算"),
        )
        .expect("接口应返回合法 JSON");
        let half: Value = serde_json::from_str(
            &analyze_baccarat_strategy_json(
                "consumed",
                8,
                "",
                0.02,
                0.0,
                10_000.0,
                1.0,
                10_000.0,
                10_000.0,
                "no_commission",
                "half_kelly",
                0.0,
            )
            .expect("免佣半凯利应该可以计算"),
        )
        .expect("接口应返回合法 JSON");

        assert_eq!(half["payout_rule"], "no_commission");
        assert_eq!(half["stake_strategy"], "half_kelly");
        let full_target = full["recommendation"]["strategy_fraction"]
            .as_f64()
            .expect("完整凯利目标比例应存在");
        let half_target = half["recommendation"]["strategy_fraction"]
            .as_f64()
            .expect("半凯利目标比例应存在");
        assert!((half_target - full_target * 0.5).abs() < 1e-12);
    }

    #[test]
    fn fixed_stake_still_obeys_the_common_risk_limits() {
        let value: Value = serde_json::from_str(
            &analyze_baccarat_strategy_json(
                "consumed", 8, "", 0.02, 0.0, 10_000.0, 1.0, 80.0, 1_000.0, "standard", "fixed",
                100.0,
            )
            .expect("固定金额应该可以计算"),
        )
        .expect("接口应返回合法 JSON");

        assert_eq!(value["stake_strategy"], "fixed");
        assert_eq!(value["fixed_stake"], 100.0);
        assert_eq!(value["recommendation"]["action"], "place");
        assert_eq!(value["recommendation"]["suggested_amount"], 80.0);
    }

    #[test]
    fn side_bet_can_be_recommended_and_is_clipped_by_its_own_limit() {
        let value: Value = serde_json::from_str(
            &analyze_baccarat_strategy_json_with_side_bets(
                "remaining",
                8,
                "AS AC AD AH AS AC",
                0.0,
                0.0,
                1_000.0,
                1.0,
                500.0,
                1_000.0,
                "standard",
                "full_kelly",
                0.0,
                0.0,
                25.0,
            )
            .expect("全是 A 的六张测试牌靴应推荐对子边注"),
        )
        .expect("接口应返回合法 JSON");

        // 两边都必然成对时，庄对与闲对 EV 相同且高于任意对子；稳定顺序优先庄对。
        assert_eq!(value["recommendation"]["candidate_bet"], "banker_pair");
        assert_eq!(value["recommendation"]["bet_category"], "side");
        assert_eq!(value["recommendation"]["action"], "place");
        assert_eq!(value["recommendation"]["suggested_amount"], 25.0);
    }

    #[test]
    fn csv_replay_uses_the_selected_payout_and_stake_strategy() {
        let csv = "__source_pk,table_id,session_id,round_no,started_at,settled_at,raw_cards,result_code\n\
                   a,1,9001,1,2026-08-20 00:00:12,2026-08-20 00:00:44,\"b:24,31,45;p:31,42,47\",36\n";
        let value: Value = serde_json::from_str(
            &replay_baccarat_csv_json(
                csv,
                8,
                0.02,
                0.0,
                10_000.0,
                1.0,
                1_000.0,
                1_000.0,
                "no_commission",
                "fixed",
                100.0,
            )
            .expect("免佣固定金额回放应该成功"),
        )
        .expect("回放接口应返回合法 JSON");

        assert_eq!(
            value["config"]["payout_rule"],
            "no_commission_banker_six_half_payout"
        );
        assert_eq!(value["config"]["stake_strategy"], "fixed");
        assert_eq!(value["config"]["fixed_stake"], 100.0);
        assert_eq!(value["summary"]["placed_bet_count"], 1);
        assert_eq!(value["bets"][0]["amount"], 100.0);
    }
}
