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
    BetPlanAction, BetPlanSkipReason, BettingPolicy, Card, CsvReplayConfig, EffectiveBetMetrics,
    KellyPolicy, MainBet, MainBetAnalysis, MainBetRules, RebateRule, Shoe, SkipReason,
    calculate_main_outcomes, replay_csv_text,
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
) -> Result<String, JsValue> {
    analyze_baccarat_strategy_json(
        source_mode,
        decks,
        cards_text,
        rebate_rate,
        minimum_effective_ev,
        bankroll,
        max_fraction,
        max_round_stake,
        table_limit,
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
) -> Result<String, JsValue> {
    replay_baccarat_csv_json(
        csv_text,
        decks,
        rebate_rate,
        minimum_effective_ev,
        initial_bankroll,
        max_fraction,
        max_round_stake,
        table_limit,
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
) -> Result<String, String> {
    if !rebate_rate.is_finite() || !(0.0..=1.0).contains(&rebate_rate) {
        return Err("返水比例必须是 0% 到 100% 之间的有限数字".to_owned());
    }
    if !minimum_effective_ev.is_finite() {
        return Err("最低有效 EV 必须是有限数字".to_owned());
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

    let rules = MainBetRules::standard();
    let rebate = if rebate_rate == 0.0 {
        RebateRule::None
    } else {
        RebateRule::AllExceptMainBetTie { rate: rebate_rate }
    };
    let policy = BettingPolicy::new(rebate, minimum_effective_ev);
    let kelly_policy = KellyPolicy::new(max_fraction, max_round_stake, table_limit)
        .map_err(|error| format!("资金策略不合法：{error}"))?;
    let weights =
        calculate_main_outcomes(&shoe).map_err(|error| format!("概率与 EV 计算失败：{error}"))?;
    let analysis = MainBetAnalysis::from_weights(weights, rules);
    let plan = kelly_policy
        .plan(&policy, weights, rules, bankroll)
        .map_err(|error| format!("下注策略计算失败：{error}"))?;
    let decision = *plan.decision();
    let quote = plan.quote();
    let (action, reason) = match *plan.action() {
        BetPlanAction::Place { .. } => ("place", None),
        BetPlanAction::Skip { reason } => ("skip", Some(skip_reason_code(reason))),
    };

    let response = BrowserAnalysis {
        source_mode: normalized_mode,
        decks,
        input_card_count: cards.len(),
        remaining_card_count: shoe.total_remaining(),
        rebate_rate,
        bets: BrowserBets {
            player: BrowserBetMetrics::from_analysis(analysis, MainBet::Player, rebate),
            banker: BrowserBetMetrics::from_analysis(analysis, MainBet::Banker, rebate),
            tie: BrowserBetMetrics::from_analysis(analysis, MainBet::Tie, rebate),
        },
        recommendation: BrowserRecommendation {
            candidate_bet: decision.candidate().as_str(),
            base_ev: decision.base_ev(),
            rebate_ev: decision.rebate_ev(),
            effective_ev: decision.effective_ev(),
            action,
            reason,
            bankroll,
            kelly_fraction: quote.map(|value| value.kelly_fraction()),
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
) -> Result<String, String> {
    let config = CsvReplayConfig::new(
        decks,
        rebate_rate,
        minimum_effective_ev,
        initial_bankroll,
        max_fraction,
        max_round_stake,
        table_limit,
    )
    .map_err(|error| format!("回放配置不合法：{error}"))?;
    let report = replay_csv_text(csv_text, config).map_err(|error| error.to_string())?;

    serde_json::to_string(&report).map_err(|error| format!("回放结果序列化失败：{error}"))
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
    bets: BrowserBets,
    recommendation: BrowserRecommendation,
}

/// 三个主注方向的指标。
#[derive(Debug, Serialize)]
struct BrowserBets {
    player: BrowserBetMetrics,
    banker: BrowserBetMetrics,
    tie: BrowserBetMetrics,
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
    base_ev: f64,
    rebate_ev: f64,
    effective_ev: f64,
    action: &'static str,
    reason: Option<&'static str>,
    bankroll: f64,
    kelly_fraction: Option<f64>,
    applied_fraction: Option<f64>,
    suggested_amount: f64,
    expected_profit: f64,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{analyze_baccarat_json, analyze_baccarat_strategy_json};

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
                "consumed", 8, "", 0.02, 0.0, 10_000.0, 0.05, 1_000.0, 1_000.0,
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
                "consumed", 8, "", 0.02, 0.50, 10_000.0, 0.05, 1_000.0, 1_000.0,
            )
            .expect("高 EV 门槛应返回 Skip 而不是接口错误"),
        )
        .expect("接口应返回合法 JSON");
        assert_eq!(skipped["recommendation"]["action"], "skip");
        assert_eq!(skipped["recommendation"]["reason"], "below_minimum_ev");
    }
}
