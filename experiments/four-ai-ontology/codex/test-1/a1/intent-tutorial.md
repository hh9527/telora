# Telora Intent Authoring Subset

Telora source files use `#` comments. Import selected public names with:

```telora
import "package/module.telora" { name };
```

Closed enum values use a leading single quote, such as `'Variant`. Arrays use `[a, b]`; records
use `{field: value}`. Arrays are homogeneous and record fields are statically checked.

Publish the result of an expression with:

```telora
export let output = expression;
```

An expressible trial imports the published `compile` function, constructs one `AnalyticsIntent`
from public enum values, calls `compile`, and exports that call as `output`. It must not emit SQL,
physical mappings, an execution plan record, or a replacement compiler.

If a request cannot be represented by the closed public vocabulary, do not substitute a nearby
identifier. Leave `intent.telora` as a refusal marker and explain the missing public concepts in
`NOTES.md`; the Host classifies that artifact as a refusal rather than a compilable intent.
