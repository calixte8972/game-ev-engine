# Rust 模块系统入门笔记

## 适用范围

这篇笔记面向刚开始学习 Rust 的开发者，并且完全结合当前的 game-ev-engine 项目讲解。

重点回答几个问题：

- package、crate、module 分别是什么；
- src/lib.rs 和 src/main.rs 有什么区别；
- mod、pub mod、use、pub use 分别做什么；
- crate、super、self 分别表示哪里；
- Rust 如何根据文件位置建立模块树；
- 当前项目中的模块如何互相调用；
- 如何设计公开 API，避免模块互相混乱；
- 常见模块错误如何排查。

---

## 1. 先建立三个概念

刚学习 Rust 时，最容易混淆的是 package、crate 和 module。

可以先用这个类比：

~~~text
package = 一个 Cargo 项目
crate   = 这个项目编译出来的一个库或程序
module  = crate 内部的命名空间和代码组织单元
~~~

### 1.1 package：Cargo 项目

当前整个目录：

~~~text
C:\Users\Lenovo\RustroverProjects\game-ev-engine
~~~

就是一个 package。

它由 Cargo.toml 描述：

~~~text
Cargo.toml
src/
tests/
benches/
docs/
~~~

Cargo.toml 中的 package name 是：

~~~toml
name = "game-ev-engine"
~~~

package 负责管理：

- 项目名称；
- 版本；
- 依赖；
- 编译目标；
- library 和 binary 的配置。

### 1.2 crate：一次编译的库或程序

当前 package 同时包含两个 crate：

~~~text
library crate：src/lib.rs
binary crate ：src/main.rs
~~~

library crate 编译成可复用的 Rust 库。

binary crate 编译成可以运行的程序。

因此当前项目既可以被其他 Rust 代码调用，也可以编译成命令行程序。

### 1.3 module：crate 内部的代码空间

module 用来：

- 组织代码；
- 划分命名空间；
- 控制可见性；
- 隐藏内部实现；
- 公开稳定接口。

例如：

~~~rust
pub mod card;
pub mod shoe;
pub mod baccarat;
~~~

这三行表示 library crate 中有三个模块：

~~~text
crate
├── card
├── shoe
└── baccarat
~~~

模块本身不会自动执行代码。它主要是在编译期建立一棵代码和名称的树。

---

## 2. 当前项目的模块树

当前项目的 source 目录大致是：

~~~text
src/
├── lib.rs
├── main.rs
├── card.rs
├── shoe.rs
├── cli.rs
└── baccarat/
    ├── mod.rs
    ├── hand.rs
    ├── enumerate.rs
    ├── point_enumerate.rs
    ├── ev.rs
    ├── analysis.rs
    ├── bet.rs
    ├── probability.rs
    ├── round.rs
    └── rule.rs
~~~

对应的 library 模块树是：

~~~text
crate
├── card
├── shoe
├── cli
└── baccarat
    ├── hand
    ├── enumerate
    ├── point_enumerate
    ├── analysis
    ├── bet
    ├── ev
    ├── probability
    ├── round
    └── rule
~~~

但是，文件存在不等于模块已经存在。

Rust 只有在某个父模块中看到 mod 声明后，才会把对应文件加入模块树。

例如 lib.rs 中有：

~~~rust
pub mod card;
pub mod shoe;
pub mod baccarat;
pub mod cli;
~~~

因此 Rust 才会加载：

~~~text
src/card.rs
src/shoe.rs
src/baccarat/mod.rs
src/cli.rs
~~~

如果把 src/new_file.rs 放进项目，但没有在父模块中写：

~~~rust
mod new_file;
~~~

那么 Rust 不会自动编译它，也不能从其他地方使用它。

---

## 3. mod 到底是什么

mod 有两个主要作用：

1. 声明一个模块；
2. 告诉 Rust 去哪里加载模块代码。

### 3.1 在 lib.rs 中声明文件模块

~~~rust
pub mod card;
~~~

这句话可以理解为：

~~~text
声明一个名为 card 的模块，
模块代码位于 src/card.rs，
并且允许 crate 外部访问它。
~~~

如果写成：

~~~rust
mod card;
~~~

模块仍然存在，但外部调用者不能访问：

~~~rust
game_ev_engine::card::Card
~~~

因为 card 模块不是公开的。

### 3.2 mod 不等于 use

这是初学者最容易混淆的地方。

~~~rust
mod card;
~~~

表示：

> 声明并加载 card 模块。

~~~rust
use crate::card::Card;
~~~

表示：

> 把已经存在的 Card 名称引入当前作用域。

可以这样理解：

~~~text
mod = 建立房间
use = 进入房间后，把某个东西带到当前桌面
~~~

只写 use 不能代替 mod。

### 3.3 文件模块的路径规则

当 lib.rs 中写：

~~~rust
mod card;
~~~

Rust 会寻找：

~~~text
src/card.rs
或者
src/card/mod.rs
~~~

当 baccarat/mod.rs 中写：

~~~rust
pub mod hand;
~~~

Rust 会寻找：

~~~text
src/baccarat/hand.rs
或者
src/baccarat/hand/mod.rs
~~~

所以当前项目中：

~~~rust
// src/lib.rs
pub mod baccarat;
~~~

对应：

~~~text
src/baccarat/mod.rs
~~~

而：

~~~rust
// src/baccarat/mod.rs
pub mod hand;
~~~

对应：

~~~text
src/baccarat/hand.rs
~~~

---

## 4. pub、pub mod 和普通 mod

Rust 中的项目默认是私有的。

### 4.1 普通 mod

~~~rust
mod probability;
~~~

表示 probability 模块只在当前 crate 内部可见，外部使用者不能直接访问：

~~~rust
game_ev_engine::baccarat::probability
~~~

### 4.2 pub mod

~~~rust
pub mod hand;
~~~

表示 hand 模块对 crate 外部公开。

外部代码可以访问：

~~~rust
game_ev_engine::baccarat::hand::BaccaratHand
~~~

### 4.3 pub(crate)

当前项目中有：

~~~rust
pub(crate) use round::resolve_point_round;
~~~

pub(crate) 的意思是：

> 对当前 crate 内部公开，但不对 crate 外部公开。

这适合内部模块共享，但不想把它放入公共 API 的情况。

例如 resolve_point_round 只是 round 模块和百家乐内部实现使用的辅助函数，就不应该让 Python 或外部 Rust 调用者直接依赖它。

### 4.4 可见性是逐层限制的

如果路径中的任何一层是私有的，外部就无法访问。

例如：

~~~rust
mod baccarat;
pub mod hand;
~~~

即使 hand 是 pub，外部仍然无法访问，因为 baccarat 本身是私有的。

公开路径的每一层都需要允许访问。

---

## 5. 当前项目中的 lib.rs

当前 src/lib.rs 是 library crate 的根模块。

它做了三件事：

1. 声明顶层模块；
2. 组织公共 API；
3. 提供 crate 根目录下的快捷名称。

### 5.1 声明模块

~~~rust
pub mod card;
pub mod shoe;
pub mod baccarat;
pub mod cli;
~~~

这建立了：

~~~text
crate::card
crate::shoe
crate::baccarat
crate::cli
~~~

### 5.2 crate 根路径

在当前 library crate 内部：

~~~rust
crate
~~~

表示当前 library crate 的根，也就是 src/lib.rs。

例如 cli.rs 中：

~~~rust
use crate::{Card, DEFAULT_DECKS, Shoe, ShoeError};
~~~

意思是从 lib.rs 的根路径取出这些名称。

如果没有 lib.rs 中的 pub use，也可以写成：

~~~rust
use crate::card::Card;
use crate::shoe::{Shoe, ShoeError};
~~~

当前项目选择了较短的形式，是因为 lib.rs 重新导出了这些类型。

### 5.3 lib.rs 中的 pub use

当前有：

~~~rust
pub use card::{Card, CardParseError, Rank, Suit};
pub use shoe::{DEFAULT_DECKS, MAX_DECKS, MIN_DECKS, Shoe, ShoeError};
~~~

这表示：

~~~text
原始位置：
game_ev_engine::card::Card

同时提供快捷位置：
game_ev_engine::Card
~~~

这叫 re-export，也就是重新导出。

百家乐模块也做了类似的事情：

~~~rust
pub use baccarat::{
    BaccaratHand,
    MainBet,
    MainBetRules,
    MainBetAnalysis,
    analyze_main_bets,
};
~~~

因此外部调用者可以写：

~~~rust
use game_ev_engine::{MainBetRules, Shoe, analyze_main_bets};
~~~

而不需要记住具体实现文件：

~~~rust
use game_ev_engine::baccarat::analysis::analyze_main_bets;
use game_ev_engine::shoe::Shoe;
~~~

这就是公共 API 和内部文件布局的分离。

---

## 6. 当前项目中的 baccarat/mod.rs

src/baccarat/mod.rs 是 baccarat 模块的根。

它首先声明子模块：

~~~rust
pub mod hand;

mod point_enumerate;
mod analysis;
mod bet;
mod ev;
mod probability;
mod round;
mod rule;
~~~

### 6.1 为什么 hand 是 pub

hand 模块中的 BaccaratHand 可能是外部调用者需要使用的领域类型，所以写成：

~~~rust
pub mod hand;
~~~

### 6.2 为什么其他模块使用普通 mod

例如：

~~~rust
mod point_enumerate;
mod probability;
mod rule;
~~~

这些模块的具体文件布局属于内部实现。调用者只需要知道：

~~~rust
calculate_main_outcomes(...)
analyze_main_bets(...)
player_should_draw(...)
~~~

不需要直接依赖 point_enumerate.rs 或 probability.rs 的文件位置。

### 6.3 通过 pub use 设计统一接口

~~~rust
pub use analysis::{BetMetrics, MainBetAnalysis, analyze_main_bets};
pub use bet::{BankerPayoutRule, MainBet, MainBetRules};
pub use ev::MainBetEv;
pub use hand::BaccaratHand;
pub use point_enumerate::calculate_main_outcomes;
pub use probability::{OutcomeWeights, ProbabilityError};
pub use round::{RoundError, RoundOutcome, RoundResult, resolve_round};
pub use rule::{banker_should_draw, player_should_draw};
~~~

这段代码的意义是：

~~~text
内部实现文件很多
        ↓
baccarat/mod.rs 统一选择公开哪些名称
        ↓
外部调用者只依赖稳定接口
~~~

如果以后把 point_enumerate.rs 改名为 exact_enumeration.rs，只要 mod.rs 的公开函数不变，外部调用者不需要修改。

这是一种很重要的模块设计思想：

> 公共 API 稳定，内部文件可以变化。

### 6.4 测试专用模块

当前有：

~~~rust
#[cfg(test)]
mod enumerate;
~~~

意思是：

> 只有运行测试时才加载 enumerate 模块。

所以 src/baccarat/enumerate.rs 是测试基准代码，不会进入正常生产构建。

#[cfg(test)] 是条件编译属性。它可以减少生产构建内容，也能把只服务于测试的代码隔离开。

---

## 7. use 的几种写法

### 7.1 引入一个类型

~~~rust
use crate::Card;
~~~

以后可以直接写：

~~~rust
let card: Card = ...;
~~~

不需要每次写完整路径。

### 7.2 引入多个类型

~~~rust
use crate::{Card, Shoe, ShoeError};
~~~

等价于：

~~~rust
use crate::Card;
use crate::Shoe;
use crate::ShoeError;
~~~

### 7.3 引入模块本身

main.rs 中：

~~~rust
use game_ev_engine::cli;
~~~

之后调用：

~~~rust
cli::parse_args(&args)
cli::build_shoe(&input)
~~~

### 7.4 使用完整路径，不引入名称

也可以直接写：

~~~rust
let args = std::env::args();
~~~

这里没有 use std::env，而是直接写完整路径。

一般来说：

- 使用次数少时，完整路径比较明确；
- 使用次数多时，可以 use 缩短代码；
- 名称容易冲突时，保留完整路径更清晰。

### 7.5 as 重命名

如果两个模块中有同名类型，可以：

~~~rust
use module_a::Config as GameConfig;
use module_b::Config as DatabaseConfig;
~~~

这样可以避免名称冲突。

---

## 8. crate、super、self 分别是什么

这三个关键字都表示路径位置。

### 8.1 crate：当前 crate 的根

在当前 library crate 中：

~~~rust
use crate::Shoe;
~~~

表示从 src/lib.rs 的根开始找 Shoe。

当前项目的 lib.rs 又通过 pub use 导出了 Shoe，所以可以找到它。

如果没有 re-export，就需要写：

~~~rust
use crate::shoe::Shoe;
~~~

### 8.2 super：父模块

在 src/baccarat/analysis.rs 中：

~~~rust
use super::{
    MainBet,
    MainBetEv,
    MainBetRules,
    OutcomeWeights,
};
~~~

analysis.rs 所在的模块是 baccarat，所以 super 指的是：

~~~text
analysis
  ↑ super
baccarat
~~~

也就是从 src/baccarat/mod.rs 暴露的名称中寻找这些类型。

### 8.3 self：当前模块

self 表示当前模块。

例如：

~~~rust
use self::helper::calculate;
~~~

表示从当前模块下的 helper 子模块中寻找 calculate。

很多情况下 self 可以省略，所以常见写法是：

~~~rust
use helper::calculate;
~~~

### 8.4 三者的图示

~~~text
crate
└── baccarat
    └── analysis

在 analysis 中：

crate  = 整个 library crate
super  = baccarat
self   = analysis
~~~

---

## 9. main.rs 和 lib.rs 的区别

当前项目同时有：

~~~text
src/lib.rs
src/main.rs
~~~

它们不是同一个模块根。

### 9.1 lib.rs 是 library crate 根

lib.rs 放置：

- Card；
- Shoe；
- baccarat 规则；
- 概率枚举；
- EV 计算；
- 可复用公共 API。

这些内容可以被：

- main.rs；
- Python 绑定；
- 集成测试；
- 其他 Rust 程序；

复用。

### 9.2 main.rs 是 binary crate 根

main.rs 负责：

- 获取命令行参数；
- 调用库函数；
- 打印结果；
- 设置程序退出码。

当前 main.rs 中：

~~~rust
use game_ev_engine::cli;
~~~

这里的 game_ev_engine 是 library crate 的名称，不是当前 binary crate 的 crate 根。

所以：

~~~rust
game_ev_engine::cli::parse_args(...)
~~~

表示从 library crate 调用 CLI 模块。

### 9.3 为什么 main.rs 可以调用 lib.rs

Cargo 发现 package 同时有 lib.rs 和 main.rs 后，会先编译 library crate，再让 binary crate 依赖这个 library crate。

可以理解为：

~~~text
main.rs
  ↓ 调用
game_ev_engine library
  ↓
Card / Shoe / baccarat / cli
~~~

这正是当前项目“核心库 + CLI 程序”的结构。

### 9.4 crate 在两个文件中的含义差异

在 library 模块中：

~~~rust
crate::Shoe
~~~

指向 lib.rs 根。

在 main.rs 中如果写：

~~~rust
crate::something
~~~

指向的是 binary crate 根，而不是 library crate 根。

这也是为什么 main.rs 通常使用：

~~~rust
use game_ev_engine::Shoe;
~~~

而 library 内部模块使用：

~~~rust
use crate::Shoe;
~~~

---

## 10. 当前 cli.rs 的模块位置

当前 lib.rs 中有：

~~~rust
pub mod cli;
~~~

所以当前 cli.rs 属于 library crate 的公开模块：

~~~text
game_ev_engine::cli
~~~

main.rs 通过：

~~~rust
use game_ev_engine::cli;
~~~

调用它。

这套写法目前可以正常工作，但从分层角度看，CLI 通常属于 binary 的入口适配层，不一定需要成为 library 的公共 API。

以后如果希望把 CLI 完全作为二进制内部模块，可以改成：

~~~rust
// src/main.rs
mod cli;
~~~

然后在 cli.rs 中使用：

~~~rust
use game_ev_engine::{Card, Shoe, ShoeError};
~~~

这时：

~~~text
CLI 只属于 main binary
核心 Card、Shoe、分析函数仍属于 library
~~~

但是当前学习阶段不需要立刻重构。先理解两种模块树的区别即可。

---

## 11. 一个真实的调用路径

执行命令：

~~~text
cargo run -- analyze consumed AS 9H KD
~~~

数据和模块的流向如下：

~~~text
main.rs
  │
  │ use game_ev_engine::cli
  ↓
game_ev_engine::cli::parse_args
  │
  │ cli.rs 中 use crate::{Card, ...}
  ↓
Card::from_str
  │
  ↓
AnalyzeInput
  │
  ↓
game_ev_engine::cli::build_shoe
  │
  │ 调用 Shoe::default 和 Shoe::remove_many
  ↓
Shoe
  │
  ↓
analyze_main_bets
  │
  │ 来自 baccarat::analysis
  ↓
calculate_main_outcomes
  │
  ↓
MainBetAnalysis
  │
  ↓
main.rs 打印结果
~~~

这里要区分两种关系：

### 模块关系

~~~text
lib.rs
└── cli.rs
~~~

表示代码在哪里、名称属于哪个命名空间。

### 函数调用关系

~~~text
parse_args
    ↓
build_shoe
    ↓
analyze_main_bets
    ↓
calculate_main_outcomes
~~~

表示程序运行时实际执行了什么。

模块关系是静态结构，函数调用关系是运行流程。两者不要混淆。

---

## 12. pub use 为什么重要

假设内部文件如下：

~~~text
baccarat/
├── point_enumerate.rs
├── probability.rs
└── analysis.rs
~~~

如果不做 re-export，外部调用者可能需要写：

~~~rust
use game_ev_engine::baccarat::analysis::analyze_main_bets;
use game_ev_engine::baccarat::point_enumerate::calculate_main_outcomes;
~~~

这会暴露内部文件布局。

当前项目通过 mod.rs 重新导出：

~~~rust
pub use analysis::analyze_main_bets;
pub use point_enumerate::calculate_main_outcomes;
~~~

调用者就可以写：

~~~rust
use game_ev_engine::baccarat::{
    analyze_main_bets,
    calculate_main_outcomes,
};
~~~

进一步，lib.rs 又把它们提升到 crate 根：

~~~rust
pub use baccarat::{analyze_main_bets, calculate_main_outcomes};
~~~

最终外部代码可以写：

~~~rust
use game_ev_engine::{
    analyze_main_bets,
    calculate_main_outcomes,
};
~~~

这是一种 API 门面设计：

~~~text
内部模块很多
    ↓
mod.rs 选择公开接口
    ↓
lib.rs 提供最常用快捷入口
~~~

好处是：

- 调用者路径更短；
- 内部文件可以重构；
- 不容易让外部依赖内部辅助函数；
- 公共接口更加集中。

---

## 13. 常见错误和排查方法

### 13.1 file not found for module

错误通常类似：

~~~text
file not found for module xxx
~~~

排查：

1. 是否写了 mod xxx；
2. 文件是否位于正确的父目录；
3. 文件名是否是 xxx.rs；
4. 如果是目录模块，是否存在 xxx/mod.rs。

例如 baccarat/mod.rs 中：

~~~rust
mod probability;
~~~

必须存在：

~~~text
src/baccarat/probability.rs
~~~

### 13.2 module is private

错误类似：

~~~text
module xxx is private
~~~

说明调用路径中的某一层没有 pub。

例如外部调用：

~~~rust
game_ev_engine::baccarat::probability::OutcomeWeights
~~~

但 baccarat/mod.rs 写的是：

~~~rust
mod probability;
~~~

解决方式有两个：

1. 把模块改成 pub mod；
2. 不公开模块，只通过 pub use 导出需要的类型。

第二种通常更适合隐藏内部实现。

### 13.3 unresolved import

错误类似：

~~~text
unresolved import
~~~

排查顺序：

1. 名称拼写是否正确；
2. 目标是否真的定义了该名称；
3. 路径的起点是否正确；
4. 是否应该使用 crate、super 或 self；
5. 目标名称是否通过 pub 暴露。

### 13.4 cannot find type 或 cannot find function

这通常表示当前作用域中没有这个名称。

可能的解决方式：

~~~rust
use crate::Shoe;
~~~

或者使用完整路径：

~~~rust
let shoe = crate::shoe::Shoe::default();
~~~

### 13.5 把 mod 和 use 写反

错误思路：

~~~rust
use crate::new_module;
~~~

但从来没有声明：

~~~rust
mod new_module;
~~~

正确顺序是：

~~~text
父模块先 mod 声明
    ↓
模块存在
    ↓
其他地方 use 引入名称
~~~

### 13.6 use 写了但仍然不能访问

use 只负责把名称引入当前作用域，不会改变可见性。

例如：

~~~rust
use crate::internal::SecretType;
~~~

如果 SecretType 没有 pub，外部仍然无法访问。

---

## 14. 如何设计好的模块

### 14.1 按业务职责划分

当前项目的划分比较清晰：

~~~text
card.rs
    负责牌、牌面、花色、字符串解析

shoe.rs
    负责牌靴数量和扣牌

baccarat/hand.rs
    负责手牌和点数

baccarat/rule.rs
    负责补牌规则

baccarat/round.rs
    负责一局牌的结果

baccarat/point_enumerate.rs
    负责概率枚举

baccarat/ev.rs
    负责 EV

baccarat/analysis.rs
    负责组合完整分析结果

cli.rs
    负责命令行输入到领域对象的转换
~~~

不要按“一个函数一个模块”机械拆分，也不要把所有代码都放在 lib.rs。

拆分标准应该是：

> 这些代码是否共同负责同一个业务概念？

### 14.2 模块边界应该隐藏实现

point_enumerate.rs 的递归细节属于内部实现，所以当前使用：

~~~rust
mod point_enumerate;
pub use point_enumerate::calculate_main_outcomes;
~~~

外部只看到函数，不依赖递归器的具体文件和辅助类型。

### 14.3 避免循环依赖

理想方向是：

~~~text
card
  ↓
shoe
  ↓
baccarat rules
  ↓
probability / EV
  ↓
CLI / Python / HTTP
~~~

如果 card 依赖 CLI，CLI 又依赖 card，就容易形成混乱。

底层领域模块不应该依赖上层入口模块。

### 14.4 依赖方向应该向内

推荐：

~~~text
入口层依赖应用层
应用层依赖领域层
基础设施适配领域层定义的接口
领域层不依赖 CLI 和数据库
~~~

这样核心计算才可以脱离终端和数据库单独测试。

---

## 15. 当前项目中适合记住的调用写法

### 从 crate 根使用公共类型

~~~rust
use game_ev_engine::{Card, Shoe, MainBetRules};
~~~

适合 main.rs、集成测试和未来 Python 绑定层。

### 在 library 内部从根路径使用

~~~rust
use crate::Shoe;
~~~

适合 analysis.rs、probability.rs 等内部模块。

### 在子模块中使用父模块导出的名称

~~~rust
use super::{MainBet, MainBetRules};
~~~

适合 baccarat 内部子模块。

### 访问具体内部模块

~~~rust
use crate::baccarat::hand::BaccaratHand;
~~~

适合确实需要明确内部位置的场景，但公共调用者更推荐使用 lib.rs 导出的短路径。

---

## 16. 学习模块系统的练习顺序

建议按下面顺序亲手练习：

### 练习一：画模块树

根据 lib.rs 和 baccarat/mod.rs，手动画出：

~~~text
crate
├── card
├── shoe
├── cli
└── baccarat
    ├── hand
    ├── analysis
    ├── bet
    └── ...
~~~

然后标记每个模块是 pub、pub(crate) 还是私有。

### 练习二：追踪一个类型

追踪 Card：

~~~text
card.rs 定义 Card
    ↓
lib.rs pub use Card
    ↓
cli.rs use crate::Card
    ↓
parse::<Card>()
~~~

### 练习三：追踪一个函数

追踪 analyze_main_bets：

~~~text
main.rs
    ↓
game_ev_engine::analyze_main_bets
    ↓
baccarat::analysis::analyze_main_bets
    ↓
calculate_main_outcomes
    ↓
OutcomeWeights
    ↓
MainBetAnalysis
~~~

### 练习四：观察可见性

尝试理解下面三种声明的区别：

~~~rust
mod helper;
pub(crate) mod helper;
pub mod helper;
~~~

分别思考：

- 当前模块能不能访问；
- 当前 crate 的其他模块能不能访问；
- 外部 crate 能不能访问。

### 练习五：观察 re-export

比较下面两种调用路径：

~~~rust
game_ev_engine::baccarat::analysis::analyze_main_bets
~~~

和：

~~~rust
game_ev_engine::analyze_main_bets
~~~

理解第二种为什么更适合公共 API。

---

## 17. 最后总结

Rust 模块系统主要解决四件事：

1. 代码放在哪里；
2. 名称属于哪个作用域；
3. 哪些内容对外公开；
4. 哪些实现细节需要隐藏。

当前项目中最重要的几个关系是：

~~~text
Cargo package
    ↓
lib crate + binary crate
    ↓
lib.rs 建立顶层模块
    ↓
baccarat/mod.rs 建立百家乐子模块
    ↓
pub / pub(crate) / 私有控制可见性
    ↓
pub use 组织稳定公共 API
    ↓
main.rs 调用 library crate
~~~

记住这几个关键区别：

~~~text
mod     = 声明和加载模块
use     = 把名称引入当前作用域
pub     = 允许更外层访问
pub use = 重新导出公共名称
crate   = 当前 crate 根
super   = 父模块
self    = 当前模块
~~~

当前项目的模块设计可以概括为：

> 用 card 和 shoe 表达基础领域，用 baccarat 表达游戏规则和计算，用 lib.rs 组织公共 API，用 main.rs 负责程序入口。

