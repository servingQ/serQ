# Braced definitions

Definitions use the same braced outline as `fn main()`. A body is either
one expression without a semicolon, or statements. Empty bodies are statements.
Statements followed by a result expression are rejected, including in unused
definitions. Calls still expand at their use sites; this introduces no function
runtime, local scope, return statement or IR change. External `--def` overrides
still supply an expression and apply only to expression definitions.

## Before and after

Before (`lib/vllm.sq`):

```serq
def reusable(x, bs) = floor((x - 1) / bs) * bs;
```

After (the same library, exercised by library and oracle tests):

```serq
def reusable(x, bs) { floor((x - 1) / bs) * bs }
```

Statement definitions keep their spelling and meaning. Generated IR and deployment
figures must remain identical. The former `= expression;` spelling is rejected
with a migration hint so that there is one spelling for each definition.

## Self-critique

The shared outline hides the expression/statement distinction until the body
is read. The missing semicolon identifies a value; braces and statement
terminators identify a statement body. Calls remain checked against that kind.
Keeping `=` as an alias would avoid migration, but retain two spellings for the
same construct. General blocks with local bindings and a result would be more
powerful, but would change the expansion model and are outside this change.
