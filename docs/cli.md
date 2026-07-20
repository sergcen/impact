# CLI and JSON reference

## Command model

```text
monorepa-impact affected [options] [-- command with {workspaces}]
monorepa-impact dependents <file> [<file>...] [options]
```

`affected` starts from Git changes and returns the workspaces those changes can reach.
`dependents` starts from one or more target modules and walks the graph in reverse to
return the files that depend on them.

Use command-specific help to see only relevant options:

```bash
monorepa-impact affected --help
monorepa-impact dependents --help
```

## Options

| Flag | Command | Purpose |
| --- | --- | --- |
| `--base <ref>` | `affected` | Compare `HEAD` and the current working tree with a base ref |
| `--specifier <name>` | `dependents` | Restrict traversal to an imported/exported binding; repeatable |
| `--direct` | `dependents` | Return immediate importers only |
| `--config <path>` | both | Load a specific JSON or JSONC configuration |
| `--json` | both | Print machine-readable output |
| `--explain` | both | Print human-readable dependency chains |
| `--no-cache` | both | Do not read or write persistent cache data |
| `--rebuild-cache` | both | Build and store a new cache generation |
| `--strict-cache` | both | Force a full graph rebuild from the working tree |
| `--trust-cache` | both | Skip automatic working-tree validation |
| `-h`, `--help` | global or command | Print relevant help |
| `-V`, `--version` | global | Print the CLI version |

Invalid combinations are rejected. Notably:

- `--direct` and `--specifier` are valid only with `dependents`;
- `--base` and child commands are valid only with `affected`;
- `--trust-cache` cannot be combined with `--strict-cache` or `--no-cache`.

### Legacy compatibility

The pre-subcommand forms remain accepted for existing automation:

```text
monorepa-impact --base <ref>
monorepa-impact --dependents <file> [--dependents <file>...]
```

New integrations should use `affected` and `dependents`. Legacy aliases return the
same text and JSON contracts and do not print deprecation warnings into automation
output.

## Affected-project mode

```bash
monorepa-impact affected --base origin/main
monorepa-impact affected --base origin/main --explain
monorepa-impact affected --base origin/main --json
```

The base ref is compared with `HEAD` through its merge base. Staged, unstaged, and
untracked working-tree files are included in the same result.

Without `--json` or `--explain`, stdout contains one sorted workspace name per line.

### Run a child command

```bash
monorepa-impact affected --base origin/main -- 'pnpm {workspaces} --if-present test'
```

`{workspaces}` expands to sorted `--filter=<workspace>` arguments. The command is not
started when the affected set is empty. Its exit status becomes the CLI exit status.

## Dependents mode

```bash
monorepa-impact dependents packages/core/src/example.ts
```

Multiple targets can be queried together:

```bash
monorepa-impact dependents packages/core/src/example.ts packages/contracts/src/model.ts
```

Restrict traversal to bindings:

```bash
monorepa-impact dependents packages/core/src/example.ts --specifier createExample --specifier ExampleModel
```

Without specifiers, traversal begins with `*`. With specifiers, only matching concrete
binding edges are followed unless a conservative wildcard edge applies.

## Explanations

`--explain` prints each selected project or dependent file followed by its reason
chain. A chain may include:

- the changed or target file;
- the importing file;
- dependency type;
- source specifier;
- imported-to-exported binding mappings across re-exports.

## JSON: affected projects

| Field | Meaning |
| --- | --- |
| `affectedFiles` | Changed files plus every file reached through reverse traversal |
| `base` | Comparison ref used by the query |
| `changedFiles` | Sorted Git and working-tree changes |
| `changedSpecifiers` | Export names whose fingerprints changed for each source file |
| `graphStats` | Cache, snapshot, validation, parsed-file, and reused-file state |
| `projects` | Sorted affected workspace names |
| `reasons` | Explanation chains keyed by workspace name |

## JSON: module dependents

| Field | Meaning |
| --- | --- |
| `dependentFiles` | Sorted direct or transitive importers |
| `direct` | Whether traversal stopped after immediate importers |
| `projects` | Sorted workspaces containing dependent files |
| `reasons` | Explanation chains keyed by dependent file |
| `targetFiles` | Normalized target modules |
| `targetSpecifiers` | Normalized requested bindings, or `*` |
| `graphStats` | Cache, snapshot, validation, parsed-file, and reused-file state |

Reason objects use camel-cased `importedSpecifier` and `exportedSpecifier` fields when
binding information is available, plus `file`, `type`, and optional `via`.

## Graph statistics

`graphStats` contains:

| Field | Meaning |
| --- | --- |
| `cache` | Whether the result was built or loaded from cache |
| `parsedFiles` | Files parsed for this snapshot or incremental refresh |
| `reusedFiles` | Files reused from existing cache records |
| `snapshot` | Snapshot state such as loaded or incremental |
| `validation` | Validation mode such as automatic or trusted |

## Exit behavior

- A successful query exits with `0`, including an empty result.
- Invalid arguments, configuration, graph, resolution, or cache errors exit non-zero.
- A child-command failure returns that command's non-zero exit code when available.

Version `1.0.1` treats the canonical commands, supported legacy aliases, configuration
keys, and public JSON fields as stable under Semantic Versioning. Minor releases may
add fields; renaming or removing a field requires a major release.
