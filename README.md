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
api_key = "${ENV_VAR}"       # API key; `${VAR}` placeholders expand at load
enabled = true               # enable/disable
timeout = 300                # seconds
max_retries = 3              # retry attempts
retry_delay = 1000           # base delay in ms (exponential backoff)
retry_rate_limit = true      # auto-retry on rate limits
retry_timeout = false        # auto-retry on timeouts
```

`api_key` supports `${VAR}` placeholders that are expanded against the
process environment when the config is loaded. Multiple placeholders in
one value are fine. Bare `$` characters (without `{`) pass through
unchanged, so literal keys like `sk-abc123` also work. If a referenced
variable is unset or empty, loading fails with a clear error rather than
silently sending `Bearer ${VAR}` to the provider.

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

### Ecosystems

Track relationships between projects and store ecosystem knowledge:

```toml
[ecosystems.myapp]
description = "MyApp web application and services"
knowledge = [
    "API uses JWT tokens with 1 hour expiration",
    "Database migrations run automatically on deploy",
    "Redis cache invalidation happens via pub/sub"
]

[ecosystems.myapp.projects.frontend]
path = "~/projects/myapp-frontend"
type = "javascript"
depends_on = ["api"]
tags = ["production", "web"]

[ecosystems.myapp.projects.api]
description = "REST API backend"
path = "~/projects/myapp-api"
type = "rust"
depends_on = ["database"]
tags = ["production", "backend"]

[ecosystems.myapp.projects.database]
description = "PostgreSQL database with migrations"
path = "~/projects/myapp-db"
type = "sql"
tags = ["infrastructure", "database"]

[ecosystems.myapp.projects.worker]
description = "Background job processor"
path = "~/projects/myapp-worker"
type = "rust"
depends_on = ["database", "api"]
tags = ["production", "background"]
```

Workflows automatically detect which ecosystem you're in and can access:

```jinja2
{{ ecosystem.name }}                    - Ecosystem name
{{ ecosystem.description }}             - Description
{{ ecosystem.knowledge }}               - Array of facts
{{ ecosystem.projects }}                - All projects
{{ ecosystem.current_project.name }}    - Current project name
{{ ecosystem.current_project.type }}    - Project type
{{ ecosystem.current_project.depends_on }} - Dependencies
{{ ecosystem.current_project.tags }}    - Project tags
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

# Store: persist data to SQLite memory database
[[steps]]
name = "save_facts"
type = "store"
prompt = "{{ steps.analyze.output }}"
depends_on = ["analyze"]
```

### Template Variables

- `{{ args.name }}`: workflow arguments
- `{{ steps.name.output }}`: previous step output
- `{{ env.VAR }}`: environment variables
- `{{ team }}`: detected team name
- `{{ ecosystem.name }}`: detected ecosystem
- `{{ ecosystem.knowledge }}`: ecosystem facts
- `{{ ecosystem.current_project }}`: current project info

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

### Store Steps and Ecosystem Memory

Store steps persist LLM analysis results to a SQLite database for later querying.

**Database Location:**
`~/.config/llm-mux/memory/<ecosystem-name>.db`

**Data Format:**
Store steps parse JSON from previous steps and save to the memory database.

Facts require:
```json
{
  "facts": [
    {
      "project": "project-name",
      "fact": "description",
      "source": "where found (e.g., Cargo.toml)",
      "confidence": 1.0
    }
  ]
}
```

Relationships require:
```json
{
  "relationships": [
    {
      "from": "source-project",
      "to": "target-project",
      "type": "depends_on|calls_api|shares_db|deploys_with",
      "evidence": "brief explanation"
    }
  ]
}
```

**Usage:**
```toml
[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
prompt = """
Analyze the codebase and return facts.

IMPORTANT: Return JSON with this exact structure:
{
  "facts": [
    {"project": "myapp", "fact": "Uses PostgreSQL", "source": "config.yml", "confidence": 1.0}
  ]
}
"""

[[steps]]
name = "store"
type = "store"
prompt = "{{ steps.analyze.output }}"
depends_on = ["analyze"]
```

See `examples/workflows/discover-ecosystem.toml` for a complete example.

## CLI Reference

```
llm-mux run <workflow> [args...]   Run a workflow
llm-mux validate <workflow>        Validate workflow syntax
llm-mux doctor                     Check backend availability
llm-mux backends                   List configured backends
llm-mux teams                      List configured teams
llm-mux roles                      List configured roles
llm-mux ecosystems                 List configured ecosystems

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

### Ecosystem-Aware Bug Hunt

```toml
name = "bug-hunt"
description = "Search for bugs across ecosystem"

[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
prompt = """
Search for potential bugs in {{ ecosystem.current_project.name }}.

Project type: {{ ecosystem.current_project.type }}

Known ecosystem facts:
{% for fact in ecosystem.knowledge %}
- {{ fact }}
{% endfor %}

Dependencies to consider:
{% for dep in ecosystem.current_project.depends_on %}
- {{ dep }}
{% endfor %}

Focus on common issues for {{ ecosystem.current_project.type }} projects.
"""
```

### Ecosystem Discovery

Analyze projects and store findings in the memory database:

```toml
name = "discover"
description = "Discover and store ecosystem facts"

[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
timeout = 180
prompt = """
Analyze the {{ ecosystem.current_project.name }} project structure.

IMPORTANT: Return JSON with this exact structure:
{
  "facts": [
    {
      "project": "project-name",
      "fact": "brief fact description",
      "source": "where you found this (e.g., Cargo.toml)",
      "confidence": 1.0
    }
  ]
}

Each fact MUST have all four fields: project, fact, source, confidence.
"""

[steps.output_schema]
type = "object"
required = ["facts"]

[steps.output_schema.properties.facts]
type = "array"

[[steps]]
name = "store"
type = "store"
prompt = "{{ steps.analyze.output }}"
depends_on = ["analyze"]
```

See `examples/workflows/discover-ecosystem.toml` for a complete implementation with multiple analysis steps and relationship discovery.

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
