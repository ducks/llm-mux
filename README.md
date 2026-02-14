# llm-mux

Multiplexer for LLMs. Route prompts to multiple backends, run them in parallel,
and orchestrate multi-step workflows.

## Quick Start

### 1. Install

```bash
# From crates.io
cargo install llm-mux

# Or from source
cargo install --path .
```

### 2. Configure Backends

Create `~/.config/llm-mux/config.toml`:

```toml
# CLI backends (any command that accepts a prompt)
[backends.claude]
command = "claude"
args = ["-p"]

[backends.codex]
command = "codex"
args = ["exec", "-q"]

[backends.ollama]
command = "ollama"
args = ["run", "llama3"]

# HTTP backends (OpenAI-compatible APIs)
[backends.openai]
command = "https://api.openai.com/v1"
model = "gpt-4"
api_key = "${OPENAI_API_KEY}"  # from environment

[backends.local]
command = "http://localhost:11434/v1"
model = "llama3"
```

### 3. Define Roles

Roles map task types to backends:

```toml
[roles.analyzer]
description = "Code analysis tasks"
backends = ["claude", "codex"]
execution = "parallel"  # first | parallel | fallback

[roles.quick]
description = "Fast responses"
backends = ["ollama"]
execution = "first"
```

### 4. Create a Workflow

Create `.llm-mux/workflows/review.toml` in your project:

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
Review these changes for bugs and improvements:

{{ steps.diff.output }}
"""
depends_on = ["diff"]
```

### 5. Run It

```bash
llm-mux run review
```

## Configuration

### Config Locations

1. `~/.config/llm-mux/config.toml` (user defaults)
2. `.llm-mux/config.toml` (project overrides)

Later files override earlier ones.

### Backend Options

```toml
[backends.example]
command = "claude"           # CLI command or HTTP URL
args = ["-p"]                # arguments for CLI backends
model = "gpt-4"              # model name for HTTP backends
api_key = "${ENV_VAR}"       # API key (supports env vars)
enabled = true               # enable/disable
timeout = 300                # seconds
max_retries = 3              # retry attempts
retry_delay = 1000           # base delay in ms (exponential backoff)
retry_rate_limit = true      # auto-retry on rate limits
retry_timeout = false        # auto-retry on timeouts
```

### Role Execution Modes

- `first`: Use first available backend (default)
- `parallel`: Run all backends, collect results
- `fallback`: Try each backend until one succeeds

### Teams

Auto-detect project type and apply team-specific settings:

```toml
[teams.rust]
description = "Rust projects"
detect = ["Cargo.toml"]
verify = "cargo clippy && cargo test"

[teams.rust.roles.analyzer]
backends = ["claude", "codex"]  # override for Rust projects
```

## Workflows

### Step Types

```toml
# Shell: run a command
[[steps]]
name = "fetch"
type = "shell"
run = "gh issue view {{ args.issue }}"

# Query: call LLM backend(s)
[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
prompt = "Analyze: {{ steps.fetch.output }}"
depends_on = ["fetch"]

# Apply: apply edits from LLM output
[[steps]]
name = "fix"
type = "apply"
source = "steps.analyze"
verify = "cargo test"
verify_retries = 2
rollback_on_failure = true
depends_on = ["analyze"]
```

### Template Variables

- `{{ args.name }}`: workflow arguments
- `{{ steps.name.output }}`: previous step output
- `{{ env.VAR }}`: environment variables
- `{{ team }}`: detected team name

### Filters

```
{{ value | shell_escape }}   # escape for shell
{{ value | json }}           # JSON encode
{{ list | join(", ") }}      # join array
{{ text | lines }}           # split into lines
{{ text | trim }}            # trim whitespace
{{ value | default("x") }}   # default if empty
```

### Conditionals

```toml
[[steps]]
name = "rust-only"
type = "shell"
run = "cargo clippy"
if = "team == 'rust'"
```

### Iteration

```toml
[[steps]]
name = "check-files"
type = "shell"
run = "wc -l {{ item }}"
for_each = "steps.list.output | lines"
```

## CLI Reference

```
llm-mux run <workflow> [args...]   Run a workflow
llm-mux validate <workflow>        Validate workflow syntax
llm-mux doctor                     Check backend availability
llm-mux backends                   List configured backends
llm-mux teams                      List configured teams
llm-mux roles                      List configured roles

Options:
  --team <name>      Override team detection
  --output <mode>    Output format: console, json, quiet
  --debug            Enable debug output
  --quiet            Suppress progress output
```

## Examples

### Simple Review

```toml
# .llm-mux/workflows/review.toml
name = "review"

[[steps]]
name = "diff"
type = "shell"
run = "git diff"

[[steps]]
name = "review"
type = "query"
role = "analyzer"
prompt = "Review this diff:\n{{ steps.diff.output }}"
depends_on = ["diff"]
```

### Parallel Analysis

```toml
name = "analyze"

[roles.multi]
backends = ["claude", "codex", "gemini"]
execution = "parallel"

[[steps]]
name = "analyze"
type = "query"
role = "multi"
prompt = "Find bugs in: {{ args.file }}"
```

### Fix with Verification

```toml
name = "fix"

[[steps]]
name = "identify"
type = "query"
role = "analyzer"
prompt = "Find the bug in {{ args.file }}"

[[steps]]
name = "fix"
type = "query"
role = "coder"
prompt = """
Fix this bug: {{ steps.identify.output }}

Return edits as JSON: {"path": "...", "old": "...", "new": "..."}
"""
depends_on = ["identify"]

[[steps]]
name = "apply"
type = "apply"
source = "steps.fix"
verify = "cargo test"
verify_retries = 2
depends_on = ["fix"]
```

## Contributing

llm-mux is built with Rust. To contribute:

```bash
git clone https://github.com/ducks/llm-mux
cd llm-mux
cargo build
cargo test
```

## Publishing

Published to [crates.io](https://crates.io/crates/llm-mux) and [GitHub](https://github.com/ducks/llm-mux).

## License

MIT
