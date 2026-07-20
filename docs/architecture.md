# Architecture and precision

Monorepa Impact maintains one file-level dependency graph for two query types:

- affected-project analysis starting from Git changes;
- direct or transitive dependents analysis starting from target modules and optional
  exported bindings.

The graph is independent of task names. Testing, typechecking, building, and deploying
are policies applied by the caller after selection.

## Graph construction

1. Workspace patterns identify named projects.
2. Include, exclude, and extension rules identify eligible files.
3. JavaScript and TypeScript files are parsed in parallel with Oxc.
4. Dependency extraction and export fingerprint extraction reuse the same parsed
   `Program`.
5. Raw specifiers are resolved to repository-relative targets.
6. Forward file records and binding-preserving reverse edges are stored in the graph.

## Dependency coverage

The extractor recognizes:

- static `import` declarations;
- `export ... from` and `export * from` re-exports;
- named, default, namespace, aliased, type-only, and star bindings;
- literal `import()` and `require()` calls;
- relative `import.meta.glob()` and `import.meta.globEager()` patterns;
- `new URL(..., import.meta.url)` assets and workers;
- CSS `@import`, `composes ... from`, and local `url()` references;
- `tsconfig` `extends`, project references, `paths`, and `baseUrl`;
- workspace package imports through `package.json#exports`.

Non-literal runtime lookup, reflection, framework string entrypoints, and generated
modules cannot always be inferred from syntax. Use `rootInputs` for implicit
repository-level influence and keep generated relationships explicit where possible.

## Resolution order

Relative imports preserve ordinary file and index resolution precedence. TypeScript
configuration contributes `baseUrl`, `paths`, `extends`, and project reference
relationships.

Workspace package imports resolve through their manifest before any configured
fallback:

1. exact `exports` subpaths;
2. wildcard `exports` subpaths;
3. nested conditional targets in `exportConditions` order;
4. conventional source mapping for existing indexed sources when a target points at
   built JavaScript or declarations;
5. the configured workspace fallback when precision cannot be proven.

When a package declares `exports`, unmatched subpaths remain unresolved. They are not
made public through an implicit filesystem fallback.

## Binding propagation

Every ordinary import edge records the imported name. Re-export edges additionally
record the name exposed to the next consumer.

For example:

```ts
// packages/money/src/index.ts
export { formatPrice as money } from "./format-price";

// apps/storefront/src/price.ts
import { money } from "@acme/money";
```

A query for `formatPrice` follows the mapping to `money` and reaches the storefront.
An unrelated export from `format-price.ts` does not propagate across this chain.

`export *` propagates named exports but excludes `default` unless an explicit default
edge exists. Type-only imports and re-exports remain distinguishable from runtime
edges.

## Affected-project analysis

Affected mode combines:

- files changed between the merge base and `HEAD`;
- staged and unstaged changes relative to `HEAD`;
- untracked working-tree files.

For changed JavaScript and TypeScript sources, export fingerprints identify which
exported declarations changed. Those names seed binding-aware reverse traversal. A
file addition, deletion, parse ambiguity, or change that cannot be assigned safely to
one export seeds `*`.

The workspace containing an affected file is selected, followed by every workspace
reached through relevant reverse edges. Package manifest changes conservatively affect
the package's indexed files because exports and resolution may have changed.

`rootInputs` can select all projects, named projects, or graph dependents for changes
that ordinary imports cannot model.

## Dependents analysis

`dependents <file>` starts reverse traversal at an existing indexed module.

- Without `--specifier`, the query starts with `*` and returns every matching direct
  and transitive importer.
- With one or more `--specifier` values, only edges that consume those bindings are
  followed.
- `--direct` stops after immediate importers.
- Repeatable target files are evaluated in one query.

The result includes dependent files, their containing workspaces, and reason chains.

## Conservative fallbacks

Monorepa Impact prefers a false positive to a false negative when a precise edge
cannot be demonstrated. The following relationships may propagate `*`:

- namespace and side-effect imports;
- dynamic imports and `require()` edges;
- glob dependencies;
- unknown binding shapes;
- unresolved workspace imports when `packageFallback` is `unresolved`.

Set `packageFallback` to `none` only when the repository is intentionally closed and
unmatched workspace imports should not influence selection.

## Determinism

Public paths and project names are repository-relative and sorted. JSON fields are
camel-cased. The public output is independent of internal cache serialization and
shard layout.

See [Cache behavior](./cache.md) for persistence and validation, and
[CLI and JSON reference](./cli.md) for result contracts.
