//! 主注概率枚举的简单性能测量程序。
//!
//! 它不是业务入口，也不验证结果是否正确；正确性由 tests 负责。这里重复运行
//! 完整八副牌枚举，观察总耗时和单次耗时，便于比较优化前后的性能。

use std::{
    env,
    hint::black_box,
    thread,
    time::{Duration, Instant},
};

use game_ev_engine::{Shoe, calculate_main_and_side_outcomes, calculate_main_outcomes};

fn argument(name: &str) -> Option<u64> {
    // 例如 name 为 "--runs" 时，只接受形如 "--runs=10" 的参数。
    let prefix = format!("{name}=");

    // find_map 会逐项检查参数：strip_prefix 不匹配时返回 None；匹配后再尝试
    // 解析 u64。找到第一个完整成功的值就停止。
    env::args()
        .skip(1)
        .find_map(|argument| argument.strip_prefix(&prefix)?.parse().ok())
}

fn main() {
    // 没有提供参数时运行一次；max(1) 防止 --runs=0 导致没有测量结果。
    let runs = argument("--runs").unwrap_or(1).max(1);
    let hold_ms = argument("--hold-ms").unwrap_or(0);
    // --side 测量回放实际使用的“主注 + 全部边注”路径；默认仍只测主注。
    let include_side_bets = env::args().any(|argument| argument == "--side");
    let shoe = Shoe::default();

    // 首次调用会生成静态的六张牌组合表。预热后再计时，避免把一次性初始化
    // 成本误当成每一局都会发生的枚举成本。
    if include_side_bets {
        black_box(calculate_main_and_side_outcomes(&shoe).expect("完整牌靴应可枚举"));
    } else {
        black_box(calculate_main_outcomes(&shoe).expect("完整牌靴应可枚举"));
    }

    // Instant 是单调时钟，适合测量一段代码经过了多长时间。
    let start = Instant::now();
    let mut last_weight = None;

    for _ in 0..runs {
        // black_box 阻止编译器因为输入和结果未被正常业务使用而删除整段计算。
        let weight = if include_side_bets {
            let (main, side) = calculate_main_and_side_outcomes(black_box(&shoe))
                .expect("完整八副牌应能够完成主注和边注枚举");
            black_box(side);
            main.total_weight()
        } else {
            calculate_main_outcomes(black_box(&shoe))
                .expect("完整八副牌应能够完成主注枚举")
                .total_weight()
        };
        last_weight = Some(black_box(weight));
    }

    let elapsed = start.elapsed();
    let total_weight = last_weight.expect("至少应执行一次计算");

    println!("pid={}", std::process::id());
    println!(
        "mode={}",
        if include_side_bets {
            "main+side"
        } else {
            "main"
        }
    );
    println!("runs={runs}");
    println!("elapsed_ms={:.3}", elapsed.as_secs_f64() * 1_000.0);
    println!(
        "per_run_ms={:.3}",
        elapsed.as_secs_f64() * 1_000.0 / runs as f64
    );
    println!("total_weight={total_weight}");

    if hold_ms > 0 {
        // 可选停留时间方便使用外部性能分析器附加到当前进程。
        thread::sleep(Duration::from_millis(hold_ms));
    }
}
