//! 标准百家乐主注的类型与净赔付规则。
//!
//! 概率层只回答“Player、Banker、Tie 各有多大概率”，不知道赌场如何赔钱。
//! 本模块把下注方向、牌局结果和赔付配置组合起来，回答：
//!
//! ```text
//! 如果下注 1 单位，并且 outcome 发生，最终净盈利是多少？
//! ```
//!
//! 统一使用“净盈利”口径非常重要：赢闲注返回 `1.0`，输注返回 `-1.0`，
//! Push 返回 `0.0`。这样 EV 层可以直接计算 `Σ 概率 × 净盈利`。

use super::RoundOutcome;

/// 可下注的三种主注。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum MainBet {
    /// 闲家获胜时下注赢。
    Player,
    /// 庄家获胜时下注赢。
    Banker,
    /// 双方最终点数相同时下注赢。
    Tie,
}

impl MainBet {
    /// 返回适合 CLI、JSON 和 Python 使用的稳定小写名称。
    pub const fn as_str(self) -> &'static str {
        // 返回 &'static str 表示这些文本直接存放在程序二进制中，生命周期贯穿
        // 整个程序；调用者不需要分配新的 String，也不需要负责释放。
        match self {
            Self::Player => "player",
            Self::Banker => "banker",
            Self::Tie => "tie",
        }
    }
}

/// 庄注的赔付方式。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BankerPayoutRule {
    /// 标准庄：庄家获胜统一按指定净赔付结算。
    Commission {
        /// 扣除佣金后的庄赢净赔付，例如 0.95。
        net_payout: f64,
    },
    /// 免佣庄：庄家最终 6 点时使用较低赔付，其他庄赢使用普通赔付。
    NoCommission {
        /// 庄家不是 6 点获胜时的净赔付。
        normal_net_payout: f64,
        /// 庄家以 6 点获胜时的特殊净赔付。
        six_net_payout: f64,
    },
}

/// 三种主注的净赔付配置。
///
/// 所有赔付值都表示“每下注 1 单位的净盈利”，不包含退回的本金：
/// 例如 `0.95` 表示赢得 0.95 单位，`-1.0` 表示输掉本金，`0.0`
/// 表示和局时 Push。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MainBetRules {
    /// 闲注获胜时的净赔付。
    player_payout: f64,
    /// 庄注赔付方式；使用枚举是因为免佣庄需要区分庄 6 与其他庄赢。
    banker_rule: BankerPayoutRule,
    /// 和注获胜时的净赔付。
    tie_payout: f64,
}

impl MainBetRules {
    /// 使用自定义的闲、庄、和净赔付创建主注规则。
    ///
    /// 参数仍然表示每下注 1 单位的净盈利，不包含本金返还。例如 `8.0`
    /// 表示和注赢得 8 个单位本金之外的净盈利。
    pub const fn with_payouts(player_payout: f64, banker_payout: f64, tie_payout: f64) -> Self {
        Self::with_banker_rule(
            player_payout,
            BankerPayoutRule::Commission {
                net_payout: banker_payout,
            },
            tie_payout,
        )
    }

    /// 使用自定义庄赔付方式创建主注规则。
    ///
    /// 该构造函数是完整入口；`with_payouts`、`standard` 和 `no_commission`
    /// 都会复用它，确保字段组装逻辑只保留一份。
    pub const fn with_banker_rule(
        player_payout: f64,
        banker_rule: BankerPayoutRule,
        tie_payout: f64,
    ) -> Self {
        Self {
            player_payout,
            banker_rule,
            tie_payout,
        }
    }

    /// 创建项目当前采用的标准百家乐主注赔付：闲 1:1、庄 0.95:1、和 8:1。
    pub const fn standard() -> Self {
        Self::with_banker_rule(1.0, BankerPayoutRule::Commission { net_payout: 0.95 }, 8.0)
    }

    /// 创建免佣庄规则：普通庄赢 1:1，庄家最终 6 点获胜 0.5:1。
    pub const fn no_commission() -> Self {
        Self::with_banker_rule(
            1.0,
            BankerPayoutRule::NoCommission {
                normal_net_payout: 1.0,
                six_net_payout: 0.5,
            },
            8.0,
        )
    }

    /// 返回闲注获胜时的净赔付。
    pub const fn player_payout(self) -> f64 {
        self.player_payout
    }

    /// 返回庄家普通获胜时的净赔付。
    pub const fn banker_payout(self) -> f64 {
        // `|` 把两个模式合并：无论标准庄还是免佣庄，都把“普通庄赢”的
        // 赔付绑定到同一个局部变量 `net_payout` 后统一返回。
        match self.banker_rule {
            BankerPayoutRule::Commission { net_payout }
            | BankerPayoutRule::NoCommission {
                normal_net_payout: net_payout,
                ..
            } => net_payout,
        }
    }

    /// 返回当前庄注赔付规则。
    pub const fn banker_rule(self) -> BankerPayoutRule {
        self.banker_rule
    }

    /// 根据庄家最终点数返回对应的庄注净赔付。
    pub const fn banker_payout_for_total(self, banker_total: u8) -> f64 {
        // 标准庄不关心最终点数；免佣庄只有最终 6 点时使用半赔。
        match self.banker_rule {
            BankerPayoutRule::Commission { net_payout } => net_payout,
            BankerPayoutRule::NoCommission {
                normal_net_payout,
                six_net_payout,
            } => {
                if banker_total == 6 {
                    six_net_payout
                } else {
                    normal_net_payout
                }
            }
        }
    }

    /// 返回和注获胜时的净赔付。
    pub const fn tie_payout(self) -> f64 {
        self.tie_payout
    }

    /// 结算一笔不需要区分庄家最终点数的主注，返回相对于本金的净盈利。
    ///
    /// 庄注在庄家获胜且规则可能区分庄 6 时，应使用
    /// [`Self::settle_with_banker_total`]。
    pub const fn settle(self, bet: MainBet, outcome: RoundOutcome) -> f64 {
        // `(bet, outcome)` 组成一个元组。match 会穷尽三种下注 × 三种结果的
        // 九种组合，未来如果枚举增加新变体，编译器会提醒这里补充规则。
        match (bet, outcome) {
            (MainBet::Player, RoundOutcome::Player) => self.player_payout,
            (MainBet::Player, RoundOutcome::Banker) => -1.0,
            (MainBet::Player, RoundOutcome::Tie) => 0.0,
            (MainBet::Banker, RoundOutcome::Player) => -1.0,
            (MainBet::Banker, RoundOutcome::Banker) => self.banker_payout(),
            (MainBet::Banker, RoundOutcome::Tie) => 0.0,
            // `|` 是模式的“或”：下注 Tie 时，Player 或 Banker 获胜都会输本金。
            (MainBet::Tie, RoundOutcome::Player | RoundOutcome::Banker) => -1.0,
            (MainBet::Tie, RoundOutcome::Tie) => self.tie_payout,
        }
    }

    /// 根据庄家最终点数结算一笔主注，返回相对于本金的净盈利。
    ///
    /// 只有“下注庄且庄家获胜”需要查看 `banker_total`；其余八种组合仍复用
    /// `settle`，避免复制 Player、Tie、输注和 Push 的结算规则。
    pub const fn settle_with_banker_total(
        self,
        bet: MainBet,
        outcome: RoundOutcome,
        banker_total: u8,
    ) -> f64 {
        // 只拦截唯一需要额外信息的组合；下划线 `_` 代表其余所有组合。
        match (bet, outcome) {
            (MainBet::Banker, RoundOutcome::Banker) => self.banker_payout_for_total(banker_total),
            _ => self.settle(bet, outcome),
        }
    }
}

impl Default for MainBetRules {
    /// `MainBetRules::default()` 与 `MainBetRules::standard()` 使用同一套标准赔付。
    fn default() -> Self {
        Self::standard()
    }
}

#[cfg(test)]
mod tests {
    use super::{MainBet, MainBetRules};
    use crate::RoundOutcome;

    #[test]
    fn standard_rules_use_net_payouts() {
        let rules = MainBetRules::standard();

        assert_eq!(rules.player_payout(), 1.0);
        assert_eq!(rules.banker_payout(), 0.95);
        assert_eq!(rules.tie_payout(), 8.0);
    }

    #[test]
    fn custom_rules_can_change_each_net_payout() {
        let rules = MainBetRules::with_payouts(1.0, 1.0, 9.0);

        assert_eq!(rules.player_payout(), 1.0);
        assert_eq!(rules.banker_payout(), 1.0);
        assert_eq!(rules.tie_payout(), 9.0);
    }

    #[test]
    fn no_commission_rules_pay_half_on_banker_six() {
        let rules = MainBetRules::no_commission();

        assert_eq!(rules.banker_payout(), 1.0);
        assert_eq!(rules.banker_payout_for_total(5), 1.0);
        assert_eq!(rules.banker_payout_for_total(6), 0.5);
        assert_eq!(
            rules.settle_with_banker_total(MainBet::Banker, RoundOutcome::Banker, 6),
            0.5
        );
        assert_eq!(
            rules.settle_with_banker_total(MainBet::Banker, RoundOutcome::Banker, 5),
            1.0
        );
    }

    #[test]
    fn settles_all_main_bet_outcome_combinations() {
        let rules = MainBetRules::standard();
        let cases = [
            (MainBet::Player, RoundOutcome::Player, 1.0),
            (MainBet::Player, RoundOutcome::Banker, -1.0),
            (MainBet::Player, RoundOutcome::Tie, 0.0),
            (MainBet::Banker, RoundOutcome::Player, -1.0),
            (MainBet::Banker, RoundOutcome::Banker, 0.95),
            (MainBet::Banker, RoundOutcome::Tie, 0.0),
            (MainBet::Tie, RoundOutcome::Player, -1.0),
            (MainBet::Tie, RoundOutcome::Banker, -1.0),
            (MainBet::Tie, RoundOutcome::Tie, 8.0),
        ];

        for (bet, outcome, expected) in cases {
            assert_eq!(rules.settle(bet, outcome), expected);
        }
    }
}
