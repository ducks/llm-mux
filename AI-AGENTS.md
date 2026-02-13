# AI Agent Guide for llm-mux

Instructions for AI coding agents working on this codebase.

## Project Overview

llm-mux is a multiplexer for LLMs. It routes prompts to multiple backends, runs
them in parallel, and orchestrates multi-step workflows. Think of it as a router
and workflow engine for LLM queries.

**Core concepts:**
- **Backends**: LLM providers (CLI commands or HTTP APIs)
- **Roles**: Map task types to backends with execution strategies
- **Workflows**: TOML pipelines with steps (shell, query, apply)
- **Templates**: Jinja2-style variable interpolation between steps

## Project Structure

```
src/
├── main.rs               # CLI entry point
├── lib.rs                # Library exports
├── process.rs            # Child process management
├── utils.rs              # Shared utilities (error classification, etc.)
├── cli/
│   ├── mod.rs
│   ├── commands.rs       # CLI command handlers
│   ├── output.rs         # Terminal output formatting
│   └── signals.rs        # Signal handling (Ctrl+C, etc.)
├── config/
│   ├── mod.rs
│   ├── loader.rs         # Config file loading and merging
│   ├── backend.rs        # Backend configuration types
│   ├── workflow.rs       # Workflow configuration types
│   └── error.rs          # Config-specific errors
├── template/
│   ├── mod.rs
│   ├── engine.rs         # MiniJinja template engine wrapper
│   ├── context.rs        # Template context building
│   ├── filters.rs        # Custom Jinja filters (shell_escape, etc.)
│   ├── conditionals.rs   # Conditional logic in templates
│   └── errors.rs         # Template-specific errors
├── workflow/
│   ├── mod.rs
│   ├── runner.rs         # Top-level workflow execution
│   ├── executor.rs       # Step execution logic
│   └── state.rs          # Workflow state management
├── role/
│   ├── mod.rs
│   ├── role_resolver.rs  # Resolve role -> backends
│   ├── role_executor.rs  # Execute queries via roles
│   └── team_detector.rs  # Auto-detect team configurations
└── apply_and_verify/
    ├── mod.rs
    ├── edit_parser.rs    # Parse LLM edit suggestions
    ├── diff_applier.rs   # Apply diffs to files
    ├── verification.rs   # Run verification commands
    ├── retry_loop.rs     # Retry failed verifications
    └── rollback.rs       # Rollback failed changes
```

## Build & Test

This project uses NixOS. Always run commands via nix-shell:

```bash
cd ~/dev/llm-mux
nix-shell --run "cargo build"
nix-shell --run "cargo test"
nix-shell --run "cargo clippy"

# Run llm-mux
nix-shell --run "cargo run -- doctor"
nix-shell --run "cargo run -- run workflow-name"
```

## Coding Conventions

### Error Handling
- Use `anyhow::Result` for fallible functions
- Use `thiserror` for custom error types
- Prefer `context()` over `unwrap()` for better error messages
- Classify backend errors in `utils.rs` for user-friendly messages

### Async
- All backend queries are async
- Use `tokio` runtime
- Use `tokio::join!` for parallel execution (not sequential reads)
- Be careful with stdout/stderr reads - use `tokio::join!` to avoid deadlocks

### Process Management
- Always reap child processes after killing them
- Capture exit codes even on read failures
- Use `kill()` + `wait()` pattern, never leave zombies

### Output
- Use `colored` crate for terminal colors
- Keep output concise
- Summarize backend errors to one-liners (see `utils.rs`)

### Templates
- Use MiniJinja (`{{ variable }}` syntax)
- `shell_escape` filter handles quoting - don't add extra quotes
- For multi-line content, use heredocs in shell steps

## Config Format

Config at `~/.config/llm-mux/config.toml`. Don't break backwards compatibility.

```toml
[backends.claude]
command = "claude"
args = ["-p"]
timeout = 120000

[backends.codex]
command = "codex"
args = ["exec", "-q"]

[roles.analyzer]
backends = ["claude", "codex"]
execution = "parallel"
```

## Workflow Format

Workflows in `.llm-mux/workflows/` or `~/.config/llm-mux/workflows/`.

```toml
name = "example"
description = "Example workflow"

[[steps]]
name = "gather"
type = "shell"
run = "git diff"

[[steps]]
name = "analyze"
type = "query"
role = "analyzer"
prompt = "Analyze: {{ steps.gather.output }}"
depends_on = ["gather"]
continue_on_error = true
min_deps_success = 2
```

Key features:
- `depends_on`: Step dependencies
- `continue_on_error`: Don't fail workflow on step failure
- `min_deps_success`: Minimum backends that must succeed
- `timeout`: Per-step timeout in milliseconds
- `verify`: Run verification after apply steps

## Things to Avoid

- Don't add dependencies without good reason
- Don't change CLI argument names (breaks scripts)
- Don't remove config fields (breaks existing configs)
- Don't use `unwrap()` on user input or network responses
- Don't read stdout and stderr sequentially (causes deadlocks)
- Don't leave child processes running after errors

## Common Pitfalls

### Deadlocks
Sequential reads of stdout/stderr can deadlock if buffer fills:
```rust
// BAD - can deadlock
let stdout = child.stdout.read_to_string()?;
let stderr = child.stderr.read_to_string()?;

// GOOD - parallel reads
let (stdout, stderr) = tokio::join!(
    read_stream(child.stdout),
    read_stream(child.stderr)
);
```

### Zombie Processes
Always reap after killing:
```rust
// BAD - leaves zombie
child.kill()?;

// GOOD - reaps process
child.kill()?;
child.wait()?;
```

### Shell Escaping
The `shell_escape` filter adds quotes, don't double-quote:
```toml
# BAD - double quotes
run = "echo '{{ value | shell_escape }}'"

# GOOD - filter handles quoting
run = "echo {{ value | shell_escape }}"
```

## Testing Locally

```bash
nix-shell --run "cargo run -- doctor"
nix-shell --run "cargo run -- run workflow-name"
nix-shell --run "cargo test"
```
