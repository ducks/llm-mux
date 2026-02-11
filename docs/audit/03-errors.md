Now I have all the files. Let me compile the findings into the audit report.

# Error Handling Audit

## Summary
status: good
unwrap_count: 2 (in non-test code)

## Findings

### Unwrap/Expect in Non-Test Code

| File | Line | Code | Severity |
|------|------|------|----------|
| `src/apply_and_verify/diff_applier.rs` | 144-147 | `unwrap_or_default()` on `SystemTime::now().duration_since()` | Low - appropriate fallback |
| `src/template/engine.rs` | 53 | `unwrap_or(0)` on line extraction | Low - appropriate fallback |

### Missing Error Context

| File | Line | Issue |
|------|------|-------|
| `src/apply_and_verify/retry_loop.rs` | 161-167 | Rollback result ignored with `let _ = rollback(...)` - failures silently swallowed |
| `src/apply_and_verify/rollback.rs` | 168 | Backup cleanup failure ignored: `let _ = fs::remove_file(&file.backup_path)` |
| `src/apply_and_verify/rollback.rs` | 201 | Same pattern in `cleanup_backups` |
| `src/cli/signals.rs` | 41 | `let _ = self.sender.send(true)` - cancellation signal failure ignored |
| `src/apply_and_verify/diff_applier.rs` | 276 | `HunkContextNotFound` error constructed with empty `PathBuf::new()` - loses path context |

### Regex Compilation

| File | Line | Issue |
|------|------|-------|
| `src/apply_and_verify/edit_parser.rs` | 136-138 | `Regex::new(...).unwrap()` - panics on invalid regex (though these are compile-time constants, they could be `lazy_static` or `once_cell`) |
| `src/apply_and_verify/edit_parser.rs` | 279 | Same pattern for JSON block extraction |

### Signal Handler Panics

| File | Line | Issue |
|------|------|-------|
| `src/cli/signals.rs` | 71-72 | `.expect()` on signal handler setup - panics if signal handling fails |
| `src/cli/signals.rs` | 92 | `.expect()` on Ctrl+C handler (Windows path) |

### Debug Output in Production

| File | Line | Issue |
|------|------|-------|
| `src/role/role_executor.rs` | 138 | `eprintln!("[DEBUG role_executor] AllFailed errors: {:?}", failed)` - debug print in non-test code |

### Proper Error Handling (Positive Examples)

- All workflow/executor errors use `thiserror` with descriptive variants
- Template errors include source locations and suggestions
- Apply-verify errors properly chain with `#[from]`
- Config loading uses `anyhow::Context` for file operation errors
- Step execution properly propagates errors with context

## Recommendations

1. **High Priority - Swallowed Errors**
   - `src/apply_and_verify/retry_loop.rs:161` - Log or propagate rollback failures; silent failure during recovery is dangerous
   - Consider returning partial success/failure info from `apply_and_verify`

2. **Medium Priority - Debug Artifacts**
   - `src/role/role_executor.rs:138` - Remove or gate behind a debug flag

3. **Medium Priority - Lazy Regex**
   - `src/apply_and_verify/edit_parser.rs` - Use `once_cell::sync::Lazy` or `std::sync::LazyLock` for regex patterns to avoid repeated compilation and document that they are infallible

4. **Low Priority - Signal Handlers**
   - `src/cli/signals.rs:71-72,92` - Consider graceful degradation if signal handlers fail to install (log warning, continue without signal handling)

5. **Low Priority - Path Context**
   - `src/apply_and_verify/diff_applier.rs:276` - Pass actual path to `HunkContextNotFound` error for better debugging
