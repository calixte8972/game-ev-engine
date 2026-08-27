use game_ev_engine::{Shoe, calculate_main_outcomes};

#[test]
fn default_eight_deck_shoe_matches_probability_baseline() {
    let weights = calculate_main_outcomes(&Shoe::default()).expect("完整八副牌应能够完成主注枚举");

    assert_eq!(weights.player_weight(), 2_230_518_282_592_256);
    assert_eq!(weights.banker_weight(), 2_292_252_566_437_888);
    assert_eq!(weights.tie_weight(), 475_627_426_473_216);
    assert_eq!(weights.total_weight(), 4_998_398_275_503_360);
    assert!(weights.banker_win_on_six_weight() > 0);
    assert!(weights.banker_win_on_six_weight() <= weights.banker_weight());

    assert!((weights.player_probability() - 0.446246609344).abs() < 1e-12);
    assert!((weights.banker_probability() - 0.458597422633).abs() < 1e-12);
    assert!((weights.tie_probability() - 0.095155968024).abs() < 1e-12);
}
