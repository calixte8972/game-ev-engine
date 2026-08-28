use crate::baccarat::RebateRule;
use crate::{MainBet, MainBetAnalysis};

pub struct BettingPolicy {
    rebate: RebateRule,
    minimum_effective_ev: f64,
}
pub enum BetAction {
    Place { bet: MainBet },
    Skip { reason: SkipReason },
}
pub enum SkipReason {
    BelowMinimumEv { effective_ev: f64, minimum_ev: f64 },
}
pub struct BetDecision {
    candidate: MainBet,
    base_ev: f64,
    rebate_ev: f64,
    effective_ev: f64,
    action: BetAction,
}
impl BettingPolicy {
    pub const fn new(rebate: RebateRule, minimum_effective_ev: f64) -> Self {
        Self {
            rebate,
            minimum_effective_ev,
        }
    }
    pub fn decide(&self, analysis: MainBetAnalysis) -> BetDecision {
        let candidate = analysis.optimal_effective_bet(self.rebate);
        let metrics = analysis.effective_metrics(candidate, self.rebate);
        let effective_ev = metrics.effective_ev();
        let action: BetAction = if effective_ev > self.minimum_effective_ev {
            BetAction::Place { bet: candidate }
        } else {
            BetAction::Skip {
                reason: SkipReason::BelowMinimumEv {
                    effective_ev,
                    minimum_ev: self.minimum_effective_ev,
                },
            }
        };
        BetDecision {
            candidate,
            base_ev: metrics.base_ev(),
            rebate_ev: metrics.base_ev(),
            effective_ev,
            action,
        }
    }
}
impl BetDecision {
    pub const fn candidate(self) -> MainBet {
        self.candidate
    }

    pub const fn base_ev(self) -> f64 {
        self.base_ev
    }

    pub const fn rebate_ev(self) -> f64 {
        self.rebate_ev
    }

    pub const fn effective_ev(self) -> f64 {
        self.effective_ev
    }

    pub const fn action(&self) -> &BetAction {
        &self.action
    }
}
