//! 标准百家乐的补牌规则。
//!
//! 这个模块是“纯规则层”：函数只根据输入点数返回 `true` 或 `false`，
//! 不会从牌靴摸牌，也不会修改任何状态。因此真实回合解析和概率枚举器
//! 都能复用同一份规则，避免两套补牌表慢慢变得不一致。

/// 根据闲家起手点数判断是否需要补第三张牌。
///
/// 调用者应传入 0～9：闲 0～5 补牌，6～7 停牌；自然 8/9 通常已经由
/// 回合流程提前拦截，但这里对 8/9 同样返回 `false`。
pub const fn player_should_draw(initial_total: u8) -> bool {
    // matches! 会把“是否匹配模式”直接转成 bool。
    // `0..=5` 是包含 0 和 5 的范围模式。
    matches!(initial_total, 0..=5)
}

/// 根据庄家起手点数和闲家可选的第三张牌点数判断庄家是否补牌。
///
/// `player_third_value` 为 `None` 表示闲家没有补第三张牌，此时庄家像闲家一样
/// 在 0～5 点补牌。`Some(0..=9)` 表示闲家已经补牌，庄家必须查完整补牌表。
pub const fn banker_should_draw(banker_initial_total: u8, player_third_value: Option<u8>) -> bool {
    // 先按闲家是否补牌分成两张表；Some 分支中再按庄家起手点数
    // 查询标准补牌表。这个嵌套 match 和真实规则表的结构一致。
    match player_third_value {
        // 闲家停牌时，庄家 0～5 补、6～7 停；自然牌由回合流程提前结束。
        None => matches!(banker_initial_total, 0..=5),
        // 闲家补牌时，下面每一行就是标准庄家补牌表的一行。
        Some(player_third_value) => match banker_initial_total {
            // 庄 0、1、2：无论闲家第三张是什么都补牌。
            0..=2 => matches!(player_third_value, 0..=9),
            // 庄 3：闲家第三张为 8 时停牌，其余点数补牌。
            3 => matches!(player_third_value, 0..=7 | 9),
            // 庄 4：仅当闲家第三张为 2～7 时补牌。
            4 => matches!(player_third_value, 2..=7),
            // 庄 5：仅当闲家第三张为 4～7 时补牌。
            5 => matches!(player_third_value, 4..=7),
            // 庄 6：仅当闲家第三张为 6 或 7 时补牌。
            6 => matches!(player_third_value, 6..=7),
            // 庄 7 停牌；8、9 理论上会作为自然牌提前结束。
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{banker_should_draw, player_should_draw};

    #[test]
    fn player_draws_on_zero_through_five_and_stands_on_six_through_nine() {
        let cases = [
            (0, true),
            (1, true),
            (2, true),
            (3, true),
            (4, true),
            (5, true),
            (6, false),
            (7, false),
            (8, false),
            (9, false),
        ];

        for (initial_total, expected) in cases {
            assert_eq!(
                player_should_draw(initial_total),
                expected,
                "闲家起手点数为 {initial_total}"
            );
        }
    }

    #[test]
    fn banker_drawing_rule_matches_the_complete_table() {
        let without_player_third = [
            true, true, true, true, true, true, false, false, false, false,
        ];
        let with_player_third = [
            [true; 10],
            [true; 10],
            [true; 10],
            [true, true, true, true, true, true, true, true, false, true],
            [
                false, false, true, true, true, true, true, true, false, false,
            ],
            [
                false, false, false, false, true, true, true, true, false, false,
            ],
            [
                false, false, false, false, false, false, true, true, false, false,
            ],
            [false; 10],
            [false; 10],
            [false; 10],
        ];

        for banker_total in 0_u8..=9 {
            assert_eq!(
                banker_should_draw(banker_total, None),
                without_player_third[usize::from(banker_total)],
                "闲家未补牌，庄家起手点数为 {banker_total}"
            );

            for player_third in 0_u8..=9 {
                assert_eq!(
                    banker_should_draw(banker_total, Some(player_third)),
                    with_player_third[usize::from(banker_total)][usize::from(player_third)],
                    "庄家起手点数为 {banker_total}，闲家第三张牌点数为 {player_third}"
                );
            }
        }
    }
}
