# std/args

```serq title="model.sq"
use "std/args";

fn main() {
  let cost_per = args.number("service", 1);
  stage svc : delay;
  server {
    run svc (cost(svc, cost_per));
  }
}
```

```serq title="workload.sq"
use "std/args";

let rate = args.number("arrival_rate", 0.3);

workload {
  arrive poisson(rate);
}
```

`args.number("name", default)` declares a numeric input as the entire
initializer of a `let` inside `main`, or of a top-level `let` in a workload
file, which has no `main`. The inputs of both files are the program's. Its external name is an identifier;
its local binding may have a different name. Read each input once and reuse
that binding. The default is a constant expression and may read earlier
constants. Values are numbers (including `inf`), never NaN. Strings,
positional arguments and implicit access to the host environment are not
part of this numeric library.

| Argument | Type | Description |
|---|---|---|
| `"name"` | string, an identifier | The input's external name. |
| `default` | `const` | The value when no input is supplied. |

`serq run model.sq --workload workload.sq --horizon 10 --arrival_rate 0.5 --service 2` supplies the values.
Inputs stand among the interpreter's own options (`--seed`, `--horizon`, …)
in any order, so an input may not take one of their names.
`--arrival_rate=0.5` works too; repeated options use the last value.
Unknown names, missing values and invalid numbers fail before simulation.
`--arrival_rate 0.5`, [instances](../program.md#instances) and Python
`sets={"arrival_rate": 0.5}` bind the same declared input. Instances and
API strings can also supply a constant expression.

Inputs become constants before IR is produced. Runtime expressions cannot
call `args.number`. Array sizes are resolved during parsing, so supplying
an input that affects an array size is refused, including through a
derived constant.

## See also

[`use`](../program.md#use), [`let`](../program.md#let),
[the CLI's `--name value` and `--instance`](../../reference/cli.md),
[`pyserq.compile`](../../python/compile.md).
