# 风控模块与真实数据回归设计

状态：设计中
目标：先快速构建“最优有效 EV 跟随检测”和历史数据回放，再考虑实时告警。
相关路线图：PROJECT_PLAN.md、docs/system_design_and_code_quality.md

## 1. 目标与边界

当前最重要的目标不是 CLI，也不是自动下注，而是回答两个问题：

1. 某一局下注发生之前，按照当时已经知道的牌靴状态，Rust 引擎认为哪一个主注的有效 EV 最大？
2. 某个玩家是否连续 10 次实际下注都跟随了这个推荐，并且最终命中？

第一版风控模块只做检测和证据保存，不执行下注、不修改资金、不封禁玩家。

风控告警是异常信号，不是作弊结论。连续命中可能来自随机性、数据偏差、规则配置错误或真实异常，必须保留完整证据供人工复核。

## 2. 推荐架构

~~~mermaid
flowchart LR
    A[业务数据库<br/>只读] --> B[Python 数据适配层]
    B --> C[按时间顺序历史回放]
    C --> D[Rust 概率 / EV / 策略核心]
    D --> E[保存下注前决策快照]
    E --> F[关联玩家实际下注]
    F --> G[结算并更新连续命中状态]
    G --> H[风控告警表]
    H --> I[人工复核或通知]
~~~

### 2.1 各层职责

| 层 | 建议实现 | 职责 |
|---|---|---|
| 规则与计算核心 | Rust | Shoe、发牌规则、概率、基础 EV、返水 EV、最优有效 EV、下注决策 |
| 数据适配与回放 | Python | 查询数据库、字段转换、排序、数据校验、调用 Rust、生成回归报告 |
| 状态与告警 | Python + 数据库 | 维护玩家连续命中状态、去重、阈值判断、保存告警证据 |
| 展示与通知 | 后续实现 | 管理后台、邮件、Webhook、IM 通知 |

这样设计的原因是：Rust 代码保持纯函数和可测试，Python 更适合快速连接现有数据库、处理历史数据和迭代风控规则。

第一版建议使用 PyO3/maturin 暴露 Rust 库，不等待 CLI 完成。CLI 是人工使用入口，而当前目标是批量回放和数据库检测。

## 3. 最关键的时间边界

风控系统必须严格区分三个时间点：

~~~text
上一局结束并更新牌靴
        ↓
下注前：构造当前 Shoe，计算概率、EV 和推荐
        ↓
玩家下注
        ↓
牌局发牌并产生 outcome
        ↓
结算，并把本局牌从 Shoe 中移除
~~~

计算推荐时只能使用下注前已经知道的信息：

- 当前牌靴已经移除的牌；
- 当前赌场的赔付规则；
- 当前玩家适用的返水规则；
- 当前策略的最低有效 EV 门槛。

不能把本局最终发出的牌提前放入 Shoe，也不能用本局 outcome 参与本局下注前的推荐。这是防止“未来信息泄漏”的核心规则。

## 4. “连续 10 次命中”的正式定义

### 4.1 推荐快照

每一局下注前生成一份不可变的 DecisionSnapshot：

~~~text
round_id
shoe_id
round_sequence
decision_time
shoe_state_hash
rules_version
rebate_rule_version
candidate_bet
action
player_base_ev
player_rebate_ev
player_effective_ev
banker_base_ev
banker_rebate_ev
banker_effective_ev
tie_base_ev
tie_rebate_ev
tie_effective_ev
minimum_effective_ev
~~~

其中 candidate_bet 和所有 EV 必须是在 outcome 产生之前计算并保存的，不能在回放结束后根据结果重新推导。

### 4.2 严格命中规则

第一版采用严格规则 EV_FOLLOW_STREAK_10：

一局被计为“命中”，必须同时满足：

1. 引擎决策为 Place，而不是 Skip；
2. 玩家确实在下注截止时间前下注；
3. 玩家下注类型等于 candidate_bet；
4. 本局已正常结算；
5. 下注结果为真正获胜。

主注命中映射：

| 玩家下注 | outcome | 是否命中 |
|---|---|---|
| Player | Player | 是 |
| Banker | Banker | 是 |
| Tie | Tie | 是 |
| Player / Banker | Tie | 否，Push 不算命中 |
| 任意主注 | 其他结果 | 否 |

Skip、没有下注、下注类型不一致、Push、取消局、数据缺失，都不能增加命中次数。

为了保持“连续”的严格含义，以上情况会把当前连续命中数重置为 0。数据缺失不能被当成命中，也不能静默跳过。

### 4.3 两种统计口径同时保存

为了避免后续争论“连续”到底是否跨越未下注局，建议同时保存两种指标：

| 指标 | 定义 | 用途 |
|---|---|---|
| strict_streak | 每一局都必须下注并跟随推荐，任何空缺或不匹配都重置 | 第一版正式告警 |
| bet_only_streak | 只看玩家实际下注的局，未下注局不计入也不重置 | 辅助分析，不直接告警 |

正式告警只使用 strict_streak >= 10，避免玩家只挑选自己想下注的局造成虚假的连续命中。

## 5. 数据模型

以下是逻辑模型，字段名可根据现有业务数据库调整。第一版不要求直接修改业务表，建议建立独立的风控/回归 schema。

### 5.1 round_records

保存标准化后的牌局事实。

| 字段 | 说明 |
|---|---|
| round_id | 业务牌局唯一 ID |
| table_id | 桌台 ID |
| shoe_id | 牌靴 ID；如果原库没有，需要先建立边界识别规则 |
| round_sequence | 牌靴内递增局号 |
| occurred_at | 牌局时间 |
| cards | 本局实际发出的完整牌面 |
| recorded_outcome | 数据库记录的 outcome |
| calculated_outcome | Rust 根据牌面重新计算的 outcome |
| round_status | settled、void、missing 等 |
| rules_version | 本局使用的规则版本 |
| validation_status | 数据校验结果 |

### 5.2 player_round_bets

保存玩家在一局中的实际下注。

| 字段 | 说明 |
|---|---|
| player_id_hash | 脱敏后的玩家 ID，不在代码库保存明文 ID |
| round_id | 关联牌局 |
| bet_type | player、banker、tie 或其他边注 |
| amount | 下注金额 |
| placed_at | 实际下注时间 |
| settlement_status | won、lost、push、void |
| net_profit | 按实际赔付规则计算的净收益 |

### 5.3 engine_decision_snapshots

保存 Rust 在下注前生成的结果。它是回归和告警的主要证据。

| 字段 | 说明 |
|---|---|
| round_id | 牌局 ID |
| policy_id | 策略配置 ID |
| input_hash | 输入牌靴和规则的哈希 |
| engine_version | Rust 引擎版本或 Git commit |
| optimal_bet | 基础 EV 最优下注，可选保留 |
| candidate_bet | 有效 EV 最优下注 |
| action | place 或 skip |
| minimum_effective_ev | 当时使用的门槛 |
| player_metrics_json | Player 完整指标 |
| banker_metrics_json | Banker 完整指标 |
| tie_metrics_json | Tie 完整指标 |
| bankroll | 生成计划时使用的可下注资金 |
| kelly_fraction | 未经过金额上限的完整凯利比例 |
| applied_fraction | 经过资金比例、单局和桌台上限后的实际比例 |
| suggested_amount | 最终建议下注金额；Skip 时为 0 |
| expected_profit | suggested_amount × effective_ev |
| created_at | 决策生成时间 |

同一个牌局如果不同玩家适用不同返水规则，需要按 policy_id 保存多份快照；不能把不同返水方案混在一份结果里。

### 5.4 player_strategy_events

这是把推荐和玩家行为关联后的派生事实。

| 字段 | 说明 |
|---|---|
| player_id_hash | 玩家脱敏 ID |
| round_id | 牌局 ID |
| candidate_bet | 引擎推荐 |
| engine_action | 引擎是否允许下注 |
| actual_bet | 玩家实际下注 |
| follows_candidate | 是否跟随推荐 |
| hit_status | hit、miss、push、skip、invalid |
| strict_streak_after | 本局处理后的严格连续命中数 |
| expected_hit_probability | 推荐下注在下注前的结果概率 |
| effective_ev | 推荐下注当时的有效 EV |

### 5.5 risk_alerts

只保存告警，不把告警当成事实数据覆盖掉。

| 字段 | 说明 |
|---|---|
| alert_id | 告警 ID |
| rule_code | 例如 EV_FOLLOW_STREAK_10 |
| player_id_hash | 玩家脱敏 ID |
| streak_length | 触发时的连续命中数 |
| first_round_id | 连续区间起点 |
| last_round_id | 触发局 |
| evidence_json | 10 局明细和关键指标 |
| status | open、reviewing、dismissed、confirmed |
| created_at | 告警创建时间 |

建议增加唯一约束：

~~~text
(rule_code, player_id_hash, last_round_id)
~~~

这样重复运行回放或重复消费消息时，不会重复创建同一告警。

## 6. 历史回放流程

历史回放是连接真实数据前必须完成的 MVP。它既是风控检测器，也是回归测试工具。

~~~text
读取一张桌台的一靴牌局
        ↓
按 shoe_id、round_sequence 排序
        ↓
创建完整 Shoe
        ↓
处理当前局之前的牌靴快照
        ↓
调用 Rust 计算概率、EV 和策略
        ↓
保存 DecisionSnapshot
        ↓
关联玩家下注并判断是否跟随、是否命中
        ↓
更新 streak 和可能的 alert
        ↓
校验当前局牌面和 outcome
        ↓
从 Shoe 移除本局实际牌面
        ↓
进入下一局
~~~

伪代码：

~~~text
for shoe in shoes_in_time_order:
    shoe_state = full_shoe()
    streak_state = load_or_create_state()

    for round in shoe.rounds_in_sequence:
        validate_round_identity(round)

        # 这里绝对不能先移除 round.cards
        decision = rust_engine.decide(shoe_state, policy)
        save_decision_snapshot(round, decision, shoe_state)

        bets = load_player_bets(round.round_id)
        events = classify_player_bets(bets, decision, round)
        update_streaks(events)
        create_alerts_when_threshold_crossed()

        validate_cards_and_outcome(round)
        shoe_state.remove_many(round.cards)
~~~

如果中间出现重复牌、缺牌、牌局顺序断裂或无法确定牌靴边界，不要继续静默计算。应该把该局标记为 invalid 或 not_computable，并在报告中统计。

## 7. Rust 与 Python 的边界

### 7.1 第一版建议的 Python API

不要让 Python 重写百家乐规则。Python 只传入标准化输入，Rust 返回可序列化结果。

建议暴露一个高层函数：

~~~text
analyze_snapshot(
    consumed_cards,
    deck_count,
    payout_rules,
    rebate_rule,
    minimum_effective_ev,
) -> decision_result
~~~

返回结果至少包含：

~~~json
{
  "engine_version": "...",
  "input_hash": "...",
  "probabilities": {
    "player": 0.0,
    "banker": 0.0,
    "tie": 0.0
  },
  "metrics": {
    "player": {
      "base_ev": 0.0,
      "rebate_ev": 0.0,
      "effective_ev": 0.0
    },
    "banker": {},
    "tie": {}
  },
  "candidate_bet": "banker",
  "action": "place",
  "minimum_effective_ev": 0.0
}
~~~

Python 不应传入本局 outcome 参与 analyze_snapshot。outcome 只在决策保存后用于结算和命中判断。

### 7.2 版本必须进入结果

真实数据回归最怕“代码变了但不知道数字为什么变”。每条决策都应记录：

- Rust engine version；
- 规则版本；
- 返水方案版本；
- 策略门槛版本；
- 输入牌靴状态哈希；
- 回放任务版本。

这样未来改变补牌规则、佣金、返水或最优下注选择时，能够区分“数据变化”和“代码变化”。

## 8. 真实数据校验

回放前先建立数据质量报告，不要直接相信业务数据库中的 outcome。

### 8.1 牌面校验

- 所有牌必须能解析为标准 ASCII 牌面；
- 单局牌数必须符合百家乐发牌路径；
- 同一局不能重复使用同一张牌超过牌靴剩余数量；
- Shoe 的剩余牌数量不能为负；
- 本局结束后由 Rust 根据牌面计算 outcome；
- calculated_outcome 与 recorded_outcome 不一致时进入异常队列。

### 8.2 顺序校验

- 同一 table_id + shoe_id 按 round_sequence 唯一排序；
- round_sequence 不能回退；
- 不同牌靴不能共用牌靴状态；
- 不确定牌靴开始位置时，不生成精确 EV；
- 决策时间必须早于下注时间，下注时间必须早于结算时间。

### 8.3 计算校验

- Player、Banker、Tie 概率和接近 1.0；
- effective_ev = base_ev + rebate_ev；
- Player/Banker 遇 Tie 的返水比例为 0；
- 按当前约定 Tie 注三种结果都能获得返水；
- Place 或 Skip 必须和最低有效 EV 规则一致；
- 同样的牌靴、规则和返水输入重复回放，输出必须一致。

## 9. 回归测试分层

### 9.1 Rust 单元测试

继续覆盖规则和数学不变量：

- 完整八副牌概率基准；
- 小牌靴的人工可计算样例；
- 返水权重；
- effective_ev 与基础 EV 相加；
- BettingPolicy::decide() 的 Place/Skip 边界；
- 严格大于或大于等于门槛的固定语义。

### 9.2 Python 适配测试

使用固定 JSON fixture 测试：

~~~text
输入：已消耗牌 + 规则 + 返水 + 门槛
输出：概率 + 三项 EV + candidate + action
~~~

这层只验证 Python 到 Rust 的数据转换，不重新验证百家乐数学。

### 9.3 真实数据回放测试

每次回放输出一份报告，至少包含：

~~~text
总牌靴数
总牌局数
可计算牌局数
无效牌局数
牌面解析失败数
记录 outcome 与计算 outcome 不一致数
概率/EV 计算失败数
玩家下注关联成功数
严格命中事件数
触发 10 连告警的玩家数
~~~

真实数据报告不能只输出“命中率”。命中率是结果统计，不能替代引擎正确性验证。

### 9.4 Golden 回归

保存少量脱敏后的固定输入和期望输出：

~~~text
tests/fixtures/
  full_shoe_standard.json
  partial_shoe_standard.json
  rebate_player_banker_tie.json
  invalid_duplicate_cards.json
  streak_ten_hits.json
  streak_broken_by_push.json
~~~

真实生产数据不提交到 Git。生产数据只在本地或受控存储中运行，测试 fixture 必须脱敏和缩小。

## 10. 告警状态机

每个玩家、每条策略规则维护一份独立状态：

~~~text
current_strict_streak
first_hit_round_id
last_processed_round_id
last_alerted_streak
state_version
~~~

处理事件：

| 事件 | 动作 |
|---|---|
| 严格命中 | current_strict_streak += 1 |
| 未下注 | 重置为 0 |
| 不跟随推荐 | 重置为 0 |
| 跟随但输 | 重置为 0 |
| Push | 重置为 0 |
| 取消局/无效局 | 标记无法判断，第一版重置并记录原因 |
| 达到 10 | 创建一条 EV_FOLLOW_STREAK_10 告警 |
| 已经告警且继续命中 | 不重复创建同一触发告警，只更新状态 |

告警必须在“从 9 变为 10”的瞬间触发，而不是每次查询都重复触发。

告警证据至少包含连续 10 局：

~~~text
player_id_hash
round_id
round_sequence
decision_time
candidate_bet
actual_bet
effective_ev
expected_hit_probability
recorded_outcome
hit_status
shoe_state_hash
engine_version
~~~

## 11. 风控指标与数学注意事项

第一版只做“连续 10 次命中”规则，但后续应增加基线指标：

- 实际命中率；
- 每局推荐下注的下注前命中概率；
- 实际命中率与期望命中率的差值；
- 跟随推荐比例；
- 被推荐为 Skip 时仍然下注的比例；
- 真实净收益与理论有效 EV 的差异；
- 按玩家、桌台、牌靴和时间窗口分组的结果。

不要用简单的固定二项分布直接估计连续 10 次的异常概率。每局的牌靴状态会变化，每局推荐下注的概率也可能不同。更合理的第一步是保存每局的 expected_hit_probability，再用逐局条件概率乘积或模拟方法估计基线。

同时必须区分：

~~~text
理论有效 EV：下注前的长期期望
实际净收益：已经发生的单局结果
~~~

实际连续盈利不能证明理论 EV 计算错误；理论 EV 为正也不能保证短期盈利。

## 12. MVP 开发顺序

### Slice A：先修复并稳定策略核心

- [x] 修复 BetDecision.rebate_ev 字段赋值；
- [x] 固定最低 EV 使用 >= 还是 >；
- [x] 为 BettingPolicy::decide() 写 Place/Skip 测试；
- [x] 从 crate 根路径导出 BettingPolicy、BetDecision、BetAction、SkipReason；
- [x] 实现完整凯利比例、庄六点拆分、单局/桌台金额上限和 BetPlan；
- [x] 为凯利公式、Push、免佣庄、策略跳过和金额上限补充单元测试；
- [x] 为决策结果补齐可序列化的稳定结构。

### Slice B：建立 Python 调用边界

- [ ] 使用 PyO3/maturin 暴露 analyze_snapshot()；
- [ ] 输入只包含下注前牌靴和规则；
- [ ] 输出包含 engine version、input hash、指标和 action；
- [ ] 用固定 JSON fixture 做 Python 适配测试。

### Slice C：离线真实数据回放

- [ ] 定义标准化 round_records 和 player_round_bets；
- [ ] 使用只读数据库账号；
- [ ] 按桌台、牌靴、局号顺序回放；
- [ ] 保存每局下注前决策快照；
- [ ] 生成数据质量和回归报告；
- [ ] 不合格数据进入 quarantine，不静默跳过。

### Slice D：连续命中风控

- [ ] 实现 strict_streak 状态机；
- [ ] 实现 EV_FOLLOW_STREAK_10；
- [ ] 保存 10 局完整证据；
- [ ] 使用唯一键保证告警幂等；
- [ ] 用固定 fixture 测试命中、Push 重置和不跟随重置。

### Slice E：实时化

- [ ] 把离线回放的输入接口替换为增量轮询或消息队列；
- [ ] 保留同一套标准化事件和状态机；
- [ ] 增加延迟、重复消息、乱序消息处理；
- [ ] 最后再接通知和后台页面。

## 13. 第一版明确不做的事情

- 本阶段只输出可审计的 BetPlan，不直接连接真实下注接口；
- 不自动封禁玩家；
- 不根据结果反推下注前推荐；
- 不把单纯的百家乐结果连胜当作玩家命中 EV；
- 不把未经验证的生产数据直接提交到 Git；
- 不在 Rust 和 Python 各写一套百家乐规则；
- 不在缺少牌靴边界或本局牌面的情况下伪造精确概率。

## 14. 当前最具体的下一步

按照最快闭环，下一次开发只做下面三件事：

1. 为决策快照增加下注前牌靴的稳定 input hash；
2. 使用 PyO3/maturin 暴露现有 `analyze_snapshot()` 高层入口；
3. 准备一份脱敏的真实数据最小样本，先完成单牌靴离线回放，不接实时告警或真实下注接口。

单牌靴回放能够正确完成后，再扩展到多桌台、多玩家和 10 连命中告警。这样每一步都有可验证的中间结果，出现异常时也能判断是数据、牌靴重建、Rust 计算还是风控状态机的问题。
