//! 标准百家乐的补牌规则。

/// 根据闲家起手点数判断是否需要补第三张牌。
pub const fn player_should_draw(initial_total: u8) -> bool {
    matches!(initial_total, 0..=5)
}

/// 根据庄家起手点数和闲家可选的第三张牌点数判断庄家是否补牌。
pub const fn banker_should_draw(banker_initial_total: u8, player_third_value: Option<u8>) -> bool {
    match player_third_value {
        None => matches!(banker_initial_total, 0..=5),
        Some(player_third_value) => match banker_initial_total {
            0..=2 => matches!(player_third_value, 0..=9),
            3 => matches!(player_third_value, 0..=7 | 9),
            4 => matches!(player_third_value, 2..=7),
            5 => matches!(player_third_value, 4..=7),
            6 => matches!(player_third_value, 6..=7),
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
