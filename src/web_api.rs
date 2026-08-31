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
    BetPlanSkipReason, BettingPolicy, BlackjackAnalysis, BlackjackRules, Card, CombinedBetPlan,
    CombinedBetPlanAction, CsvReplayConfig, EffectiveBetMetrics, KellyPolicy, MainBet,
    MainBetAnalysis, MainBetRules, RebateRule, Shoe, SideBet, SideBetAnalysis, SideBetMetrics,
    SideBetRoundLimits, SideBetRules, SkipReason, StakeSizingStrategy, analyze_blackjack_hand,
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    allow_multiple_bets: bool,
) -> Result<String, JsValue> {
    analyze_baccarat_strategy_json_with_side_bets_and_multiple(
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
        strategy_parameter,
        minimum_side_bet_ev,
        side_bet_limit,
        allow_multiple_bets,
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    lucky_bet_max_round: u32,
    allow_multiple_bets: bool,
) -> Result<String, JsValue> {
    replay_baccarat_csv_json_with_side_bets_and_lucky_limit_and_multiple(
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
        strategy_parameter,
        minimum_side_bet_ev,
        side_bet_limit,
        lucky_bet_max_round,
        allow_multiple_bets,
    )
    .map_err(|message| JsValue::from_str(&message))
}

/// 使用十一种独立边注局数限制的新版 CSV 回放入口。
///
/// `side_bet_round_limits_json` 使用 [`SideBetRoundLimits`] 的稳定 JSON 字段；
/// 通过单个结构化参数传递，避免 WASM 函数继续增加十一个位置参数。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = replayBaccaratCsvWithSideBetLimits)]
pub fn replay_baccarat_csv_with_side_bet_limits(
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    side_bet_round_limits_json: &str,
    allow_multiple_bets: bool,
) -> Result<String, JsValue> {
    replay_baccarat_csv_json_with_side_bet_round_limits_and_multiple(
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
        strategy_parameter,
        minimum_side_bet_ev,
        side_bet_limit,
        side_bet_round_limits_json,
        allow_multiple_bets,
    )
    .map_err(|message| JsValue::from_str(&message))
}

/// 在浏览器中分析一手已经发出的二十一点起手牌。
///
/// 与百家乐下一局预测不同，这个入口位于“玩家已下注并看到起手牌”之后：
/// 它负责比较停牌、补牌、加倍、分牌和投降，不会拿动作 EV 倒推初始下注金额。
/// `current_base_stake` 只用于告诉页面加倍或分牌还需要追加多少钱。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = analyzeBlackjack)]
pub fn analyze_blackjack(
    source_mode: &str,
    decks: u8,
    shoe_cards_text: &str,
    player_cards_text: &str,
    dealer_upcard_text: &str,
    dealer_hits_soft_17: bool,
    blackjack_payout: f64,
    late_surrender: bool,
    current_base_stake: f64,
) -> Result<String, JsValue> {
    analyze_blackjack_json(
        source_mode,
        decks,
        shoe_cards_text,
        player_cards_text,
        dealer_upcard_text,
        dealer_hits_soft_17,
        blackjack_payout,
        late_surrender,
        current_base_stake,
    )
    .map_err(|message| JsValue::from_str(&message))
}

/// 普通 Rust 测试与 WebAssembly 共用的二十一点 JSON 适配函数。
///
/// `consumed` 模式中的 `shoe_cards_text` 只填写本手开始前已经离开牌靴的牌；
/// 本函数随后再扣除玩家两张牌与庄家明牌。`remaining` 模式则要求输入当前未知
/// 牌靴的完整集合，玩家牌和庄家明牌已经不在其中，因此不会重复扣除。
#[allow(clippy::too_many_arguments)]
pub fn analyze_blackjack_json(
    source_mode: &str,
    decks: u8,
    shoe_cards_text: &str,
    player_cards_text: &str,
    dealer_upcard_text: &str,
    dealer_hits_soft_17: bool,
    blackjack_payout: f64,
    late_surrender: bool,
    current_base_stake: f64,
) -> Result<String, String> {
    if !current_base_stake.is_finite() || current_base_stake <= 0.0 {
        return Err("当前底注必须是有限正数".to_owned());
    }

    let shoe_cards = parse_cards(shoe_cards_text)?;
    let player_cards = parse_cards(player_cards_text)?;
    if player_cards.len() != 2 {
        return Err(format!(
            "玩家起手牌必须正好是 2 张，当前输入了 {} 张",
            player_cards.len()
        ));
    }
    let dealer_cards = parse_cards(dealer_upcard_text)?;
    if dealer_cards.len() != 1 {
        return Err(format!(
            "庄家明牌必须正好是 1 张，当前输入了 {} 张",
            dealer_cards.len()
        ));
    }
    let dealer_upcard = dealer_cards[0];
    let normalized_mode = source_mode.trim().to_ascii_lowercase();
    let shoe = match normalized_mode.as_str() {
        "consumed" => {
            let mut shoe = Shoe::new(decks).map_err(|error| format!("副牌数不合法：{error}"))?;
            shoe.remove_many(&shoe_cards)
                .map_err(|error| format!("历史已消耗牌无法从牌靴扣除：{error}"))?;
            shoe.remove_many(&player_cards)
                .map_err(|error| format!("玩家起手牌无法从牌靴扣除：{error}"))?;
            shoe.remove(dealer_upcard)
                .map_err(|error| format!("庄家明牌无法从牌靴扣除：{error}"))?;
            shoe
        }
        "remaining" => Shoe::from_remaining(decks, &shoe_cards)
            .map_err(|error| format!("剩余牌无法构成合法牌靴：{error}"))?,
        _ => return Err("输入模式必须是 consumed 或 remaining".to_owned()),
    };

    let rules = BlackjackRules {
        dealer_hits_soft_17,
        blackjack_payout,
        late_surrender,
        ..BlackjackRules::standard()
    };
    let analysis = analyze_blackjack_hand(&shoe, &player_cards, dealer_upcard, rules)
        .map_err(|error| format!("二十一点 EV 计算失败：{error}"))?;
    let additional_stake_units = match analysis.optimal_action.as_str() {
        "double" | "split" => 1.0,
        _ => 0.0,
    };
    let response = BrowserBlackjackAnalysis {
        source_mode: normalized_mode,
        decks,
        input_shoe_card_count: shoe_cards.len(),
        remaining_card_count: shoe.total_remaining(),
        current_base_stake,
        additional_stake_units,
        suggested_additional_stake: additional_stake_units * current_base_stake,
        analysis,
    };
    serde_json::to_string(&response).map_err(|error| format!("无法生成 JSON：{error}"))
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
    strategy_parameter: f64,
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
        strategy_parameter,
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
) -> Result<String, String> {
    analyze_baccarat_strategy_json_with_side_bets_and_multiple(
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
        strategy_parameter,
        minimum_side_bet_ev,
        side_bet_limit,
        false,
    )
}

/// 主注和边注共同参与策略，并可选择一局下注多个合格目标。
///
/// `allow_multiple_bets = false` 保留旧行为，只返回有效 EV 最高的一项；开启后
/// 会返回所有达到各自 EV 门槛的目标，并让它们共享本局的资金上限。
#[allow(clippy::too_many_arguments)]
pub fn analyze_baccarat_strategy_json_with_side_bets_and_multiple(
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    allow_multiple_bets: bool,
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
    let stake_strategy = parse_stake_strategy(stake_strategy, strategy_parameter)?;
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
    let side_rules = SideBetRules::default();
    let side_analysis = SideBetAnalysis::calculate(side_weights, side_rules);
    let plans = if allow_multiple_bets {
        let plans = kelly_policy
            .plan_all_multiple_with_side_bet_filter(
                &policy,
                weights,
                rules,
                side_weights,
                side_rules,
                bankroll,
                |_| true,
            )
            .map_err(|error| format!("下注策略计算失败：{error}"))?;

        // 没有任何目标达到门槛时，多注接口返回空列表；保留一个旧版单注
        // 计划，这样页面仍能显示“最优候选”和明确的跳过原因。
        if plans.is_empty() {
            vec![
                kelly_policy
                    .plan_all(&policy, weights, rules, side_weights, side_rules, bankroll)
                    .map_err(|error| format!("下注策略计算失败：{error}"))?,
            ]
        } else {
            plans
        }
    } else {
        vec![
            kelly_policy
                .plan_all(&policy, weights, rules, side_weights, side_rules, bankroll)
                .map_err(|error| format!("下注策略计算失败：{error}"))?,
        ]
    };
    let primary_plan = plans.first().expect("浏览器分析至少应该返回一个下注计划");
    let recommendation = browser_recommendation(*primary_plan, bankroll);
    let recommendations: Vec<_> = plans
        .iter()
        .copied()
        .map(|plan| browser_recommendation(plan, bankroll))
        .collect();
    let total_suggested_amount = recommendations
        .iter()
        .filter(|recommendation| recommendation.action == "place")
        .map(|recommendation| recommendation.suggested_amount)
        .sum();

    let response = BrowserAnalysis {
        source_mode: normalized_mode,
        decks,
        input_card_count: cards.len(),
        remaining_card_count: shoe.total_remaining(),
        rebate_rate,
        payout_rule: payout_rule_code,
        stake_strategy: stake_strategy.as_str(),
        strategy_parameter: stake_strategy.parameter(),
        fixed_stake: stake_strategy.fixed_amount(),
        minimum_main_bet_ev: minimum_effective_ev,
        minimum_side_bet_ev,
        side_bet_limit,
        allow_multiple_bets,
        bets: BrowserBets {
            player: BrowserBetMetrics::from_analysis(analysis, MainBet::Player, rebate),
            banker: BrowserBetMetrics::from_analysis(analysis, MainBet::Banker, rebate),
            tie: BrowserBetMetrics::from_analysis(analysis, MainBet::Tie, rebate),
        },
        side_bet_rules: "pairs_5_11_perfect_25_big_0_5_small_1_5_lucky_6_12_18_lucky_7_6_15_super_30_40_100_dragon_1_2_3_5_10_30_natural_push",
        side_bets: BrowserSideBets {
            any_pair: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::AnyPair),
                "5:1",
                rebate,
            ),
            banker_pair: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::BankerPair),
                "11:1",
                rebate,
            ),
            player_pair: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::PlayerPair),
                "11:1",
                rebate,
            ),
            perfect_pair: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::PerfectPair),
                "25:1",
                rebate,
            ),
            big: BrowserSideBetMetrics::new(side_analysis.metrics(SideBet::Big), "0.5:1", rebate),
            small: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::Small),
                "1.5:1",
                rebate,
            ),
            lucky_seven: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::LuckySeven),
                "闲2张 6:1 / 闲3张 15:1",
                rebate,
            ),
            super_lucky_seven: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::SuperLuckySeven),
                "总4张 30:1 / 5张 40:1 / 6张 100:1",
                rebate,
            ),
            lucky_six: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::LuckySix),
                "庄2张 12:1 / 庄3张 18:1",
                rebate,
            ),
            banker_dragon_bonus: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::BankerDragonBonus),
                "点差4/5/6/7/8/9：1/2/3/5/10/30:1；Natural赢/双方Natural和为Push",
                rebate,
            ),
            player_dragon_bonus: BrowserSideBetMetrics::new(
                side_analysis.metrics(SideBet::PlayerDragonBonus),
                "点差4/5/6/7/8/9：1/2/3/5/10/30:1；Natural赢/双方Natural和为Push",
                rebate,
            ),
        },
        recommendation,
        recommendations,
        total_suggested_amount,
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
    strategy_parameter: f64,
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
        strategy_parameter,
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
) -> Result<String, String> {
    replay_baccarat_csv_json_with_side_bets_and_lucky_limit(
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
        strategy_parameter,
        minimum_side_bet_ev,
        side_bet_limit,
        0,
    )
}

/// 主注、边注和幸运 6/7 局数限制共同参与的完整 CSV 回放入口。
#[allow(clippy::too_many_arguments)]
pub fn replay_baccarat_csv_json_with_side_bets_and_lucky_limit(
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    lucky_bet_max_round: u32,
) -> Result<String, String> {
    replay_baccarat_csv_json_with_side_bets_and_lucky_limit_and_multiple(
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
        strategy_parameter,
        minimum_side_bet_ev,
        side_bet_limit,
        lucky_bet_max_round,
        false,
    )
}

/// 主注、边注、幸运 6/7 局数限制和同局多下注共同参与的完整 CSV 回放入口。
#[allow(clippy::too_many_arguments)]
pub fn replay_baccarat_csv_json_with_side_bets_and_lucky_limit_and_multiple(
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    lucky_bet_max_round: u32,
    allow_multiple_bets: bool,
) -> Result<String, String> {
    // 旧入口只有一个幸运玩法上限；其余玩法采用新的业务默认值。
    let limits = SideBetRoundLimits {
        lucky_six: lucky_bet_max_round,
        lucky_seven: lucky_bet_max_round,
        super_lucky_seven: lucky_bet_max_round,
        ..SideBetRoundLimits::default()
    };
    let limits_json = serde_json::to_string(&limits)
        .map_err(|error| format!("边注局数限制序列化失败：{error}"))?;

    replay_baccarat_csv_json_with_side_bet_round_limits_and_multiple(
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
        strategy_parameter,
        minimum_side_bet_ev,
        side_bet_limit,
        &limits_json,
        allow_multiple_bets,
    )
}

/// 使用十一种独立边注局数限制和同局多下注配置执行 CSV 回放。
#[allow(clippy::too_many_arguments)]
pub fn replay_baccarat_csv_json_with_side_bet_round_limits_and_multiple(
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
    strategy_parameter: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    side_bet_round_limits_json: &str,
    allow_multiple_bets: bool,
) -> Result<String, String> {
    // 兼容仍在浏览器中打开的旧页面：旧页面没有这两个后来新增的字段，
    // `undefined` 经过 wasm-bindgen 会变成 NaN。只有两项同时缺失时才按旧版
    // 语义回退；若调用者只传了一个非法值，仍交给领域层返回明确错误。
    let legacy_side_fields_missing =
        !minimum_side_bet_ev.is_finite() && !side_bet_limit.is_finite();
    let (minimum_side_bet_ev, side_bet_limit) = if legacy_side_fields_missing {
        (minimum_effective_ev, max_round_stake)
    } else {
        (minimum_side_bet_ev, side_bet_limit)
    };
    let side_bet_round_limits: SideBetRoundLimits =
        serde_json::from_str(side_bet_round_limits_json)
            .map_err(|error| format!("边注最晚下注局数配置无效：{error}"))?;
    let (rules, _) = parse_payout_rule(payout_rule)?;
    let stake_strategy = parse_stake_strategy(stake_strategy, strategy_parameter)?;
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
    .map_err(|error| format!("回放配置不合法：{error}"))?
    .with_side_bet_round_limits(side_bet_round_limits)
    .with_multiple_bets(allow_multiple_bets);
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
fn parse_stake_strategy(
    input: &str,
    strategy_parameter: f64,
) -> Result<StakeSizingStrategy, String> {
    match input.trim().to_ascii_lowercase().as_str() {
        "full_kelly" => Ok(StakeSizingStrategy::FullKelly),
        "half_kelly" => Ok(StakeSizingStrategy::HalfKelly),
        "quarter_kelly" => Ok(StakeSizingStrategy::QuarterKelly),
        "custom_kelly" => Ok(StakeSizingStrategy::CustomKelly {
            fraction: strategy_parameter,
        }),
        "fixed" => Ok(StakeSizingStrategy::Fixed {
            amount: strategy_parameter,
        }),
        "bankroll_fraction" => Ok(StakeSizingStrategy::FixedBankrollFraction {
            fraction: strategy_parameter,
        }),
        "target_expected_profit" => Ok(StakeSizingStrategy::TargetExpectedProfit {
            amount: strategy_parameter,
        }),
        "target_volatility" => Ok(StakeSizingStrategy::TargetVolatility {
            fraction: strategy_parameter,
        }),
        _ => Err(
            "金额策略必须是 full_kelly、half_kelly、quarter_kelly、custom_kelly、fixed、bankroll_fraction、target_expected_profit 或 target_volatility"
                .to_owned(),
        ),
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

/// 把一个核心下注计划转换成浏览器稳定的 JSON 行。
///
/// 多注模式和旧版单注模式共用这一个转换函数，避免前端收到两套字段含义不
/// 一致的结果。`plan` 已经完成 EV 门槛、凯利公式和所有金额上限检查。
fn browser_recommendation(plan: CombinedBetPlan, bankroll: f64) -> BrowserRecommendation {
    let decision = *plan.decision();
    let quote = plan.quote();
    let (action, reason) = match *plan.action() {
        CombinedBetPlanAction::Place { .. } => ("place", None),
        CombinedBetPlanAction::Skip { reason } => ("skip", Some(skip_reason_code(reason))),
    };

    BrowserRecommendation {
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

/// 二十一点页面需要的结果包装。
///
/// `analysis` 保留 Rust 核心的完整动作 EV；外层只增加输入概要和“本手还需
/// 追加多少筹码”。初始底注在看到牌之前已经发生，不能用事后动作 EV 重新决定。
#[derive(Debug, Serialize)]
struct BrowserBlackjackAnalysis {
    source_mode: String,
    decks: u8,
    input_shoe_card_count: usize,
    remaining_card_count: u16,
    current_base_stake: f64,
    additional_stake_units: f64,
    suggested_additional_stake: f64,
    #[serde(flatten)]
    analysis: BlackjackAnalysis,
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
    strategy_parameter: Option<f64>,
    fixed_stake: Option<f64>,
    minimum_main_bet_ev: f64,
    minimum_side_bet_ev: f64,
    side_bet_limit: f64,
    /// 是否允许一局把多个达到门槛的目标一起下注。
    allow_multiple_bets: bool,
    bets: BrowserBets,
    side_bet_rules: &'static str,
    side_bets: BrowserSideBets,
    /// 主推荐在第一项；多注模式下其余合格目标也会出现在这里。
    recommendation: BrowserRecommendation,
    recommendations: Vec<BrowserRecommendation>,
    total_suggested_amount: f64,
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
    perfect_pair: BrowserSideBetMetrics,
    big: BrowserSideBetMetrics,
    small: BrowserSideBetMetrics,
    lucky_seven: BrowserSideBetMetrics,
    super_lucky_seven: BrowserSideBetMetrics,
    lucky_six: BrowserSideBetMetrics,
    banker_dragon_bonus: BrowserSideBetMetrics,
    player_dragon_bonus: BrowserSideBetMetrics,
}

/// 边注的一行概率、基础 EV 与赔付说明。
#[derive(Debug, Serialize)]
struct BrowserSideBetMetrics {
    probability: f64,
    /// 不含返水的边注基础 EV；保留 `ev` 是为了兼容旧前端和已有调用者。
    ev: f64,
    base_ev: f64,
    rebate_ev: f64,
    effective_ev: f64,
    house_edge: f64,
    rtp: f64,
    effective_house_edge: f64,
    effective_rtp: f64,
    payout: &'static str,
}

impl BrowserSideBetMetrics {
    fn new(metrics: SideBetMetrics, payout: &'static str, rebate: RebateRule) -> Self {
        let base_ev = metrics.ev();
        // 所有边注结果都按实际下注额返水，因此期望返水就是返水率本身。
        let rebate_ev = rebate.rate_for_side_bet();
        let effective_ev = base_ev + rebate_ev;
        Self {
            probability: metrics.probability(),
            ev: base_ev,
            base_ev,
            rebate_ev,
            effective_ev,
            house_edge: metrics.house_edge(),
            rtp: metrics.rtp(),
            effective_house_edge: -effective_ev,
            effective_rtp: 1.0 + effective_ev,
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
        analyze_baccarat_strategy_json_with_side_bets,
        analyze_baccarat_strategy_json_with_side_bets_and_multiple, analyze_blackjack_json,
        replay_baccarat_csv_json, replay_baccarat_csv_json_with_side_bet_round_limits_and_multiple,
        replay_baccarat_csv_json_with_side_bets,
        replay_baccarat_csv_json_with_side_bets_and_lucky_limit_and_multiple,
    };

    #[test]
    fn blackjack_browser_api_removes_visible_cards_and_returns_action_evs() {
        let json = analyze_blackjack_json(
            "consumed",
            "8".parse().unwrap(),
            "",
            "5S 6H",
            "6C",
            false,
            1.5,
            true,
            100.0,
        )
        .expect("完整八副牌的 11 对 6 应能分析");
        let value: Value = serde_json::from_str(&json).expect("接口应返回合法 JSON");

        assert_eq!(value["source_mode"], "consumed");
        assert_eq!(value["remaining_card_count"], 413);
        assert_eq!(value["player_total"], 11);
        assert_eq!(value["optimal_action"], "double");
        assert_eq!(value["suggested_additional_stake"], 100.0);
        assert!(value["actions"]["double"].as_f64().is_some());
    }

    #[test]
    fn blackjack_browser_api_rejects_wrong_visible_card_counts() {
        let player_error =
            analyze_blackjack_json("consumed", 8, "", "5S", "6C", false, 1.5, true, 100.0)
                .expect_err("玩家只有一张牌必须报错");
        assert!(player_error.contains("正好是 2 张"));

        let dealer_error =
            analyze_blackjack_json("consumed", 8, "", "5S 6H", "", false, 1.5, true, 100.0)
                .expect_err("缺少庄家明牌必须报错");
        assert!(dealer_error.contains("正好是 1 张"));
    }

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
            "pairs_5_11_perfect_25_big_0_5_small_1_5_lucky_6_12_18_lucky_7_6_15_super_30_40_100_dragon_1_2_3_5_10_30_natural_push"
        );
        assert_eq!(value["side_bets"]["banker_pair"]["payout"], "11:1");
        assert_eq!(value["side_bets"]["perfect_pair"]["payout"], "25:1");
        assert_eq!(value["side_bets"]["big"]["payout"], "0.5:1");
        assert_eq!(value["side_bets"]["small"]["payout"], "1.5:1");
        assert_eq!(
            value["side_bets"]["lucky_six"]["payout"],
            "庄2张 12:1 / 庄3张 18:1"
        );
        assert!(
            value["side_bets"]["banker_dragon_bonus"]["probability"]
                .as_f64()
                .expect("庄龙宝概率应为数字")
                > 0.0
        );
        assert!(
            value["side_bets"]["player_dragon_bonus"]["probability"]
                .as_f64()
                .expect("闲龙宝概率应为数字")
                > 0.0
        );
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
    fn fixed_bankroll_fraction_is_exposed_and_sizes_from_current_bankroll() {
        let value: Value = serde_json::from_str(
            &analyze_baccarat_strategy_json(
                "consumed",
                8,
                "",
                0.02,
                0.0,
                10_000.0,
                1.0,
                1_000.0,
                1_000.0,
                "standard",
                "bankroll_fraction",
                0.02,
            )
            .expect("固定本金比例应该可以计算"),
        )
        .expect("接口应返回合法 JSON");

        assert_eq!(value["stake_strategy"], "bankroll_fraction");
        assert_eq!(value["strategy_parameter"], 0.02);
        assert_eq!(value["fixed_stake"], Value::Null);
        assert_eq!(value["recommendation"]["action"], "place");
        assert_eq!(value["recommendation"]["suggested_amount"], 200.0);
    }

    #[test]
    fn invalid_custom_kelly_fraction_is_rejected_by_the_web_api() {
        let error = analyze_baccarat_strategy_json(
            "consumed",
            8,
            "",
            0.02,
            0.0,
            10_000.0,
            1.0,
            1_000.0,
            1_000.0,
            "standard",
            "custom_kelly",
            1.01,
        )
        .expect_err("超过 100% 的自定义凯利必须被拒绝");

        assert!(error.contains("custom_kelly"));
        assert!(error.contains("0..=1"));
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
    fn multiple_strategy_returns_all_eligible_targets_and_shares_round_limit() {
        let value: Value = serde_json::from_str(
            &analyze_baccarat_strategy_json_with_side_bets_and_multiple(
                "consumed", 8, "", 0.0, -1.0, 1_000.0, 1.0, 500.0, 1_000.0, "standard", "fixed",
                100.0, -1.0, 25.0, true,
            )
            .expect("同局多下注应该返回多个目标"),
        )
        .expect("接口应返回合法 JSON");

        assert_eq!(value["allow_multiple_bets"], true);
        assert!(
            value["recommendations"]
                .as_array()
                .expect("多注结果应为数组")
                .len()
                >= 2
        );
        assert!(
            value["total_suggested_amount"]
                .as_f64()
                .expect("多注合计金额应为数字")
                <= 500.0 + 1e-12
        );
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

    #[test]
    fn csv_replay_accepts_legacy_pages_without_side_bet_fields() {
        let csv = "__source_pk,table_id,session_id,round_no,started_at,settled_at,raw_cards,result_code\n\
                   a,1,9001,1,2026-08-20 00:00:12,2026-08-20 00:00:44,\"b:24,31,45;p:31,42,47\",36\n";
        let value: Value = serde_json::from_str(
            &replay_baccarat_csv_json_with_side_bets(
                csv,
                8,
                0.02,
                0.0,
                10_000.0,
                1.0,
                1_000.0,
                1_000.0,
                "standard",
                "fixed",
                100.0,
                f64::NAN,
                f64::NAN,
            )
            .expect("旧页面缺少边注字段时应沿用主注门槛与单局限额"),
        )
        .expect("兼容回放应返回合法 JSON");

        assert_eq!(value["config"]["minimum_side_bet_ev"], 0.0);
        assert_eq!(value["config"]["side_bet_limit"], 1_000.0);
        assert_eq!(value["config"]["lucky_bet_max_round"], Value::Null);
        assert_eq!(value["config"]["side_bet_round_limits"]["big"], 20);
        assert_eq!(value["config"]["side_bet_round_limits"]["perfect_pair"], 45);
    }

    #[test]
    fn csv_replay_accepts_independent_side_bet_round_limits() {
        let csv = "__source_pk,table_id,session_id,round_no,started_at,settled_at,raw_cards,result_code\n\
                   a,1,9001,1,2026-08-20 00:00:12,2026-08-20 00:00:44,\"b:24,31,45;p:31,42,47\",36\n";
        let limits = r#"{
            "any_pair":12,"banker_pair":13,"player_pair":14,"perfect_pair":45,
            "big":20,"small":20,"lucky_seven":31,"super_lucky_seven":32,
            "lucky_six":33,"banker_dragon_bonus":40,"player_dragon_bonus":41
        }"#;
        let value: Value = serde_json::from_str(
            &replay_baccarat_csv_json_with_side_bet_round_limits_and_multiple(
                csv, 8, 0.02, 0.0, 10_000.0, 1.0, 1_000.0, 1_000.0, "standard", "fixed", 100.0,
                0.0, 100.0, limits, false,
            )
            .expect("十一种独立边注局数限制应该可以回放"),
        )
        .expect("回放结果应该是合法 JSON");

        assert_eq!(value["config"]["side_bet_round_limits"]["any_pair"], 12);
        assert_eq!(value["config"]["side_bet_round_limits"]["lucky_six"], 33);
        assert_eq!(
            value["config"]["side_bet_round_limits"]["player_dragon_bonus"],
            41
        );
        // 三种幸运玩法上限不相同时，旧兼容字段无法用一个值表达，应为 null。
        assert_eq!(value["config"]["lucky_bet_max_round"], Value::Null);
    }

    #[test]
    fn csv_replay_settles_multiple_bets_from_the_same_round_together() {
        let csv = "__source_pk,table_id,session_id,round_no,started_at,settled_at,raw_cards,result_code\n\
                   a,1,9001,1,2026-08-20 00:00:12,2026-08-20 00:00:44,\"b:24,31,45;p:31,42,47\",36\n";
        let value: Value = serde_json::from_str(
            &replay_baccarat_csv_json_with_side_bets_and_lucky_limit_and_multiple(
                csv, 8, 0.0, -1.0, 10_000.0, 1.0, 1_000.0, 1_000.0, "standard", "fixed", 100.0,
                -1.0, 100.0, 0, true,
            )
            .expect("多注回放应该成功"),
        )
        .expect("回放接口应返回合法 JSON");

        assert_eq!(value["config"]["allow_multiple_bets"], true);
        assert!(
            value["summary"]["placed_bet_count"]
                .as_u64()
                .expect("下注笔数应为整数")
                > 1
        );
        assert!(
            value["summary"]["maximum_round_stake"]
                .as_f64()
                .expect("最大单局下注应为数字")
                > value["summary"]["maximum_single_stake"]
                    .as_f64()
                    .expect("最大单笔下注应为数字")
        );
        assert!(
            value["summary"]["maximum_profit"]
                .as_f64()
                .expect("模拟最大盈利应为数字")
                >= 0.0
        );
        assert!(value["bets"].as_array().expect("明细应为数组").len() > 1);
    }
}
