//! 完整八副牌概率的外部基线测试。
//!
//! 这个文件位于 `tests/`，会像外部使用者一样只访问 crate 的公开 API。
//! 固定整数权重可以发现枚举路径被意外修改；固定概率则方便人工核对常见结果。

use game_ev_engine::{Shoe, calculate_main_outcomes};

#[test]
fn default_eight_deck_shoe_matches_probability_baseline() {
    // expect 表示这个场景按设计必须成功；如果失败，测试应立即停止并显示原因。
    let weights = calculate_main_outcomes(&Shoe::default()).expect("完整八副牌应能够完成主注枚举");

    // 先检查精确整数权重。概率只是下面这些整数除以共同分母后的展示形式。
    assert_eq!(weights.player_weight(), 2_230_518_282_592_256);
    assert_eq!(weights.banker_weight(), 2_292_252_566_437_888);
    assert_eq!(weights.tie_weight(), 475_627_426_473_216);
    assert_eq!(weights.total_weight(), 4_998_398_275_503_360);
    assert!(weights.banker_win_on_six_weight() > 0);
    assert!(weights.banker_win_on_six_weight() <= weights.banker_weight());

    // 浮点数不适合直接 == 比较，所以检查与基线之差是否小于允许误差。
    assert!((weights.player_probability() - 0.446246609344).abs() < 1e-12);
    assert!((weights.banker_probability() - 0.458597422633).abs() < 1e-12);
    assert!((weights.tie_probability() - 0.095155968024).abs() < 1e-12);
}
