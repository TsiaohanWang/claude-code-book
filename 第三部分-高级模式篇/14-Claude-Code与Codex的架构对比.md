# 第 14 章 Claude Code 与 Codex 的架构对比

> **学习目标：** 阅读本章后，你将能够：
>
> - 理解 Claude Code 和 Codex 在控制平面上的根本分歧
> - 对比两种查询循环/线程模型的设计哲学
> - 分析工具治理的两种路径：运行时编排 vs 策略语言
> - 评估"运行时灵活性"与"制度化显式性"的权衡
> - 为自建 Agent Harness 选择合适的学习对象

---

## 14.1 为什么要对比？

Claude Code 和 Codex 是当前最具代表性的两个 AI 编程 Agent。它们面对相同的核心问题——如何让一个不稳定的模型在真实工程环境中安全运行——但给出了截然不同的答案。

对比它们不是为了分出高下，而是为了理解**设计空间的边界**。当你自己构建 Agent 系统时，你需要在这些边界中做出选择。理解两种选择的后果，比记住某个产品的细节更有价值。

> **核心结论：** Claude Code 从运行时经验中生长出来；Codex 从制度设计中生长出来。前者更擅长让系统在现场保持稳定，后者更擅长让系统在组织中维持秩序。

---

## 14.2 **控制平面**（Control Plane——系统中负责决策和管理的部分，与负责实际数据处理的"数据平面"相对）：动态组装 vs 结构化片段

### Claude Code 的动态组装线

Claude Code 的系统 Prompt 不是固定文档，而是一条生产线：默认规则构成基础，追加规则叠加需求，Agent 角色注入身份，CLAUDE.md 和记忆注入本地条件。灵活性让同一个循环能处理多种场景，但排序至关重要——错误的排序会稀释指令或让冲突溜走。

运行时治理是必需的：控制被持续注入、覆盖、压缩或裁剪，循环在每一轮重新计算"现在什么最重要"。

核心直觉：**控制跟随场景——它不能冻结为静态规则。**

### Codex 的归档室方案

Codex 坚持可识别的片段。`ContextualUserFragmentDefinition` 这样的名称突出了类型、边界、包装规则和消息转换。AGENTS.md、技能和用户指令是被标记的上下文单元，系统可以识别和操纵它们——更强的可调试性，以及通往更程序化治理的路径，因为每条指令已经适合一个类型层次。

`fragment.rs` 定义了 `AGENTS_MD_START_MARKER`、`AGENTS_MD_END_MARKER`、`SKILL_OPEN_TAG`、`SKILL_CLOSE_TAG` 等常量；`ContextualUserFragmentDefinition::wrap()` 和 `into_message()` 将片段转换为 `ResponseItem::Message`。Codex 努力不让模型猜测规则来自哪里。

### 两种组装方式的对比

```
// Claude Code 动态组装
system_prompt = concat(
    default_prompt,           // 基础
    append_prompt,            // 叠加需求
    agent_prompt,             // 角色
    claudemd_layers,          // 团队/个人/项目
    memory_sections,          // 会话记忆
    output_style              // 表达纪律
)
// 每轮重新计算：记忆预取、折叠、微压缩、自动压缩

// Codex 片段组装
for frag in [agents_md, skill, user_instructions]:
    body = ContextualUserFragmentDefinition::wrap(
        START_MARKER, content, END_MARKER,
        meta { source_dir, name, path }
    )
    msg = frag.into_message()  // → ResponseItem::Message
    thread.append(msg)
```

| 维度 | Claude Code | Codex |
|------|-------------|-------|
| 灵活性 | 高（运行时动态组合） | 中（结构化但较重） |
| 可调试性 | 中（需要追踪运行时状态） | 高（片段有明确标记和来源） |
| 冲突解决 | 依赖排序和覆盖规则 | 依赖类型层次和优先级声明 |
| 适合场景 | 场景多变、规则频繁调整 | 规则稳定、需要明确治理边界 |

---

## 14.3 查询循环 vs 线程/Rollout/状态

### Claude Code：连续性压缩在主循环中

Claude Code 的核心围绕 `query()` 和 `queryLoop()`，将许多关键问题推入循环状态：当前消息序列、工具使用上下文、压缩追踪、输出 token 恢复计数器、待处理摘要、轮次计数、转换原因。

"Agent 如何保持存活"的答案是运行时风格的——连续性主要由循环维护，骨架感觉像一个自纠正的对话引擎，而非由外部状态模型驱动的系统。

优势是具体的：工具返回排序、输出截断、`prompt_too_long` 事件、历史裁剪、微压缩和用户插入——这些都在循环内部处理，Claude Code 将它们视为合法的循环状态而非回避它们。

### Codex：连续性分布在 Thread、Rollout 和状态桥中

Codex 看起来更像账本式。从 `core/src/lib.rs` 开始，连续性分布在 `codex_thread`、`thread_manager`、`rollout`、`state_db_bridge`、`state` 和 `message_history` 之间。

在 SDK 层，`Thread` 已经是外部开发者的一等概念：它拥有 `id`，通过 `runStreamed()` 或 `run()` 运行，`thread.started` 事件报告线程 ID。轮级执行条件——审批策略、工作目录、沙箱模式、网络访问、附加目录——都是显式参数，与线程执行紧密耦合。

线程主权是字面意义上的：`runStreamedInternal()` 调用 `normalizeInput()` 分离文本和图片，`createOutputSchemaFile()` 准备 Schema 文件，然后将 `threadId`、`approvalPolicy`、`sandboxMode`、`workingDirectory`、`networkAccessEnabled` 和 `additionalDirectories` 传入 `_exec.run()`。

连续性不再是"循环还在继续"，而是"一个线程被记录并约束在显式的状态结构中"。

### 状态主权的位置

| 维度 | Claude Code | Codex |
|------|-------------|-------|
| 状态主权 | 循环拥有状态 | Thread/Rollout 拥有状态 |
| 恢复优势 | 现场就近处理（反应式压缩、中断清理） | 可追溯性（线程 ID、Rollout 记录、状态桥） |
| 审计能力 | 中（依赖运行时日志） | 高（Rollout 提供完整执行记录） |
| 产品接口 | 面向运行时问题 | 面向治理问题 |

用一个比喻：Claude Code 更像现场急救队——擅长让执行继续下去。Codex 更像有档案的调度中心——擅长解释连续性是如何维持的。

---

## 14.4 工具治理：运行时编排 vs 策略语言

### Claude Code：现场监管

工具系统有强烈的现场调度感：并发取决于 Schema 和 `isConcurrencySafe()`，上下文修改保持重放顺序，流式执行必须处理中断、合成结果和 UI 反馈。工具调用被视为有后果的过程，而非单点动作——Harness 附着在模型上，像一个现场主管，看着哪个工具先走、哪个可以并行、哪个必须串行、结果如何记账、中途停止时怎么办。

Bash 被以近乎偏执的显式性对待——成熟的系统通常在最危险的接口周围最为挑剔。

### Codex：合规办公室

Codex 将对风险操作的控制表达为形式化的接口约束。`exec_command` 显式拥有字段——`cmd`、`workdir`、`shell`、`tty`、`yield_time_ms`、`max_output_tokens`、`login` 以及审批相关参数——而非接受单个字符串命令。正确用法被编码在工具定义本身中。

审批和升级是显式参数，`request_permissions` 是一个独立工具，`execpolicy` 是一个独立 crate——`Policy`、`Rule`、`Evaluation`、`Decision`、parser——执行边界变成了一个小型策略语言，而非一堆 `if/else` 检查。

```
// Claude Code 运行时审批
decision = hasPermissionsToUseTool(tool, input, ctx)  // allow | deny | ask
match decision:
    allow: schedule_with_concurrency_check(tool)
    deny:  reject(reason)
    ask:   route_to(coordinator | swarm | interactive)

// Codex 策略评估 (codex-rs/execpolicy/src/decision.rs)
policy = PolicyParser.parse(source)
for rule in policy.rules:
    eval = rule.evaluate(exec_command { cmd, workdir, shell, ... })
    if eval.matches: return Decision::{Allow | Prompt | Forbidden}
return Decision::default  // Prompt (需要用户确认)
```

| 维度 | Claude Code | Codex |
|------|-------------|-------|
| 决策类型 | allow / deny / ask | Allow / Prompt / Forbidden |
| 决策位置 | 调用点附近（上下文敏感） | 独立策略引擎（规则驱动） |
| 可移植性 | 中（规则嵌入运行时逻辑） | 高（策略可解析、可移植） |
| 团队治理 | 中（需要理解运行时） | 高（策略文件可 PR 审查） |
| 灵活性 | 高（运行时可动态调整） | 中（修改策略需要重新部署） |

---

## 14.5 技能、钩子与本地规则

### Claude Code：情境治理链

Claude Code 将技能、钩子、权限和工具提示编织成一条情境治理链，让本地规则搭载主循环。CLAUDE.md 是本地公告板，与记忆和技能配对，适合注册常识、禁忌和本地规则。

钩子系统在 31 个生命周期节点提供扩展点，通过 JSON 输入输出协议与 Harness 交互。灵活性很高，但规则的来源和优先级有时不够显式。

### Codex：结构化资产

Codex 将外部能力拉入统一的工具系统——MCP 资源、动态工具和工具发现期望扩展成为 Schema 定义、规则治理的工具对象，而非运行时理解。技能通过指纹安装，钩子事件有明确的类型层次。

一旦生态系统增长，"扩展如何服从通用规则"就变成了压舱石：尽早想清楚边界迁移的团队，其扩展生态不会退化为杂物间。

---

## 14.6 收敛与分叉

### 它们在哪里汇聚？

如果必须给出最短结论：是的，它们真正汇聚。原因是直接的——Claude Code 和 Codex 都不将模型视为值得直接信任的执行者。两者都接受：

- Prompt 不能控制一切
- 工具必须被约束
- 长会话需要状态治理
- 本地规则必须进入系统
- 多智能体执行需要角色划分和验证

换言之，两者都已经超越了"更强的模型会自己解决系统问题"的天真阶段。一旦系统到达这个阶段，它就不再把 Agent 当作只是带了几个工具的聊天机器人。

### 它们在哪里分叉？

但把它们称为根本相同就太粗糙了。

Claude Code 的主轴：
- 从查询循环开始
- 在运行时处理连续性
- 用压缩、工具编排、中断和恢复维护秩序
- 通过技能、钩子和验证连接现场规则和团队制度

Codex 的主轴：
- 从显式模块边界和显式控制层开始
- 将指令变为片段
- 将工具变为 Schema
- 将执行边界变为策略
- 将会话变为 Thread/Rollout/State
- 将本地规则和钩子变为结构化资产和事件系统

前者感觉像一个从机械经验中生长出来的系统。后者感觉像一个从制度设计中生长出来的系统。

### 两种政治形式

更尖锐但更准确地说，它们是 Harness 的两种不同政治形式：

**Claude Code 更接近运行时共和。** 大量权力集中在主循环和现场调度中，秩序通过与现实的持续协商来维护。它不反制度；制度只是倾向于服务于活跃会话。

**Codex 更接近宪政控制平面。** 权力首先被写入类型、片段、策略、线程和事件系统。运行时当然仍然判断，但它在一个更显式的框架内判断。

这不是美学差异。这是系统权力的分配。谁定义边界、谁解释状态、谁拥有执行的最终权威——这些最终都出现在架构中。

---

## 14.7 对自建者的启示

### 三类团队，三个方向

**类型一：原型存在，长会话失控。** 优先学 Claude Code。先稳定循环，制度可以等。

**类型二：规则倍增，来源分散。** 优先学 Codex。先让规则显式化，运行时优化可以等。

**类型三：从零开始。** 选择一个主要矛盾，围绕它设计骨架，对立面只做到最低可行。

### 从 Claude Code 学什么？

- 查询循环的状态思维
- 压缩和上下文治理
- 工具编排和中断处理
- 子智能体生命周期和验证独立性
- 将失败路径视为主路径

### 从 Codex 学什么？

- 指令片段化
- 工具 Schema 化
- 审批和策略的显式表达
- Thread/Rollout/State 基础设施
- 钩子事件和技能资产管理

### 一个危险的误解

显式性和灵活性不是天然敌人。说"显式控制层"就想象沉重缓慢僵化的系统，说"运行时灵活性"就想象经验可以先撑着——这两种直觉都不明智。

一个好的第三方系统不会取两者平均值——它区分哪些规则必须先写下来、哪些判断可以留在运行时、哪些状态必须持久化、哪些经验只需要活在会话记忆中。

> **最终判断：** 从 Claude Code 主要学习系统如何在现场保持稳定；从 Codex 主要学习系统如何在组织中维持秩序。只学前者的团队往往经验丰富但制度贫乏。只学后者的团队往往制度优雅但现场脆弱。更好的做法不是选边站，而是根据你的主要矛盾决定先长哪根骨头。

---

## 关键要点

1. **控制平面的分歧是根本性的。** Claude Code 动态组装 Prompt，Codex 使用结构化片段。前者灵活但难以形式化，后者显式但较重。

2. **状态主权的位置决定了系统中心。** Claude Code 的循环拥有状态，Codex 的 Thread/Rollout 拥有状态。谁拥有连续性，谁就定义了 Harness 的中心。

3. **工具治理是两种政治形式。** 运行时编排（现场主管）vs 策略语言（合规办公室）。前者灵活，后者可审计。

4. **它们真正汇聚，但走不同的路。** 两者都承认模型不可靠、Harness 是秩序来源。但一个信任运行时纪律更多，一个信任显式控制层更多。

5. **选择取决于你的主要矛盾。** "长会话失控"→ 学 Claude Code。"规则散乱"→ 学 Codex。从零开始 → 先选一个矛盾，再搭骨架。

---

## 实战练习

### 练习 1：运行两种 Prompt 组装方式的对比

以下代码实现了第 14 章的核心对比——Claude Code 的动态 Prompt 组装 vs Codex 的结构化片段。复制到 `mini-comparison.ts` 后用 `npx tsx mini-comparison.ts` 运行。

> **源码参考：** Claude Code 动态组装对应 `src/utils/systemPrompt.ts` 中的 `buildEffectiveSystemPrompt()`；Codex 片段对应 `codex-cli/src/instructions/fragment.rs` 中的 `ContextualUserFragmentDefinition`。

```typescript
// mini-comparison.ts — Claude Code vs Codex 架构对比（~60 行）
// 源码参考：Claude Code src/utils/systemPrompt.ts, Codex codex-cli/src/instructions/fragment.rs

// ── Claude Code: 动态 Prompt 组装 ────────────────────────
function buildClaudeCodePrompt(config: { cwd: string; tools: string[]; claudeMd: string; memory: string }): string {
  return [
    "You are Claude Code, an interactive coding agent.",
    `Working directory: ${config.cwd}`,
    `Available tools: ${config.tools.join(", ")}`,
    config.claudeMd ? `\n# Project Instructions\n${config.claudeMd}` : "",
    config.memory ? `\n# Memory\n${config.memory}` : "",
  ].filter(Boolean).join("\n");
}

// ── Codex: 结构化片段 ────────────────────────────────────
interface Fragment { marker: string; source: string; content: string; }
function buildCodexPrompt(fragments: Fragment[]): string {
  return fragments.map(f => `<${f.marker} source="${f.source}">\n${f.content}\n</${f.marker}>`).join("\n\n");
}

// ── 权限模型对比 ─────────────────────────────────────────
function claudeCodePermission(tool: string, mode: string, rules: { allow: string[]; deny: string[] }): string {
  if (rules.deny.some(r => tool.includes(r))) return "deny";
  if (rules.allow.some(r => tool.includes(r))) return "allow";
  if (mode === "bypass") return "allow";
  return "ask";
}

interface PolicyRule { action: "allow" | "deny"; pattern: string; }
function codexPermission(tool: string, policy: PolicyRule[]): string {
  for (const rule of policy) { if (tool.match(new RegExp(rule.pattern.replace("*", ".*")))) return rule.action; }
  return "ask";
}

function main() {
  console.log("=== Claude Code vs Codex 对比测试 ===\n");

  console.log("1. Prompt 组装方式:");
  const ccPrompt = buildClaudeCodePrompt({ cwd: "/home/user/project", tools: ["read_file", "edit_file", "bash"], claudeMd: "Use TypeScript strict mode.", memory: "User prefers concise output." });
  console.log(`  Claude Code: ${ccPrompt.split("\n").length} 行, 动态组装`);
  const codexPrompt = buildCodexPrompt([
    { marker: "AGENTS_MD", source: "project", content: "Use TypeScript strict mode." },
    { marker: "USER_INSTRUCTIONS", source: "user", content: "User prefers concise output." },
  ]);
  console.log(`  Codex: ${codexPrompt.split("\n").length} 行, 结构化片段`);

  console.log("\n2. 权限模型:");
  console.log(`  Claude Code (bash, default): ${claudeCodePermission("bash", "default", { allow: ["read_file"], deny: ["npm publish"] })} — 运行时决策`);
  console.log(`  Codex (bash_rm_*): ${codexPermission("bash_rm_rf", [{ action: "deny", pattern: "bash_rm*" }])} — 策略评估`);

  console.log("\n3. 收敛点（两者都同意）:");
  ["模型不可靠 → harness 提供秩序", "工具必须被约束 → 调度纪律", "长会话需要状态治理", "多智能体需要角色划分"].forEach(c => console.log(`  ✅ ${c}`));
}
main();
```

### 练习 2：选择你的主要矛盾

根据你的团队情况，判断应该优先学习哪个系统：

| 你的团队情况 | 主要矛盾 | 优先学习 |
|------------|---------|---------|
| 有 Agent 原型，长会话经常崩溃 | 系统活不够久 | ? |
| 规则散落各处，没人知道约束在哪 | 系统越来越难治理 | ? |
| 从零开始，没有成熟系统 | ? | 选一个矛盾先解决 |

**参考答案：** 1→Claude Code（先稳定循环）；2→Codex（先让规则显式化）；3→根据第一阶段风险选择
