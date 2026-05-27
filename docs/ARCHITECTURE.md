# Architecture — Rust-Only Features

This document tracks features that exist in the Rust implementation but not
in the upstream Python `mempalace` repo. These features must be preserved
during Python-parity refactoring and must not be replaced with Python-
equivalent implementations.

## Feature Table

| Feature | File | Description | Risk if Removed |
|---------|------|-------------|-----------------|
| hermes_integration | `hermes_integration.rs` | Multi-agent Agent Mail coordination | Agents cannot coordinate |
| MiningMode::Auto | `cli.rs` (lines 361, 730, 924, 929) | Auto-detect mining targets | Loss of automatic mode |
| XDG base dirs | `config.rs` | XDG-compliant config/data paths | Breaks Linux/macOS conventions |
| doctor subcommand | `cli.rs` | Self-diagnostics and repair CLI | Loss of self-healing diagnostics |
| languages module | `languages.rs` | i18n and language detection | Loss of language support |

## Detailed Descriptions

### 1. Hermes Integration (`hermes_integration.rs`)

**Location:** `crates/core/src/hermes_integration.rs`

This module provides multi-agent coordination via the Agent Mail system. It enables
separate agent processes to communicate, coordinate on tasks, share context, and
resolve conflicts through a message-passing architecture with file-based reservations
to prevent concurrent edits to the same resources.

**Why it exists in Rust:** The Rust implementation supports parallel agent execution
with explicit coordination mechanisms that Python's simpler architecture does not
require. The Python version runs primarily sequentially with less need for inter-
process communication.

**What breaks if removed:** Agents cannot coordinate on shared tasks. File conflict
resolution disappears. The multi-agent workflow orchestration becomes impossible.

---

### 2. MiningMode::Auto (`cli.rs` lines 361, 730, 924, 929)

**Location:** `crates/core/src/cli.rs`

The `MiningMode::Auto` variant enables automatic mining mode where the CLI
automatically detects what targets to mine based on the project context, file
types present, and previously mined patterns. This removes the need for users to
explicitly specify mining targets.

**Why it exists in Rust:** The Rust version implements intelligent auto-detection
as a first-class CLI mode, whereas Python relies on explicit enumeration of mining
targets. This enables a simpler UX for new users.

**What breaks if removed:** Loss of automatic mode — users must manually specify
all mining targets. The `mpr mine --auto` command fails. Simplified onboarding UX
disappears.

---

### 3. XDG Base Directories (`config.rs`)

**Location:** `crates/core/src/config.rs`

Uses the `dirs_next` crate (or similar XDG-compatible library) to locate config
and data directories according to the XDG Base Directory Specification. On Linux
and macOS, this means config lives in `$XDG_CONFIG_HOME/mempalace` (default:
`~/.config/mempalace`) and data lives in `$XDG_DATA_HOME/mempalace` (default:
`~/.local/share/mempalace`). On Windows, it falls back to platform-specific paths.

**Why it exists in Rust:** Rust's ecosystem convention favors XDG compliance for
cross-platform consistency. The Python version uses `platformdirs` or direct
platform-specific paths (`~/.mempalace` on Unix, `%APPDATA%` on Windows).

**What breaks if removed:** Linux/macOS users lose XDG-compliant paths. Config bleeds
into home directory. Distribution packaging (Debian, Arch, etc.) may break since
they expect XDG locations.

---

### 4. Doctor Subcommand (`cli.rs`)

**Location:** `crates/core/src/cli.rs` — `doctor` subcommand

The `mpr doctor` subcommand runs self-diagnostics on the installation, checking:
database integrity, config file validity, installed language models, hook scripts,
disk space, and connectivity to external services. It can automatically repair
certain issues (stale locks, corrupted indexes, missing directories).

**Why it exists in Rust:** Rust's emphasis on zero-config tooling and self-contained
binaries leads to stronger built-in diagnostic capabilities. Python's mempalace
relies on external scripts or manual troubleshooting.

**What breaks if removed:** Loss of self-healing diagnostics. Users cannot run
`mpr doctor` to diagnose problems. Automated repair capabilities disappear. Support
burden increases.

---

### 5. Languages Module (`languages.rs`)

**Location:** `crates/core/src/languages.rs`

Provides internationalization (i18n) support and language detection for the CLI.
This includes utilities for determining source code languages present in a project,
localization string management, and runtime language detection based on environment
variables or system locale.

**Why it exists in Rust:** The Rust version implements language detection as a
first-class module with explicit types and logic. It supports Rust-specific i18n
patterns and can be extended for localization of CLI output.

**What breaks if removed:** Language detection for multi-language projects is
weakened. Any localization or internationalization of CLI output becomes
impossible. Language-based mining filters may not work correctly.

---

## Preservation Tests

The following scenarios would fail if these Rust-only features were removed:

| Feature | Test That Would Fail |
|---------|---------------------|
| hermes_integration | `cargo test --package mempalace-core --test agent_mail_integration` — tests agent coordination |
| MiningMode::Auto | `cargo run -- mine --auto .` — auto-detection fails silently or errors |
| XDG base dirs | Config/data dirs not created in XDG locations on Linux; `ls ~/.config/mempalace` returns empty |
| doctor subcommand | `cargo run -- doctor` → "unrecognized command `doctor`" |
| languages module | `cargo test --package mempalace-core -- languages` — language detection tests fail |

---

## References

- **Python equivalent:** None for all 5 features (Rust-only)
- **Related beads:** `mr-3r8` (this document), `mr-iwi` (Python parity tracking)
- **Upstream Python repo:** `milla-jovovich/mempalace`

---

*Last updated: 2026-05-27*
*This document must be updated whenever Rust-only features are added or removed.*