# Microscope numeric configuration

**Built:** Microscope parses numeric environment settings at startup, after
initializing its existing tracing subscriber. Unset and empty values use their
defaults without logging. A nonempty malformed or overflowing value uses the
same default and emits one warning naming the setting, supplied value and fallback.
The supplied value uses debug formatting so control characters cannot create
additional log lines.

| Setting | Default |
|---|---|
| `MICROSCOPE_AUDIT_WINDOW_HOURS` | 24 |
| `MICROSCOPE_AUDIT_MAX_ROWS` | 0, meaning the adapter's default row limit |
| `MICROSCOPE_SUGGEST_TTL_SECS` | 120 |

Valid numeric values retain their existing meaning. Unit tests in
`crates/microscope/src/config.rs` cover valid values, silent defaults, malformed
values, overflow and log-line encoding without changing the process environment.
