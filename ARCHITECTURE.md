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

## Workflow Structure

### Header

```toml
name = "fix"
description = "Fix a GitHub issue"
version = 1

# Workflow-level defaults (steps can override)
timeout = 300000
continue_on_error = false

# Arguments passed from CLI
[args]
issue = { required = true, description = "GitHub issue number" }
branch = { required = false, default = "fix/{{ args.issue }}" }
```

### Step Types

Three step types, explicit not inferred:

```toml
# Shell: run a command
[[steps]]
name = "fetch"
type = "shell"
run = "gh issue view {{ args.issue }} --json title,body"

# Query: call LLM(s)
[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
prompt = "Analyze this: {{ steps.fetch.output }}"

# Apply: make changes to files
[[steps]]
name = "fix"
type = "apply"
source = "steps.consensus.edits"
verify = "{{ team.verify }}"
```

**Why explicit types?**
- lok inferred from presence of `shell` vs `prompt` vs `apply_edits`
- Led to confusing combinations and edge cases
- Explicit is clearer, validates better

### Parallel Execution

```toml
[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
parallel = true          # Run ALL backends in role simultaneously
prompt = "Find bugs..."
```

When `parallel = true`:
- All backends in the role run concurrently
- Results collected as array: `steps.analyze.outputs`
- Individual results: `steps.analyze.outputs[0]`, `steps.analyze.outputs.claude`
- Step succeeds if `min_success` backends succeed (default: 1)

When `parallel = false` (default):
- First available backend runs
- Result is single value: `steps.analyze.output`

### Dependencies and Data Flow

```toml
[[steps]]
name = "synthesize"
type = "query"
role = "synthesizer"
depends_on = ["analyze", "security"]
prompt = """
Analyzer findings:
{{ steps.analyze.outputs | join("\n---\n") }}

Security findings:
{{ steps.security.output }}
"""
```

**Template helpers:**
- `{{ steps.X.output }}` - single output (string)
- `{{ steps.X.outputs }}` - parallel outputs (array)
- `{{ steps.X.outputs | join(sep) }}` - join array
- `{{ steps.X.outputs.claude }}` - specific backend's output
- `{{ args.issue }}` - CLI argument
- `{{ team.verify }}` - team config value
- `{{ env.GITHUB_TOKEN }}` - environment variable

### Conditionals

```toml
[[steps]]
name = "create_pr"
type = "shell"
run = "gh pr create --title '{{ steps.fix.summary }}'"
if = "steps.fix.action == 'fix'"

[[steps]]
name = "close_issue"
type = "shell"
run = "gh issue close {{ args.issue }} --comment 'False positive'"
if = "steps.fix.action == 'close'"
```

**Cleaner than lok's** `if = 'equals(fix.action, "fix")'` syntax.

### Error Handling

```toml
[[steps]]
name = "risky"
type = "query"
role = "analyzer"
continue_on_error = true   # Soft fail - workflow continues
retries = 3                # Retry on failure
retry_delay = 2000         # Exponential backoff starting point
timeout = 60000            # Step timeout

[[steps]]
name = "handler"
type = "query"
depends_on = ["risky"]
prompt = """
{% if steps.risky.failed %}
Handle error: {{ steps.risky.error }}
{% else %}
Process: {{ steps.risky.output }}
{% endif %}
"""
```

**Step result fields:**
- `.output` / `.outputs` - the result
- `.failed` - boolean
- `.error` - error message if failed
- `.duration_ms` - how long it took
- `.backend` - which backend ran (for non-parallel)
- `.backends` - which backends ran (for parallel)

### Apply Steps

```toml
[[steps]]
name = "apply_fix"
type = "apply"
source = "steps.consensus"          # Step with edits
verify = "cargo clippy && cargo test"
rollback_on_failure = true          # Requires git-agent

# Or inline edits
[[steps]]
name = "apply_inline"
type = "apply"
edits = [
  { file = "src/main.rs", old = "...", new = "..." }
]
verify = "{{ team.verify }}"
```

**Source step must output:**
```json
{
  "edits": [
    { "file": "path/to/file", "old": "exact match", "new": "replacement" }
  ]
}
```

### Iteration

```toml
[[steps]]
name = "read_files"
type = "shell"
for_each = "steps.consensus.files"   # Array to iterate
run = "cat {{ item }}"               # {{ item }} is current element
```

Results collected as array matching input order.

### Step Groups (New)

Logical grouping for organization and shared config:

```toml
[[groups]]
name = "analysis"
parallel = true
continue_on_error = true

[[groups.steps]]
name = "codex_scan"
type = "query"
backend = "codex"                    # Can use backend directly in groups
prompt = "Find issues..."

[[groups.steps]]
name = "claude_scan"
type = "query"
backend = "claude"
prompt = "Find issues..."

# Reference group output
[[steps]]
name = "synthesize"
type = "query"
role = "synthesizer"
depends_on = ["analysis"]            # Depends on whole group
prompt = "{{ groups.analysis.outputs }}"
```

**Why groups?**
- Cleaner than N steps with same config
- Explicit "these run together"
- Can add/remove backends without changing structure

## Improvements Over lok

| lok | llmux |
|-----|-------|
| `backend = "codex"` | `role = "analyzer"` |
| Inferred step type | Explicit `type = "shell"/"query"/"apply"` |
| `if = 'equals(x, "y")'` | `if = "x == 'y'"` |
| `{{ steps.X.output }}` only | `.output`, `.outputs`, `.failed`, `.error` |
| Manual parallel steps | `parallel = true` |
| `apply_edits = true` | `type = "apply"` with `source` |
| No args definition | `[args]` with required/default |
| No step groups | `[[groups]]` for logical batches |

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

Only tool-level utilities. Everything else is a workflow.

```bash
llmux doctor              # Check backends, roles, teams
llmux context             # Gather/seed context
llmux run <workflow>      # Run a workflow
llmux backends            # List configured backends
llmux teams               # List configured teams
llmux workflows           # List available workflows
llmux validate <workflow> # Validate workflow before running
```

### Context Flag

All commands accept `--context` to pass files explicitly to backends:

```bash
llmux run review --context ARCHITECTURE.md,README.md
llmux run audit --context src/auth.rs
```

The context files are:
- Read and prepended to prompts
- Available to agentic backends as explicit references
- Logged for debugging/tracing

## Shipped Workflows

Bundled but not special - users can override or ignore.

```bash
llmux run hunt [dir]           # Find bugs
llmux run audit [dir]          # Security audit
llmux run diff [dir]           # Review changes
llmux run fix <issue> [dir]    # Fix GitHub issue
llmux run review <pr> [dir]    # Review PR
```

All actions go through `llmux run`. No magic commands.

## Directory Handling

All commands accept an optional directory argument. Defaults to cwd.

```bash
llmux run hunt                 # Run in current directory
llmux run hunt ./my-project    # Run in specified directory
llmux doctor ~/dev/lok         # Check backends for that project
llmux context ~/dev/discourse  # Seed context for that project
```

Team auto-detection uses the target directory.

## Context and Project Setup

### `llmux context` Flow

1. Navigate to project directory
2. Run `llmux context`
3. Tool scans the repo:
   - Detects project type (Cargo.toml → rust, Gemfile → ruby, etc.)
   - Analyzes file structure
   - Identifies key patterns and entry points
4. Suggests configuration:
   - Team assignment
   - Role mappings for this project
   - Relevant workflows
5. Saves to `.llmux/`:
   - `config.toml` - team, roles, project-specific settings
   - `context.md` - cached project understanding for prompts

```bash
$ cd ~/dev/my-rust-project
$ llmux context

Scanning project...
  Detected: Rust (Cargo.toml)
  Structure: src/, tests/, benches/
  Entry: src/main.rs, src/lib.rs

Suggested config:
  Team: rust
  Verify: cargo clippy && cargo test
  Roles:
    analyzer: [codex, claude]
    reviewer: [claude]
    security: [gemini]

Save to .llmux/config.toml? [Y/n]
```

### Task Context (Ephemeral)

Per-workflow context gathered at runtime:
- Files referenced in issue/PR
- Related code via keyword search
- Dependencies of affected code

Passed to prompts, not persisted.

## Example Workflows

### hunt.toml - Find bugs

```toml
name = "hunt"
description = "Find bugs and code issues"

[[steps]]
name = "analyze"
role = "analyzer"
parallel = true
prompt = """
Find bugs, code smells, and issues. Look for:
- Error handling problems
- Potential panics/crashes
- N+1 queries
- Unused code

List up to 10 issues with file:line references.
"""

[[steps]]
name = "synthesize"
role = "synthesizer"
depends_on = ["analyze"]
prompt = """
Consolidate these findings from multiple analyzers:

{{ steps.analyze.outputs }}

Deduplicate, rank by severity, output a final list.
"""
```

### audit.toml - Security audit

```toml
name = "audit"
description = "Security vulnerability scan"

[[steps]]
name = "scan"
role = "security"
parallel = true
prompt = """
Find security vulnerabilities:
- Injection (SQL, command, code)
- Auth/authz bypasses
- Sensitive data exposure
- Insecure dependencies

Be specific with file paths and line numbers.
"""

[[steps]]
name = "report"
role = "synthesizer"
depends_on = ["scan"]
output_format = "json"
prompt = """
Consolidate security findings:

{{ steps.scan.outputs }}

Output JSON:
{
  "critical": [...],
  "high": [...],
  "medium": [...],
  "low": [...]
}
"""
```

### fix.toml - Fix a GitHub issue

```toml
name = "fix"
description = "Analyze and fix a GitHub issue"
args = ["issue"]

[[steps]]
name = "fetch"
shell = "gh issue view {{ args.issue }} --json title,body,comments"

[[steps]]
name = "analyze"
role = "analyzer"
parallel = true
depends_on = ["fetch"]
prompt = """
Analyze this issue and propose a fix:

{{ steps.fetch.output }}

Output:
1. Root cause
2. Files to change
3. Proposed fix with code snippets
"""

[[steps]]
name = "consensus"
role = "synthesizer"
depends_on = ["analyze"]
output_format = "json"
prompt = """
Multiple backends proposed fixes:

{{ steps.analyze.outputs }}

Synthesize into a consensus approach. Output JSON:
{
  "approach": "description of the fix",
  "files": ["path/to/file.rs"],
  "edits": [{"file": "...", "old": "...", "new": "..."}]
}
"""

[[steps]]
name = "apply"
depends_on = ["consensus"]
apply_edits = true
verify = "{{ team.verify }}"
source = "steps.consensus"
```

### review-pr.toml - Review a pull request

```toml
name = "review-pr"
description = "Multi-backend PR review"
args = ["pr"]

[[steps]]
name = "fetch"
shell = "gh pr view {{ args.pr }} --json title,body,files,diff"

[[steps]]
name = "review"
role = "reviewer"
parallel = true
depends_on = ["fetch"]
prompt = """
Review this PR:

{{ steps.fetch.output }}

Check for:
- Code quality and style
- Potential bugs
- Missing tests
- Documentation gaps
"""

[[steps]]
name = "security"
role = "security"
depends_on = ["fetch"]
prompt = """
Security review of this PR:

{{ steps.fetch.output }}

Look for vulnerabilities introduced by these changes.
"""

[[steps]]
name = "summarize"
role = "synthesizer"
depends_on = ["review", "security"]
prompt = """
Combine review feedback:

Reviews:
{{ steps.review.outputs }}

Security:
{{ steps.security.output }}

Output a final review: approve, request changes, or comment.
"""

[[steps]]
name = "comment"
depends_on = ["summarize"]
shell = "gh pr review {{ args.pr }} --body '{{ steps.summarize.output }}'"
```

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

## Error Handling & Retries

### Error Philosophy

1. **Capture everything** - never throw away error context
2. **Categorize errors** - retryable vs permanent
3. **Retry intelligently** - different strategies for different failures
4. **Surface clearly** - errors should explain what went wrong and why

### Error Categories

```rust
enum ErrorKind {
    // Retryable - transient failures
    RateLimit { retry_after: Duration },
    Timeout { elapsed: Duration },
    NetworkError { message: String },
    BackendUnavailable { backend: String },

    // Retryable with modification
    OutputParseFailed { raw: String, expected: String },
    VerificationFailed { command: String, stderr: String },

    // Not retryable - permanent failures
    ConfigError { message: String },
    FileNotFound { path: String },
    TemplateError { template: String, error: String },
    InvalidWorkflow { errors: Vec<String> },
    AuthError { backend: String },
}
```

### Error Context

Every error captures full context:

```rust
struct StepError {
    kind: ErrorKind,
    step: String,
    backend: Option<String>,

    // Timing
    started_at: DateTime,
    failed_at: DateTime,
    duration_ms: u64,

    // What we sent
    prompt: Option<String>,
    command: Option<String>,

    // What we got back
    stdout: Option<String>,
    stderr: Option<String>,
    raw_response: Option<String>,
    exit_code: Option<i32>,
    http_status: Option<u16>,

    // Retry state
    attempt: u32,
    max_attempts: u32,
    will_retry: bool,
    retry_delay_ms: Option<u64>,
}
```

### Retry Layers

**Layer 1: Backend-level (automatic)**

Backends handle their own transient failures:

```toml
[backends.gemini]
command = "npx"
args = ["@google/gemini-cli"]

# Backend-specific retry config
retry_rate_limit = true      # Auto-retry 429s
retry_timeout = true         # Auto-retry timeouts
max_retries = 3
base_delay_ms = 1000         # Exponential backoff
```

This happens inside the backend wrapper, invisible to workflows.

**Layer 2: Step-level (configured)**

Steps can retry on specific conditions:

```toml
[[steps]]
name = "analyze"
type = "query"
role = "analyzer"

# Retry config
retries = 2
retry_on = ["timeout", "rate_limit", "parse_error"]
retry_delay_ms = 2000

# Don't retry these
fail_on = ["auth_error", "config_error"]
```

**Layer 3: Verification retry (special case)**

When `type = "apply"` and verification fails:

```toml
[[steps]]
name = "fix"
type = "apply"
source = "steps.consensus"
verify = "cargo clippy"

# Verification retry - re-query LLM with error
verify_retries = 2
verify_retry_prompt = """
The fix failed verification:

{{ error.stderr }}

Original fix:
{{ source.edits }}

Provide corrected edits.
"""
```

### Error Output

When a step fails, full context available:

```toml
[[steps]]
name = "handle_failure"
type = "query"
depends_on = ["risky_step"]
if = "steps.risky_step.failed"
prompt = """
Step failed. Debug info:

Error: {{ steps.risky_step.error.kind }}
Message: {{ steps.risky_step.error.message }}

Command: {{ steps.risky_step.error.command }}
Exit code: {{ steps.risky_step.error.exit_code }}
Stderr: {{ steps.risky_step.error.stderr }}

Attempt: {{ steps.risky_step.error.attempt }}/{{ steps.risky_step.error.max_attempts }}
Duration: {{ steps.risky_step.error.duration_ms }}ms

What went wrong and how to fix it?
"""
```

### Workflow Validation

Before running, validate:

```bash
$ llmux validate workflow.toml

Checking workflow.toml...
  ✓ Syntax valid
  ✓ All step dependencies exist
  ✓ No circular dependencies
  ✓ All roles defined in config
  ✓ All templates parse correctly
  ✓ Required args documented

Ready to run.
```

Catches errors before wasting API calls:
- Missing step references in `depends_on`
- Undefined roles
- Template syntax errors (typos like `{{ steps.anaylze.output }}`)
- Circular dependencies
- Missing required args

### CLI Error Display

```
$ llmux run fix 123

Running fix...
  ✓ fetch (2.1s)
  ✓ analyze (parallel: codex, claude) (8.3s)
  ✓ consensus (4.2s)
  ✗ apply (1.2s)

Step 'apply' failed:

  Error: VerificationFailed

  Command: cargo clippy
  Exit code: 1

  Stderr:
    error[E0382]: borrow of moved value: `data`
     --> src/main.rs:42:15
      |
    40 |     let result = process(data);
      |                          ---- value moved here
    42 |     println!("{}", data);
      |                    ^^^^ value borrowed here after move

  Attempt: 1/3
  Retrying in 2s with error context...
```

### Structured Step Output

Steps communicate via structured data, not freeform text. This makes inter-step
data reliable and parseable.

**Step output schema:**

```toml
[[steps]]
name = "analyze"
type = "query"
role = "analyzer"

# Define expected output structure
[steps.output_schema]
type = "object"
properties = { findings = "array", severity = "string", files = "array" }
required = ["findings"]
```

The prompt automatically includes output format instructions. Response is parsed
and validated before passing to dependent steps.

**Accessing structured output:**

```toml
[[steps]]
name = "synthesize"
depends_on = ["analyze"]
prompt = """
Findings to consolidate:

{% for result in steps.analyze.outputs %}
## {{ result.backend }}
Severity: {{ result.data.severity }}
Files: {{ result.data.files | join(", ") }}
Findings:
{% for f in result.data.findings %}
- {{ f }}
{% endfor %}
{% endfor %}
"""
```

**Why structured?**
- No more regex to extract JSON from markdown code blocks
- Validation catches bad LLM output early
- Templates access fields directly, not string parsing
- Parallel outputs have consistent shape

**Fallback for freeform:**

When no schema defined, output is raw text in `.text` field:

```toml
{{ steps.freeform.output.text }}
```

### Output Handlers

Separate from step output - these handle workflow-level output (console, logs).

**Built-in:**
- `console` - pretty terminal output
- `json` - structured JSON to stdout
- `log` - write to `.llmux/logs/`

**Plugins:**
- `arf-trace` - full reasoning traces for debugging
- `markdown` - formatted reports
- `github` - post to issues/PRs

```toml
[output]
handlers = ["console", "log"]

[output.log]
dir = ".llmux/logs"
retain_days = 7
```

```bash
llmux run fix 123                    # Default handlers
llmux run fix 123 --output json      # JSON to stdout
llmux run fix 123 --trace            # Full reasoning trace
llmux run fix 123 --quiet            # Suppress console
```

### Debug Mode

Full trace for debugging:

```bash
$ llmux run fix 123 --debug

[12:34:56.123] Starting workflow: fix
[12:34:56.124] Args: { issue: 123 }
[12:34:56.125] Team: rust (auto-detected from Cargo.toml)
[12:34:56.126]
[12:34:56.127] Step: fetch
[12:34:56.128]   Type: shell
[12:34:56.129]   Command: gh issue view 123 --json title,body
[12:34:58.234]   Exit code: 0
[12:34:58.235]   Output: {"title": "Fix bug in parser", ...}
[12:34:58.236]   Duration: 2107ms
[12:34:58.237]
[12:34:58.238] Step: analyze (parallel)
[12:34:58.239]   Role: analyzer
[12:34:58.240]   Backends: [codex, claude]
[12:34:58.241]   Prompt: |
[12:34:58.242]     Analyze this issue and propose a fix:
[12:34:58.243]     {"title": "Fix bug in parser", ...}
...
```

Logs written to `.llmux/logs/` for post-mortem.

## Findings from llmux Architecture Debate

Multi-backend review (Codex, Claude, Gemini) of this architecture doc identified:

### Critical Issues to Fix

**1. Shell Injection - Needs Concrete Design**

The doc acknowledges the risk but examples show unsafe patterns:
```toml
# UNSAFE - shown in doc
run = "gh pr review {{ args.pr }} --body '{{ steps.summarize.output }}'"
```

**Solution:** Structured command arrays OR mandatory escape filter:
```toml
# Option A: Array form (no shell)
run = ["gh", "pr", "review", "{{ args.pr }}", "--body", "{{ steps.summarize.output }}"]

# Option B: Explicit escaping required
run = "gh pr review {{ args.pr }} --body '{{ steps.summarize.output | shell_escape }}'"
```

**2. Apply Step - Use Diffs Not Exact Match**

`old`/`new` exact string replacement is too brittle for LLM output. LLMs hallucinate
whitespace constantly.

**Solution:** Support unified diffs:
```json
{
  "edits": [
    { "file": "src/main.rs", "diff": "@@ -10,3 +10,5 @@\n context\n-old line\n+new line" }
  ]
}
```

Or fuzzy matching with whitespace normalization.

**3. Template Syntax Collision**

Doc mixes two systems:
- `if = "steps.fix.action == 'fix'"` - simple expression
- `{% if steps.risky.failed %}` - Jinja control flow

**Solution:** Pick one. Recommend: use Tera/minijinja for all templating, document
the expression language explicitly.

**4. Examples Contradict Design**

Doc says explicit `type = "shell"` but examples use:
```toml
shell = "gh issue view ..."  # Missing type
apply_edits = true           # Should be type = "apply"
```

**Solution:** Fix all examples to match stated design.

**5. Output Schema - Use Real JSON Schema**

Current syntax is ambiguous:
```toml
[steps.output_schema]
properties = { findings = "array" }
```

**Solution:** Use actual JSON Schema:
```toml
[steps.output_schema]
type = "object"
required = ["findings"]
properties.findings = { type = "array", items = { type = "string" } }
```

### Missing Pieces to Add

**6. Human Approval Step**
```toml
[[steps]]
name = "confirm"
type = "input"
prompt = "Apply these changes? [y/N]"
options = ["y", "n"]
```

**7. Cancellation Behavior**

When parallel step has `min_success = 2` and 2 backends succeed, what happens to
the 3rd still running? Define: cancel immediately vs run to completion.

**8. Context Window Strategy**

When synthesizing outputs from 4 parallel backends, prompts can exceed limits.
Define: truncation, summarization, or error.

**9. Checkpointing Granularity**

State persistence needs concrete design:
- Where stored? `.llmux/state/{workflow}-{timestamp}.json`
- What's saved? Step outputs, timing, errors
- How to resume? `llmux run fix 123 --resume <checkpoint-id>`

---

## Historical: Findings from lok Debate

Earlier review of lok codebase identified these gaps (many now addressed above):

### Must Have

- **Shell injection protection**: Escape/sanitize LLM output before shell interpolation
- **Timeout unit consistency**: Pick one (ms everywhere) and stick to it
- **Workflow validation**: `#[serde(deny_unknown_fields)]`, cycle detection, template validation
- **Graceful shutdown**: Signal handling, don't leave partial state on Ctrl+C
- **State persistence / checkpointing**: Resume from failed step, don't re-run everything

### Should Have

- **Observability**: Structured logging, timing, traces (not just `eprintln!`)
- **Step-level caching**: Skip steps whose inputs haven't changed
- **Rate limiting / cost controls**: `max_concurrent_requests`, token budgets
- **Context window management**: Pruning, summarization for long outputs
- **Backend health checks**: `doctor --verify` with real test request
- **Output schema validation**: Validate LLM output against expected structure
- **Global concurrency cap**: Prevent resource exhaustion on large workflows

### Nice to Have

- **Fail-fast parallelism**: Cancel sibling steps on hard failure
- **Workflow composition**: `include` / `extends` for reuse
- **Dry-run mode**: Preview what workflow will do without running
- **Consensus strategy plugins**: Custom voting logic beyond exact-string match
- **Rollback strategies**: `["git", "backup", "none"]` not just git-agent

## Open Questions

1. How do parallel role results get combined? (concatenate, vote, synthesize?)
2. Should teams inherit from a base team?
3. How to handle role execution timeout per-backend?
4. Should context seed be automatic or explicit?
5. What's the checkpointing granularity? Per-step? Per-workflow?
6. How to handle secrets in prompts/outputs?
