# llm-mux

**The Makefile for LLMs.**

Just as Make orchestrates shell commands, llm-mux orchestrates LLM calls. Write a TOML workflow, run it with one command, get results from any model - or all of them at once.

```toml
# .llm-mux/workflows/review.toml
name = "review"

[[steps]]
name = "diff"
type = "shell"
run = "git diff HEAD~1"

[[steps]]
name = "analyze"
type = "query"
role = "analyzer"       # runs Claude + Gemini in parallel
prompt = "Review these changes:\n\n{{ steps.diff.output }}"
depends_on = ["diff"]

[[steps]]
name = "synthesize"
type = "query"
role = "coder"
prompt = """
Turn these reviews into one minimal patch.
Return only a unified diff or a JSON edits array:

{{ steps.analyze.output }}
"""
depends_on = ["analyze"]

[[steps]]
name = "fix"
type = "apply"
source = "synthesize"
verify = "cargo test"   # rolls back if tests fail
rollback_on_failure = true
depends_on = ["synthesize"]
```

```bash
llm-mux run review
```

No SDK. No Python. No boilerplate. A single Rust binary, a config file, done.

## Built-in repository review

`repository-review` gathers a bounded Git diff, asks every backend in the
`default` role for an independent review, reconciles the findings with one
backend, and produces a structured patch.

```bash
llm-mux run repository-review
llm-mux run repository-review base=origin/main scope=src
```

Review and patch generation do not modify the repository. Applying is explicit:

```bash
llm-mux run repository-review base=origin/main apply=true
```

Applied edits are transactional and must pass `git diff --check`; otherwise
they are rolled back. Use `llm-mux runs show <id>` to inspect every model’s
prompt, output, usage, and cost, or `llm-mux runs resume <id>` after fixing a
provider or configuration failure.

## Why llm-mux

You have API keys for Claude, Gemini, and a local Ollama instance. Right now you're copy-pasting the same prompt into each one and manually merging the results. llm-mux removes that entirely.

- **Route to multiple models at once** - run a role across Claude + Gemini in parallel, get both responses
- **Chain steps together** - shell commands, LLM queries, file edits, and verification steps compose naturally
- **Apply and verify** - LLM suggests edits, llm-mux applies them, runs your test suite, rolls back on failure
- **Dry-run first** - `--dry-run` shows what shell and apply steps would do before touching anything
- **Inspect and resume runs** - prompts, outputs, provider usage, and failures are kept in a durable SQLite ledger
- **Works anywhere** - single binary with direct HTTP support; CLI backends use their corresponding installed tools

llm-mux grew out of [lok](https://github.com/ducks/lok), an earlier take on the
same idea. Lok is no longer developed; this is where the work continues.

## Install

```bash
cargo install llm-mux
```

Or download a prebuilt binary from [releases](https://github.com/ducks/llm-mux/releases).

## Setup

### 1. Configure backends

Create `~/.config/llm-mux/config.toml`:

```toml
[backends.claude]
command = "claude"
args = ["-p"]

[backends.gemini]
command = "npx"
args = ["@google/gemini-cli", "-m", "gemini-2.0-flash"]

[backends.ollama]
command = "http://localhost:11434/v1"
model = "llama3"

# Any OpenAI-compatible HTTP endpoint works
[backends.openai]
command = "https://api.openai.com/v1"
model = "gpt-4o"
api_key = "${OPENAI_API_KEY}"
```

### 2. Define roles

Roles map task types to one or more backends:

```toml
[roles.default]
description = "General queries and built-in workflows"
backends = ["claude", "gemini"]
execution = "first"

[roles.analyzer]
description = "Code analysis"
backends = ["claude", "gemini"]
execution = "parallel"    # first | parallel | fallback

[roles.quick]
description = "Fast local queries"
backends = ["ollama"]
execution = "first"

[roles.coder]
description = "Turn findings into one structured patch"
backends = ["claude"]
execution = "first"
```

`llm-mux init --global` puts every detected backend in the `default` role.
Ordinary queries use its first backend; workflow steps can opt into parallel
execution.

### 3. Create a workflow

Workflows live in `.llm-mux/workflows/` (project) or `~/.config/llm-mux/workflows/` (global):

```toml
name = "review"
description = "Review code changes"

[[steps]]
name = "diff"
type = "shell"
run = "git diff HEAD~1"

[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
prompt = """
Review these changes for bugs, security issues, and improvements:

{{ steps.diff.output }}
"""
depends_on = ["diff"]
```

### 4. Run

```bash
llm-mux run review
llm-mux run review --dry-run    # preview without executing
```

## Workflow Steps

### shell - run a command

```toml
[[steps]]
name = "fetch"
type = "shell"
run = "gh pr diff {{ args.pr }}"
```

### query - call LLM backend(s)

```toml
[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
prompt = "Find bugs in:\n\n{{ steps.fetch.output }}"
depends_on = ["fetch"]
```

### apply - apply LLM-suggested edits

```toml
[[steps]]
name = "fix"
type = "apply"
source = "analyze"
verify = "cargo test"
rollback_on_failure = true
depends_on = ["analyze"]
```

### store - persist findings to SQLite memory

```toml
[[steps]]
name = "save"
type = "store"
prompt = "{{ steps.analyze.output }}"
depends_on = ["analyze"]
```

## Template Variables

Inside prompts and shell commands:

```
{{ args.name }}                   workflow arguments
{{ steps.name.output }}           previous step output
{{ env.VAR }}                     environment variables
{{ team }}                        auto-detected team
{{ ecosystem.name }}              detected ecosystem
{{ ecosystem.knowledge }}         stored ecosystem facts
{{ ecosystem.current_project }}   current project info
```

Filters: `shell_escape`, `json`, `join`, `lines`, `trim`, `truncate_chars`,
`default`.

## Configuration Reference

### Backend options

```toml
[backends.example]
command = "claude"        # CLI command or HTTP base URL
args = ["-p"]             # CLI arguments
model = "gpt-4o"          # model name (HTTP backends)
api_key = "${ENV_VAR}"    # API key; ${VAR} expands from environment
enabled = true
timeout = 300             # seconds
max_retries = 3
input_cost_per_million = 2.50   # optional; enables cost estimates
output_cost_per_million = 10.00 # optional; enables cost estimates
```

Token usage is recorded when a provider reports it. Cost is shown as unknown
unless pricing is configured explicitly; llm-mux does not assume model prices.

## Run history and resume

Every workflow execution is assigned a run ID and stored in
`~/.config/llm-mux/runs.db`. The ledger includes resolved prompts or commands,
outputs, errors, duration, backend/model, token usage, and estimated cost.

```bash
llm-mux runs show 42
llm-mux runs resume 42
```

Resume reloads the workflow from its original project directory, restores only
successful prior step results, and executes the unfinished dependency tail as a
new run. Existing run records are immutable.

### Role execution modes

- `first` - use first available backend
- `parallel` - run all backends, collect all results
- `fallback` - try each backend until one succeeds

### Teams

Auto-detect project type and apply team-specific backend overrides:

```toml
[teams.rust]
detect = ["Cargo.toml"]
verify = "cargo clippy && cargo test"

[teams.rust.roles.analyzer]
backends = ["claude", "codex"]
```

### Ecosystems

Track multi-project systems and seed context into prompts:

```toml
[ecosystems.myapp]
knowledge = [
    "API uses JWT tokens with 1 hour expiration",
    "Redis cache invalidation happens via pub/sub",
]

[ecosystems.myapp.projects.api]
path = "~/projects/myapp-api"
type = "rust"
depends_on = ["database"]
```

## CLI Reference

```
llm-mux run <workflow> [args...]   Run a workflow
llm-mux run <workflow> --dry-run   Preview without executing
llm-mux runs show <id>              Inspect a recorded run
llm-mux runs resume <id>            Resume unfinished steps as a new run
llm-mux validate <workflow>        Validate workflow syntax
llm-mux doctor                     Check backend availability
llm-mux backends                   List configured backends
llm-mux teams                      List configured teams
llm-mux roles                      List configured roles
llm-mux ecosystems                 List configured ecosystems
llm-mux init --global              Generate starter config
llm-mux init --project             Generate project config

Global options:
  --team <name>      Override auto-detected team
  --output <mode>    console | json | quiet
  --debug            Enable debug output
```

## Examples

### Parallel bug hunt across models

```toml
name = "bug-hunt"

[[steps]]
name = "read"
type = "shell"
run = "cat {{ args.file }}"

[[steps]]
name = "hunt"
type = "query"
role = "analyzer"         # parallel across all backends in role
prompt = "Find every bug in this file:\n\n{{ steps.read.output }}"
depends_on = ["read"]
```

### Fix and verify

```toml
name = "fix"

[[steps]]
name = "identify"
type = "query"
role = "analyzer"
prompt = "Identify the bug in {{ args.file }}"

[[steps]]
name = "patch"
type = "query"
role = "coder"
prompt = """
Fix this bug: {{ steps.identify.output }}

Return only JSON in this format:
{"edits":[{"path":"src/file.rs","old":"exact old text","new":"replacement text"}]}
"""
depends_on = ["identify"]

[[steps]]
name = "apply"
type = "apply"
source = "patch"
verify = "cargo test"
rollback_on_failure = true
depends_on = ["patch"]
```

### Iterate over files

```toml
[[steps]]
name = "list"
type = "shell"
run = "git diff --name-only HEAD~1"

[[steps]]
name = "review-each"
type = "query"
role = "analyzer"
for_each = "steps.list.output | lines"
prompt = "Review the changed file {{ item }}. Inspect it in the working tree."
depends_on = ["list"]
```

## Contributing

```bash
git clone https://github.com/ducks/llm-mux
cd llm-mux
cargo build
cargo test
```

## License

MIT
