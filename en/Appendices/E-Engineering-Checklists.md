# Appendix E: Engineering Checklists

> This appendix provides five actionable checklists covering the full lifecycle of an Agent Harness from development to production. Each checklist can be copied directly into your project management tool.

---

## E.1 Pre-Run Checklist

Before starting an Agent session, verify the following items:

### Environment Checks

- [ ] Node.js version ≥ 18 (`node --version`)
- [ ] Git version ≥ 2.30 (`git --version`)
- [ ] Network connectivity is working (Anthropic API is reachable)
- [ ] API key is configured and valid
- [ ] Working directory exists with read/write permissions

### Permission Configuration Checks

- [ ] Permission mode has been explicitly selected (default/auto/bypass)
- [ ] Deny/ask rules are configured for high-risk tools (Bash, FileWrite)
- [ ] Project-specific rules are written into CLAUDE.md
- [ ] Sensitive files/directories are added to deny rules (.env, credentials, secrets)

### MCP Server Checks

- [ ] Configured MCP servers can connect successfully
- [ ] Permission rules are configured for MCP tools
- [ ] MCP servers from unknown sources have been security-reviewed

---

## E.2 Agent Harness Design Review Checklist

When designing or reviewing an Agent Harness architecture, check each item:

### Core Loop

- [ ] The conversation loop uses AsyncGenerator (not a simple while + callback)
- [ ] Loop state uses immutable objects (a new State is created on each continue)
- [ ] The `transition` field records the reason for each state transition
- [ ] Dependency injection makes the core loop testable (QueryDeps interface)
- [ ] Termination reasons have fine-grained classification (≥ 8 types)

### Tool System

- [ ] Tool definitions follow a unified interface (name, Schema, permissions, execution, UI)
- [ ] Zod Schema is used for runtime parameter validation
- [ ] Tool registration supports conditional registration and lazy loading
- [ ] The concurrency partitioning algorithm schedules based on `isConcurrencySafe()`
- [ ] Every tool has a complete set of UI rendering methods (6 lifecycle methods)

### Permission System

- [ ] Permission checking is a four-stage pipeline (validateInput → rule matching → context evaluation → interactive confirmation)
- [ ] Deny rules take priority over allow (deny > ask > allow)
- [ ] Permission rules support multi-source priority (session > cli > project > user)
- [ ] High-risk tools (Bash) have additional context evaluation logic
- [ ] Permission decisions can be persisted (to avoid repeated prompts to the user)

---

## E.3 Context Governance Configuration Checklist

### Window Calculation

- [ ] Effective window = model window - min(max output tokens, 20000)
- [ ] Warning threshold = effective window - 20000 tokens
- [ ] Auto-compact threshold = effective window - 13000 tokens
- [ ] Blocking threshold = effective window - 3000 tokens

### Compression Strategy

- [ ] Four-level compression is configured: Snip → Microcompact → Context Collapse → AutoCompact
- [ ] Compression order is arranged from lightweight to heavyweight
- [ ] Auto-compact circuit breaker threshold = 3 (`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES`)
- [ ] Compression's own `prompt_too_long` has fallback handling (`truncateHeadForPTLRetry`)

### Memory System

- [ ] Memory types are a closed set of four (user/feedback/project/reference)
- [ ] MEMORY.md index has been initialized
- [ ] Only information that cannot be derived from project state is saved
- [ ] Memory files have reasonable size limits

---

## E.4 Security Hardening Checklist

### Input Security

- [ ] All tool inputs are validated with Zod Schema
- [ ] File paths are normalized and checked against a whitelist
- [ ] Bash commands undergo risk assessment and pattern matching
- [ ] External data (API responses, MCP results) is not trusted directly

### Execution Security

- [ ] High-risk operations (file deletion, Git push, npm publish) require user confirmation
- [ ] Tool execution has timeout limits
- [ ] Output size has an upper bound (to prevent context explosion)
- [ ] Sub-agents have independent permission scopes

### State Security

- [ ] State updates use an immutable pattern (full replacement rather than field mutation)
- [ ] Sub-agents cannot modify parent agent state
- [ ] Interruption handling has ledger closure (every tool_use has a corresponding tool_result)
- [ ] Sensitive information does not appear in logs or memory

### Supply Chain Security

- [ ] MCP server sources have been reviewed
- [ ] Hook script sources have been reviewed
- [ ] Skill file sources have been reviewed
- [ ] Third-party tool permission scopes have been restricted

---

## E.5 Production Deployment Checklist

### Observability

- [ ] Every state transition in the conversation loop is logged
- [ ] Tool execution success/failure/latency metrics are collected
- [ ] Context usage is monitored (token utilization rate)
- [ ] Auto-compact triggers and outcomes are recorded
- [ ] Circuit breaker state changes trigger alerts

### Fault Tolerance

- [ ] API call failures have a retry mechanism (exponential backoff)
- [ ] `prompt_too_long` has a layered recovery strategy
- [ ] `max_output_tokens` truncation has a continuation-first recovery
- [ ] Auto-compact has circuit breaker protection
- [ ] Interruption handling has ledger closure

### Performance

- [ ] Startup time < 2 seconds (lazy loading, parallel prefetch)
- [ ] Tools execute concurrently (safe tools are scheduled in parallel)
- [ ] Cache-aware prompt construction (hit API-side cache)
- [ ] Streaming output (users do not wait for the full response)

### Team Collaboration

- [ ] CLAUDE.md is under version control
- [ ] Permission configuration is under version control
- [ ] A rule review process exists (PR review)
- [ ] A failure encoding mechanism exists (learn from errors and update rules)
- [ ] New team members can use the Agent independently without verbal coaching
