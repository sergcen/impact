---
name: monorepa-find-dependencies
description: Find direct and transitive module dependents, binding-level consumers, affected files and workspaces, and explanation chains in JavaScript or TypeScript repositories with the Monorepa native dependency graph. Use when a user asks what depends on a file or export, what a refactor can affect, which projects changed in a branch, why a workspace is affected, or where an import/re-export chain leads.
---

# Find dependencies with Monorepa

Use the bundled wrapper for read-only impact and reverse-dependency queries. It prefers
an installed `monorepa-impact` binary, detects the source checkout, and falls
back to `@monorepa/impact@1` through npm without changing the target project's
manifest.

## Choose the query

- For “what depends on this file?”, use `--file <path>`.
- For “who uses this export?”, add `--specifier <name>`.
- For immediate importers only, add `--direct`.
- For “what does this branch affect?”, use `--affected` and optionally `--base <ref>`.
- For several targets or bindings, repeat `--file` or `--specifier`.

Interpret “dependencies of X” carefully. This skill's native query follows reverse
edges: files that depend on X. If the user explicitly asks what X imports, inspect X's
outgoing import declarations instead and state that this is the opposite direction.

## Run the query

Resolve the repository root first, normally with `git rev-parse --show-toplevel`.
Resolve `<skill-dir>` as the directory containing this `SKILL.md`.

```bash
node "<skill-dir>/scripts/search.cjs" \
  --root "<repository-root>" \
  --file "packages/ui/src/button.tsx"
```

Binding-selective query:

```bash
node "<skill-dir>/scripts/search.cjs" \
  --root "<repository-root>" \
  --file "packages/ui/src/button.tsx" \
  --specifier "Button" \
  --direct
```

Affected-project query:

```bash
node "<skill-dir>/scripts/search.cjs" \
  --root "<repository-root>" \
  --affected \
  --base "origin/main"
```

The wrapper always requests JSON, auto-detects pnpm, npm, Yarn, and Lerna workspace
patterns, and uses an empty workspace set for file queries in a single-package
repository. Pass `--config <path>` only when the repository already needs custom
Monorepa configuration.

## Interpret the result

For file queries, focus on:

- `dependentFiles`: matching direct or transitive importers;
- `projects`: workspaces containing those importers;
- `reasons`: import and re-export chains, including binding mappings;
- `graphStats`: cache state and parsed/reused file counts.

For affected queries, focus on:

- `changedFiles` and `changedSpecifiers`: the Git inputs;
- `affectedFiles`: graph propagation;
- `projects`: workspaces to act on;
- `reasons`: why each workspace was selected.

Lead with the answer, then give the shortest useful chain. State whether the query was
direct or transitive and whether a binding filter was used. Do not dump full JSON
unless the user requests it.

## Accuracy rules

- Treat an empty result as a valid answer; do not replace it with text search.
- Use repository-relative target paths. If a target is missing, locate candidates with
  `rg --files` before retrying.
- Use `default` for a default export and `*` only when the user requests all bindings.
- Keep automatic cache validation enabled. Use `--trust-cache` only when the user has
  explicitly supplied an external freshness guarantee.
- Explain conservative wildcard propagation for namespace, side-effect, dynamic,
  glob, `require()`, or unresolved workspace edges.
- Use text search only as a clearly labeled supplement for non-literal runtime strings
  or framework conventions that static imports cannot represent.
- Never invoke the CLI child-command mode from this skill.

## Failure handling

If no local binary exists, the wrapper may obtain `@monorepa/impact@1` with npm on
macOS and Linux. On Windows, install `@monorepa/impact` in the target repository
first; the wrapper runs its native executable directly instead of passing user input
through a command shell. Use `--no-download` when network access is disallowed. If the
package cannot be obtained, report the exact installation issue and recommend a local
installation; do not silently substitute a less precise dependency engine.
