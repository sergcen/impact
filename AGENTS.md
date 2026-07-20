# Monorepa Impact contributor and agent guide

These instructions apply to the entire repository. Read them before changing graph
semantics, parsing, resolution, cache formats, or CLI output.

## Mission

Maintain one universal native dependency graph for:

- Git-based affected-project selection;
- direct and transitive module dependents;
- binding- and specifier-level selectivity.

Task names such as `typecheck`, `test`, and `build` must never change graph topology.
Command applicability belongs to the caller.

## Non-negotiable invariants

1. Keep dependency analysis and CLI startup native. JavaScript is limited to the
   install-time `npm/install.cjs` platform selection and `npm/*.cjs` release metadata
   tooling. The installed CLI must execute the Rust binary directly without a Node.js
   launcher. Do not move parsing, resolution, graph construction, caching, or analysis
   into Node.js.
2. Parse JavaScript and TypeScript with Oxc. Dependency extraction and export
   fingerprint extraction must use the same parsed `Program`; never parse one source
   file twice.
3. Preserve concrete binding information for imports and re-exports. Named, default,
   aliased, type-only, namespace, and star cases must remain distinguishable.
4. Resolve workspace imports through exact, conditional, and wildcard
   `package.json#exports` entries before considering fallbacks. A package with
   `exports` must not expose unmatched subpaths implicitly.
5. Prefer conservative false positives to false negatives when precision cannot be
   proven. Namespace, side-effect, dynamic, glob, `require()`, and unresolved
   workspace edges may propagate `*`.
6. Validate the Git working tree automatically for default cached queries. Modified,
   staged, untracked, deleted, renamed, and repeatedly edited dirty files must
   refresh the graph incrementally. `--trust-cache` is the explicit validation bypass,
   while `--strict-cache` always forces a rebuild.
7. Keep the public JSON interface stable and camel-cased. Prefer additive fields;
   renamed or removed fields require an explicit migration and test updates.
8. Write cache generations atomically. All cache artifacts must use the versioned
   MessagePack + LZ4 binary codec. Metadata must reference only complete generations,
   unchanged reverse shards may be shared by immutable filename, and reverse queries
   must not require full-graph deserialization.
9. Bump `CACHE_VERSION` when stored graph data, metadata, shard layout, or cache
   compatibility changes.

## Module map

| Area | File | Responsibility |
| --- | --- | --- |
| CLI | `rust/main.rs` | argument validation, mode dispatch, output, child commands |
| Models | `rust/model.rs` | graph edges, bindings, projects, reasons, graph statistics |
| Configuration | `rust/config.rs` | defaults, JSON/JSONC loading, globs, config signatures |
| Parsing | `rust/extract.rs` | one-pass Oxc dependency and export extraction |
| Resolution | `rust/resolver.rs` | relative files, tsconfig aliases, workspace exports |
| Graph/cache | `rust/graph.rs` | indexing, reverse shards, metadata, atomic generations |
| Selection | `rust/analysis.rs` | affected propagation and reverse queries |
| Git | `rust/git.rs` | changed files, comparison refs, changed specifiers |
| Workspaces | `rust/workspaces.rs` | workspace patterns and manifest discovery |
| Architecture docs | `docs/architecture.md` | graph inputs, resolution, binding propagation, and precision rules |
| Configuration docs | `docs/configuration.md` | workspace discovery, root inputs, file selection, and config reference |
| Cache docs | `docs/cache.md` | validation, incremental refresh, storage, and benchmark contract |
| CLI docs | `docs/cli.md` | commands, JSON fields, explanations, and exit behavior |
| Native E2E | `tests/native_e2e.rs` | real CLI processes in temporary Git workspaces |
| Performance | `tests/performance.rs` | separate-process p95 budgets for warm cache paths |
| npm installer | `npm/install.cjs` | install-time platform detection and native linking |
| npm platform map | `npm/platform.cjs` | OS, CPU, and Linux libc package selection |
| npm packages | `npm/*/package.json` | OS/CPU constraints and prebuilt binary archives |
| Codex skill | `skills/monorepa-find-dependencies` | installable dependency-search workflow and native CLI wrapper |
| Publishing | `.github/workflows/publish.yml` | build, package, provenance, npm publication |

## Change workflow

1. Inspect every relevant path before editing. Behavioral changes often cross the
   model, parser or resolver, traversal, and cache layers.
2. Decide whether the change affects cache compatibility, configuration signatures,
   JSON output, or only in-memory analysis.
3. Add a focused unit test for parsing, resolution, or traversal logic.
4. Add or update native E2E coverage for user-visible CLI behavior, cache transitions,
   workspace resolution, and package exports.
5. Run the narrowest useful check while iterating, then complete the full verification
   matrix before handoff.
6. Update `README.md` and this guide when commands, configuration, guarantees, layout,
   or contributor expectations change.
7. Keep the root npm version, all platform package versions, and root optional dependency
   versions identical.
8. When changing the installable skill, keep its examples aligned with the public CLI,
   run the skill validator, and run `npm run test:skill`.

## Hot-path rules

- Do not regress the shard-only `dependents` path into workspace discovery or
  full-graph deserialization.
- Compile glob patterns once per analysis, not once per file or edge.
- Keep Oxc parsing parallel through Rayon during graph construction.
- Clean warm automatic validation must use the native validation snapshot without
  starting Git. After a native mismatch, use at most one Git status operation and hash
  contents only for reported dirty or untracked paths.
- Deserialize validation paths as borrowed strings from the decompressed MessagePack
  payload. Validation grouping must use generic repository-relative path prefixes and
  must not depend on workspace discovery, manifests, or directory naming conventions.
  On Unix, keep these groups balanced and reuse each opened prefix for `fstatat`
  batches; retain the exact portable metadata fallback.
- Incremental refreshes must apply the Git delta to cached file records. Do not list or
  parse the complete repository for an ordinary source edit.
- Preserve raw dependencies and cached workspace manifests so file-set and resolution
  changes can be re-resolved without reparsing unchanged source files.
- Persist ordered resolution candidates through the selected target. Additions and
  deletions must re-resolve only candidate or glob watchers while preserving extension
  and index-file precedence.
- Keep metadata, forward records, unresolved edges, validation snapshots, and reverse
  shards in the shared compressed binary cache format. Public CLI output remains JSON,
  and reverse queries must continue to read target shards only.
- Keep `--trust-cache` on the shard-only path without Git, workspace discovery, or
  full-graph deserialization.
- Run the cache benchmark after any hot-path change. Do not raise the 5 ms p95 budget
  without documenting and justifying the new target.

## Precision rules

### Imports and re-exports

- Ordinary imports affect the importer only when the changed target binding matches.
- Re-exports preserve imported-to-exported name mappings through transitive traversal.
- `export *` excludes `default` unless an explicit default edge exists.
- Namespace, side-effect, dynamic, glob, and unknown bindings are conservative
  wildcards.

### Package exports

- Honor `exportConditions` order.
- Support exact and wildcard subpaths plus nested conditional targets.
- Prefer existing source targets. Map conventional `dist` JavaScript or declaration
  targets back to source only when the indexed source file exists.
- Keep unmatched subpaths unresolved when a manifest declares `exports`.

## Cache checklist

When cached structures or semantics change:

- update the relevant cache version;
- use Serde defaults only for intentionally compatible fields;
- verify Git `HEAD`, config signature, config file state, and invalidating root inputs;
- verify automatic working-tree signatures for clean, staged, modified, repeatedly
  edited dirty, untracked, deleted, and renamed files;
- verify incremental parsed/reused counts, file-set re-resolution, and reuse of
  immutable reverse-shard files;
- verify clean automatic hits succeed without Git on `PATH` and native validation
  detects file, directory, and Git index changes before the exact fallback;
- preserve temporary-write-then-rename behavior;
- ensure metadata cannot select incomplete or obsolete generations;
- test cold build, automatic warm hit/rebuild, trusted bypass, strict rebuild, Git
  `HEAD` invalidation, and incomplete generations.

## Required verification

```bash
# Unit, E2E, and documentation tests
cargo test

# Formatting and Clippy with warnings denied
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings

# Required after cache or hot-path changes
cargo test --release --test performance -- --ignored --nocapture

# Required after npm metadata, installer, or publishing changes
npm run test:package

# Final whitespace check
git diff --check
```

For a CLI- or config-only change, also exercise the command against a temporary
fixture or add it to `tests/native_e2e.rs`. Never validate cache behavior only against
this repository's existing cache.

## Handoff checklist

Report:

- behavior and CLI/configuration contracts changed;
- cache version changes, if any;
- focused tests added or updated;
- test, lint, and benchmark results;
- npm metadata and package archive validation results;
- remaining conservative cases and known false-positive risks.
