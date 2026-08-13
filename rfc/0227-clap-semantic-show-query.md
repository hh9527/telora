# RFC 0227: Clap-based semantic `show` queries

- Status: Implemented
- Depends on: RFC 0039, RFC 0040, RFC 0042, RFC 0044, RFC 0059, RFC 0142, RFC 0228
- Tracking issue: #47

## Summary

Telora replaces its hand-written CLI argument dispatch with a single `clap`
command model and makes `show` the canonical command for semantic observation:

```text
telora show <module-id>
    [-p <substring>]
    [-k <kind>[,<kind>...]]
    [--exports]

telora show <module-id>
    --at <line>[:<column>]
```

The current working directory establishes the crate by nearest-ancestor
`telora-deps.json` lookup. The positional `<module-id>` selects the stable
logical root to analyze, using the same canonical spellings as the module
resolver:

```text
@src/model.telora
@bin/main.telora
@test/model.telora
ontology/lib.telora
```

Named queries inspect top-level local definitions by default. `-p` performs a
case-sensitive literal substring match over semantic names; it is not a glob
or regular expression. `-k` filters local definitions by the surface binding
kinds `type`, `let`, `def`, and `import`. `--exports` switches the query domain
from local definitions to the selected module's public interface and therefore
conflicts with `-k`.

`--at` selects the independent position-query form and conflicts with `-p`,
`-k`, and `--exports`. `line` and `column` are one-based. A line-only query
returns semantic facts intersecting that source line; a line-and-column query
returns facts containing that exact source position.

Every successful `show` query writes deterministic JSON Lines to standard
output. Each line is one versioned semantic-query record. Human-readable
compiler diagnostics remain on standard error. Empty named queries produce no
lines and succeed.

The overlapping `telora types` command is removed. Strict validity remains the
responsibility of `telora check <module-id>`; `show` observes a recoverable
workspace and explicitly reports whether each fact is authoritative or a
recovery/debug observation.

## Motivation

The current CLI exposes two overlapping type-observation surfaces:

```text
telora types <physical-root-file>
telora show <physical-root-file> [at <physical-source-file> <line> <column>]
```

`types` strictly loads one physical root and prints top-level types as prose.
`show` builds a recoverable workspace but prints modules, definitions,
references, expressions, diagnostics, and the entire type graph in one large
human-readable report. Large family- and recursive-metadata programs make both
reports difficult to inspect. Tool clients resort to shell pipes, output
redirection, or manually paged invocations, and bounded tool output often
truncates the relevant fact.

The prose also puts facts with different authority on adjacent rows. A
definition's quantified scheme may be precise while an uninstantiated nested
expression fact displays `Any` or an opaque recovery reference. Those rows are
valid observations of different semantic layers, but their current rendering
makes the latter look like degradation of the former.

The existing CLI parser compounds the problem. It matches argument slices by
hand, which is adequate for one fixed positional argument but becomes brittle
once `show` needs required options, repeat rejection, comma-delimited values,
structured position parsing, order-independent flags, conflicts, and generated
help. Extending that parser would duplicate behavior already provided and
tested by `clap`.

The semantic workspace already has the correct underlying model: canonical
logical module identities, detached snapshot facts, stable-for-one-snapshot
IDs, complete/recoverable fact states, public export records, and position
queries. The CLI should expose a small stable query protocol over that model
instead of serializing the whole internal snapshot by default.

## Command model

The `telora` binary adopts `clap` derive declarations for the complete command
tree. All existing commands move to that model in the same implementation:

```text
telora run <binary-name> [-C <context>] [--input <file|->]
telora run -S <file> [--input <file|->]
telora exec --dry-run <module.telora> [-- <arguments>...]
telora build --dry-run <module.telora>
telora check <module-id>
telora show <module-id> ...
telora lsp
```

This RFC changes the public form of `show` and removes `types`. `run`, `exec`,
`build`, `check`, and `lsp` retain their existing semantics while their
argument parsing migrates to `clap`. Parser migration must not change their
exit status, `--` handling, input semantics, execution protocol, or effect
boundary.

The initial `show` grammar is equivalent to:

```text
ShowArgs :=
  module_id: String
  -p, --pattern: non-empty String       optional, at most once
  -k, --kind: Kind[,Kind...]            optional, at most once
  --exports                             optional
  --at: Line[:Column]                   optional, at most once

Kind := type | let | def | import
```

`clap` owns unknown-option, missing-value, duplicate-option, and conflict
diagnostics. Custom value parsers own semantic validation of `Kind` and
`Line[:Column]`. Invalid CLI syntax exits nonzero without constructing a
workspace.

`--at` conflicts with `--pattern`, `--kind`, and `--exports`. `--exports`
conflicts with `--kind`. An omitted `-p` means no name filter. An omitted `-k`
means all supported top-level local definition kinds. There is no wildcard
sentinel.

## Resolution context and module selection

The resolver discovers context from CWD as specified by RFC 0228. It uses the
nearest ancestor `telora-deps.json` to determine:

- the containing crate and source root;
- `telora-deps.json` and dependency roots;
- `src`, `src/bin`, and `tests` physical roots;
- exact module formats and private-module boundaries; and
- the complete module graph reachable from that entry.

`<module-id>` is parsed as a canonical logical root identity, not interpreted
as a filesystem path. `show` recovers the workspace rooted at that exact
module and retains its identity in every record.

The accepted identity forms are exactly those published by the resolver:

- `@src/<relative-path>` for a source in the root entry's crate;
- `@bin/<relative-path>` for a Host-selectable application root;
- `@test/<relative-path>` for a Host-selectable test root;
- `<dependency>/<relative-path>` for a declared dependency source; and
- declared dependency source modules.

Physical paths, absolute paths, `file:` URIs, lexical `.`/`..`, and ad hoc
identity aliases are rejected. Module selection must use structured resolver
identity where available; it must not recover semantics by parsing diagnostic
display strings.

When the requested root is unknown, the command fails without guessing a
physical path. An unavailable dependency retained by recovery may still
produce recovery-state definition or diagnostic records after its importing
root is selected.

## Named local queries

Without `--exports` or `--at`, `show` queries top-level local definitions owned
by the selected module. Nested parameters, pattern bindings, and closure-local
definitions are not part of this name-oriented surface; they remain available
through position queries.

`-p <substring>` applies:

```text
definition.name.contains(substring)
```

The match is case-sensitive and uses the stored UTF-8 String. It performs no
case folding, Unicode normalization, glob expansion, regular-expression
interpretation, fuzzy matching, tokenization, or shell expansion. `*` and `?`
are ordinary literal characters. An empty substring is rejected rather than
treated as an undocumented spelling for all names.

`-k` accepts a comma-separated set of surface binding kinds. Ordering is not
significant and duplicates are deduplicated. Empty elements, unknown kinds,
leading commas, and trailing commas are errors. The mapping to internal HIR
definition categories is centralized and tested:

- `type` selects authored type bindings;
- `let` selects authored immutable value bindings;
- `def` selects declared/defined function slots and ordinary `def` bindings;
- `import` selects selective, namespace, and other local import bindings.

Compiler-internal categories such as closure parameters, pattern bindings,
native slots, generated definitions, and recovery placeholders are not
silently mapped to one of these surface kinds. If a later use case requires
them, it must add an explicit stable record kind.

Named records sort by name, then surface kind, then source location. Compact
snapshot IDs never determine public order.

## Export queries

`--exports` switches the query domain to the selected module's public
`ModuleInterface`. It is not a definition kind filter. This distinction is
required by Telora's module semantics:

```telora
let internal = 1;
export {internal as public};
```

`internal` is a local `let`; `public` is an interface name and does not create
a local binding. Aliased exports, exported imported bindings, namespace
modules, generic schemes, and recursive TypeMetadata therefore cannot be
faithfully represented by pretending every export has one local binding kind.

`--exports` may be combined with `-p`, applying the same literal substring
match to public names. It conflicts with `-k`. Export records sort by public
name. The first implementation reports the exact public type or scheme already
available in the semantic interface. It does not invent an origin definition
when the current snapshot export DTO does not preserve one.

## Position queries

`--at` accepts either:

```text
<line>
<line>:<column>
```

Both values are decimal, one-based positive integers. Zero, signs, whitespace,
additional separators, missing components, and integer overflow are errors.

A line-and-column query converts the logical module's source and requested
position through the snapshot's UTF-aware source mapping. It emits every
semantic record whose authored range contains the resulting position, ordered
from stable high-level identity to local detail:

1. definition;
2. reference;
3. expression fact; and
4. resolved type fact.

A line-only query emits the distinct records whose authored ranges intersect
that complete source line, using the same kind order and then source range.
One semantic fact is emitted at most once even if it has multiple locations on
the line.

`--at` is meaningful only for source-backed modules. Static-data or synthetic
modules without a queryable source fail with a clear diagnostic. A valid
position with no semantic fact produces zero JSONL records and succeeds. A
position outside the source is an error.

## JSON Lines protocol

Standard output is a machine-readable JSON Lines stream. Each non-empty line
is exactly one JSON object followed by `\n`; pretty printing, headers, summary
footers, ANSI escapes, and interleaved prose are forbidden. Serialization uses
the existing `serde` and `serde_json` dependencies of the `telora` binary.

Every record contains:

```json
{
  "schema": "telora.show/v1",
  "module": "@src/model.telora",
  "record": "definition",
  "name": "compile",
  "kind": "def",
  "state": "authoritative",
  "scheme": "Fn(Query) -> Plan",
  "location": {"line": 13, "column": 1}
}
```

The common fields are:

- `schema`: fixed `telora.show/v1` protocol discriminator;
- `module`: canonical logical module ID;
- `record`: `definition`, `export`, `reference`, `expression`, or `type`;
- `state`: `authoritative`, `recovery`, or `debug`; and
- `location`: when the fact has an authored source location, one-based line and
  column plus an optional end position.

Fields that do not apply are omitted rather than encoded as misleading empty
Strings. Named definition records include `name`, surface `kind`, semantic fact
state, and exactly one of an authoritative `scheme`, a known monomorphic
`type`, or a structured recovery status. Export records include public `name`
and exact public `type` or `scheme`. Reference records include the authored
name and target identity when known. Expression and type records are explicitly
`debug` unless the snapshot API identifies them as authoritative published
facts.

Recovery status is structured sufficiently to distinguish `Unknown`,
`Conflicted`, and `Incomputable` and to preserve causal identities and
diagnostic references. It must not be flattened into a type String such as
`Any`. A known `Any` and an unavailable fact are distinct records.

The JSON DTO is a stable CLI protocol, not a mechanical serialization of Rust
semantic structs. Snapshot-local numeric IDs may be included as optional debug
fields but are not persistent identities and cannot be required for sorting or
cross-invocation correlation. Adding optional fields is compatible within v1;
renaming fields, changing their meaning, or changing record categories
requires a protocol version change.

Serialization failure is fatal and produces no malformed partial JSON object.
Ordinary compiler/debug sinks must not write to standard output during a
`show` query.

## Authority and strictness

`show` always builds the recoverable workspace selected by `<module-id>` in the
CWD-discovered crate. It is an
observation command and succeeds when recovery can produce a queryable
snapshot, even if that snapshot contains diagnostics or unavailable facts.

Fact authority is determined from semantic data, not from whether its rendered
type looks precise:

- an exact published definition scheme or complete known binding fact is
  `authoritative`;
- a fact retained around parse, resolution, tool-stage, or dependency failure
  is `recovery` and carries its state/reason;
- nested expression/reference/type observations that are not independently
  instantiated contracts are `debug`.

In particular, an expression-level `Any` does not replace an enclosing
definition's quantified scheme. They appear as separate records with separate
states.

`telora check <module-id>` is the strict publication gate. It returns
success only when the selected root and its dependency graph satisfy existing
strict loading rules. Consumers that require authoritative results run
`check` before `show` or reject non-authoritative JSONL records explicitly.

## Removal of `types`

`telora types` is removed in the same implementation rather than retained as a
second formatting and strictness surface. Its useful queries become:

```text
# All local authored types in the root entry
telora show @bin/main.telora -k type

# One family or related set of definitions in a library module
telora show @src/model.telora -p Relation -k type,def

# Public type surface
telora show @src/model.telora -p Relation --exports
```

Scripts requiring strict validity use:

```text
telora check @bin/main.telora
telora show @bin/main.telora -k type
```

There is no hidden `show` mode that changes from recovery to strict loading.
This keeps validity and observation as explicit, composable operations.

## Error and exit behavior

The command exits nonzero for:

- invalid or conflicting CLI arguments;
- failure to discover a crate manifest from CWD;
- failure to construct a recoverable workspace;
- an invalid or unknown canonical module ID;
- a module without source used with `--at`;
- an invalid or out-of-range position; or
- JSON serialization or output failure.

A valid query with no matching names or facts writes zero bytes to standard
output and exits successfully. Semantic diagnostics retained in a recoverable
snapshot do not by themselves make `show` fail; their presence is represented
by record state and separate diagnostic observation APIs rather than prose
mixed into the JSONL stream.

`clap` usage errors and operational diagnostics are written to standard error.
No error path writes a success-shaped JSON record.

## Documentation and experiment migration

After implementation:

- `docs/design/LANGUAGE.md` records the canonical Host CLI and the separation
  between strict `check` and recoverable JSONL `show`;
- the ontology experiment's `docs/TELORA-CLI.md`, goals, validation commands,
  and opencode permissions use logical root module IDs;
- agent-facing documentation explains `-p` as literal substring matching and
  recommends semantic filters instead of shell `grep`, `head`, or redirection;
- references to `telora types` are removed from maintained documentation and
  experiment plans; and
- historical experiment archives remain unchanged.

The documentation must use canonical logical module IDs in examples and must
not describe physical source paths as query identities.

## Implementation plan

1. Add `clap` with derive support to the `telora` binary crate and model every
   existing subcommand in one typed command tree.
2. Preserve current `run`, `exec`, `build`, `check`, and `lsp` semantics while
   migrating their argument parsing.
3. Extend the semantic workspace module record or query API where necessary to
   retain and select structured canonical `ModuleId` rather than matching
   display paths.
4. Add typed parsers for the `-k` set and `--at` value and encode all declared
   conflicts in `clap` metadata.
5. Implement local-definition and public-export queries over one selected
   module, including literal name filtering and deterministic sorting.
6. Implement line and exact-position queries using the selected logical
   module's source and existing UTF-aware position conversion.
7. Define serde DTOs for `telora.show/v1`, including structured fact state,
   authority, causes, diagnostics, types, schemes, and locations.
8. Remove the old prose snapshot renderer, physical-path `at` form, and
   `types` subcommand.
9. Update the language SSOT and maintained experiment documentation and
   validation wrappers.

## Acceptance criteria

1. All CLI subcommands use one `clap` command model; no parallel hand-written
   top-level option parser remains.
2. `show` requires one canonical root `<module-id>`, discovers the crate from
   CWD, and can select `@src/...`, `@bin/...`, `@test/...`, and dependency
   modules.
3. Unknown module IDs fail with a deterministic inventory of queryable IDs and
   never cause an unreferenced source to load.
4. `-p` performs case-sensitive literal substring matching; `*` and `?` have
   no special meaning; empty patterns are rejected.
5. `-k type,let,def,import` maps only to the documented surface binding kinds,
   accepts order-independent duplicates, and rejects empty or unknown values.
6. `--exports` queries public names and exact interface types/schemes, composes
   with `-p`, and conflicts with `-k`.
7. `--at line[:column]` uses one-based UTF-aware positions, conflicts with
   named/export filters, and supports both line intersection and exact-point
   queries.
8. Every successful record is one valid `telora.show/v1` JSON object on one
   line; empty matches are successful empty output; ordering is deterministic
   across repeated runs.
9. Definition schemes, monomorphic facts, recovery states, and uninstantiated
   debug facts are represented distinctly; a debug `Any` cannot be mistaken
   for replacement of an authoritative scheme.
10. Aliased exports, exported imported bindings, recursive TypeMetadata,
    parameterized families, damaged modules, blocked facts, static-data
    modules, and UTF-8 positions have focused CLI regressions.
11. `types` and the old physical-path `show ... at ...` interface are absent
    from help and maintained documentation.
12. `run`, `exec`, `build`, `check`, and `lsp` retain their Host protocols while
    adopting RFC 0228 root selection; `check <module-id>` is the strict gate.
13. Formatting, warning-denied Clippy, CLI tests, core tests, and the complete
    workspace suite pass.

## Rejected alternatives

### Keep `types` and add independent filters

Two commands would continue to disagree about strictness, output schema,
module selection, and fact authority. Strict validity is clearer as an
explicit `check` followed by one observation protocol.

### Treat exports as a definition kind

Export aliases do not create local bindings, and re-exported imported or
namespace values may not have a local authored kind corresponding to the
public name. `--exports` is a query-domain switch, not `-k export`.

### Accept a physical module filename

Physical paths conflate crate discovery with logical identity, leak
machine-specific paths, and cannot name dependency or runtime modules
uniformly. CWD establishes crate context; `<module-id>` selects the stable
analysis root within that context.

### Encode position in the module ID

Forms such as `@src/lib.telora:13:15` overload canonical identity syntax and
require ambiguous suffix parsing. `--at 13:15` keeps identity and query
coordinates separate and allows `clap` to enforce conflicts.

### Use glob or regular-expression patterns

The demonstrated need is to locate semantic names containing a known fragment.
Literal `String::contains` behavior is deterministic, requires no pattern
language or escaping rules, and treats shell metacharacters as ordinary data.

### Add global `--skip` and `--limit`

Global pagination over heterogeneous modules, definitions, references,
expressions, and type nodes has no stable unit. Named and position queries
reduce output at the semantic source. Bounded pagination can be proposed later
for one concrete record domain if measured output still requires it.

### Preserve the prose snapshot as default output

The current report is useful for ad hoc compiler debugging but unsuitable as a
stable composable protocol. A future explicit internal dump command may expose
implementation details; it must not define `show` semantics or mix prose with
JSONL.

### Continue hand-written argument parsing

The new grammar has required options, conflicts, delimited values, structured
parsers, repeat handling, generated help, and order-independent flags. A local
parser would add project-specific mechanics without strengthening Telora's
language or semantic model.

## Implementation result

Implemented with one `clap` command tree, removal of `types`, literal name and
kind filters, export-domain queries, one-based position queries, and stable
`telora.show/v1` JSONL records. Definition facts preserve schemes and recovery
authority; nested expression observations are explicitly `debug` records.
