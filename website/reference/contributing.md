# Contributing

PRs welcome. MemPalace Rust is open source and we welcome contributions of all sizes — from typo fixes to new features.

## Getting Started

```bash
git clone https://github.com/quangdang46/mempalace_rust.git
cd mempalace_rust
cargo build --release
```

## Running Tests

```bash
# Run all tests
cargo test --all

# Run with output
cargo test --all -- --nocapture

# Run specific test
cargo test test_name
```

All tests must pass before submitting a PR. Tests should run without API keys or network access.

## Running Benchmarks

```bash
# Run memory stack benchmarks
cargo bench

# Run specific benchmark
cargo bench -- search_memories
```

## Building Documentation

```bash
# Build the website locally
cd website && bun install && bun run docs:build
```

## PR Guidelines

1. Fork the repo and create a feature branch: `git checkout -b feat/my-thing`
2. Write your code
3. Add or update tests if applicable
4. Run `cargo fmt`, `cargo clippy`, and `cargo test --all` — everything must pass
5. Commit with clear [conventional commits](https://www.conventionalcommits.org/):
   - `feat: add Notion export format`
   - `fix: handle empty transcript files`
   - `docs: update MCP tool descriptions`
   - `bench: add LoCoMo turn-level metrics`
6. Push to your fork and open a PR against `main`

## Code Style

- **Formatting**: `cargo fmt` (rustfmt)
- **Linting**: `cargo clippy`
- **Naming**: `snake_case` for functions/variables, `PascalCase` for types
- **Docstrings**: on all public modules and functions
- **Error handling**: Use `Result` types with descriptive error messages
- **Dependencies**: Minimize. Don't add new deps without discussion.

## Good First Issues

Check the [Issues](https://github.com/quangdang46/mempalace_rust/issues) tab:

- **New chat formats** — add import support for Cursor, Copilot, or other AI tool exports
- **Room detection** — improve pattern matching in `room_detector_local.rs`
- **Tests** — increase coverage, especially for `knowledge_graph.rs` and `palace_graph.rs`
- **Entity detection** — better name disambiguation in `entity_detector.rs`
- **Docs** — improve examples, add tutorials

## Architecture Decisions

If you're planning a significant change, open an issue first. Key principles:

- **Verbatim first** — never summarize user content. Store exact words.
- **Local first** — everything runs on the user's machine. No cloud dependencies.
- **Zero API by default** — core features must work without any API key.
- **Palace structure is scoping, not magic** — wings, halls, and rooms act as metadata filters in the underlying vector store. They make scoping predictable when a palace holds many unrelated projects; they are not a novel retrieval mechanism.

## Community

- [Discord](https://discord.com/invite/ycTQQCu6kn)
- [GitHub Issues](https://github.com/quangdang46/mempalace_rust/issues) — bug reports and feature requests
- [GitHub Discussions](https://github.com/quangdang46/mempalace_rust/discussions) — questions and ideas

## License

MIT — your contributions will be released under the same license.
