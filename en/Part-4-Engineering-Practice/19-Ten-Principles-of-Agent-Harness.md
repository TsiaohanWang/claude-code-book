# Chapter 19: Ten Principles of Agent Harness

> *"My way is pervaded by one thread."* — Confucius, *Analerta*, Li Ren

**Reading guide:** This chapter is a distillation of the entire book. The ten principles are abstracted from the detailed analyses in the preceding seventeen chapters. Each principle is annotated with its source chapter, the corresponding implementation location in Claude Code, and transferable application scenarios. If you are short on time, this chapter alone conveys roughly 80% of the book's core insights.

---

## Why Principles Matter

Specific technologies become obsolete — Bun may be superseded, Zod may be replaced by a better library, and Claude Code itself may be overtaken by a new product. But **design principles** do not expire. They are generalized wisdom distilled from concrete practice, portable to any agent system.

To borrow a metaphor: technologies are the "fish"; principles are the "fishing." Give a person a fish and you feed them for a day; teach them to fish and you feed them for a lifetime. Once you internalize the principles, you can make consistent design decisions in novel situations — even ones Claude Code has never encountered.

---

## Principle 1: Constrained Execution Is the Core Capability

> **The key capability of an agent system is constrained execution.**

The core capability of an agent is not "intelligence" — the model's intelligence is provided by the LLM. The core capability is **constraint**: ensuring that the model's intelligence is safely, reliably, and efficiently translated into concrete actions.

An unconstrained agent is like a race car without brakes — the faster it goes, the more dangerous it becomes. Constraints do not limit capability; they make capability usable.

**Claude Code implementation locations:**
- Four-stage permission pipeline (Chapter 4)
- Tool scheduling discipline: concurrency partitioning, serial execution (Chapter 3)
- High-density constraints on high-risk tools: BashTool prompt rules (Chapter 3)

**Transferable scenarios:** Any system involving LLM interaction with the external world — automated operations, data processing pipelines, CI/CD integration.

---

## Principle 2: Prompt Is the Control Plane, Not Personality Decoration

> **Prompt is part of the control plane, not personality decoration.**

Many people still treat prompts as text that "sets the AI's persona." But in an agent system, the prompt is the **behavioral control plane** — it defines execution boundaries, failure behavior, and reporting responsibilities.

Claude Code's system prompt is not "You are a helpful assistant." It is a layered assembly of control directives: default rules → project rules → custom rules → agent role → memory injection → output style. Each layer has an explicit priority and override relationship.

**Claude Code implementation locations:**
- `getSystemPrompt()` layered assembly logic (Chapter 1)
- CLAUDE.md proximity-based priority (Chapter 5, Chapter 17)
- Cache-aware system prompt construction (Chapter 1)

**Transferable scenarios:** Any system that uses an LLM — do not treat the prompt as an afterthought; design it as part of the architecture.

---

## Principle 3: Errors Are Part of the Main Path

> **Errors are part of the main path, not exceptions to it.**

Traditional software systems treat errors as "exceptions" — the happy path is the body text, and catch blocks are footnotes. Agent systems cannot afford this. `prompt_too_long`, `max_output_tokens` truncation, tool execution failures, user interrupts — these are not edge cases; they are structural norms.

A mature agent system is not one that "never makes mistakes," but one that "knows how to recover when it does."

**Claude Code implementation locations:**
- Withheld mechanism: recover first, then report (Chapter 8)
- Layered recovery strategy: flush collapse → reactive compaction → surfacing (Chapter 8)
- Circuit breaker pattern: preventing recovery from becoming an infinite loop (Chapter 8)
- Fine-grained classification of ten termination reasons (Chapter 2)

**Transferable scenarios:** Any long-running automation system — error handling is not an afterthought; it is a core design concern.

---

## Principle 4: Tool Calls Must Obey Scheduling Discipline

> **Tool calls must obey scheduling discipline.**

When a model can call tools, risk shifts from "rhetorical risk" (saying the wrong thing) to "execution risk" (doing the wrong thing). The key question is: **who decides how tools are executed?**

Claude Code's answer: the runtime decides. Based on a tool's `isConcurrencySafe()` property, the runtime determines whether to execute in parallel or serially. Based on `isReadOnly()`, it decides whether a permission check is needed. Based on `isDestructive()`, it determines the risk level.

Tools are not a natural extension of model capability; they are managed execution units that require scheduling discipline.

**Claude Code implementation locations:**
- `partitionToolCalls()` concurrency partitioning algorithm (Chapter 3)
- `StreamingToolExecutor` four-stage state machine (Chapter 3)
- Context modifier buffering and replay (Chapter 3)

**Transferable scenarios:** Any multi-tool orchestration system — do not let the model directly determine execution order; let the runtime schedule based on tool properties.

---

## Principle 5: The Stronger the Capability, the Finer the Control

> **The stronger the capability, the finer the control.**

Among all tools, Bash is the least trustworthy — it has virtually no domain boundaries: it can directly manipulate files, processes, networks, and Git repositories, compounded by shell redirections and pipe statements. Any system that over-trusts Bash typically incurs real-world consequences.

Claude Code's approach: equip the Bash tool with extremely detailed prompt rules — do not modify git config casually, do not skip hooks, do not blindly `git add .`, do not use `--amend` after a pre-commit failure, do not commit code unless explicitly asked, do not push by default.

This is not over-engineering; it is the fundamental principle that **high-risk interfaces require high-density constraints**.

**Claude Code implementation locations:**
- Detailed operational rules in `BashTool/prompt.ts` (Chapter 3)
- Context evaluation in the four-stage permission pipeline (Chapter 4)
- Priority ordering of Bash rule matching (Chapter 4)

**Transferable scenarios:** Any tool involving file system operations, database modifications, or network requests — the stronger the capability, the finer the constraints.

---

## Principle 6: Layer Recovery Paths; Make Them Countable, Rate-Limited, and Breakable

> **Layer recovery paths; make them countable, rate-limited, and breakable.**

Recovery is not a binary "try or don't try" choice. A well-designed recovery system should be:

1. **Layered:** First attempt the least costly strategy (flush collapse), then escalate to costlier strategies (reactive compaction), and only surface the error as a last resort.
2. **Countable:** Record the outcome of each recovery attempt.
3. **Rate-limited:** Limit the number of recovery attempts per unit of time.
4. **Breakable:** Stop attempting after consecutive failures reach a threshold.

A recovery system without brakes is like a car without brakes — it is not recovering; it is accelerating.

**Claude Code implementation locations:**
- `hasAttemptedReactiveCompact` anti-loop guard (Chapter 8)
- `MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3` circuit breaker threshold (Chapter 8)
- `truncateHeadForPTLRetry()` self-compaction recovery (Chapter 8)
- Two-stage recovery for `max_output_tokens` (Chapter 8)

**Transferable scenarios:** Any system with automatic retry/recovery mechanisms — recovery logic must have its own governance.

---

## Principle 7: Context Governance Is a Budgeting Regime, Not a Truncation Operation

> **Context governance is a budgeting regime, not a truncation operation.**

The context window is an agent's most precious resource. Managing it should not be "truncate when full," but rather a fine-grained budgeting regime:

- Safe zone (0–85%): normal operation
- Warning zone (85–90%): alert the user
- Danger zone (90–95%): trigger automatic compaction
- Blocking zone (95–100%): reject new requests

The four-level compaction strategy (Snip → Microcompact → Context Collapse → AutoCompact) is ordered from lightweight to heavyweight, with each step first attempting the least costly approach.

**Claude Code implementation locations:**
- Effective window formula and threshold constants (Chapter 7)
- Four-level progressive compaction pipeline (Chapter 7, Chapter 2)
- Cache-aware compaction strategy (Chapter 7)

**Transferable scenarios:** Any system with resource constraints — do not wait until resources are exhausted; build early-warning and graduated-response mechanisms.

---

## Principle 8: Memory Stores Only What Cannot Be Derived

> **Memory stores only what cannot be derived from the current state.**

Code patterns, architecture, file structure, and Git history are all obtainable in real time via tools — none of these belong in memory. Memory should store only **information that cannot be derived from the current project state**: the user's role and preferences, validated practices, project decision context, and external reference links.

This principle prevents memory system bloat and staleness. If information can be derived from project state, it should not be memorized — because when the project state changes, the copy in memory becomes an outdated "phantom."

**Claude Code implementation locations:**
- Four closed memory types: user / feedback / project / reference (Chapter 6)
- Metadata-only storage strategy for MEMORY.md indexing (Chapter 6)
- Memory inheritance mechanism in Fork mode (Chapter 6)

**Transferable scenarios:** Any system with persistent storage — not all information is worth saving; store only what cannot be derived.

---

## Principle 9: Multi-Agent Work and Verification Must Be Separated

> **Multi-agent work and verification must not be blended into one vague mechanism.**

When a system evolves from "one agent self-rescuing" to "one agent delegates, another verifies," errors and recovery are no longer purely single-threaded problems — they become organizational design problems.

Claude Code's coordinator pattern follows the "orchestrate, don't execute" constraint: the Coordinator distributes tasks and aggregates results but does not directly execute tools. Verification is independent of implementation — the verifier ≠ the implementer.

This separation ensures that the implementer's bias does not contaminate verification results, and the verifier's feedback can be independently processed by the implementer.

**Claude Code implementation locations:**
- Dual gating in the coordinator pattern (Chapter 11)
- Independent context and permission scope for sub-agents (Chapter 10)
- Context inheritance and isolation in Fork mode (Chapter 10)

**Transferable scenarios:** Any system involving code review, testing, or auditing — the executor and the verifier should be different roles.

---

## Principle 10: Structure Is More Reliable Than Smartness

> **Structure is less flashy than smartness, but usually more reliable.**

Models make mistakes. Tools amplify consequences. Context inflates. State pollutes subsequent turns. Users interrupt. Failures repeat.

Faced with these realities, a system cannot rely on "smartness" to maintain order — it must rely on **structure**. Structure is less glamorous than smartness, but usually more reliable.

**Claude Code implementation locations:**
- Immutable state transitions (Chapter 1, Chapter 2)
- Dependency injection making tests possible (Chapter 2)
- Type-system-enforced interface contracts (Chapter 3)
- Defense-in-depth in the four-stage permission pipeline (Chapter 4)

**Transferable scenarios:** Everything — do not believe the fantasy that "a stronger model will fix system problems." Stronger models only mean more tool calls, longer execution chains, and more severe failure consequences. Engineering structure is the only reliable source of order.

---

## Quick-Reference Table

| # | Principle | One-liner | Source Chapters |
|---|-----------|-----------|-----------------|
| 1 | Constrained execution is the core capability | Not smartness — constraints make agents usable | Chapters 1, 3, 4 |
| 2 | Prompt is the control plane | Not personality decoration — it is the behavioral boundary | Chapters 1, 5 |
| 3 | Errors are part of the main path | Not exceptions — they are structural norms | Chapters 2, 8 |
| 4 | Tool calls obey scheduling discipline | The runtime decides parallel/serial, not the model | Chapter 3 |
| 5 | The stronger the capability, the finer the control | Stronger capability → finer constraints | Chapters 3, 4 |
| 6 | Recovery must be layered and breakable | Countable, rate-limited, and breakable | Chapter 8 |
| 7 | Context governance is a budgeting regime | Early warning + graduated response, not truncation | Chapters 2, 7 |
| 8 | Memory stores only what cannot be derived | If it can be derived, do not memorize it | Chapter 6 |
| 9 | Work and verification must be separated | The implementer ≠ the verifier | Chapters 10, 11 |
| 10 | Structure is more reliable than smartness | Structure is the only reliable source of order | Entire book |

---

## Closing Remarks

These ten principles are not dogma — they are generalized wisdom distilled from the concrete practice of Claude Code. When you encounter scenarios in your own projects that Claude Code does not cover, return to these principles and ask yourself:

1. Does my system have sufficient constraints? (Principle 1)
2. Is my prompt controlling behavior or decorating personality? (Principle 2)
3. Is my error handling part of the main path? (Principle 3)
4. Do my tools have scheduling discipline? (Principle 4)
5. Do high-risk operations have sufficient constraints? (Principle 5)
6. Does my recovery logic have brakes? (Principle 6)
7. Is my context management a budgeting regime or brute-force truncation? (Principle 7)
8. Does my memory store only what cannot be derived? (Principle 8)
9. Are execution and verification separated? (Principle 9)
10. Am I relying on smartness or on structure? (Principle 10)

Answer these ten questions well, and you will possess a design audit checklist portable to any agent system.
