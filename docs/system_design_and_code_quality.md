# 系统设计与高质量代码学习笔记

> 这篇笔记结合当前的 game-ev-engine 项目，记录如何从“能运行的代码”逐步设计成“边界清楚、容易测试、方便扩展的系统”。

## 0. 三个核心观点

1. 系统不是功能的堆积，而是一条清晰的数据流。
2. 代码质量的核心不是代码越多越好，而是每个模块只负责一件明确的事。
3. 先把业务事实和不变量建模清楚，再选择框架、数据库和通信方式。

当前项目的主线是：

~~~text
牌面输入
    ↓
牌靴状态 Shoe
    ↓
枚举所有可能回合
    ↓
Player / Banker / Tie 概率
    ↓
赔付规则
    ↓
EV、House Edge、RTP、最优下注
~~~

只要这条数据流清楚，CLI、Python 接口和风控告警都只是不同的入口或出口。

---

## 1. 设计一个系统的顺序

### 1.1 先定义业务事实

先回答：系统里真实存在什么东西？

当前项目的业务对象包括：

- Card：一张具体扑克牌；
- Shoe：当前牌靴状态；
- RoundOutcome：一局的结果；
- OutcomeWeights：枚举得到的精确权重；
- MainBetRules：主注赔付规则；
- MainBetAnalysis：一次完整分析结果。

这些对象不是为了“看起来像面向对象”，而是为了把业务概念和约束放到代码中。

例如：

- 一张牌必须有牌面和花色；
- 八副牌最多只能有八张 AS；
- 牌靴剩余数量不能为负数；
- 概率权重之和必须等于总权重；
- RTP = 1 + EV；
- 最优下注是 EV 最大的下注，不一定代表一定盈利。

这些就是业务不变量。

### 1.2 定义输入和输出

CLI 分析功能的输入输出可以定义为：

~~~text
输入：analyze consumed AS 9H KD
输出：三种下注的概率、EV、House Edge、RTP 和最优下注
~~~

当前采用两种输入模式：

~~~text
analyze consumed <已消耗牌...>
analyze remaining <全部剩余牌...>
~~~

必须明确：

- consumed 是部分信息，可以只输入已经看到的牌；
- remaining 是完整集合，必须输入当前所有剩余牌；
- 两种输入不能混用；
- 牌必须是具体牌，例如 AS、10H、KD。

### 1.3 画出数据流

写函数以前，先写出：

~~~text
原始参数
    ↓ 解析
结构化输入 AnalyzeInput
    ↓ 构造
Shoe
    ↓ 计算
MainBetAnalysis
    ↓ 展示
终端文本 / JSON / Python 对象
~~~

如果一个函数同时跨越多个阶段，例如既解析参数、又扣牌、又打印结果，通常说明职责混在了一起。

### 1.4 最后选择技术

技术选择应该服从业务，而不是先决定“我要用某个框架”。

当前项目的合理顺序：

1. 先用 Rust 标准库完成 CLI；
2. 核心计算稳定后，再增加 Python 绑定；
3. 需要持久化时，再增加数据库；
4. 需要实时告警时，再增加事件处理和通知系统。

这叫渐进式设计：先解决当前问题，不为还不存在的问题提前增加复杂度。

---

## 2. 当前项目如何分层

目标架构：

~~~text
┌──────────────────────────────────────────┐
│  入口层：CLI / Python / HTTP / 风控任务     │
└──────────────────┬───────────────────────┘
                   ↓
┌──────────────────────────────────────────┐
│  应用编排层：输入 → 构造牌靴 → 分析 → 返回   │
└──────────────────┬───────────────────────┘
                   ↓
┌──────────────────────────────────────────┐
│  领域核心：Card / Shoe / Round / EV       │
└──────────────────┬───────────────────────┘
                   ↓
┌──────────────────────────────────────────┐
│  基础设施：数据库 / 文件 / 日志 / 通知       │
└──────────────────────────────────────────┘
~~~

当前代码大致对应：

| 层 | 当前文件 | 主要职责 |
|---|---|---|
| 领域模型 | card.rs | 牌面、花色、牌的解析 |
| 领域状态 | shoe.rs | 牌靴数量和扣牌不变量 |
| 领域规则 | baccarat/hand.rs、rule.rs、round.rs | 手牌、补牌、回合规则 |
| 计算核心 | baccarat/point_enumerate.rs、probability.rs、ev.rs | 概率和 EV |
| 结果门面 | baccarat/analysis.rs | 对外提供一次完整分析 |
| 入口适配 | cli.rs、main.rs | 读取参数和展示结果 |

以后增加 Python 或数据库时，尽量让它们调用已有的领域接口，不要复制一份概率和 EV 算法。

---

## 3. CLI 的正确数据流

CLI 不应该直接参与概率计算，只负责把用户文字转换成核心层能理解的类型。

~~~text
std::env::args()
    ↓
parse_args()
    ↓
Command::Analyze(AnalyzeInput)
    ↓
build_shoe()
    ↓
Shoe
    ↓
analyze_main_bets()
    ↓
MainBetAnalysis
    ↓
print_analysis()
~~~

每个函数的职责：

### parse_args

只负责：

~~~text
String → CardSource + Vec<Card>
~~~

它不应该创建 Shoe、扣牌、计算概率或打印最终分析结果。

### build_shoe

只负责：

~~~text
AnalyzeInput → Shoe
~~~

它根据 CardSource 选择：

- Consumed：创建完整牌靴，再调用 remove_many；
- Remaining：根据完整剩余牌集合创建牌靴。

### analyze_main_bets

只负责核心计算：

1. 枚举可能的发牌路径；
2. 生成三种结果权重；
3. 根据赔付规则计算 EV；
4. 选择 EV 最大的下注。

### print_analysis

只负责展示结果，不重新计算业务公式。

例如不要在 CLI 中再次计算 House Edge 和 RTP。它们已经属于 BetMetrics，CLI 直接读取现有方法即可。

---

## 4. 为什么使用枚举和结构体

### 4.1 用枚举表示有限选择

~~~rust
pub enum CardSource {
    Consumed,
    Remaining,
}
~~~

这比使用布尔值更清晰：

~~~rust
is_remaining: bool
~~~

布尔值只能表达“是或不是”，枚举直接表达业务概念。这属于类型驱动设计：尽量让类型表达规则，让编译器帮助检查错误。

### 4.2 用结构体绑定相关数据

~~~rust
pub struct AnalyzeInput {
    pub source: CardSource,
    pub cards: Vec<Card>,
}
~~~

source 和 cards 永远作为一组输入传递，不会出现函数参数顺序混乱。

### 4.3 不要过早使用 trait

如果当前只有两种输入模式，直接使用 match 就足够：

~~~rust
match input.source {
    CardSource::Consumed => { /* ... */ }
    CardSource::Remaining => { /* ... */ }
}
~~~

只有当未来出现很多种来源，并且它们拥有相同的行为时，才考虑抽象成 trait。

原则是：先有重复的真实需求，再做抽象。

---

## 5. 如何设计错误处理

错误应该按发生的层次建模：

~~~text
参数错误       → CliError
牌面格式错误   → CardParseError
牌靴数量错误   → ShoeError
概率计算错误   → ProbabilityError
~~~

不要把所有错误都变成“发生错误”，因为调用者无法判断下一步应该怎么处理。

Rust 中优先使用：

~~~rust
Result<T, E>
~~~

并用问号运算符传播错误：

~~~rust
let shoe = build_shoe(input)?;
let analysis = analyze_main_bets(&shoe, rules)?;
~~~

unwrap 和 expect 只适合测试中确定一定成立的条件，或经过证明不可能失败的内部不变量。用户输入、文件、数据库、网络数据都不应该直接 unwrap。

---

## 6. 如何写出更好的函数

### 6.1 一个函数只回答一个问题

~~~rust
fn build_shoe(input: &AnalyzeInput) -> Result<Shoe, CliError>
~~~

它只回答：

> 根据输入构造什么牌靴？

不要让一个函数同时读取命令行、解析牌面、修改牌靴、枚举概率和打印结果。

### 6.2 使用有意义的名字

~~~rust
let consumed_cards = ...;
let remaining_shoe = ...;
let optimal_bet = ...;
~~~

好名字可以减少注释需求。

### 6.3 注释解释“为什么”

不太有价值：

~~~rust
// 加一
count += 1;
~~~

更有价值：

~~~rust
// remaining 模式传入的是完整剩余牌集合，因此从零开始计数，
// 而不是从完整牌靴扣除。
count += 1;
~~~

代码说明“做了什么”，注释说明“为什么这样做”。

### 6.4 保护不变量

不要允许外部直接修改牌靴计数，而应该通过：

~~~rust
shoe.remove(card)?;
shoe.restore(card)?;
Shoe::from_remaining(decks, cards)?;
~~~

这叫封装。状态的创建和修改集中在少数几个入口，可以防止非法状态扩散。

### 6.5 让核心函数尽量纯粹

理想的核心函数是：

~~~text
输入相同
    ↓
输出相同
~~~

例如 analyze_main_bets 只接收 Shoe 和规则，不自己读取数据库、环境变量或终端。

这属于 Functional Core / Imperative Shell：

- 外壳负责输入输出和副作用；
- 核心负责确定性的业务计算。

好处是核心容易测试，也容易被 Python、Web 和风控服务复用。

---

## 7. 测试如何设计

测试不是最后才补，而是用来确认每一层的契约。

### 7.1 单元测试

测试一个函数的局部规则：

- Card 能否解析 AS；
- Shoe::remove 是否减少数量；
- from_remaining 是否正确累计重复牌；
- 补牌规则是否正确；
- EV 公式是否正确。

### 7.2 集成测试

测试模块之间的连接：

~~~text
Shoe → calculate_main_outcomes → OutcomeWeights
~~~

例如完整八副牌应保持已知基线概率。

### 7.3 CLI 测试

测试真实输入：

- analyze consumed AS 9H KD；
- analyze remaining 牌列表；
- 缺少命令；
- 未知输入类型；
- 非法牌面；
- 牌数量超过容量。

### 7.4 测试边界

必须考虑空输入、牌靴只剩五张牌、同一张牌重复超过容量、权重溢出和数据库重复记录。

### 7.5 浮点数比较

不要直接比较浮点数相等：

~~~rust
assert_eq!(actual, expected);
~~~

使用误差：

~~~rust
assert!((actual - expected).abs() < 1e-12);
~~~

金额、账务和结算数据以后应优先考虑整数最小单位或定点数；概率和展示指标可以使用 f64，但要明确误差范围。

---

## 8. 未来风控告警系统如何设计

目标是发现：

> 某个玩家是否连续多局选择了当时 EV 最大的下注。

首先要把“命中”定义清楚。不要只叫 hit，因为它可能表示赢牌，也可能表示命中最优建议。建议使用明确字段：

~~~text
optimal_bet_match
~~~

### 8.1 推荐数据流

~~~text
牌局记录 / 玩家下注记录
        ↓
数据校验
        ↓
取得下注前的牌靴快照
        ↓
计算当局 optimal_bet
        ↓
比较玩家实际下注与 optimal_bet
        ↓
更新玩家连续命中 streak
        ↓
达到 10 局则生成告警
        ↓
去重后发送通知
~~~

### 8.2 每局建议保存的字段

~~~text
player_id
round_id
round_time
chosen_bet
player_probability
banker_probability
tie_probability
player_ev
banker_ev
tie_ev
optimal_bet
optimal_ev
optimal_bet_match
shoe_snapshot_id
calculation_version
~~~

重点是保存计算快照和版本，而不是只保存最终的 optimal_bet。否则规则改变时，无法解释当时为什么产生告警。

### 8.3 连续计数逻辑

按 player_id 分组，并按牌局顺序处理：

~~~text
如果 optimal_bet_match = true：
    current_streak += 1
否则：
    current_streak = 0

如果 current_streak >= 10：
    生成告警
~~~

还要定义：

- 连续是按玩家自己的下注局，还是全局牌局；
- 中间没有下注是否中断；
- 同一局重复上报如何处理；
- 第 10 局报一次，还是之后每局都报；
- 规则版本改变后是否重新计数。

### 8.4 推荐架构

第一版可以使用定时任务：

~~~text
数据库
    ↓ 每分钟查询新增记录
风控计算服务
    ↓
连续计数和告警状态表
    ↓
通知服务
~~~

数据量增大后再升级为：

~~~text
牌局事件 → 消息队列 → 风控消费者 → 告警表 → 通知消费者
~~~

不要一开始就引入消息队列。先验证规则和数据质量，再根据吞吐量升级。

### 8.5 风控系统的可靠性

- round_id 建立唯一约束，保证幂等；
- 告警记录保存 streak_start 和 streak_end；
- 计算失败进入错误队列，不能静默丢失；
- 规则、赔付配置和算法版本可追溯；
- 数据库时间和牌局顺序统一；
- 通知失败需要重试，但不能无限重复发送。

---

## 9. 当前项目的渐进式路线

### Phase 5：CLI

1. parse_args；
2. build_shoe；
3. 调用 analyze_main_bets；
4. 打印完整结果；
5. 补 CLI 测试；
6. 再考虑 JSON 输出。

### Phase 6：Python 调用

1. 稳定 Rust 公共 API；
2. 明确 Python 输入输出结构；
3. 把 Rust 错误转换成 Python 异常；
4. 用 Python 测试完整牌靴和特殊牌靴；
5. 保证 Python 不需要理解枚举内部细节。

### Phase 7：数据持久化

1. 设计牌局和下注表；
2. 保存牌靴快照或可重放的消耗牌；
3. 保存算法和规则版本；
4. 增加幂等约束；
5. 增加数据校验。

### Phase 8：风控告警

1. 定义 optimal_bet_match；
2. 实现连续计数；
3. 实现告警去重；
4. 增加通知重试；
5. 再根据数据量考虑消息队列。

---

## 10. 每次写代码前后的检查清单

### 写之前

- 这个功能的输入是什么？
- 输出是什么？
- 哪些条件属于非法输入？
- 哪些数据必须始终保持一致？
- 逻辑属于 CLI、应用层还是领域核心？
- 是否已经有函数可以复用？

### 写之后

- 函数是否只负责一个职责？
- 是否能用类型表达业务概念？
- 是否存在 unwrap 处理用户输入？
- 错误是否说明具体原因？
- 正常路径和边界路径是否都有测试？
- 是否重复实现了已有公式？
- 核心逻辑是否依赖终端、数据库或环境变量？
- 是否运行了：

~~~powershell
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
~~~

## 总结

写系统时，先画数据流，再划分职责；写代码时，先定义类型和错误，再实现正常路径；写完以后，用测试验证边界。

当前项目最重要的架构原则：

> CLI 负责接收输入，领域核心负责计算，基础设施负责保存和传输；任何一层都不要越权替代另一层的职责。

