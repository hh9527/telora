# RFC 0256: Structured CLI Query Protocol

- Status: Proposed
- Tracking issue: #121
- Supersedes: the RFC 0227 `show` command surface and CLI coordinate convention
- Depends on: RFC 0039, RFC 0040, RFC 0042, RFC 0044, RFC 0059, RFC 0227, RFC 0228

## Summary

Telora exposes semantic observation through one structured query command:

```text
telora query modules [-p <substring>]
telora query exports <module-id> [-p <substring>]
telora query at <module-id>[:<line>[:<column>]]
    [-p <substring>]
    [-k <kind>[,<kind>...]]
```

`q` is a visible alias for `query`. `-C <context>` is a global option of the
query command and may appear before or after its subcommand. Maintained
documentation uses the latter form:

```text
telora q modules -C path/to/context
```

The old `show` command is removed without a compatibility alias. Query results
use the `telora.query/v1` JSON Lines schema.

CLI JSONL locations have one fixed coordinate contract:

- `line` and `end_line` are one-based;
- `column` and `end_column` are zero-based UTF-8 byte offsets; and
- source ranges are half-open `[start, end)` ranges.

LSP locations remain independent and follow the position encoding negotiated
with the client. Human terminal diagnostics retain their character-oriented
rendering.

## Motivation

RFC 0227 established a stable semantic observation surface, but encoded four
different query domains as one positional module plus mutually exclusive
flags. Adding module discovery made the grammar harder to explain:

```text
telora show [<module-id>] [--modules] [--exports] [--at ...] [-p ...] [-k ...]
```

The optional module position, mode flags, and conditional conflicts obscure
the actual model. Module discovery, public-interface inspection, module-scope
symbol inspection, and source-position inspection are distinct queries with
different inputs and filters. They should be represented by the command tree,
not by runtime interpretation of one argument bag.

The old position contract also used one-based Unicode-scalar columns. JSONL is
a machine protocol, while terminal columns are a human presentation concern.
Fixed UTF-8 byte columns correspond directly to Telora source ranges, avoid
display-width ambiguity, and do not require protocol negotiation. LSP already
has its own negotiated UTF-8, UTF-16, or UTF-32 coordinate boundary.

## Command model

The authoritative grammar is:

```text
QueryArgs :=
  -C <context>                     optional global argument
  QueryCommand

QueryCommand :=
  modules [-p <substring>]
  | exports <module-id> [-p <substring>]
  | at <module-selector> [-p <substring>] [-k <kinds>]

module-selector :=
  <module-id>
  | <module-id>:<line>
  | <module-id>:<line>:<column>
```

Canonical module IDs cannot contain `:`, so the position suffix is
unambiguous. `line` is a positive decimal integer. `column` is a non-negative
decimal integer and must resolve to a UTF-8 scalar boundary in the selected
line. Signs, whitespace, empty components, additional separators, and integer
overflow are errors.

`clap` owns command selection, required arguments, duplicate options, and
ordinary option syntax. A typed module-selector parser owns suffix validation.
Invalid query syntax fails before workspace discovery or recovery begins.

## Module catalog query

`query modules` lists the module view of the crate selected by nearest-ancestor
`telora-deps.json` discovery from `-C` or CWD. It does not parse, recover,
analyze, or evaluate any module.

The catalog contains:

- this crate's `@src/...`, `@bin/...`, and `@test/...` modules, including
  public, `.priv.*`, and `.native.telora` modules;
- public source modules of manifest dependencies; and
- public modules registered by the Host.

It excludes dependency bin/test roots and dependency or Host private/native
module identities. Resolver format overrides and supported Telora, JSON, TOML,
and YAML formats apply. Physical paths are never public identities.

Each record contains the canonical module ID, `crate` / `dependency` / `host`
origin, `public` / `private` / `native` visibility, and resolver format.
Records sort by canonical module ID. `-p` applies one case-sensitive literal
`String::contains` match to that ID.

## Export query

`query exports <module-id>` recovers the selected module and queries its public
Module interface. Export aliases and re-exports remain interface facts; they
are not projected back into fictional local definitions.

`-p` applies a case-sensitive literal substring match to the public export
name. Records sort by public name and retain exact published type or scheme
information and fact authority.

## At query

`query at` treats the module itself as the coarsest semantic location.

With a bare module ID:

```text
telora query at @src/model.telora
```

the command queries top-level local symbols owned by that module. `-p` filters
the symbol name and `-k` filters the surface kinds `type`, `let`, `def`, and
`import` exactly as established by RFC 0227.

With a line or exact position:

```text
telora query at @src/model.telora:13
telora query at @src/model.telora:13:0
```

the command returns definition, reference, and expression facts intersecting
the line or containing the exact point. `-p` and `-k` are invalid in these
forms because the result domain is not limited to named symbols.

Line lookup excludes the line terminator. Exact positions at the line end are
valid. A byte column inside a multi-byte UTF-8 scalar, beyond the line content,
or on a nonexistent line is an error. A valid location with no matching fact
produces no records and succeeds.

## JSON Lines and coordinates

Every successful query record is one compact JSON object followed by `\n`.
The schema discriminator is `telora.query/v1`. Empty results write no bytes
and succeed. Ordering is deterministic and never depends on snapshot-local
numeric IDs.

All Telora CLI JSONL schemas, including query facts and check/run diagnostics,
use this source coordinate convention:

```json
{
  "line": 13,
  "column": 0,
  "end_line": 13,
  "end_column": 6
}
```

Lines are one-based. Columns are zero-based UTF-8 byte offsets from the start
of their respective line. End positions are exclusive. The encoding is fixed
by the schema; the CLI does not expose a position-encoding option.

The source database's terminal-oriented scalar position helpers do not change.
CLI JSONL uses `DocumentText` conversion with `PositionEncoding::Utf8`. LSP
continues to convert through its negotiated encoding and retains the LSP
protocol's zero-based line and character convention.

## Errors and compatibility

Operational query failures are emitted as `telora.query/v1` diagnostic records
and return nonzero. `clap` syntax failures remain ordinary stderr usage
diagnostics. Compiler facts retained by recovery do not make a query fail.

The following forms are removed:

```text
telora show <module-id>
telora show <module-id> --exports
telora show <module-id> --at <line>[:<column>]
telora show --modules
```

No `show` alias or argument-rewrite compatibility layer remains. This keeps
generated help, permission rules, tutorials, and Agent behavior on one command
grammar.

## Implementation plan

1. Add RFC 0227-compatible query DTO behavior under `telora.query/v1` and
   model `query` as a nested `clap` command with visible alias `q`.
2. Implement `modules`, `exports`, and typed `at` arguments; remove all
   mode-flag conflicts and the old `show` command.
3. Keep module catalog construction inside `ModuleResolver`, including crate
   visibility, dependency/Host filtering, canonicalization, format overrides,
   deterministic ordering, and literal filtering.
4. Parse module selectors and convert line/column input through UTF-8
   `DocumentText` positions without changing terminal source rendering.
5. Change the shared CLI JSONL location projection to one-based lines and
   zero-based UTF-8 byte columns; retain negotiated LSP conversion unchanged.
6. Migrate CLI regressions, performance fixtures, LANGUAGE SSOT, tutorials,
   and maintained experiment instructions from `show` to `query`.
7. Run formatting and the complete workspace regression suite.

## Acceptance criteria

1. `telora query -h` presents exactly the `modules`, `exports`, and `at`
   query domains with meaningful generated help; `telora q` is a visible alias.
2. `query modules` lists this crate's public/private/native modules and only
   external public modules, including supported static-data formats, in stable
   canonical-ID order.
3. `query exports` reports the exact public interface and supports literal
   public-name filtering.
4. Bare `query at <module-id>` reports top-level local symbols and supports
   literal name and surface-kind filtering.
5. Suffixed `query at` supports one-based lines and zero-based UTF-8 columns,
   rejects non-boundary or out-of-range positions, and rejects `-p`/`-k`.
6. Query output uses only `telora.query/v1`; `show` is absent from help and
   rejected as an unknown command.
7. All CLI JSONL source locations use one-based lines, zero-based UTF-8 byte
   columns, and half-open ranges. Unicode regression cases distinguish byte
   columns from Unicode-scalar and UTF-16 columns.
8. LSP encoding negotiation and terminal human diagnostics are unchanged.
9. LANGUAGE SSOT and maintained command examples describe only the accepted
   query grammar and coordinate contract.
10. Formatting, CLI/core/LSP tests, and the complete workspace suite pass.

## Rejected alternatives

### Keep `show` or rename it to `info`

`show` emphasizes presentation and `info` suggests one human-oriented summary.
The surface performs typed, filtered queries over several semantic domains and
returns a machine protocol. `query` states that contract directly.

### Keep `symbols` and add a separate position command

The module itself is already a useful semantic location. `query at <module>`
and its line/column refinements form one hierarchy without inventing separate
`symbols` and `facts` concepts.

### Retain mode flags

An optional module positional combined with `--modules`, `--exports`, and
`--at` recreates conditional requirements and conflicts that the command tree
can express structurally.

### Reuse LSP coordinates in JSONL

LSP positions depend on negotiated client capabilities and use zero-based
lines. CLI JSONL must be deterministic without a client handshake. The two
protocol boundaries therefore convert independently from shared byte ranges.

### Make JSONL position encoding configurable

Changing coordinate meaning per invocation makes stored JSONL records
ambiguous and complicates every consumer. UTF-8 byte columns are the fixed CLI
schema contract; consumers needing negotiated positions should use LSP.

