# Performance Audit

## Summary
status: some_issues

## Issues Found
- src/workflow/runner.rs:80 - O(n) linear search for step by name inside execution loop; could use HashMap for O(1) lookup
- src/workflow/runner.rs:164 - topological_sort clones step names repeatedly (`.to_string()`) inside recursive visit function
- src/workflow/runner.rs:305 - `json_to_minijinja_value` clones strings from JSON values unnecessarily
- src/role/team_detector.rs:80 - `detect_team` clones the entire teams HashMap on each call to create TeamDetector
- src/apply_and_verify/edit_parser.rs:136-138 - Regex compiled on every call to `parse_unified_diff`; should use `lazy_static` or `OnceLock`
- src/apply_and_verify/edit_parser.rs:279 - Regex compiled on every call to `extract_json_block`
- src/apply_and_verify/diff_applier.rs:306 - `normalize_whitespace` called twice per line during fuzzy matching (lines 290, 358)
- src/apply_and_verify/diff_applier.rs:310 - `match_indices` creates Vec allocation; only first match needed
- src/template/engine.rs:51 - Environment cloned on every render call
- src/template/conditionals.rs:26-27 - New minijinja Environment created for every condition evaluation
- src/template/conditionals.rs:62-63 - New minijinja Environment created for every expression evaluation
- src/template/errors.rs:162 - Levenshtein uses O(n*m) matrix allocation; could use two-row optimization
- src/template/filters.rs:123 - `filter_trim` allocates twice (`.to_string().trim().to_string()`)

## Recommendations
1. **High priority**: Cache compiled regexes in `edit_parser.rs` using `std::sync::LazyLock` or `once_cell::sync::Lazy` - currently recompiled on every parse
2. **High priority**: Reuse minijinja Environment in template engine instead of cloning per render - create Environment once and use `render_str` or similar
3. **Medium priority**: Build step lookup HashMap once before execution loop in `runner.rs:80` instead of iterating steps array per step
4. **Medium priority**: Pass `&HashMap<String, TeamConfig>` by reference in `detect_team` instead of cloning
5. **Low priority**: Use two-row Levenshtein algorithm to reduce memory from O(n*m) to O(min(n,m))
6. **Low priority**: Avoid double string allocation in `filter_trim` by trimming in place or using a single allocation
