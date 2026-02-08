# llmux Architecture

Multiplexer for your LLMs. Routes prompts to multiple backends, combines results.

## What It Is

- Declarative multi-LLM workflow engine
- Teams and roles for semantic orchestration
- Focused on review, audit, and fix workflows

## What It's Not

- Not a code generator (no spec, implement, scaffold)
- Not a single-LLM wrapper
- Not an autonomous agent

## Core Hierarchy

```
Backends → Roles → Teams → Workflows
    │        │       │         │
    │        │       │         └── Execution primitive (TOML)
    │        │       └── Domain config (ruby, rust, security)
    │        └── Task types (analyzer, reviewer, synthesizer)
    └── Raw LLM connections (claude, codex, gemini, ollama)
```

### Backends

Raw connections to LLMs. Configuration only, no logic.

```toml
[backends.claude]
command = "claude"

[backends.codex]
command = "codex"
args = ["exec", "--json", "-s", "read-only"]

[backends.ollama]
command = "http://localhost:11434"
model = "qwen3-coder-next"
```

### Roles

Task types mapped to backends. Each role has different strengths.

```toml
[roles.analyzer]
description = "Find bugs, patterns, code smells"
backends = ["codex", "claude"]

[roles.reviewer]
description = "Code review, style, best practices"
backends = ["claude"]

[roles.security]
description = "Vulnerability scanning, security audit"
backends = ["gemini", "claude"]

[roles.synthesizer]
description = "Consolidate findings, prioritize, decide"
backends = ["claude"]
```

Roles can:
- Run a single backend (first available)
- Run all backends in parallel
- Run with fallback chain

### Teams

Domain-specific configurations. Wire roles to backends, define verification.

```toml
[teams.rust]
description = "Rust development"
detect = ["Cargo.toml"]
verify = "cargo clippy && cargo test"

[teams.rust.roles]
analyzer = ["codex", "qwen"]
reviewer = ["claude"]
security = ["gemini", "claude"]
synthesizer = ["claude"]

[teams.ruby]
description = "Ruby/Rails development"
detect = ["Gemfile"]
verify = "bundle exec rspec && rubocop"

[teams.ruby.roles]
analyzer = ["codex", "claude"]
reviewer = ["claude"]
security = ["gemini"]
synthesizer = ["claude"]
```

Teams can:
- Auto-detect from project files
- Override role defaults
- Define domain-specific verification
- Add context files (always include in prompts)

### Workflows

The execution primitive. TOML files that define multi-step pipelines.

```toml
name = "hunt"
description = "Find bugs in the codebase"

[[steps]]
name = "analyze"
role = "analyzer"
parallel = true
prompt = "Find bugs, code smells, N+1 queries..."

[[steps]]
name = "synthesize"
role = "synthesizer"
depends_on = ["analyze"]
prompt = "Consolidate findings: {{ steps.analyze.outputs }}"
```

Workflows use roles, not backends directly. The team resolves roles to backends
at runtime.

## Command Flow

```
User: llmux hunt --team rust

1. Load team config (rust)
2. Auto-detect if no --team (find Cargo.toml → rust)
3. Resolve workflow (hunt)
4. For each step:
   a. Resolve role → backends (analyzer → [codex, qwen])
   b. Execute (parallel or sequential)
   c. Pass output to next step
5. Run verification if edits applied
```

## Built-in Commands

Focused on review/audit/fix:

```bash
llmux doctor              # Check backends and teams
llmux hunt                # Find bugs (analyzer role)
llmux audit               # Security audit (security role)
llmux diff                # Review changes (reviewer role)
llmux fix <issue>         # Fix GitHub issue (full pipeline)
llmux review <pr>         # Review PR (reviewer + security)
```

## Context Seeding

Two layers of context:

### Project Seed (Persistent)

Baseline understanding cached per-project:
- File structure
- Key patterns and conventions
- Architecture overview
- Entry points

Generated once, stored in `.llmux/context.md`, refreshed on demand.

### Task Seed (Ephemeral)

Focused context per-task:
- Files referenced in issue/PR
- Related code via keyword search
- Dependencies of affected code

Generated per-run, passed to prompts.

## Configuration Hierarchy

```
1. Project:  .llmux/config.toml
2. User:     ~/.config/llmux/config.toml
3. Defaults: Built into binary
```

Later configs override earlier. Teams and workflows follow same pattern.

## Workflow Resolution

```
1. Project:  .llmux/workflows/{name}.toml
2. User:     ~/.config/llmux/workflows/{name}.toml
3. Built-in: Embedded in binary
```

## Key Differences from lok

| lok (v1) | llmux (v2) |
|----------|------------|
| Backends hardcoded per step | Roles resolve to backends |
| Manual workflow wiring | Teams auto-configure |
| Kitchen sink (spec, implement) | Focused on review/audit/fix |
| Emerged organically | Designed for roles/teams |

## File Structure

```
~/.config/llmux/
  config.toml           # User config (backends, roles, teams)
  workflows/            # User workflows

.llmux/
  config.toml           # Project overrides
  workflows/            # Project workflows
  context.md            # Cached project seed
```

## Implementation Notes

### Workflow Engine

The workflow engine from lok is solid. Keep it, but:
- Steps reference roles instead of backends
- Team config resolves roles at runtime
- Add `parallel = true` for role-wide parallel execution

### Role Execution

```rust
enum RoleExecution {
    First,      // Use first available backend
    Parallel,   // Run all backends, collect results
    Fallback,   // Try each until one succeeds
}
```

### Team Detection

Check for marker files in order:
- `Cargo.toml` → rust
- `Gemfile` → ruby
- `package.json` → node
- `go.mod` → go
- etc.

Allow `--team` override. Warn if auto-detect fails.

## Open Questions

1. How do parallel role results get combined? (concatenate, vote, synthesize?)
2. Should teams inherit from a base team?
3. How to handle role execution timeout per-backend?
4. Should context seed be automatic or explicit?
