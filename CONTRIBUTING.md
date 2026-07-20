# Contributing to Monorepa Impact

Thank you for helping improve Monorepa Impact.

## Before you start

- Search existing issues and pull requests before opening a duplicate.
- Open an issue before a large behavioral or public-interface change.
- Read [AGENTS.md](./AGENTS.md). Its correctness, precision, and cache invariants are
  part of the project contract.
- Keep the relevant guide in `docs/` aligned when changing graph semantics,
  configuration, cache behavior, CLI flags, or JSON output.
- Keep changes focused. Separate refactors from behavior changes when practical.

## Development setup

Install Rust 1.89 or newer and Node.js 22 or newer, then verify the repository:

```bash
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
npm run test:package
```

Build and exercise the release CLI:

```bash
cargo build --release
./target/release/monorepa --help
```

## Tests

- Add unit coverage for parser, resolver, traversal, and configuration logic.
- Add native E2E coverage for user-visible CLI behavior, cache transitions, workspace
  exports, affected-project selection, and reverse-dependency results.
- Rust tests must use temporary repositories and fixtures and must not depend on a
  developer's global Git configuration, installed Node.js runtime, or existing project
  cache.
- Keep core Rust tests independent of Node.js. Test npm launcher and packaging behavior
  through the dedicated npm validation commands.
- When changing `skills/monorepa-find-dependencies`, run the skill validator and
  `npm run test:skill`; keep its prompts, wrapper, and README example aligned.
- Run the benchmark after changing cache or hot-path behavior:

```bash
cargo test --release --test performance -- --ignored --nocapture
```

## Pull requests

A pull request should:

- explain the user-visible behavior and motivation;
- call out JSON, configuration, or cache compatibility changes;
- include focused tests;
- update README and contributor documentation when contracts change;
- pass tests, formatting, Clippy, and `git diff --check`;
- document remaining conservative behavior or known false-positive risk.

When changing versions, update the root package, every platform package, and all root
`optionalDependencies` together. Never publish the root package before its platform
packages are available at the same version.

Do not include generated build output, local caches, editor settings, or unrelated
formatting changes.
