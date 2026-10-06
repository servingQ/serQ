# Explicit entry points and program inputs

An executable source file declares `fn main()`. Its body constructs the
deployment, workload, session and run settings. Definitions and constants
outside it do not start a simulation. The session body still executes once
per arrival: `main` is the construction entry point, not another session.

The wrong assumption behind unrestricted `--set` was that every named
constant was a public experiment parameter. It made internal calculations
part of the CLI by accident. The frontend now refuses undeclared inputs:
a plain `let` is fixed, and `args.number` explicitly exposes an input.
This is a source boundary check, not a new simulation rule.

## Before and after

Before, copied from `examples/multi-turn/vllm.sq` at `55e5453`:

```serq
let Lambda = 0.3;      // sessions per second
let p = 0.9;           // continue after a turn
let Z = 3.0;           // tool time (s)
```

Any of these constants could be replaced by `--set`, and the file had no
entry point. After, those declarations are inside `fn main()`:

```serq
let Lambda = args.number("Lambda", 0.3);      // sessions per second
let p = 0.9;           // continue after a turn
let Z = args.number("Z", 3.0);           // tool time (s)
```

The file imports `use "std/args";`. `Lambda` and `Z` are public inputs;
`p` is the program's fixed definition. CLI, instances and the Python API
all bind the same external names. Binding names can differ from option names.

A complete runnable example is `examples/single-turn/arguments.sq`:

```sh
serq run examples/single-turn/arguments.sq --json -- --requests 3 --service 2
```

It reports three arrivals and completions and `elapsed` count 3, mean 2:
three requests start at zero and each spends two seconds in a delay stage.
Supplying `--seconds_per_unit 5` fails because that constant is internal.
The report's shape and the program's default numerical meaning do not change.

## Mechanism and scope

The frontend supplies `std/args` as an installed standard library module.
`args.number("name", default)` is a declaration-time library call, used as
a main-local `let` initializer. Its result is a constant; it does not add
runtime environment access. This first library only serves numbers, the
source language's value type. NaN, missing CLI values, invalid option names,
duplicate input declarations and unknown options fail before execution.

After `--`, the CLI accepts `--name value` or `--name=value`. Before the
separator are interpreter options. `--set name=expr` remains an expression
transport for declared inputs, as do instance bindings and Python `sets`.
Last supplied value wins. None can replace a private constant. The separate
`--def` facility remains source substitution for expression definitions.

Global definitions precede the one `fn main()`. Main-local names are never
visible in later top-level items; these items are refused. Imported source
libraries still hold definitions and imports only, so importing a file
cannot run its main or construct a deployment. Definitions may also live
inside main, using the existing lexical expansion rules.

Array sizes are already expanded during parsing. As before, a supplied input
cannot change such a size, directly or through a derived constant. The check
tracks the local binding, not the option name, so a differently named input
cannot accidentally prohibit an unrelated constant.

## IR and migration

Entry points and inputs belong to the frontend. Linking resolves them to the
same `ir::Program` fields and expressions as before. There are no new IR
variants, meanings or versions, and no changes to the Lean fragment. The
oracle IR and drawing goldens must continue to compare equal.

Executable examples and tutorial files now declare main. Existing semantic
tests that assemble deployment fragments use a test-only main-body builder;
entry-point and input-boundary regressions exercise the public API directly.
Instances and definition-only libraries are not wrapped. Previous source
programs must add an entry point, and sweep parameters must opt in through
`args.number`. This is an intentional source compatibility change.

## Self-critique

- **A `param` keyword** would separate public and private constants, but would
  give a language keyword the job the requested input library can do. It is
  not added alongside `args.number`.
- **Keeping implicit execution for old programs** would leave two entry-point
  rules and defeat the requirement that executable code be explicit. Old
  source programs are migrated instead.
- **Reading arbitrary host arguments at runtime** would make a serialized IR
  insufficient to reproduce a run. Inputs are resolved before IR is emitted.
- **A generic function runtime or general module system** is not required to
  name the construction entry point. Existing `def` expansion remains the
  reuse mechanism. `fn` currently introduces only main; this limitation is
  diagnosed rather than implying that arbitrary functions execute.
- **All expressions calling `args.number`** would obscure the declaration of
  the program's input interface. Read an input in one `let`, then compute
  derived values with ordinary expressions. Required inputs, strings and
  automatic program-specific help can be added when their use cases are
  specified; this library makes the numeric/default contract explicit.

## Review

1. **Purpose kept.** Main identifies construction; the workload and server
   remain the serving specification. Release/debug semantic tests, oracle IR
   comparisons and drawing goldens preserve the same numerical behavior.
   No interpreter or IR code changes. The pinned citation checker resolves
   all 102 references.
2. **Simpler possible.** These are frontend constructs, with no runtime
   function or input node. The existing input transports share one linker
   check. `param` and implicit execution are rejected above.
3. **Complexity did not grow in the kernel.** One keyword (`fn`, 106 total)
   replaces implicit construction. The numeric library replaces the rule
   that all constants are externally replaceable. IR fields, variants and
   the Lean fragment are unchanged; generated Lean files remain current.
4. **Intention plainer.** `fn main()` names the entry point and
   `args.number("name", default)` names a public numeric input. External and
   local names are separate. Unknown-input errors list the public names.
   The docs lexer also recognizes the dot in `args.number` as punctuation;
   its omission was a rendering bug on valid source, so no linker check applies.
5. **Evidence is the program.** The runnable argument example gives three
   completions at two seconds, as derived above. The new boundary tests use
   public compilation directly, including CLI/instance agreement. Python
   tests exercise the same interface. All 90 existing PD sweep configurations
   were linked, including their source edits; their full simulations were
   not rerun because neither defaults nor engine behavior changed.
6. **One change.** Source entry points and explicit inputs, including examples,
   tests, API documentation, the docs lexer and keyword metrics. The repository
   has no separate editor grammar. Committed oracle IR and drawing assets do
   not change.

Validation: `make check`, Python binding tests from a freshly built local
wheel, `mkdocs build --strict`, generated Lean freshness checks and pinned
citation checks. `IR_VERSION` stays 11.
