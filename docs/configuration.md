# Configuration reference

Monorepa Impact automatically loads `affected.config.json` or
`affected.config.jsonc` from the repository root. Use `--config <path>` to select a
different file.

JSONC comments and trailing commas are accepted. Unknown fields inside `rootInputs`
are rejected. Arrays replace their defaults rather than extending them.

## Complete example

```json
{
  "base": "origin/main",
  "workspacePatterns": ["apps/*", "packages/*", "tooling/*"],
  "exportConditions": ["import", "browser", "default", "types", "require"],
  "packageFallback": "unresolved",
  "include": ["**/*"],
  "exclude": [
    "**/node_modules/**",
    "**/dist/**",
    "**/build/**",
    "**/coverage/**"
  ],
  "extensions": [".ts", ".tsx", ".js", ".jsx", ".json", ".css", ".svg"],
  "cache": {
    "enabled": true,
    "directory": "node_modules/.cache/monorepa-impact"
  },
  "rootInputs": [
    {
      "patterns": ["tsconfig.base.json", "workspace.yaml"],
      "projects": "all",
      "invalidateGraph": true
    }
  ]
}
```

## Options

| Option | Default | Meaning |
| --- | --- | --- |
| `base` | `origin/master` | Git comparison ref used when `--base` is omitted |
| `workspaceFile` | `pnpm-workspace.yaml` | Manifest containing a top-level `packages:` list |
| `workspacePatterns` | unset | Explicit project globs; overrides `workspaceFile` |
| `exportConditions` | `import`, `browser`, `default`, `types`, `require` | Ordered conditions for package export resolution |
| `packageFallback` | `unresolved` | Handling for unresolved workspace imports: `unresolved` or `none` |
| `include` | `**/*` | Files eligible for graph indexing |
| `exclude` | common generated directories | Files removed from graph indexing |
| `extensions` | supported code, config, style, asset, and font extensions | File suffixes eligible for indexing |
| `rootInputs` | empty | Implicit repository-level influence rules |
| `cache.enabled` | `true` | Enables persistent graph caching |
| `cache.directory` | `node_modules/.cache/monorepa-impact` | Repository-relative cache location |

## Workspace discovery

When `workspacePatterns` is absent, `workspaceFile` must contain a top-level
`packages:` list such as:

```yaml
packages:
  - "apps/*"
  - "packages/*"
  - "!packages/legacy-*"
```

Patterns are evaluated in order. A later negative pattern can remove an earlier
match. Each matched directory with a readable `package.json` and string `name` becomes
a project.

Use `workspacePatterns` when the package manager stores workspace configuration in a
different shape:

```json
{
  "workspacePatterns": ["apps/*", "packages/*"]
}
```

## Export conditions

`exportConditions` controls nested conditional `package.json#exports` resolution. The
first matching target in the configured order wins.

```json
{
  "exportConditions": ["import", "browser", "default", "types", "require"]
}
```

Exact and wildcard subpaths are supported. Declaring `exports` closes undeclared
subpaths; `packageFallback` does not expose them implicitly.

## Package fallback

```json
{
  "packageFallback": "unresolved"
}
```

- `unresolved` conservatively links uncertain workspace imports to affected workspace
  traversal.
- `none` ignores that fallback. Use it only when unmatched workspace imports are known
  to be irrelevant or invalid.

## Root inputs

Some files affect projects without being imported: root TypeScript configuration,
workspace manifests, code-generation inputs, global lint configuration, or deployment
metadata.

```json
{
  "rootInputs": [
    {
      "patterns": ["tsconfig.base.json"],
      "projects": "dependents",
      "invalidateGraph": true
    },
    {
      "patterns": ["tooling/global-policy.json"],
      "projects": ["@acme/admin", "@acme/storefront"],
      "invalidateGraph": false
    }
  ]
}
```

`projects` accepts:

- `"all"` for every discovered workspace;
- `"dependents"` to traverse the graph;
- an array of exact workspace names.

Set `invalidateGraph` when the input can change file discovery or resolution, not only
project selection.

## File selection

`include`, `exclude`, and `extensions` define the indexed file set. Compile patterns
once per analysis by keeping these arrays focused and avoiding redundant globs.

Because arrays replace their defaults, a custom `exclude` list should repeat every
generated directory that must remain outside the graph.

## Cache configuration

```json
{
  "cache": {
    "enabled": true,
    "directory": "node_modules/.cache/monorepa-impact"
  }
}
```

Disabling the configured cache has the same persistent-cache effect as using
`--no-cache` for every query. See [Cache behavior](./cache.md) for validation and
control flags.
