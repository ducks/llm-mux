# Example Workflows

This directory contains example workflows that demonstrate llm-mux capabilities.

## Knowledge Building Workflows

### learn-from-pr.toml

Learn patterns from GitHub pull requests by analyzing what changed and why.

**Usage:**
```bash
llm-mux run learn-from-pr pr=123
```

**What it does:**
- Fetches PR data via `gh pr view`
- Analyzes file changes, patterns, and conventions
- Extracts entities (components, patterns) and relationships
- Stores knowledge in memory database
- Summarizes learnings

**Use case:** Build up knowledge about your codebase by analyzing merged PRs. After analyzing 20-50 PRs, you'll have a rich knowledge base of patterns, conventions, and component relationships.

### query-knowledge.toml

Query the learned knowledge base to get insights and recommendations.

**Usage:**
```bash
llm-mux run query-knowledge question="How do I add a new API endpoint?" db=myproject
```

**What it does:**
- Queries the memory database for relevant patterns
- Synthesizes answer from learned knowledge
- Provides specific examples and recommendations
- Suggests files that typically change together

**Use case:** Ask questions about your codebase and get answers based on learned patterns from actual PRs.

## Discovery Workflows

### discover-entities.toml

Discover structured entities with properties (uses normalized entity storage).

**Usage:**
```bash
llm-mux run discover-entities
```

**What it does:**
- Analyzes project structure (Cargo.toml, package.json, etc.)
- Extracts dependencies, features, services
- Stores entities with properties in memory database

### discover-ecosystem.toml

Build a comprehensive map of a project ecosystem with multiple related repositories.

**Usage:**
```bash
llm-mux run discover-ecosystem paths=/path/to/repo1,/path/to/repo2
```

**What it does:**
- Analyzes multiple related projects
- Maps dependencies and relationships between projects
- Stores ecosystem knowledge in memory database

## Performance Workflows

### perf-analysis.toml

Analyze performance characteristics and identify bottlenecks.

**Usage:**
```bash
llm-mux run perf-analysis
```

**What it does:**
- Searches for performance-critical code patterns
- Identifies potential bottlenecks (N+1 queries, etc.)
- Generates performance improvement recommendations

## Creating Your Own Workflows

Workflows are TOML files that define a series of steps. Common step types:

- **shell**: Run shell commands (git, gh, build tools, etc.)
- **query**: Ask LLM backends to analyze data
- **store**: Save extracted knowledge to memory database
- **apply**: Apply code changes with verification

See the [workflow documentation](../../docs/workflows.md) for details on creating custom workflows.

## Typical Workflow Pattern

1. **Discover** - Use discovery workflows to build initial knowledge
2. **Learn** - Regularly run learn-from-pr on new PRs
3. **Query** - Ask questions to get guidance from learned patterns
4. **Iterate** - Knowledge compounds over time

## Memory Database

Learned knowledge is stored in `~/.config/llm-mux/memory/{db-name}.db` as SQLite databases.

Each project should have its own database. Use the `db` argument to specify which project's knowledge to query.
