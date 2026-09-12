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
//!
//! 一次回放可以拆成四个阶段：
//!
//! 1. `load_rounds` 把 CSV 行读成结构化记录，并先验证牌面、发牌顺序和结果码；
//! 2. `replay_rounds` 按桌台/牌靴分组，只放行能从第 1 局连续重建的场次；
//! 3. 每一局先用发牌前的 `Shoe` 做概率、策略和金额计算，再用数据库真实结果结算；
//! 4. 结算完成后才从牌靴扣除本局牌，并更新滚动本金、回撤和各类统计。
//!
//! 这种顺序是回测可信度的核心：如果先扣牌或先读取真实结果再决定下注，就会把
//! 本局之后才知道的信息泄漏到下注前，得到看起来很好但无法实际执行的结果。

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    fmt,
};

use serde::{Deserialize, Serialize};

use crate::{Card, Rank, Shoe, Suit};

use super::{
    BaccaratHand, BetTarget, BettingPolicy, CombinedBetPlanAction, KellyPolicy, MainBet,
    MainBetRules, OutcomeWeights, ProgressionOutcome, RebateRule, RoundOutcome, SideBet,
    SideBetRules, SideBetWeights, StakeProgression, StakeSizingStrategy,
    calculate_main_and_side_outcomes, resolve_round,
};

/// 每种边注在一靴牌中的最后可下注局数。
///
/// 字段值 `N` 表示第 1..=N 局可以下注，从第 N+1 局开始禁用；`0` 表示
/// 不限制。每种玩法独立配置，避免“大/小 20 局限制”意外影响对子或龙宝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct SideBetRoundLimits {
    /// 任意对子最后可下注的局号；0 表示不限制。
    pub any_pair: u32,
    /// 庄对最后可下注的局号；0 表示不限制。
    pub banker_pair: u32,
    /// 闲对最后可下注的局号；0 表示不限制。
    pub player_pair: u32,
    /// 完美对子最后可下注的局号；0 表示不限制。
    pub perfect_pair: u32,
    /// 大最后可下注的局号；0 表示不限制。
    pub big: u32,
    /// 小最后可下注的局号；0 表示不限制。
    pub small: u32,
    /// 幸运 7 最后可下注的局号；0 表示不限制。
    pub lucky_seven: u32,
    /// 超级幸运 7 最后可下注的局号；0 表示不限制。
    pub super_lucky_seven: u32,
    /// 幸运 6 最后可下注的局号；0 表示不限制。
    pub lucky_six: u32,
    /// 庄龙宝最后可下注的局号；0 表示不限制。
    pub banker_dragon_bonus: u32,
    /// 闲龙宝最后可下注的局号；0 表示不限制。
    pub player_dragon_bonus: u32,
}

impl Default for SideBetRoundLimits {
    fn default() -> Self {
        Self {
            // 第 51 局起停止大多数普通边注，所以默认最后可下注局数为 50。
            any_pair: 50,
            banker_pair: 50,
            player_pair: 50,
            // 完美对子从第 46 局起停止。
            perfect_pair: 45,
            // 大/小从第 21 局起停止。
            big: 20,
            small: 20,
            lucky_seven: 50,
            // 超级幸运 7 规则更严格：第 31 局起停止下注。
            super_lucky_seven: 30,
            lucky_six: 50,
            banker_dragon_bonus: 50,
            player_dragon_bonus: 50,
        }
    }
}

impl SideBetRoundLimits {
    /// 判断指定玩法在当前局号是否仍可以进入 EV 比较。
    pub const fn allows(self, side_bet: SideBet, round_no: u32) -> bool {
        let max_round = match side_bet {
            SideBet::AnyPair => self.any_pair,
            SideBet::BankerPair => self.banker_pair,
            SideBet::PlayerPair => self.player_pair,
            SideBet::PerfectPair => self.perfect_pair,
            SideBet::Big => self.big,
            SideBet::Small => self.small,
            SideBet::LuckySeven => self.lucky_seven,
            SideBet::SuperLuckySeven => self.super_lucky_seven,
            SideBet::LuckySix => self.lucky_six,
            SideBet::BankerDragonBonus => self.banker_dragon_bonus,
            SideBet::PlayerDragonBonus => self.player_dragon_bonus,
        };

        // `0` 是业务约定的“不限局数”，不能按普通的 `round_no <= 0`
        // 处理；有上限时则包含最后一局，例如上限 20 允许第 20 局。
        max_round == 0 || round_no <= max_round
    }

    /// 兼容旧报告字段：三种幸运玩法上限相同时返回该值，否则返回 `None`。
    const fn common_lucky_max_round(self) -> Option<u32> {
        if self.lucky_six > 0
            && self.lucky_six == self.lucky_seven
            && self.lucky_six == self.super_lucky_seven
        {
            Some(self.lucky_six)
        } else {
            None
        }
    }
}

/// 一次 CSV 回放使用的完整策略配置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CsvReplayConfig {
    /// 每个可回放牌靴包含的副牌数。
    decks: u8,
    /// 主注赔付规则，例如标准庄佣金或庄免佣。
    rules: MainBetRules,
    /// 通过 EV 门槛后的金额策略。
    stake_strategy: StakeSizingStrategy,
    /// 返水率的小数形式，例如 0.009 表示 0.9%。
    rebate_rate: f64,
    /// 主注允许进入金额计算的最低有效 EV。
    minimum_effective_ev: f64,
    /// 边注允许进入金额计算的最低有效 EV。
    minimum_side_bet_ev: f64,
    /// 回放开始时的本金；随后每局都使用上一局结算后的滚动余额。
    initial_bankroll: f64,
    /// 单局下注最多占当前本金的比例。
    max_fraction: f64,
    /// 本系统施加的单局总下注金额上限。
    max_round_stake: f64,
    /// 桌台允许的单局总下注金额上限。
    table_limit: f64,
    /// 单笔边注自己的金额上限。
    side_bet_limit: f64,
    /// 十一种边注各自允许下注的最后局数；0 表示该玩法不限制。
    side_bet_round_limits: SideBetRoundLimits,
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
            side_bet_round_limits: SideBetRoundLimits::default(),
            allow_multiple_bets: false,
        })
    }

    /// 设置幸运 6/7 可以参与策略的最后一局。
    ///
    /// `0` 表示不限制；`N > 0` 表示第 1..=N 局允许，从第 N+1 局起禁用。
    /// 该限制只移除三种幸运边注，其他主注和边注仍照常比较 EV。
    pub fn with_lucky_bet_max_round(mut self, max_round: u32) -> Self {
        self.side_bet_round_limits.lucky_six = max_round;
        self.side_bet_round_limits.lucky_seven = max_round;
        self.side_bet_round_limits.super_lucky_seven = max_round;
        self
    }

    /// 覆盖十一种边注各自的最后可下注局数。
    pub fn with_side_bet_round_limits(mut self, limits: SideBetRoundLimits) -> Self {
        self.side_bet_round_limits = limits;
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
    /// 实际执行本次回放的配置快照。
    pub config: CsvReplayConfigSnapshot,
    /// 输入 CSV 的行数、时间范围和重复键统计。
    pub dataset: CsvDatasetReport,
    /// 牌面/结果校验及可回放场次数量。
    pub quality: CsvQualityReport,
    /// 可回放局的下注、盈亏、本金和风险指标。
    pub summary: CsvReplaySummary,
    /// 只保存真正下注的局，直接回答“什么时候可以下注”。
    pub bets: Vec<CsvBetDetail>,
    /// 为兼容旧版 JSON 保留的字段。当前版本保留全部下注明细，因此恒为 0。
    pub omitted_bet_details: u64,
}

/// 报告携带的配置快照，避免结果脱离参数后被误读。
#[derive(Debug, Serialize)]
pub struct CsvReplayConfigSnapshot {
    /// 回放所使用的副牌数。
    pub decks: u8,
    /// 可读的主注赔付规则代码。
    pub payout_rule: &'static str,
    /// 可读的金额策略代码。
    pub stake_strategy: &'static str,
    /// 该策略的参数；不需要参数的策略为 `None`。
    pub strategy_parameter: Option<f64>,
    /// 固定金额策略的金额；其他策略为 `None`。
    pub fixed_stake: Option<f64>,
    /// 可读的返水规则代码。
    pub rebate_rule: &'static str,
    /// 返水率的小数值，例如 0.009 表示 0.9%。
    pub rebate_rate: f64,
    /// 主注最低有效 EV。
    pub minimum_effective_ev: f64,
    /// 边注最低有效 EV。
    pub minimum_side_bet_ev: f64,
    /// 回放起点本金。
    pub initial_bankroll: f64,
    /// 本金更新方式；当前为按时间顺序共享滚动本金。
    pub bankroll_mode: &'static str,
    /// 单局本金比例上限。
    pub max_fraction: f64,
    /// 系统单局金额上限。
    pub max_round_stake: f64,
    /// 桌台单局金额上限。
    pub table_limit: f64,
    /// 边注单笔金额上限。
    pub side_bet_limit: f64,
    /// 十一种边注各自的最后可下注局数；0 表示不限制。
    pub side_bet_round_limits: SideBetRoundLimits,
    /// null 表示不限制；正整数 N 表示仅前 N 局允许幸运 6/7。
    /// 这是旧页面兼容字段；新页面应读取 `side_bet_round_limits`。
    pub lucky_bet_max_round: Option<u32>,
    /// 是否允许一局同时保存多笔下注明细。
    pub allow_multiple_bets: bool,
}

/// CSV 文件与时间范围的基础画像。
#[derive(Debug, Default, Serialize)]
pub struct CsvDatasetReport {
    /// CSV 中成功读取的总数据行数。
    pub total_rows: u64,
    /// 出现过的桌台数量。
    pub table_count: usize,
    /// `(table_id, session_id)` 牌靴场次数量。
    pub session_count: usize,
    /// 从可选的 `started_at` 提取到的业务日期数量。
    pub business_date_count: usize,
    /// 最早业务日期；没有数据时为空字符串。
    pub business_date_min: String,
    /// 最晚业务日期；没有数据时为空字符串。
    pub business_date_max: String,
    /// 最早开局时间；没有数据时为空字符串。
    pub started_at_min: String,
    /// 最晚开局时间；没有数据时为空字符串。
    pub started_at_max: String,
    /// 最早开奖时间；没有数据时为空字符串。
    pub settled_at_min: String,
    /// 最晚开奖时间；没有数据时为空字符串。
    pub settled_at_max: String,
    /// 重复 `__source_pk` 的行数；精简格式没有该列时不检查。
    pub duplicate_source_pk_rows: u64,
    /// 重复 `(table_id, session_id, round_no)` 的行数；大于 0 时拒绝执行。
    pub duplicate_round_keys: u64,
}

/// 判断哪些牌靴可以安全回放的数据质量指标。
#[derive(Debug, Default, Serialize)]
pub struct CsvQualityReport {
    /// 牌面合法、发牌顺序正确，且可选结果码一致的行数。
    pub valid_card_rows: u64,
    /// 开奖内容为空、无法判断牌局的行数。
    pub empty_card_rows: u64,
    /// 牌面格式、牌数或来源牌码非法的行数。
    pub invalid_card_rows: u64,
    /// 数据库结果与 Rust 根据牌面推导结果不一致的行数。
    pub outcome_mismatch_rows: u64,
    /// 从第 1 局开始的牌靴数量。
    pub sessions_starting_at_one: u64,
    /// 从中途局号开始、因此无法知道之前已消耗哪些牌的牌靴数量。
    pub sessions_starting_mid_shoe: u64,
    /// 局号不连续的牌靴数量。
    pub sessions_with_round_gaps: u64,
    /// 至少包含一行空牌面的牌靴数量。
    pub sessions_with_empty_cards: u64,
    /// 至少包含一行校验失败记录的牌靴数量。
    pub sessions_with_invalid_rows: u64,
    /// 通过全部完整性条件、实际参与策略回放的牌靴数量。
    pub fully_observable_sessions: u64,
    /// 被隔离、没有参与策略回放的行数。
    pub quarantined_rounds: u64,
}

/// 按方向保存计数，字段名直接使用稳定业务名称。
#[derive(Debug, Default, Serialize)]
pub struct CsvBetCounts {
    /// 闲主注实际执行的笔数。
    pub player: u64,
    /// 庄主注实际执行的笔数。
    pub banker: u64,
    /// 和主注实际执行的笔数。
    pub tie: u64,
    /// 任意对子实际执行的笔数。
    pub any_pair: u64,
    /// 庄对实际执行的笔数。
    pub banker_pair: u64,
    /// 闲对实际执行的笔数。
    pub player_pair: u64,
    /// 完美对子实际执行的笔数。
    pub perfect_pair: u64,
    /// 大实际执行的笔数。
    pub big: u64,
    /// 小实际执行的笔数。
    pub small: u64,
    /// 幸运 7 实际执行的笔数。
    pub lucky_seven: u64,
    /// 超级幸运 7 实际执行的笔数。
    pub super_lucky_seven: u64,
    /// 幸运 6 实际执行的笔数。
    pub lucky_six: u64,
    /// 庄龙宝实际执行的笔数。
    pub banker_dragon_bonus: u64,
    /// 闲龙宝实际执行的笔数。
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

/// 单个下注方向在整次回放中的资金表现。
///
/// 这些字段必须在真实结算发生时一起累计，不能由浏览器根据当前页明细推算。
/// 回放明细可以分页，但这里始终覆盖整份 CSV，因此适合做总额和盈亏对账。
#[derive(Debug, Default, Serialize)]
pub struct CsvBetPerformance {
    /// 该方向真正执行的下注笔数。
    pub count: u64,
    /// 该方向基础赔率结算为赢的笔数；不把返水当成游戏命中。
    pub win_count: u64,
    /// 该方向基础赔率结算为输的笔数，用于计算分类亏损率。
    pub loss_count: u64,
    /// 该方向基础赔率结算为 Push 的笔数。
    pub push_count: u64,
    /// 该方向所有实际下注金额之和。
    pub total_stake: f64,
    /// 该方向累计净盈亏，已经包含实际获得的返水。
    pub total_profit: f64,
    /// 该方向所有正收益下注的金额之和；亏损下注不会先与它抵消。
    pub gross_profit: f64,
    /// 该方向所有负收益下注的绝对金额之和，始终使用非负数表示。
    pub gross_loss: f64,
    /// 不含返水的基础游戏正收益之和，供“毛盈利 + 返水 - 毛亏损”瀑布图使用。
    pub base_gross_profit: f64,
    /// 不含返水的基础游戏负收益绝对值之和。
    pub base_gross_loss: f64,
}

/// 按下注方向拆分的笔数、下注额和净盈亏。
#[derive(Debug, Default, Serialize)]
pub struct CsvBetBreakdown {
    pub player: CsvBetPerformance,
    pub banker: CsvBetPerformance,
    pub tie: CsvBetPerformance,
    pub any_pair: CsvBetPerformance,
    pub banker_pair: CsvBetPerformance,
    pub player_pair: CsvBetPerformance,
    pub perfect_pair: CsvBetPerformance,
    pub big: CsvBetPerformance,
    pub small: CsvBetPerformance,
    pub lucky_seven: CsvBetPerformance,
    pub super_lucky_seven: CsvBetPerformance,
    pub lucky_six: CsvBetPerformance,
    pub banker_dragon_bonus: CsvBetPerformance,
    pub player_dragon_bonus: CsvBetPerformance,
}

impl CsvBetBreakdown {
    /// 在唯一的真实结算点记录一笔下注，确保数量、金额和盈亏使用同一口径。
    fn record(&mut self, bet: BetTarget, amount: f64, base_game_profit: f64, rebate_income: f64) {
        let performance = match bet {
            BetTarget::Main(MainBet::Player) => &mut self.player,
            BetTarget::Main(MainBet::Banker) => &mut self.banker,
            BetTarget::Main(MainBet::Tie) => &mut self.tie,
            BetTarget::Side(SideBet::AnyPair) => &mut self.any_pair,
            BetTarget::Side(SideBet::BankerPair) => &mut self.banker_pair,
            BetTarget::Side(SideBet::PlayerPair) => &mut self.player_pair,
            BetTarget::Side(SideBet::PerfectPair) => &mut self.perfect_pair,
            BetTarget::Side(SideBet::Big) => &mut self.big,
            BetTarget::Side(SideBet::Small) => &mut self.small,
            BetTarget::Side(SideBet::LuckySeven) => &mut self.lucky_seven,
            BetTarget::Side(SideBet::SuperLuckySeven) => &mut self.super_lucky_seven,
            BetTarget::Side(SideBet::LuckySix) => &mut self.lucky_six,
            BetTarget::Side(SideBet::BankerDragonBonus) => &mut self.banker_dragon_bonus,
            BetTarget::Side(SideBet::PlayerDragonBonus) => &mut self.player_dragon_bonus,
        };

        let actual_profit = base_game_profit + rebate_income;
        performance.count += 1;
        performance.total_stake += amount;
        performance.total_profit += actual_profit;

        if base_game_profit > 0.0 {
            performance.win_count += 1;
            performance.base_gross_profit += base_game_profit;
        } else if base_game_profit < 0.0 {
            performance.loss_count += 1;
            performance.base_gross_loss += -base_game_profit;
        } else {
            performance.push_count += 1;
        }

        // 实际毛盈利/毛亏损包含返水，回答“最终资金变化由哪些玩法贡献”；
        // 基础毛盈利/毛亏损则把返水拆开，专门用于严格对账的瀑布图。
        if actual_profit > 0.0 {
            performance.gross_profit += actual_profit;
        } else if actual_profit < 0.0 {
            performance.gross_loss += -actual_profit;
        }
    }

    /// 汇总所有方向，用于测试和调试时验证分类数据能与总报告完全对账。
    fn totals(&self) -> (u64, f64, f64) {
        [
            &self.player,
            &self.banker,
            &self.tie,
            &self.any_pair,
            &self.banker_pair,
            &self.player_pair,
            &self.perfect_pair,
            &self.big,
            &self.small,
            &self.lucky_seven,
            &self.super_lucky_seven,
            &self.lucky_six,
            &self.banker_dragon_bonus,
            &self.player_dragon_bonus,
        ]
        .into_iter()
        .fold((0, 0.0, 0.0), |(count, stake, profit), item| {
            (
                count + item.count,
                stake + item.total_stake,
                profit + item.total_profit,
            )
        })
    }

    /// 分别汇总正收益和负收益绝对值。二者不能先做净额抵消，因为贡献图需要回答
    /// “哪些玩法带来盈利”和“哪些玩法造成亏损”两个不同问题。
    fn gross_totals(&self) -> (f64, f64) {
        [
            &self.player,
            &self.banker,
            &self.tie,
            &self.any_pair,
            &self.banker_pair,
            &self.player_pair,
            &self.perfect_pair,
            &self.big,
            &self.small,
            &self.lucky_seven,
            &self.super_lucky_seven,
            &self.lucky_six,
            &self.banker_dragon_bonus,
            &self.player_dragon_bonus,
        ]
        .into_iter()
        .fold((0.0, 0.0), |(profit, loss), item| {
            (profit + item.gross_profit, loss + item.gross_loss)
        })
    }

    /// 汇总不含返水的游戏毛盈利和毛亏损，二者之差必须等于基础游戏净输赢。
    fn base_gross_totals(&self) -> (f64, f64) {
        [
            &self.player,
            &self.banker,
            &self.tie,
            &self.any_pair,
            &self.banker_pair,
            &self.player_pair,
            &self.perfect_pair,
            &self.big,
            &self.small,
            &self.lucky_seven,
            &self.super_lucky_seven,
            &self.lucky_six,
            &self.banker_dragon_bonus,
            &self.player_dragon_bonus,
        ]
        .into_iter()
        .fold((0.0, 0.0), |(profit, loss), item| {
            (profit + item.base_gross_profit, loss + item.base_gross_loss)
        })
    }
}

/// 全部可回放局的策略和盈亏汇总。
#[derive(Debug, Default, Serialize)]
pub struct CsvReplaySummary {
    /// 实际参与回放的完整牌靴数量。
    pub replayed_sessions: u64,
    /// 实际完成“决策、结算、扣牌”的局数。
    pub replayed_rounds: u64,
    /// 是否因为滚动本金耗尽而提前结束，而不是自然处理完全部可回放局。
    pub stopped_early: bool,
    /// 提前停止的稳定原因代码；当前为 `bankroll_depleted`。
    pub stop_reason: Option<&'static str>,
    /// 触发提前停止时尚未处理的桌台编号。
    pub stop_table_id: Option<u64>,
    /// 触发提前停止时尚未处理的牌靴编号。
    pub stop_session_id: Option<u64>,
    /// 触发提前停止时尚未处理的局号。
    pub stop_round_no: Option<u32>,
    /// 概率缓存命中的次数；相同 52 类牌靴状态会复用结果。
    pub probability_cache_hits: u64,
    /// 概率缓存未命中的次数；每次未命中会重新枚举当前牌靴。
    pub probability_cache_misses: u64,
    /// 每局被策略选作首要候选的目标计数。
    pub candidate_bets: CsvBetCounts,
    /// 通过策略和资金检查、真正执行的下注分类计数。
    pub placed_bets: CsvBetCounts,
    /// 每种真实下注的笔数、累计下注额和包含返水后的累计净盈亏。
    pub bet_breakdown: CsvBetBreakdown,
    /// 所有真实下注明细的总笔数；同局多注时一局可贡献多笔。
    pub placed_bet_count: u64,
    /// 计划存在但最终没有执行的下注计划数量。
    pub skipped_bets: u64,
    /// 按每一笔下注的基础游戏净收益统计的赢局笔数。
    pub wins: u64,
    /// 按每一笔下注的基础游戏净收益统计的输局笔数。
    pub losses: u64,
    /// 按每一笔下注的基础游戏净收益统计的 Push 笔数。
    pub pushes: u64,
    /// `wins / placed_bet_count`；没有下注时为 `None`。
    pub hit_rate: Option<f64>,
    /// 每个可回放局首要候选有效 EV 的平均值。
    pub average_candidate_effective_ev: Option<f64>,
    /// 可回放局首要候选有效 EV 的最小值。
    pub minimum_candidate_effective_ev: Option<f64>,
    /// 可回放局首要候选有效 EV 的最大值。
    pub maximum_candidate_effective_ev: Option<f64>,
    /// 所有真实下注金额之和。
    pub total_stake: f64,
    /// 每笔下注报价中的期望盈利金额之和，不是真实已实现盈利。
    pub total_expected_profit: f64,
    /// 只包含牌局赔付/亏损的累计金额，不包含返水。
    pub base_game_profit: f64,
    /// 按真实下注金额累计得到的返水收入。
    pub rebate_income: f64,
    /// 最终累计真实盈利，等于基础输赢加返水。
    pub total_profit: f64,
    /// 回放开始时本金。
    pub initial_bankroll: f64,
    /// 回放结束时本金，即初始本金加累计真实盈利。
    pub final_bankroll: f64,
    /// 回放期间每局结算后出现过的最高本金，初始本金也作为第一个基准点参与比较。
    pub maximum_bankroll: f64,
    /// 回放期间的最大累计盈利：最高本金减去初始本金，而不是某一笔下注的盈利。
    pub maximum_profit: f64,
    /// 回放期间每局结算后出现过的最低本金；它不把下注尚未开奖时的临时占用算作余额。
    pub minimum_bankroll: f64,
    /// `total_profit / initial_bankroll`，表示相对初始本金的累计回报率。
    pub return_on_initial: f64,
    /// 从历史峰值本金到随后本金低点的最大绝对下降金额。
    pub maximum_drawdown: f64,
    /// 最大回撤除以当时历史峰值本金的比例。
    pub maximum_drawdown_rate: f64,
    /// 所有真实下注中，单笔下注金额的最大值。
    pub maximum_single_stake: f64,
    /// 同一局所有下注金额之和的最大值，用来观察同局多注的最大风险敞口。
    pub maximum_round_stake: f64,
}

/// 一笔真实下注明细。
#[derive(Debug, Serialize)]
pub struct CsvBetDetail {
    /// 本局开局时间；用于跨桌台建立确定的回放顺序。
    pub started_at: String,
    /// 来源桌台编号。
    pub table_id: u64,
    /// 来源牌靴/场次编号。
    pub session_id: u64,
    /// 该牌靴内的局号。
    pub round_no: u32,
    /// 实际下注目标的稳定字符串，例如 `banker` 或 `lucky_six`。
    pub bet: &'static str,
    /// 真实开奖结果：`player`、`banker` 或 `tie`。
    pub outcome: &'static str,
    /// 闲家最终手牌，例如 `JD 2H 7H`。
    pub player_cards: String,
    /// 庄家最终手牌，例如 `4D JD 5H`。
    pub banker_cards: String,
    /// 闲家最终点数，供前端与具体牌面一起显示。
    pub player_total: u8,
    /// 庄家最终点数，供前端与具体牌面一起显示。
    pub banker_total: u8,
    /// 该下注的基础结算结果：`win`、`loss` 或 `push`。
    pub result: &'static str,
    /// 下注前按概率计算的有效 EV，已经包含返水影响。
    pub effective_ev: f64,
    /// 完整凯利公式根据收益分布算出的原始比例。
    pub kelly_fraction: f64,
    /// 所选金额策略转换出的目标比例，尚未经过风险上限。
    pub strategy_fraction: f64,
    /// 经过本金比例、单局、桌台和边注上限后的实际比例。
    pub applied_fraction: f64,
    /// 这笔下注实际使用的金额。
    pub amount: f64,
    /// 下注前 EV 乘以实际金额得到的理论期望盈利。
    pub expected_profit: f64,
    /// 只按主注/边注赔率结算的真实输赢金额。
    pub base_game_profit: f64,
    /// 这笔下注按规则实际获得的返水金额。
    pub rebate_income: f64,
    /// `base_game_profit + rebate_income`，这笔下注最终改变的本金。
    pub actual_profit: f64,
    /// 同一局全部下注一起结算后的本金余额。
    pub bankroll_after: f64,
}

/// CSV 中的一局。额外列会被 serde 自动忽略。
///
/// 真正必需的只有牌靴、子局数和牌面。数据库完整格式中的来源键、桌台、时间和
/// 结果码都保留为可选增强信息：有值时继续去重、排序和交叉校验，没有时则使用
/// 默认桌台、CSV 原始行顺序以及 Rust 从牌面推导出的结果。
#[derive(Debug, Deserialize, Serialize)]
struct CsvRound {
    #[serde(default, rename = "__source_pk")]
    source_pk: Option<String>,
    #[serde(default = "default_table_id")]
    table_id: u64,
    #[serde(alias = "牌靴")]
    session_id: u64,
    #[serde(alias = "子局数")]
    round_no: u32,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    settled_at: Option<String>,
    #[serde(alias = "牌面")]
    raw_cards: String,
    #[serde(default)]
    result_code: Option<u64>,
    /// 分批传输时保留原始 CSV 行号；普通 CSV 没有该列时由读取器补齐。
    #[serde(default)]
    source_order: Option<usize>,
}

const fn default_table_id() -> u64 {
    1
}

/// 单局完成格式、发牌规则和结果一致性校验后的状态。
///
/// `Option` 不是为了隐藏错误，而是区分三种状态：没有开奖牌面、牌面存在但
/// 校验失败、以及可以安全用于牌靴重建的完整记录。`validation_error` 保存前
/// 两种状态的原因，后续按整个牌靴进行隔离。
#[derive(Debug)]
struct LoadedRound {
    table_id: u64,
    session_id: u64,
    round_no: u32,
    started_at: String,
    /// CSV 中的原始行顺序；精简格式没有时间列时用它建立全局资金时间线。
    source_order: usize,
    /// 区分真实时间和仅供明细展示的“CSV 第 N 行”占位文字。
    has_started_at: bool,
    cards: Option<Vec<Card>>,
    outcome: Option<RoundOutcome>,
    banker_total: Option<u8>,
    validation_error: Option<String>,
}

/// 概率预计算结果的稳定键。
///
/// 子 Worker 只负责“当前牌靴状态 -> 概率权重”，而本金与倍投必须留在
/// 协调 Worker 的最终顺序回放中。用桌台、牌靴和局号做键，可以把并行阶段
/// 得到的结果准确交还给原始 CSV 的时间线，而不依赖子 Worker 完成顺序。
type ReplayRoundKey = (u64, u64, u32);

/// 一个回放局在下注前的概率快照。
///
/// 这份 DTO 会在 WASM 与多个 Web Worker 之间传输，因此只包含可序列化的
/// 数字和稳定业务键，不携带 Shoe、策略状态或本金。后面的顺序回放仍会
/// 根据原始 CSV 读取真实牌面并统一结算。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PreparedReplayWeight {
    pub table_id: u64,
    pub session_id: u64,
    pub round_no: u32,
    pub weights: OutcomeWeights,
    pub side_weights: SideBetWeights,
}

/// 一批子 Worker 完成的概率预计算结果。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PreparedReplayWeights {
    pub rounds: Vec<PreparedReplayWeight>,
}

/// 在内存中读取并回放一个或多个业务日期的 CSV。
///
/// 所有可观测牌靴共享一份滚动本金。有完整 `started_at` 时按时间排序；精简
/// CSV 没有时间时按文件行顺序回放。因此下一局凯利金额使用的是上一笔真实
/// 结算后的余额，而不是每局重复使用初始本金。
pub fn replay_csv_text(
    csv_text: &str,
    config: CsvReplayConfig,
) -> Result<CsvReplayReport, CsvReplayError> {
    // 先完整读取和校验数据，再进入资金回放。这样“数据质量问题”和“策略结果”
    // 不会在同一个循环中相互干扰；同时可以在报告里明确指出哪些场次被隔离。
    let (rounds, dataset, mut quality) = load_rounds(csv_text)?;
    // replay_rounds 内部会为每个可回放场次创建独立牌靴，但本金按全局时间线共享。
    let (summary, bets) = replay_rounds_with_prepared_weights(&rounds, config, &mut quality, None)?;

    Ok(build_replay_report(config, dataset, quality, summary, bets))
}

/// 只做“牌靴重建 + 概率枚举”，供并行 Web Worker 使用。
///
/// 这个阶段绝不读取本金、下注策略或真实结果结算。多个子 Worker 可以各自
/// 处理不同的完整牌靴；最终报告仍通过 [`replay_csv_text_with_prepared_weights`]
/// 按原始时间线统一计算金额、倍投、资金和提前停止。
pub fn prepare_csv_weights(
    csv_text: &str,
    decks: u8,
) -> Result<PreparedReplayWeights, CsvReplayError> {
    let (rounds, _, mut quality) = load_rounds(csv_text)?;
    let (eligible_indices, eligible_sessions) = collect_eligible_indices(&rounds, &mut quality);

    let mut shoes = HashMap::new();
    for key in eligible_sessions {
        let shoe =
            Shoe::new(decks).map_err(|error| CsvReplayError::Configuration(error.to_string()))?;
        shoes.insert(key, shoe);
    }

    let mut probability_cache =
        HashMap::<[u8; Card::DISTINCT_COUNT], (OutcomeWeights, SideBetWeights)>::new();
    let mut prepared = Vec::with_capacity(eligible_indices.len());

    for index in eligible_indices {
        let round = &rounds[index];
        let key = (round.table_id, round.session_id);
        let shoe = shoes.get_mut(&key).expect("可预计算场次应该已经创建牌靴");
        let card_counts = shoe.card_counts();
        let (weights, side_weights) = if let Some(weights) = probability_cache.get(&card_counts) {
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
            // 缓存只服务当前小批次，不能随百万局输入无限增长。
            if probability_cache.len() >= 512 {
                probability_cache.clear();
            }
            probability_cache.insert(card_counts, weights);
            weights
        };

        prepared.push(PreparedReplayWeight {
            table_id: round.table_id,
            session_id: round.session_id,
            round_no: round.round_no,
            weights,
            side_weights,
        });

        let cards = round.cards.as_deref().expect("可预计算局应该有牌面");
        shoe.remove_many(cards)
            .map_err(|error| CsvReplayError::ShoeState {
                table_id: round.table_id,
                session_id: round.session_id,
                round_no: round.round_no,
                message: error.to_string(),
            })?;
    }

    Ok(PreparedReplayWeights { rounds: prepared })
}

/// 使用子 Worker 已经计算好的概率权重，按原始 CSV 的时间顺序完成最终回放。
///
/// 概率快照只是一份可验证的只读输入；本金、倍投级数、同局多注、返水和
/// “本金耗尽后停止”仍然全部在这里的单一顺序循环中更新，因此并行不会改变
/// 原有资金策略的语义。
pub fn replay_csv_text_with_prepared_weights(
    csv_text: &str,
    config: CsvReplayConfig,
    prepared: PreparedReplayWeights,
) -> Result<CsvReplayReport, CsvReplayError> {
    let (rounds, dataset, mut quality) = load_rounds(csv_text)?;
    let prepared_map = prepared_weight_map(prepared)?;
    let (summary, bets) =
        replay_rounds_with_prepared_weights(&rounds, config, &mut quality, Some(&prepared_map))?;

    Ok(build_replay_report(config, dataset, quality, summary, bets))
}

fn build_replay_report(
    config: CsvReplayConfig,
    dataset: CsvDatasetReport,
    quality: CsvQualityReport,
    summary: CsvReplaySummary,
    bets: Vec<CsvBetDetail>,
) -> CsvReplayReport {
    let omitted_bet_details = summary.placed_bet_count.saturating_sub(bets.len() as u64);

    CsvReplayReport {
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
            bankroll_mode: "shared_running_bankroll_timestamp_or_csv_order",
            max_fraction: config.max_fraction,
            max_round_stake: config.max_round_stake,
            table_limit: config.table_limit,
            side_bet_limit: config.side_bet_limit,
            side_bet_round_limits: config.side_bet_round_limits,
            lucky_bet_max_round: config.side_bet_round_limits.common_lucky_max_round(),
            allow_multiple_bets: config.allow_multiple_bets,
        },
        dataset,
        quality,
        summary,
        bets,
        omitted_bet_details,
    }
}

/// 单靴校验结果给浏览器做磁盘索引；昂贵枚举在后续有界 Worker 任务中执行。
#[allow(dead_code)]
pub(crate) fn inspect_stream_shoe(csv: &str) -> Result<String, String> {
    let (rounds, dataset, mut quality) = load_rounds(csv).map_err(|e| e.to_string())?;
    let (indices, _) = collect_eligible_indices(&rounds, &mut quality);
    serde_json::to_string(&serde_json::json!({
        "dataset": dataset, "quality": quality,
        "eligible": !indices.is_empty(),
        "all_have_time": indices.iter().all(|i| rounds[*i].has_started_at)
    }))
    .map_err(|e| e.to_string())
}

/// 权重保留为 Rust JSON 字符串，不让 JS Number 舍入 u64 枚举权重。
#[derive(Deserialize, Serialize)]
#[allow(dead_code)]
struct StreamPacket {
    source: CsvRound,
    prepared: PreparedReplayWeight,
    last_in_shoe: bool,
}

#[allow(dead_code)]
pub(crate) fn prepare_stream_shoe(
    csv: &str,
    decks: u8,
    timestamp_order: bool,
) -> Result<String, String> {
    let (rounds, _, mut quality) = load_rounds(csv).map_err(|e| e.to_string())?;
    let (mut indices, sessions) = collect_eligible_indices(&rounds, &mut quality);
    if sessions.len() != 1 {
        return Err("概率任务必须是一副已校验的完整牌靴".into());
    }
    // 单靴任务里的记录未必已经按时间排列。主回放在多桌数据中使用的排序键
    // 是“开局时间 -> 桌台 -> 牌靴 -> 局号”，这里必须使用同一套顺序，才能
    // 保证某一靴的最后一局不会被错误地提前释放，后续局也不会重新从完整牌靴开始。
    if timestamp_order {
        indices.sort_by(|left, right| {
            let left_round = &rounds[*left];
            let right_round = &rounds[*right];
            left_round
                .started_at
                .cmp(&right_round.started_at)
                .then_with(|| left_round.table_id.cmp(&right_round.table_id))
                .then_with(|| left_round.session_id.cmp(&right_round.session_id))
                .then_with(|| left_round.round_no.cmp(&right_round.round_no))
                .then_with(|| left_round.source_order.cmp(&right_round.source_order))
        });
    } else {
        indices.sort_by_key(|i| rounds[*i].source_order);
    }
    let mut reader = csv::Reader::from_reader(csv.trim_start_matches('\u{feff}').as_bytes());
    let sources = reader
        .deserialize::<CsvRound>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut sources: Vec<_> = sources.into_iter().map(Some).collect();
    let mut shoe = Shoe::new(decks).map_err(|e| e.to_string())?;
    let mut output = Vec::with_capacity(indices.len());
    for (position, &index) in indices.iter().enumerate() {
        let round = &rounds[index];
        let (weights, side_weights) =
            calculate_main_and_side_outcomes(&shoe).map_err(|e| e.to_string())?;
        let packet = StreamPacket {
            source: sources[index].take().expect("每行只生成一次"),
            prepared: PreparedReplayWeight {
                table_id: round.table_id,
                session_id: round.session_id,
                round_no: round.round_no,
                weights,
                side_weights,
            },
            last_in_shoe: position + 1 == indices.len(),
        };
        output.push(serde_json::json!({
            "source_order": round.source_order,
            "payload": serde_json::to_string(&packet).map_err(|e| e.to_string())?
        }));
        shoe.remove_many(round.cards.as_deref().expect("已校验牌面"))
            .map_err(|e| e.to_string())?;
    }
    serde_json::to_string(&output).map_err(|e| e.to_string())
}

/// 持久会话只保存资金状态，push 后由 JS 将这批明细落盘，不累计百万条 Vec。
#[allow(dead_code)]
pub(crate) struct StreamingReplay {
    config: CsvReplayConfig,
    state: Option<ReplayState>,
}

#[allow(dead_code)]
impl StreamingReplay {
    pub(crate) fn new(config: CsvReplayConfig, sessions: u64) -> Self {
        Self {
            config,
            state: Some(ReplayState::new(config, sessions)),
        }
    }

    pub(crate) fn push(&mut self, json: &str) -> Result<String, String> {
        let packets: Vec<StreamPacket> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if packets.len() > 256 {
            return Err("结算批次不能超过 256 局".into());
        }
        let mut rounds = Vec::with_capacity(packets.len());
        let mut prepared = PreparedReplayWeights::default();
        for packet in &packets {
            let source = &packet.source;
            let mut quality = CsvQualityReport::default();
            let (cards, outcome, banker_total, error) = validate_source_round(source, &mut quality);
            if error.is_some() || cards.is_none() {
                return Err("流式结算收到未通过校验的牌局".into());
            }
            if (source.table_id, source.session_id, source.round_no)
                != (
                    packet.prepared.table_id,
                    packet.prepared.session_id,
                    packet.prepared.round_no,
                )
            {
                return Err("牌局和权重的业务键不一致".into());
            }
            let source_order = source.source_order.unwrap_or(0);
            let started_at = source
                .started_at
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("CSV 第 {} 行", source_order + 2));
            rounds.push(LoadedRound {
                table_id: source.table_id,
                session_id: source.session_id,
                round_no: source.round_no,
                started_at,
                source_order,
                has_started_at: source
                    .started_at
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()),
                cards,
                outcome,
                banker_total,
                validation_error: None,
            });
            prepared.rounds.push(packet.prepared.clone());
        }
        let map = prepared_weight_map(prepared).map_err(|e| e.to_string())?;
        // 出错后不允许沿用可能只结算了一半的状态；调用者必须终止本次运行。
        let state = self.state.take().ok_or("回放会话已经结束或失败")?;
        let ordered = rounds.iter().collect::<Vec<_>>();
        let (mut state, bets) = settle_replay_batch(&ordered, self.config, state, Some(&map))
            .map_err(|e| e.to_string())?;
        for packet in &packets {
            if packet.last_in_shoe {
                state.release_shoe((packet.source.table_id, packet.source.session_id));
            }
        }
        let output =
            serde_json::to_string(&serde_json::json!({ "summary": state.summary, "bets": bets }))
                .map_err(|e| e.to_string())?;
        self.state = Some(state);
        Ok(output)
    }

    pub(crate) fn finish(&mut self) -> Result<String, String> {
        let state = self.state.take().ok_or("回放会话已经结束或失败")?;
        let report = build_replay_report(
            self.config,
            CsvDatasetReport::default(),
            CsvQualityReport::default(),
            state.summary,
            Vec::new(),
        );
        serde_json::to_string(&report).map_err(|e| e.to_string())
    }
}

fn prepared_weight_map(
    prepared: PreparedReplayWeights,
) -> Result<HashMap<ReplayRoundKey, (OutcomeWeights, SideBetWeights)>, CsvReplayError> {
    let mut map = HashMap::with_capacity(prepared.rounds.len());
    for item in prepared.rounds {
        if !item.weights.weights_sum_to_total()
            || item.weights.total_weight() != item.side_weights.total_weight()
        {
            return Err(CsvReplayError::PreparedWeights(
                "子 Worker 返回了不完整的概率权重".to_owned(),
            ));
        }

        let key = (item.table_id, item.session_id, item.round_no);
        if map.insert(key, (item.weights, item.side_weights)).is_some() {
            return Err(CsvReplayError::PreparedWeights(format!(
                "子 Worker 重复返回桌 {} 牌靴 {} 第 {} 局的概率权重",
                item.table_id, item.session_id, item.round_no
            )));
        }
    }
    Ok(map)
}

/// 读取 CSV、检查重复键并提前验证每一局的牌面和结果。
fn load_rounds(
    csv_text: &str,
) -> Result<(Vec<LoadedRound>, CsvDatasetReport, CsvQualityReport), CsvReplayError> {
    // UTF-8 BOM 常见于 Windows/Excel 导出的 CSV。只去掉文件开头的 BOM，
    // 不修改后续字段内容，避免第一列列名带隐藏字符导致反序列化失败。
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

    // 读取阶段同时建立三类集合：来源主键检查重复，局键检查业务重复，
    // 桌台/场次/日期集合用于生成数据集画像。
    for (source_order, row) in reader.deserialize::<CsvRound>().enumerate() {
        let source = row.map_err(|error| CsvReplayError::Csv(error.to_string()))?;
        dataset.total_rows += 1;

        let duplicate_source_pk = source
            .source_pk
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|source_pk| !source_keys.insert(source_pk.to_owned()))
            .unwrap_or(false);
        if duplicate_source_pk {
            dataset.duplicate_source_pk_rows += 1;
        }
        if !round_keys.insert((source.table_id, source.session_id, source.round_no)) {
            dataset.duplicate_round_keys += 1;
        }
        tables.insert(source.table_id);
        sessions.insert((source.table_id, source.session_id));

        let started_at = source
            .started_at
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        if let Some(value) = started_at {
            update_min_max(
                &mut dataset.started_at_min,
                &mut dataset.started_at_max,
                value,
            );

            let date = value
                .get(..10)
                .ok_or_else(|| CsvReplayError::InvalidTimestamp(value.to_owned()))?
                .to_owned();
            dates.insert(date.clone());
            update_min_max(
                &mut dataset.business_date_min,
                &mut dataset.business_date_max,
                &date,
            );
        }
        if let Some(value) = source
            .settled_at
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            update_min_max(
                &mut dataset.settled_at_min,
                &mut dataset.settled_at_max,
                value,
            );
        }

        // 牌面校验只产生“可用结果或错误说明”，不会在这里扣除任何牌；
        // 只有后面的 replay_rounds 确认整靴可回放后才会改变 Shoe。
        let (cards, outcome, banker_total, validation_error) =
            validate_source_round(&source, &mut quality);
        rounds.push(LoadedRound {
            table_id: source.table_id,
            session_id: source.session_id,
            round_no: source.round_no,
            started_at: started_at
                .map(str::to_owned)
                .unwrap_or_else(|| format!("CSV 第 {} 行", source_order + 2)),
            source_order: source.source_order.unwrap_or(source_order),
            has_started_at: started_at.is_some(),
            cards,
            outcome,
            banker_total,
            validation_error,
        });
    }

    if dataset.total_rows == 0 {
        // 空文件没有可报告的数据，也没有可回放的牌靴，直接作为输入错误返回。
        return Err(CsvReplayError::EmptyDataset);
    }

    dataset.table_count = tables.len();
    dataset.session_count = sessions.len();
    dataset.business_date_count = dates.len();

    if dataset.duplicate_source_pk_rows > 0 || dataset.duplicate_round_keys > 0 {
        // 重复行不能安全地“猜”是重复同步还是两局不同数据；拒绝回放比重复
        // 计算或重复扣牌更安全，调用者应先清洗数据。
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
    // 校验顺序从便宜到昂贵：先解析来源 payload，再用统一规则解析牌序，
    // 最后才比较数据库结果。每一步失败都会返回足够信息，供整靴隔离统计。
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
    // Rust 结果是回放可信度的基准；数据库 result_code 只作为待核对的外部字段。
    let calculated = result.outcome();
    let banker_total = result.banker_hand().total();

    // 三列精简格式没有外部结果码。此时以统一规则引擎计算的结果作为真值；
    // 完整数据库格式提供了结果码时，仍保留这道交叉校验以发现脏数据。
    let Some(result_code) = source.result_code else {
        quality.valid_card_rows += 1;
        return (Some(parsed), Some(calculated), Some(banker_total), None);
    };

    let recorded = match decode_recorded_outcome(result_code) {
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

/// 隔离不完整牌靴，并返回需要进入全局资金时间线的行索引。
fn collect_eligible_indices(
    rounds: &[LoadedRound],
    quality: &mut CsvQualityReport,
) -> (Vec<usize>, Vec<(u64, u64)>) {
    // HashMap 保存原始行索引而不是复制整行，先按 `(table_id, session_id)` 分组，
    // 再在每组内按局号排序。这样可以分别验证每靴的连续性，并保留原始字符串。
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
        // 下面四个条件共同定义“可观测牌靴”：必须从第 1 局开始、局号连续、
        // 每局有牌且每局校验成功。少一个条件都无法可靠知道下一局开始前的牌靴。
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
            // 隔离是按整靴进行的，而不是跳过单行继续猜测；否则中间缺一局时，
            // 后续所有 Shoe 状态都会偏移，产生系统性错误概率。
            quality.quarantined_rounds += indices.len() as u64;
            continue;
        }

        quality.fully_observable_sessions += 1;
        eligible_sessions.push(key);
        eligible_indices.extend(indices);
    }

    // 完整数据库格式用开局时间合并多桌资金线；只要有一行缺少时间，就统一
    // 退回 CSV 原始行顺序，避免混用两种排序规则造成不稳定或反直觉的顺序。
    let all_have_started_at = eligible_indices
        .iter()
        .all(|index| rounds[*index].has_started_at);
    if all_have_started_at {
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
    } else {
        eligible_indices.sort_by_key(|index| rounds[*index].source_order);
    }

    (eligible_indices, eligible_sessions)
}

/// 跨批次持有的唯一资金状态。概率缓存有界，明细由调用者及时排出。
pub(crate) struct ReplayState {
    shoes: HashMap<(u64, u64), Shoe>,
    probability_cache: HashMap<[u8; Card::DISTINCT_COUNT], (OutcomeWeights, SideBetWeights)>,
    summary: CsvReplaySummary,
    current_bankroll: f64,
    peak_bankroll: f64,
    effective_ev_sum: f64,
    progression_states: HashMap<(u64, u64, &'static str), StakeProgression>,
}

impl ReplayState {
    fn new(config: CsvReplayConfig, sessions: u64) -> Self {
        Self {
            shoes: HashMap::new(),
            probability_cache: HashMap::new(),
            summary: CsvReplaySummary {
                replayed_sessions: sessions,
                initial_bankroll: config.initial_bankroll,
                final_bankroll: config.initial_bankroll,
                maximum_bankroll: config.initial_bankroll,
                minimum_bankroll: config.initial_bankroll,
                ..Default::default()
            },
            current_bankroll: config.initial_bankroll,
            peak_bankroll: config.initial_bankroll,
            effective_ev_sum: 0.0,
            progression_states: HashMap::new(),
        }
    }

    /// 完整牌靴最后一局结算后释放牌靴和倍投状态；不重置共享本金。
    #[allow(dead_code)]
    fn release_shoe(&mut self, key: (u64, u64)) {
        self.shoes.remove(&key);
        self.progression_states
            .retain(|(table, shoe, _), _| (*table, *shoe) != key);
    }
}

/// 旧的一次性入口与新的流式入口共用这个结算函数，避免维护两份金额算法。
fn replay_rounds_with_prepared_weights(
    rounds: &[LoadedRound],
    config: CsvReplayConfig,
    quality: &mut CsvQualityReport,
    prepared_weights: Option<&HashMap<ReplayRoundKey, (OutcomeWeights, SideBetWeights)>>,
) -> Result<(CsvReplaySummary, Vec<CsvBetDetail>), CsvReplayError> {
    let (indices, sessions) = collect_eligible_indices(rounds, quality);
    let ordered = indices
        .iter()
        .map(|index| &rounds[*index])
        .collect::<Vec<_>>();
    let (state, details) = settle_replay_batch(
        &ordered,
        config,
        ReplayState::new(config, sessions.len() as u64),
        prepared_weights,
    )?;
    Ok((state.summary, details))
}

fn settle_replay_batch(
    rounds: &[&LoadedRound],
    config: CsvReplayConfig,
    state: ReplayState,
    prepared_weights: Option<&HashMap<ReplayRoundKey, (OutcomeWeights, SideBetWeights)>>,
) -> Result<(ReplayState, Vec<CsvBetDetail>), CsvReplayError> {
    // 通过资格筛选后，构造本次回放固定不变的规则对象。真正随每局变化的只有
    // Shoe、当前 bankroll、局号过滤结果和概率缓存键。
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

    let ReplayState {
        mut shoes,
        mut probability_cache,
        mut summary,
        mut current_bankroll,
        mut peak_bankroll,
        mut effective_ev_sum,
        mut progression_states,
    } = state;
    let mut details = Vec::new();
    for round in rounds {
        // 下注前检查本金，而不是等下一局算完才发现无法继续。所有可回放局
        // 仍然已经通过数据质量校验，但本金耗尽后再继续计算没有实际意义，
        // 因此报告会保留已完成局数并带上明确的停止位置。
        if current_bankroll <= 0.0 {
            summary.stopped_early = true;
            summary.stop_reason = Some("bankroll_depleted");
            summary.stop_table_id = Some(round.table_id);
            summary.stop_session_id = Some(round.session_id);
            summary.stop_round_no = Some(round.round_no);
            break;
        }

        let key = (round.table_id, round.session_id);
        let shoe = shoes
            .entry(key)
            .or_insert_with(|| Shoe::new(config.decks).expect("已验证副牌数"));

        // 时间顺序的关键点：这里的 shoe 仍是本局发牌前状态。先用它计算概率、
        // 策略和金额，随后才允许读取 outcome 进行结算，最后才扣牌。
        let (weights, side_weights) = if let Some(prepared) = prepared_weights {
            // 子 Worker 已经完成了昂贵的概率枚举；这里仅按业务键取快照，
            // 然后继续使用同一条顺序资金逻辑。缺少快照时宁可明确失败，
            // 也不能悄悄混入不同计算路径导致结果不可比较。
            summary.probability_cache_hits += 1;
            prepared
                .get(&(round.table_id, round.session_id, round.round_no))
                .copied()
                .ok_or_else(|| {
                    CsvReplayError::PreparedWeights(format!(
                        "缺少桌 {} 牌靴 {} 第 {} 局的预计算权重",
                        round.table_id, round.session_id, round.round_no
                    ))
                })?
        } else {
            let card_counts = shoe.card_counts();
            if let Some(weights) = probability_cache.get(&card_counts) {
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
                if probability_cache.len() >= 512 {
                    probability_cache.clear();
                }
                probability_cache.insert(card_counts, weights);
                weights
            }
        };

        // 局数限制在候选比较之前过滤。被禁用的边注不应该先赢得“最优候选”
        // 再被事后否决，否则会遮挡仍然可以下注的主注/其他边注。
        let side_bet_allowed = |side_bet| {
            config
                .side_bet_round_limits
                .allows(side_bet, round.round_no)
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
                    side_bet_allowed,
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
                        side_bet_allowed,
                    )
                    .map_err(|error| CsvReplayError::Strategy(error.to_string()))?,
            );
        }

        if config.stake_strategy.is_progression() {
            // KellyPolicy 先按“基础金额”生成报价；这里根据该目标自己的
            // 历史级数放大，再重新套用本局总风险上限，避免倍投绕过风控。
            for plan in &mut plans {
                if !matches!(plan.action(), CombinedBetPlanAction::Place { .. }) {
                    continue;
                }
                let bet = plan.decision().candidate();
                let state_key = (round.table_id, round.session_id, bet.as_str());
                let multiplier = progression_states
                    .entry(state_key)
                    .or_default()
                    .multiplier(config.stake_strategy);
                plan.scale_amount(multiplier);
            }

            let requested_total: f64 = plans
                .iter()
                .filter_map(|plan| match plan.action() {
                    CombinedBetPlanAction::Place { amount, .. } => Some(*amount),
                    CombinedBetPlanAction::Skip { .. } => None,
                })
                .sum();
            let common_limit = kelly_policy.combined_risk_limit(current_bankroll);
            let scale = if requested_total > common_limit && requested_total > 0.0 {
                common_limit / requested_total
            } else {
                1.0
            };
            for plan in &mut plans {
                plan.scale_amount(scale);
            }
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

        // 只有真的存在 Place 计划时才需要构造完整 RoundResult。纯跳过局不需要
        // 再做一次具体牌解析，但后面仍会扣除已验证的牌面以推进牌靴状态。
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
                            // 大小、对子、幸运 6/7、龙宝等边注都不属于
                            // “庄/闲遇和不返水”的例外，因此按实际下注额返水。
                            rebate.rate_for_side_bet(),
                        ),
                    };
                    let base_game_profit = amount * base_profit_per_unit;
                    let rebate_income = amount * rebate_per_unit;
                    let actual_profit = base_game_profit + rebate_income;
                    round_profit += actual_profit;

                    summary.placed_bets.increment(bet);
                    summary
                        .bet_breakdown
                        .record(bet, amount, base_game_profit, rebate_income);
                    summary.placed_bet_count += 1;
                    summary.total_stake += amount;
                    summary.total_expected_profit += quote.expected_profit();
                    summary.base_game_profit += base_game_profit;
                    summary.rebate_income += rebate_income;

                    // 赢/输/Push 分类使用“基础赔率结果”，不把正返水误判成游戏赢；
                    // 这样命中率表示玩法命中，而利润字段另行包含返水。
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

                    if config.stake_strategy.is_progression() {
                        let progression_outcome = match result {
                            "win" => ProgressionOutcome::Win,
                            "loss" => ProgressionOutcome::Loss,
                            _ => ProgressionOutcome::Push,
                        };
                        progression_states
                            .entry((round.table_id, round.session_id, bet.as_str()))
                            .or_default()
                            .record(config.stake_strategy, progression_outcome);
                    }

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

    // 分类指标和顶部汇总来自同一个结算循环。这里保留调试期对账断言，未来新增
    // 玩法时如果只更新其中一处，测试构建会立刻暴露遗漏，而不会悄悄显示错账。
    let (breakdown_count, breakdown_stake, breakdown_profit) = summary.bet_breakdown.totals();
    debug_assert_eq!(breakdown_count, summary.placed_bet_count);
    debug_assert!(
        (breakdown_stake - summary.total_stake).abs() < 1e-7 * (1.0 + summary.total_stake.abs())
    );
    debug_assert!(
        (breakdown_profit - summary.total_profit).abs() < 1e-7 * (1.0 + summary.total_stake.abs())
    );
    let (gross_profit, gross_loss) = summary.bet_breakdown.gross_totals();
    debug_assert!(
        (gross_profit - gross_loss - summary.total_profit).abs()
            < 1e-7 * (1.0 + summary.total_stake.abs())
    );
    let (base_gross_profit, base_gross_loss) = summary.bet_breakdown.base_gross_totals();
    debug_assert!(
        (base_gross_profit - base_gross_loss - summary.base_game_profit).abs()
            < 1e-7 * (1.0 + summary.total_stake.abs())
    );
    debug_assert!(
        (base_gross_profit + summary.rebate_income - base_gross_loss - summary.total_profit).abs()
            < 1e-7 * (1.0 + summary.total_stake.abs())
    );

    Ok((
        ReplayState {
            shoes,
            probability_cache,
            summary,
            current_bankroll,
            peak_bankroll,
            effective_ev_sum,
            progression_states,
        },
        details,
    ))
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
///
/// 来源数据按“庄家字段在前、闲家字段在后”保存，但百家乐实际发牌是
/// `P1 -> B1 -> P2 -> B2 -> P3 -> B3`。这里显式重排一次，后续所有规则解析
/// 都只面对统一的真实发牌顺序。
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
    // 供应商同一方的牌以逗号分隔，空片段（例如结尾逗号）表示没有额外第三张，
    // 不能把空片段当作一张非法牌。
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
    // 来源牌码采用“花色块 × 20 + 牌面序号”的约定：
    // 1..13 是第一种花色，21..33 是第二种，以此类推。先拆块，再映射到
    // 项目自己的 Suit/Rank 枚举，避免把供应商数字直接带入核心算法。
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
    // 数据库 result 可能还包含对子、大小等其他位，因此只读取最低三位的
    // 主结果标志。这里要求恰好是 1/2/4 中的一种，避免把组合位误当作主结果。
    match result_code & 0b111 {
        0b001 => Ok(RoundOutcome::Banker),
        0b010 => Ok(RoundOutcome::Player),
        0b100 => Ok(RoundOutcome::Tie),
        bits => Err(ProviderDataError::MainOutcomeBits { result_code, bits }),
    }
}

fn outcome_name(outcome: RoundOutcome) -> &'static str {
    // 不直接序列化 Rust Debug 文本，明确映射能稳定 JSON 字段，并与前端标签键一致。
    match outcome {
        RoundOutcome::Player => "player",
        RoundOutcome::Banker => "banker",
        RoundOutcome::Tie => "tie",
    }
}

fn update_min_max(minimum: &mut String, maximum: &mut String, value: &str) {
    // 时间和日期使用可按字典序比较的 ISO 文本格式，因此无需先解析成时间对象；
    // 对空字符串单独处理，保证第一条记录会成为初始边界。
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
    /// 回放配置不符合领域约束，例如本金或 EV 参数非法。
    Configuration(String),
    /// CSV 结构或字段类型无法读取。
    Csv(String),
    /// 文件没有数据行。
    EmptyDataset,
    /// `started_at` 不足以提取 YYYY-MM-DD 日期。
    InvalidTimestamp(String),
    /// 来源唯一键或业务局键出现重复。
    DuplicateKeys {
        source_pk_rows: u64,
        round_keys: u64,
    },
    /// 某一局的概率枚举失败；错误中带有定位信息。
    Probability {
        table_id: u64,
        session_id: u64,
        round_no: u32,
        message: String,
    },
    /// 方向策略或金额策略生成计划失败。
    Strategy(String),
    /// 并行概率预计算结果缺失、重复或未通过完整性校验。
    PreparedWeights(String),
    /// 已验证牌面无法从当前牌靴扣除，通常意味着数据有重复或副牌数配置错误。
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
            Self::PreparedWeights(message) => write!(formatter, "并行概率结果无效：{message}"),
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
        CsvReplayConfig, SideBetRoundLimits, StreamingReplay, parse_raw_cards, prepare_stream_shoe,
        provider_card, replay_csv_text,
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
    fn three_column_csv_is_enough_for_a_complete_replay() {
        let csv = "session_id,round_no,raw_cards\n\
                   9001,1,\"b:24,31,45;p:31,42,47\"\n\
                   9001,2,\"b:73,62,;p:53,8,\"\n";

        let report = replay_csv_text(csv, strategy()).expect("三列 CSV 应该可以直接回放");

        assert_eq!(report.dataset.total_rows, 2);
        assert_eq!(report.dataset.table_count, 1);
        assert_eq!(report.dataset.session_count, 1);
        assert_eq!(report.dataset.business_date_count, 0);
        assert!(report.dataset.started_at_min.is_empty());
        assert!(report.dataset.settled_at_min.is_empty());
        assert_eq!(report.quality.fully_observable_sessions, 1);
        assert_eq!(report.quality.valid_card_rows, 2);
        assert_eq!(report.quality.outcome_mismatch_rows, 0);
        assert_eq!(report.summary.replayed_rounds, 2);
        assert_eq!(report.bets[0].table_id, 1);
        assert_eq!(report.bets[0].started_at, "CSV 第 2 行");
    }

    #[test]
    fn three_column_csv_accepts_chinese_headers() {
        let csv = "牌靴,子局数,牌面\n\
                   9100,1,\"b:24,31,45;p:31,42,47\"\n";

        let report = replay_csv_text(csv, strategy()).expect("中文三列表头应该可以回放");

        assert_eq!(report.dataset.total_rows, 1);
        assert_eq!(report.dataset.session_count, 1);
        assert_eq!(report.quality.valid_card_rows, 1);
        assert_eq!(report.summary.replayed_rounds, 1);
    }

    #[test]
    fn streaming_session_matches_one_shot_replay_across_batches() {
        let csv = "session_id,round_no,raw_cards\n\
                   9150,1,\"b:24,31,45;p:31,42,47\"\n\
                   9150,2,\"b:73,62,;p:53,8,\"\n";
        let expected = replay_csv_text(csv, strategy()).expect("一次性回放应该成功");
        let prepared: Vec<serde_json::Value> = serde_json::from_str(
            &prepare_stream_shoe(csv, 8, false).expect("单靴概率预计算应该成功"),
        )
        .expect("预计算批次 JSON 应该有效");
        let mut stream = StreamingReplay::new(strategy(), 1);
        for item in prepared {
            let payload = item["payload"].as_str().expect("每批应该有牌局 payload");
            stream
                .push(&format!("[{payload}]"))
                .expect("增量结算应该成功");
        }
        let actual: serde_json::Value =
            serde_json::from_str(&stream.finish().expect("流式回放应该可以结束"))
                .expect("流式报告 JSON 应该有效");
        assert_eq!(
            actual["summary"]["replayed_rounds"],
            expected.summary.replayed_rounds
        );
        assert_eq!(
            actual["summary"]["placed_bet_count"],
            expected.summary.placed_bet_count
        );
        assert_eq!(
            actual["summary"]["final_bankroll"],
            expected.summary.final_bankroll
        );
        assert_eq!(
            actual["summary"]["total_profit"],
            expected.summary.total_profit
        );
    }

    #[test]
    fn streaming_session_uses_the_same_timestamp_order_as_one_shot_replay() {
        // 文件行顺序故意把第 2 局放在第 1 局前面；完整时间字段存在时，资金线
        // 应按开局时间处理，且牌靴最后一个时间点之后才允许释放牌靴状态。
        let csv = "session_id,round_no,started_at,raw_cards\n\
                   9151,1,2026-08-20 00:00:54,\"b:24,31,45;p:31,42,47\"\n\
                   9151,2,2026-08-20 00:00:12,\"b:73,62,;p:53,8,\"\n";
        let expected = replay_csv_text(csv, strategy()).expect("一次性时间排序回放应该成功");
        let prepared: Vec<serde_json::Value> = serde_json::from_str(
            &prepare_stream_shoe(csv, 8, true).expect("时间排序预计算应该成功"),
        )
        .expect("时间排序批次 JSON 应该有效");
        let mut stream = StreamingReplay::new(strategy(), 1);
        for item in prepared {
            stream
                .push(&format!(
                    "[{}]",
                    item["payload"].as_str().expect("每批应该有牌局 payload")
                ))
                .expect("时间排序增量结算应该成功");
        }
        let actual: serde_json::Value =
            serde_json::from_str(&stream.finish().expect("时间排序流式回放应该可以结束"))
                .expect("时间排序流式报告 JSON 应该有效");
        assert_eq!(
            actual["summary"]["replayed_rounds"],
            expected.summary.replayed_rounds
        );
        assert_eq!(
            actual["summary"]["placed_bet_count"],
            expected.summary.placed_bet_count
        );
        let actual_final = actual["summary"]["final_bankroll"]
            .as_f64()
            .expect("时间排序流式最终本金应该是数字");
        assert!(
            (actual_final - expected.summary.final_bankroll).abs() < 1e-9,
            "时间排序流式与一次性回放的最终本金应一致：{actual_final} != {}",
            expected.summary.final_bankroll
        );
    }

    #[test]
    fn depleted_bankroll_stops_before_the_next_round_and_reports_position() {
        // 三局都让玩家以 9 点获胜。完整牌靴的最优主注通常是庄，
        // 因而前两局下注庄并依次输掉 1、2 元；第三局开始前本金耗尽。
        let csv = "session_id,round_no,raw_cards\n\
                   9200,1,\"b:10,11,12;p:23,27,9\"\n\
                   9200,2,\"b:30,31,32;p:51,52,29\"\n\
                   9200,3,\"b:70,71,72;p:53,33,69\"\n";
        let config = CsvReplayConfig::with_strategy(
            8,
            MainBetRules::standard(),
            StakeSizingStrategy::Martingale { amount: 1.0 },
            0.0,
            -1.0,
            3.0,
            1.0,
            3.0,
            3.0,
        )
        .expect("本金耗尽测试配置应该合法");

        let report = replay_csv_text(csv, config).expect("两局牌面应该可以回放");

        assert_eq!(report.summary.replayed_rounds, 2);
        assert!(report.summary.stopped_early);
        assert_eq!(report.summary.stop_reason, Some("bankroll_depleted"));
        assert_eq!(report.summary.stop_round_no, Some(3));
        assert_eq!(report.summary.final_bankroll, 0.0);
        assert_eq!(report.bets.len(), 2);
        assert_eq!(report.bets[0].amount, 1.0);
        assert_eq!(report.bets[1].amount, 2.0);
    }

    #[test]
    fn default_side_bet_round_limits_match_the_table_rules() {
        let limits = SideBetRoundLimits::default();

        for side_bet in [SideBet::Big, SideBet::Small] {
            assert!(limits.allows(side_bet, 20));
            assert!(!limits.allows(side_bet, 21));
        }
        assert!(limits.allows(SideBet::PerfectPair, 45));
        assert!(!limits.allows(SideBet::PerfectPair, 46));
        assert!(limits.allows(SideBet::SuperLuckySeven, 30));
        assert!(!limits.allows(SideBet::SuperLuckySeven, 31));

        for side_bet in SideBet::ALL {
            if matches!(
                side_bet,
                SideBet::Big | SideBet::Small | SideBet::PerfectPair | SideBet::SuperLuckySeven
            ) {
                continue;
            }
            assert!(limits.allows(side_bet, 50));
            assert!(!limits.allows(side_bet, 51));
        }
    }

    #[test]
    fn each_side_bet_round_limit_can_be_customised_independently() {
        let limits = SideBetRoundLimits {
            any_pair: 10,
            banker_pair: 30,
            player_pair: 0,
            ..SideBetRoundLimits::default()
        };

        assert!(!limits.allows(SideBet::AnyPair, 11));
        assert!(limits.allows(SideBet::BankerPair, 30));
        assert!(!limits.allows(SideBet::BankerPair, 31));
        assert!(limits.allows(SideBet::PlayerPair, 999));
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
    fn replay_applies_rebate_to_every_side_bet_and_counts_each_category() {
        let csv = format!(
            "{HEADER}a,1,9100,1,2026-08-20 00:00:12,2026-08-20 00:00:44,\"b:24,31,45;p:31,42,47\",36\n"
        );
        // 固定金额不要求正凯利；把两个 EV 门槛降到 -10 后，单局多注模式会
        // 为 3 种主注和 11 种边注各生成一笔，便于一次验证完整分类。
        let config = CsvReplayConfig::with_side_bets(
            8,
            MainBetRules::standard(),
            StakeSizingStrategy::Fixed { amount: 1.0 },
            0.02,
            -10.0,
            -10.0,
            10_000.0,
            1.0,
            100.0,
            100.0,
            100.0,
        )
        .expect("完整分类回放配置应该合法")
        .with_multiple_bets(true);

        let report = replay_csv_text(&csv, config).expect("单局完整牌靴应该可以回放");
        let counts = &report.summary.placed_bets;

        assert_eq!(report.summary.placed_bet_count, 14);
        assert_eq!(counts.player, 1);
        assert_eq!(counts.banker, 1);
        assert_eq!(counts.tie, 1);
        assert_eq!(counts.any_pair, 1);
        assert_eq!(counts.banker_pair, 1);
        assert_eq!(counts.player_pair, 1);
        assert_eq!(counts.perfect_pair, 1);
        assert_eq!(counts.big, 1);
        assert_eq!(counts.small, 1);
        assert_eq!(counts.lucky_seven, 1);
        assert_eq!(counts.super_lucky_seven, 1);
        assert_eq!(counts.lucky_six, 1);
        assert_eq!(counts.banker_dragon_bonus, 1);
        assert_eq!(counts.player_dragon_bonus, 1);

        // 每种玩法的资金表现必须与旧的分类笔数保持一致；固定 1 元策略下，
        // 14 种玩法各下一笔，所以分类下注额合计应为 14 元。
        let breakdown = &report.summary.bet_breakdown;
        for performance in [
            &breakdown.player,
            &breakdown.banker,
            &breakdown.tie,
            &breakdown.any_pair,
            &breakdown.banker_pair,
            &breakdown.player_pair,
            &breakdown.perfect_pair,
            &breakdown.big,
            &breakdown.small,
            &breakdown.lucky_seven,
            &breakdown.super_lucky_seven,
            &breakdown.lucky_six,
            &breakdown.banker_dragon_bonus,
            &breakdown.player_dragon_bonus,
        ] {
            assert_eq!(performance.count, 1);
            assert_eq!(
                performance.win_count + performance.loss_count + performance.push_count,
                performance.count
            );
            assert!((performance.total_stake - 1.0).abs() < 1e-12);
            assert!(
                (performance.gross_profit - performance.gross_loss - performance.total_profit)
                    .abs()
                    < 1e-12
            );
        }
        let (breakdown_count, breakdown_stake, breakdown_profit) = breakdown.totals();
        assert_eq!(breakdown_count, report.summary.placed_bet_count);
        assert!((breakdown_stake - report.summary.total_stake).abs() < 1e-12);
        assert!((breakdown_profit - report.summary.total_profit).abs() < 1e-12);
        let (gross_profit, gross_loss) = breakdown.gross_totals();
        assert!((gross_profit - gross_loss - report.summary.total_profit).abs() < 1e-12);
        let (base_gross_profit, base_gross_loss) = breakdown.base_gross_totals();
        assert!(
            (base_gross_profit - base_gross_loss - report.summary.base_game_profit).abs() < 1e-12
        );
        assert!(
            (base_gross_profit + report.summary.rebate_income
                - base_gross_loss
                - report.summary.total_profit)
                .abs()
                < 1e-12
        );

        let side_bets = report
            .bets
            .iter()
            .filter(|detail| !matches!(detail.bet, "player" | "banker" | "tie"))
            .collect::<Vec<_>>();
        assert_eq!(side_bets.len(), 11);
        for detail in side_bets {
            assert!(
                (detail.rebate_income - detail.amount * 0.02).abs() < 1e-12,
                "{} 没有按下注额计算返水",
                detail.bet
            );
            assert!(
                (detail.actual_profit - (detail.base_game_profit + detail.rebate_income)).abs()
                    < 1e-12
            );
        }
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
