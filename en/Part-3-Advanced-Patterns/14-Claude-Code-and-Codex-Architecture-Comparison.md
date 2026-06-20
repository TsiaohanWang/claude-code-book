# Chapter 14: Claude Code and Codex Architecture Comparison

> **Learning Objectives:** After reading this chapter, you will be able to:
>
> - Understand the fundamental divergence between Claude Code and Codex in their control planes
> - Compare the design philosophies of their query loops / thread models
> - Analyze two paths for tool governance: runtime orchestration vs. policy language
> - Evaluate the tradeoffs between "runtime flexibility" and "institutional explicitness"
> - Choose the right reference system when building your own agent harness

---

## 14.1 Why Compare?

Claude Code and Codex are the two most representative AI coding agents today. They face the same core problem — how to run an unstable model safely in a real engineering environment — but offer radically different answers.

Comparing them is not about declaring a winner. It is about understanding the **boundaries of the design space**. When you build your own agent system, you will need to make choices within these boundaries. Understanding the consequences of both options is more valuable than memorizing the details of any single product.

> **Core Conclusion:** Claude Code grew from runtime experience; Codex grew from institutional design. The former is better at keeping a system stable in the field; the latter is better at maintaining order within an organization.

---

## 14.2 Control Plane: Dynamic Assembly vs. Structured Fragments

### Claude Code's Dynamic Assembly Line

Claude Code's system prompt is not a fixed document. It is a production line: default rules form the foundation, appended rules layer on requirements, agent role injection provides identity, and CLAUDE.md plus memory inject local conditions. Flexibility lets the same loop handle diverse scenarios, but ordering is critical — wrong ordering dilutes instructions or lets conflicts slip through.

Runtime governance is mandatory: control is continuously injected, overridden, compressed, or trimmed. The loop recalculates "what matters most right now" on every turn.

Core intuition: **control follows context — it cannot be frozen into static rules.**

### Codex's Archive Room Approach

Codex insists on recognizable fragments. Names like `ContextualUserFragmentDefinition` surface type, boundary, wrapping rules, and message transformations. AGENTS.md, skills, and user instructions are tagged context units that the system can identify and manipulate — stronger debuggability and a path toward more programmatic governance, because each instruction already fits a type hierarchy.

`fragment.rs` defines constants like `AGENTS_MD_START_MARKER`, `AGENTS_MD_END_MARKER`, `SKILL_OPEN_TAG`, `SKILL_CLOSE_TAG`, and so on. `ContextualUserFragmentDefinition::wrap()` and `into_message()` transform fragments into `ResponseItem::Message`. Codex works hard to ensure the model never has to guess where a rule came from.

### Comparing the Two Assembly Approaches

```
// Claude Code dynamic assembly
system_prompt = concat(
    default_prompt,           // foundation
    append_prompt,            // layered requirements
    agent_prompt,             // role
    claudemd_layers,          // team / personal / project
    memory_sections,          // session memory
    output_style              // expression discipline
)
// Recalculated every turn: memory prefetch, folding, micro-compression, auto-compression

// Codex fragment assembly
for frag in [agents_md, skill, user_instructions]:
    body = ContextualUserFragmentDefinition::wrap(
        START_MARKER, content, END_MARKER,
        meta { source_dir, name, path }
    )
    msg = frag.into_message()  // → ResponseItem::Message
    thread.append(msg)
```

| Dimension | Claude Code | Codex |
|-----------|-------------|-------|
| Flexibility | High (dynamic runtime composition) | Medium (structured but heavier) |
| Debuggability | Medium (requires runtime state tracing) | High (fragments have explicit tags and origins) |
| Conflict Resolution | Relies on ordering and override rules | Relies on type hierarchy and priority declarations |
| Best For | Highly variable scenarios, frequently adjusted rules | Stable rules, clear governance boundaries |

---

## 14.3 Query Loop vs. Thread / Rollout / State

### Claude Code: Continuity Compressed into the Main Loop

Claude Code's core revolves around `query()` and `queryLoop()`, pushing many critical concerns into loop state: the current message sequence, tool usage context, compression tracking, output token recovery counters, pending summaries, turn counts, and transition reasons.

The answer to "how does the agent stay alive" is runtime-flavored — continuity is maintained primarily by the loop. The skeleton feels like a self-correcting conversation engine rather than a system driven by an external state model.

The advantages are concrete: tool return ordering, output truncation, `prompt_too_long` events, history trimming, micro-compression, and user insertion — all handled inside the loop. Claude Code treats these as legitimate loop state rather than avoiding them.

### Codex: Continuity Distributed Across Thread, Rollout, and State Bridge

Codex looks more ledger-based. Starting from `core/src/lib.rs`, continuity is distributed across `codex_thread`, `thread_manager`, `rollout`, `state_db_bridge`, `state`, and `message_history`.

At the SDK layer, `Thread` is already a first-class concept for external developers: it owns an `id`, runs via `runStreamed()` or `run()`, and the `thread.started` event reports the thread ID. Turn-level execution conditions — approval policy, working directory, sandbox mode, network access, additional directories — are all explicit parameters, tightly coupled to thread execution.

Thread sovereignty is literal: `runStreamedInternal()` calls `normalizeInput()` to separate text and images, `createOutputSchemaFile()` prepares the schema file, then passes `threadId`, `approvalPolicy`, `sandboxMode`, `workingDirectory`, `networkAccessEnabled`, and `additionalDirectories` into `_exec.run()`.

Continuity is no longer "the loop is still going." It is "a thread is recorded and bound within explicit state structures."

### Where State Sovereignty Lives

| Dimension | Claude Code | Codex |
|-----------|-------------|-------|
| State Sovereignty | Loop owns state | Thread / Rollout owns state |
| Recovery Advantage | Field-proximate handling (reactive compression, interruption cleanup) | Traceability (thread ID, Rollout records, state bridge) |
| Audit Capability | Medium (relies on runtime logs) | High (Rollout provides complete execution records) |
| Product Interface | Oriented toward runtime problems | Oriented toward governance problems |

To use an analogy: Claude Code is more like a field emergency team — good at keeping execution going. Codex is more like a dispatch center with archives — good at explaining how continuity was maintained.

---

## 14.4 Tool Governance: Runtime Orchestration vs. Policy Language

### Claude Code: Field Supervision

The tool system has a strong feel of field dispatch: concurrency depends on schema and `isConcurrencySafe()`, context modifications maintain replay order, and streaming execution must handle interruptions, synthetic results, and UI feedback. Tool calls are treated as consequential processes, not single-point actions — the harness attaches to the model like a field supervisor, watching which tool goes first, which can run in parallel, which must be serialized, how results are accounted for, and what happens on mid-stream stops.

Bash is treated with near-paranoid explicitness — mature systems tend to be most meticulous around their most dangerous interfaces.

### Codex: Compliance Office

Codex expresses control over risky operations as formal interface constraints. `exec_command` explicitly owns fields — `cmd`, `workdir`, `shell`, `tty`, `yield_time_ms`, `max_output_tokens`, `login`, and approval-related parameters — rather than accepting a single string command. Correct usage is encoded in the tool definition itself.

Approval and escalation are explicit parameters. `request_permissions` is a standalone tool. `execpolicy` is a standalone crate — `Policy`, `Rule`, `Evaluation`, `Decision`, parser — turning execution boundaries into a small policy language rather than a pile of `if/else` checks.

```
// Claude Code runtime approval
decision = hasPermissionsToUseTool(tool, input, ctx)  // allow | deny | ask
match decision:
    allow: schedule_with_concurrency_check(tool)
    deny:  reject(reason)
    ask:   route_to(coordinator | swarm | interactive)

// Codex policy evaluation
policy = PolicyParser.parse(source)
for rule in policy.rules:
    eval = rule.evaluate(exec_command { cmd, workdir, shell, ... })
    if eval.matches: return Decision::{Allow | Deny | RequestPermissions}
return Decision::default
```

| Dimension | Claude Code | Codex |
|-----------|-------------|-------|
| Decision Location | Near the call site (context-sensitive) | Independent policy engine (rule-driven) |
| Portability | Medium (rules embedded in runtime logic) | High (policies are parseable and portable) |
| Team Governance | Medium (requires understanding the runtime) | High (policy files are PR-reviewable) |
| Flexibility | High (runtime-adjustable on the fly) | Medium (policy changes require redeployment) |

---

## 14.5 Skills, Hooks, and Local Rules

### Claude Code: Contextual Governance Chain

Claude Code weaves skills, hooks, permissions, and tool hints into a contextual governance chain that lets local rules ride on the main loop. CLAUDE.md is the local bulletin board, paired with memory and skills, suitable for registering common knowledge, prohibitions, and local rules.

The hook system provides extension points at 26 lifecycle nodes, interacting with the harness through a JSON input/output protocol. Flexibility is high, but the origin and priority of rules are sometimes insufficiently explicit.

### Codex: Structured Assets

Codex pulls external capabilities into a unified tool system — MCP resources, dynamic tools, and tool discovery are expected to expand into schema-defined, rule-governed tool objects rather than runtime interpretation. Skills are installed via fingerprinting, and hook events have clear type hierarchies.

Once the ecosystem grows, "how extensions obey common rules" becomes ballast: teams that think through boundary migration early will find that their extension ecosystem does not degenerate into a junk drawer.

---

## 14.6 Convergence and Divergence

### Where Do They Converge?

If forced to give the shortest conclusion: yes, they truly converge. The reason is straightforward — neither Claude Code nor Codex treats the model as a trustworthy executor. Both accept:

- Prompts cannot control everything
- Tools must be constrained
- Long sessions require state governance
- Local rules must enter the system
- Multi-agent execution requires role separation and verification

In other words, both have moved past the naive stage where "a stronger model will solve the system's problems on its own." Once a system reaches this stage, it no longer treats the agent as merely a chatbot with a few tools attached.

### Where Do They Diverge?

But calling them fundamentally identical would be too crude.

Claude Code's main axis:
- Starts from the query loop
- Handles continuity at runtime
- Maintains order through compression, tool orchestration, interruption, and recovery
- Connects field rules and team institutions through skills, hooks, and validation

Codex's main axis:
- Starts from explicit module boundaries and explicit control layers
- Turns instructions into fragments
- Turns tools into schemas
- Turns execution boundaries into policy
- Turns sessions into Thread / Rollout / State
- Turns local rules and hooks into structured assets and event systems

The former feels like a system grown from mechanical experience. The latter feels like a system grown from institutional design.

### Two Political Forms

More pointedly but more accurately: they are two different political forms of a harness:

**Claude Code is closer to a runtime republic.** Significant power concentrates in the main loop and field dispatch. Order is maintained through continuous negotiation with reality. It is not anti-institution; institutions simply tend to serve the active session.

**Codex is closer to a constitutional control plane.** Power is first written into types, fragments, policies, threads, and event systems. Runtime still exercises judgment, of course, but within a more explicit framework.

This is not an aesthetic difference. It is about the distribution of system power. Who defines boundaries, who interprets state, who holds final authority over execution — all of this ultimately shows up in the architecture.

---

## 14.7 Implications for Self-Builders

### Three Team Types, Three Directions

**Type 1: Prototype exists, long sessions are out of control.** Prioritize learning from Claude Code. Stabilize the loop first; institutions can wait.

**Type 2: Rules are multiplying, sources are scattered.** Prioritize learning from Codex. Make rules explicit first; runtime optimization can wait.

**Type 3: Starting from scratch.** Choose one primary contradiction, design the skeleton around it, and implement the opposing dimension at minimum viable level only.

### What to Learn from Claude Code?

- Query loop state thinking
- Compression and context governance
- Tool orchestration and interruption handling
- Sub-agent lifecycle and verification independence
- Treating failure paths as primary paths

### What to Learn from Codex?

- Instruction fragmentation
- Tool schema formalization
- Explicit approval and policy expression
- Thread / Rollout / State infrastructure
- Hook events and skill asset management

### A Dangerous Misconception

Explicitness and flexibility are not natural enemies. Hearing "explicit control layer" and imagining a heavy, slow, rigid system; or hearing "runtime flexibility" and imagining that experience can just hold things together — neither intuition is wise.

A good third-party system does not take the average of the two. It distinguishes which rules must be written down first, which judgments can stay at runtime, which state must be persisted, and which experience only needs to live in session memory.

> **Final Judgment:** From Claude Code, primarily learn how a system stays stable in the field. From Codex, primarily learn how a system maintains order within an organization. Teams that only learn the former tend to be experienced but institutionally poor. Teams that only learn the latter tend to be institutionally elegant but fragile in the field. The better approach is not to pick a side, but to decide which bone to grow first based on your primary contradiction.

---

## Key Takeaways

1. **The control plane divergence is fundamental.** Claude Code dynamically assembles prompts; Codex uses structured fragments. The former is flexible but hard to formalize; the latter is explicit but heavier.

2. **Where state sovereignty lives defines the system's center.** Claude Code's loop owns state; Codex's Thread / Rollout owns state. Whoever owns continuity defines the harness's center.

3. **Tool governance is two political forms.** Runtime orchestration (field supervisor) vs. policy language (compliance office). The former is flexible; the latter is auditable.

4. **They truly converge, but take different paths.** Both acknowledge that models are unreliable and that the harness is the source of order. But one trusts runtime discipline more, and the other trusts explicit control layers more.

5. **The choice depends on your primary contradiction.** "Long sessions out of control" → learn from Claude Code. "Rules are scattered" → learn from Codex. Starting from scratch → choose one contradiction first, then build the skeleton.
