//! 生成可以直接交给网页回放的三列随机百家乐 CSV。
//!
//! 真正的洗牌、补牌和牌面生成位于库中的 `baccarat::simulation`。这个二进制
//! 只负责解析命令行和写文件，浏览器随机回测也调用同一个生成器。

use std::{
    env,
    error::Error,
    fmt,
    fs::{self, File},
    path::PathBuf,
};

use game_ev_engine::{BaccaratSimulationConfig, write_baccarat_csv};

const DEFAULT_SHOES: u64 = 100;
const DEFAULT_DECKS: u8 = 8;
const DEFAULT_ROUNDS_PER_SHOE: u32 = 60;
const DEFAULT_SEED: u64 = 20_260_902;
const DEFAULT_START_SESSION_ID: u64 = 1_000_000;

#[derive(Debug, Clone)]
struct GeneratorConfig {
    output: PathBuf,
    shoes: u64,
    decks: u8,
    rounds_per_shoe: u32,
    seed: u64,
    start_session_id: u64,
}

impl GeneratorConfig {
    fn from_args() -> Result<Self, GeneratorArgumentsError> {
        let mut arguments = env::args().skip(1);
        let output = arguments
            .next()
            .ok_or_else(|| GeneratorArgumentsError(usage("缺少输出 CSV 路径")))?;
        let mut config = Self {
            output: PathBuf::from(output),
            shoes: DEFAULT_SHOES,
            decks: DEFAULT_DECKS,
            rounds_per_shoe: DEFAULT_ROUNDS_PER_SHOE,
            seed: DEFAULT_SEED,
            start_session_id: DEFAULT_START_SESSION_ID,
        };

        for argument in arguments {
            let (name, value) = argument.split_once('=').ok_or_else(|| {
                GeneratorArgumentsError(usage(&format!(
                    "参数必须使用 --name=value 格式：{argument}"
                )))
            })?;
            match name {
                "--shoes" => config.shoes = parse_number(name, value)?,
                "--decks" => config.decks = parse_number(name, value)?,
                "--rounds-per-shoe" => config.rounds_per_shoe = parse_number(name, value)?,
                "--seed" => config.seed = parse_number(name, value)?,
                "--start-session-id" => config.start_session_id = parse_number(name, value)?,
                _ => {
                    return Err(GeneratorArgumentsError(usage(&format!("未知参数：{name}"))));
                }
            }
        }

        // 在真正创建文件前调用领域配置校验，避免参数错误时留下半个 CSV。
        config.simulation()?;
        Ok(config)
    }

    fn simulation(&self) -> Result<BaccaratSimulationConfig, GeneratorArgumentsError> {
        BaccaratSimulationConfig::new(
            self.shoes,
            self.decks,
            self.rounds_per_shoe,
            self.seed,
            self.start_session_id,
        )
        .map_err(GeneratorArgumentsError::from)
    }
}

fn parse_number<T>(name: &str, value: &str) -> Result<T, GeneratorArgumentsError>
where
    T: std::str::FromStr,
{
    value
        .parse::<T>()
        .map_err(|_| GeneratorArgumentsError(format!("参数 {name} 不是有效整数：{value}")))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("随机百家乐 CSV 生成失败：{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let config = GeneratorConfig::from_args()?;
    if let Some(parent) = config
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let file = File::create(&config.output)?;
    let summary = write_baccarat_csv(file, config.simulation()?)?;
    eprintln!(
        "生成完成：{} 靴，{} 局（闲胜 {}、庄胜 {}、和 {}）\n输出：{}",
        summary.shoes,
        summary.rounds,
        summary.player_wins,
        summary.banker_wins,
        summary.ties,
        config.output.display()
    );
    Ok(())
}

fn usage(message: &str) -> String {
    format!(
        "{message}\n\
用法：cargo run --release --bin generate_baccarat_csv -- <output.csv> \\\n+  [--shoes=100] [--decks=8] [--rounds-per-shoe=60] [--seed=20260902] \\\n+  [--start-session-id=1000000]"
    )
}

#[derive(Debug)]
struct GeneratorArgumentsError(String);

impl From<game_ev_engine::BaccaratSimulationError> for GeneratorArgumentsError {
    fn from(error: game_ev_engine::BaccaratSimulationError) -> Self {
        Self(error.to_string())
    }
}

impl fmt::Display for GeneratorArgumentsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for GeneratorArgumentsError {}
