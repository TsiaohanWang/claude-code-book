# 从零构建 Mini Claude Code —— Rust Agent 开发实战教程

> 基于 Claude Code 架构深度剖析，用 Rust 从零实现一个具备核心功能的 AI 编程 Agent。

---

## 教程定位

Claude Code 是一个超过 50 万行 TypeScript 代码的生产级 AI 编程 Agent。
本教程系列的目标是：**提取 Claude Code 的核心设计模式，用 Rust 从零实现一个 Mini Claude Code**，
让你在动手实践中掌握 Agent 开发的核心思想。

**为什么用 Rust？** 因为 Rust 的类型系统和所有权模型天然适合构建安全可靠的 Agent 系统。
本教程参考了 OpenAI Codex（用 Rust 编写）的架构，同时映射到 Claude Code 的设计模式。

**前置知识：** 本教程面向有一定编程基础的读者。建议先阅读 claude-code-book 的第 0 章（预备知识），
了解 TypeScript 异步编程、Zod 验证、React/Ink 等概念后再开始。

## 教程结构

11 章内容，由易到难，每章对应 claude-code-book 的一个或多个章节：

| 章 | 标题 | 核心概念 | 对应书章节 |
|----|------|----------|-----------|
| 01 | [与模型对话 —— 构建 API 客户端](./ch01_llm_client/) | Anthropic Messages API、SSE 流式 | Ch00, Ch01 |
| 02 | [Agent 的心脏 —— 对话循环](./ch02_agent_loop/) | while(true) 循环、工具调用、依赖注入 | Ch02 |
| 03 | [Agent 的双手 —— 工具系统](./ch03_tool_system/) | Tool trait、Registry、Router、并发安全 | Ch03 |
| 04 | [安全护栏 —— 权限管线](./ch04_permissions/) | 四阶段管线、五种权限模式、规则匹配 | Ch04 |
| 05 | [工作记忆 —— 上下文管理](./ch05_context/) | 有效窗口、五级压缩、断路器 | Ch07 |
| 06 | [长期记忆 —— 记忆系统](./ch06_memory/) | 四种记忆类型、YAML Frontmatter、MEMORY.md | Ch06 |
| 07 | [实时反馈 —— 流式架构](./ch07_streaming/) | SSE 流式、StreamingToolExecutor、并发执行 | Ch03, Ch15 |
| 08 | [扩展机制 —— 钩子系统](./ch08_hooks/) | 五种 Hook 类型、事件匹配、优先级链 | Ch09 |
| 09 | [外部集成 —— MCP 协议](./ch09_mcp/) | JSON-RPC 2.0、工具发现、三段式命名 | Ch13 |
| 10 | [协作之力 —— 多智能体](./ch10_multi_agent/) | Fork 模式、协调器、工具隔离 | Ch10, Ch11 |
| 11 | [组装完成 —— Mini Claude Code](./ch11_mini_claude/) | 完整集成、端到端 | Ch17 |

## 架构对照表

```
Claude Code (50万+ 行 TypeScript)        Mini Claude Code (教程版)
─────────────────────────────────       ─────────────────────────────────
src/services/api/                  ───▶  ch01: Anthropic API 客户端
src/query.ts (queryLoop)           ───▶  ch02: 对话循环 (while true)
src/Tool.ts + src/tools/           ───▶  ch03: 工具系统 (Trait/Registry/Router)
src/hooks/useCanUseTool.tsx        ───▶  ch04: 权限管线 (四阶段)
src/services/compact/              ───▶  ch05: 上下文管理 (五级压缩)
src/memdir/                        ───▶  ch06: 记忆系统 (四类型)
src/services/tools/StreamingTool    ───▶  ch07: 流式架构
src/hooks/                         ───▶  ch08: 钩子系统 (五种类型)
src/mcp/                           ───▶  ch09: MCP 协议
src/agents/ + src/tools/AgentTool  ───▶  ch10: 多智能体
全部模块                           ───▶  ch11: Mini Claude Code
```

## 核心设计模式

### 1. Agent Loop（对话循环）—— 最核心

```rust
// 对应 Claude Code 的 queryLoop() (src/query.ts)
loop {
    let response = client.send(&messages, &tools).await?;
    match response {
        ResponseItem::ToolUse { calls } => {
            for call in &calls {
                let result = router.execute(&call.id, &call.name, &call.input);
                messages.push(tool_result(call.id, result));
            }
            // 继续循环 —— 让模型看到工具结果
        }
        ResponseItem::Message { content } => {
            return Ok(content);  // 回合结束
        }
    }
}
```

### 2. 四阶段权限管线

```
工具调用请求
    → 阶段一: validateInput (Zod Schema 验证)
    → 阶段二: hasPermissionsToUseTool (规则匹配, deny > ask > allow)
    → 阶段三: checkPermissions (工具特定检查)
    → 阶段四: 交互式提示 (用户确认)
```

### 3. 五级上下文压缩

```
Tool Result 预算裁剪 → Snip → Microcompact → Context Collapse → AutoCompact
    (成本递增 →)
```

## 环境准备

```bash
# 1. 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustc --version  # 需要 1.75+

# 2. 进入教程目录
cd claude-code-book/tutorials

# 3. 设置 API Key（Ch01-02 需要，其他章节有 mock 模式）
export ANTHROPIC_API_KEY="sk-ant-your-key-here"

# 4. 编译检查
cargo check

# 5. 运行第一个演示（无需 API Key）
cargo run -p ch03-tool-system

# 6. 运行完整 Mini Claude Code（需要 API Key）
cargo run -p ch11-mini-claude
```

## 运行所有测试

```bash
cargo test
```

## 与 Claude Code 的一致性说明

| 维度 | Claude Code 实现 | 教程实现 | 说明 |
|------|-----------------|----------|------|
| **API 端点** | /v1/messages (Messages API) | /v1/messages | ✅ 完全一致 |
| **消息格式** | {role, content} + tool_use blocks | {role, content} JSON | ⚠️ 简化但语义一致 |
| **工具接口** | Tool<Input,Output,Progress> 五要素 | ToolHandler trait 三要素 | ⚠️ 教学简化 |
| **权限系统** | 七层纵深防御 | 四阶段管线 | ⚠️ 教学简化 |
| **上下文压缩** | 五级压缩 + 断路器 | 五级压缩 + 断路器 | ✅ 模式一致 |
| **记忆系统** | 四类型 + 语义召回 | 四类型 + BM25 模拟 | ⚠️ 教学简化 |
| **钩子系统** | 31 事件 × 5 类型 | 5 事件 × 5 类型 | ⚠️ 教学简化 |

**设计原则：**
1. **保留核心模式**：Agent Loop、Tool Registry/Router、权限管线、上下文压缩等核心设计模式与 Claude Code 一致
2. **简化实现细节**：去掉生产级的错误恢复、遥测、MCP 完整实现等复杂机制
3. **保持可扩展性**：教程代码的架构设计允许逐步添加 Claude Code 的高级功能
