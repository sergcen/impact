# Cache behavior

The cache makes repeated graph queries fast without asking users to manage freshness
manually. Its default directory is:

```text
node_modules/.cache/monorepa-impact
```

## Normal lifecycle

1. A cold query builds the complete graph and writes a generation atomically.
2. A clean warm query validates the repository and reuses the generation.
3. A working-tree change incrementally refreshes affected records and reverse shards.
4. An incompatible, incomplete, or invalid generation falls back to a full rebuild.

Automatic validation covers:

- `HEAD` changes;
- the Git index;
- staged and unstaged modifications;
- untracked files;
- deleted and renamed files;
- repeated edits to files that were already dirty;
- relevant directory additions and removals;
- configuration and graph-invalidating root inputs.

## Incremental refresh

Ordinary source edits parse only files whose content fingerprints changed. Cached raw
dependencies and workspace manifests allow resolution changes to be applied without
reparsing unrelated source files.

File additions, removals, and renames re-resolve only importers that watch affected
ordered candidates or glob patterns. This preserves extension and index-file
precedence without listing and parsing the entire repository.

An incremental result reports `snapshot: "incremental"` with exact `parsedFiles` and
`reusedFiles` counts under `graphStats`.

## Control flags

| Flag | Behavior |
| --- | --- |
| `--strict-cache` | Rebuild the graph from the current working tree even if validation would pass |
| `--rebuild-cache` | Create and store a new graph generation |
| `--no-cache` | Do not read or write persistent cache data |
| `--trust-cache` | Load a compatible snapshot without working-tree validation |

`--trust-cache` is intended for workflows whose external cache key already includes
every freshness input. It cannot be combined with `--strict-cache` or `--no-cache`.

## Warm reverse queries

Reverse edges are stored in immutable hash shards. A warm `dependents` query opens
only the shards for its requested target files and does not need to deserialize the
complete graph.

Clean automatic validation runs natively without starting Git. The validation
snapshot is decoded with borrowed strings. On Unix and macOS, paths are grouped into
balanced repository-relative prefixes and validated through reused directory handles.
When native metadata detects a mismatch, validation falls back to at most one exact
Git status operation and hashes only reported dirty or untracked paths.

## Storage format

Every cache artifact uses the same versioned binary codec: MessagePack compressed with
LZ4. This includes:

- metadata;
- forward graph records;
- unresolved workspace edges;
- validation snapshots;
- reverse shards.

Generations use temporary-write-then-rename behavior. Metadata references only
complete artifacts, and unchanged immutable reverse shards may be shared across
generations. Public CLI JSON is independent of the internal cache format.

## Benchmark

The performance test launches the optimized CLI in 60 separate processes for each
warm-query mode and enforces a 5 ms p95 ceiling on a deterministic fixture:

```bash
cargo test --release --test performance -- --ignored --nocapture
```

It measures:

- trusted shard-only reverse queries;
- automatically validated reverse queries with Git removed from `PATH`.

The threshold protects the intended hot path. It is not a universal latency promise
for every machine or repository size.
