use std::{
    env,
    hint::black_box,
    thread,
    time::{Duration, Instant},
};

use game_ev_engine::{Shoe, calculate_main_outcomes};

fn argument(name: &str) -> Option<u64> {
    let prefix = format!("{name}=");

    env::args()
        .skip(1)
        .find_map(|argument| argument.strip_prefix(&prefix)?.parse().ok())
}

fn main() {
    let runs = argument("--runs").unwrap_or(1).max(1);
    let hold_ms = argument("--hold-ms").unwrap_or(0);
    let shoe = Shoe::default();

    let start = Instant::now();
    let mut last_weights = None;

    for _ in 0..runs {
        let weights =
            calculate_main_outcomes(black_box(&shoe)).expect("完整八副牌应能够完成主注枚举");
        last_weights = Some(black_box(weights));
    }

    let elapsed = start.elapsed();
    let weights = last_weights.expect("至少应执行一次计算");

    println!("pid={}", std::process::id());
    println!("runs={runs}");
    println!("elapsed_ms={:.3}", elapsed.as_secs_f64() * 1_000.0);
    println!(
        "per_run_ms={:.3}",
        elapsed.as_secs_f64() * 1_000.0 / runs as f64
    );
    println!("total_weight={}", weights.total_weight());

    if hold_ms > 0 {
        thread::sleep(Duration::from_millis(hold_ms));
    }
}
