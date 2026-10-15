# serQ: the text syntax, the semantics, and the vLLM correspondence

This reference defines the text syntax and its semantics. Text compiles to
[the IR](ir.md), which the interpreter runs and the Lean generators read.
For individual constructs and examples, use the [API reference](api/index.md).

## Overview {#1-what-serq-is-for}

A serving deployment is a program. The program names the resources of the
deployment (memory pools, stages) and gives the path every request runs
(the `server`). How sessions arrive and how a session's turns evolve (the
workload) is a file of its own, given when the program runs
(`serq run DEPLOYMENT --workload WORKLOAD`, [paired files](#paired-files)).
The pair is simulated (`serq run`), stated about and proved about in Lean
(a fragment of the IR, `docs/lean.md`), and drawn (`serq draw`).

The organising idea: a deployment does four things to a request. It makes
it **wait for a resource**, **runs** it on a stage, **frees** the resource
(possibly keeping a prefix cached), and **sends it somewhere next**. The
first three are one scoped statement (`hold`); a resource is a pool,
whether it is KV memory or request slots; and the colocated engine that
prefills in the compute its decode step leaves is a stage kind of its own
(`step`).

## Syntax {#2-syntax}

```
program  := (constant | definition | import)* fn main ( ) { item* }   -- the deployment: no workload item
workloadfile := (import | constant | input | definition | wstage | wpool)* workload { wlitem* } (import | constant | input | definition | wstage | wpool)*
                                             -- the workload, a file of its own: no fn main, exactly one workload
input    := let NAME = args.number ( "argument_name" , expr ) ;   -- in a workload file, at the top level
wstage   := stage NAME [ '[' N ']' ] : kind ;   -- a stage the workload runs on itself
wpool    := pool NAME { poolopt* }           -- a pool the workload holds itself (no `on`): a session cap
constant := let NAME = expr ;
definition := def NAME ( NAME , ... ) ( { expr } | block )
import   := use "std/args" ; | use "std/kvt" ; | use "std/share" ;   -- a program is one file (the deployment), run with one workload file
item     := let NAME = expr ;
          | let NAME = args.number ( "argument_name" , expr ) ;
          | def NAME ( NAME , ... ) { expr }   -- a name for an expression: NAME ( arg , ... )
          | def NAME ( NAME , ... ) block      -- a name for statements: NAME ( arg , ... ) ;
          | pool NAME [ '[' N ']' ] { poolopt* }
          | stage NAME [ '[' N ']' ] : kind ;
          | device NAME [ '[' N ']' ] { resource* }      -- what an engine runs on (below, *Engines on devices*)
          | engine NAME [ '[' N ']' ] on DEVICE { eitem* }   -- a stage that runs iterations (the IR's step stage)
          | pool NAME on OWNER { poolopt* }    -- a capacity of a device or an engine, as a pool
          | pool NAME on ENGINE.DEVICE { poolopt* }   -- the device's capacity, whose queue the engine admits: the name before the dot is that engine
          | server [ ( [hidden] NAME , … ) ] [ reply ( NAME , … ) ] block   -- request handling, and what a request and its reply carry; the session lives inside the workload file's workload
          | queue NAME [ '[' expr ']' ] [ : ROLE [, ROLE]* ] { qitem* }   -- a station: its pools, stage and entries (below, *Queues*)
          | kvt.nixl_pull ( QUEUE , QUEUE [, latency = expr ] , share = POLICY ) ;   -- the source, its reader, the read (below, *Queues*)
          | kvt.nixl_push ( QUEUE , QUEUE [, latency = expr ] , share = POLICY ) ;   -- the source, its reader, the write
          | gauge NAME = expr ;              -- the time average of a function of the state (Gauges)
          | claim NAME [given ( expr )] : every iteration of STAGE ( expr ) ;   -- a proposition about every path (Claims)
          | claim NAME [given ( expr )] : some iteration of STAGE ( expr ) ;
          | claim NAME [given ( expr )] : at end ( expr ) ;
qitem    := pool NAME { poolopt* }              -- the queue's own; only its entries hold it
          | device NAME { resource* }           -- the member's device, `QUEUE.NAME`
          | engine on DEVICE { eitem* }         -- the queue's stage, named after the queue
          | pool NAME on OWNER { poolopt* }     -- on the queue's own device, or on the queue (its engine)
          | pool NAME on QUEUE.DEVICE { poolopt* }   -- on the queue's own device, admitted by the queue's engine (named QUEUE)
          | serve kind [ latency expr ] ;       -- the queue's stage, named after the queue; `latency` a link's
          | nic kind ;                          -- the queue's NIC, the stage `QUEUE.nic`
          | VERB [ ( NAME, ... ) ] [ from NAME ] block   -- an entry of one of the queue's roles
poolopt  := cap expr ;                       -- capacity in units (default inf)
          | block expr ;                     -- allocate and cache in blocks
          | evict lru ; | evict by ( expr , ... ) ;   -- eviction order (ascending keys)
          | preempt none ; | preempt lifo ;  -- what a failed growth does
          | preempt by ( expr , ... ) [requeue head | requeue tail] ;   -- the victim: least keys
          | preempt leases by ( expr , ... ) [every ( expr ) [suspend by ( expr , ... )]] ;
                                             -- the victim: the lease with the least keys, which ends;
                                             -- every: at the pool's ticks; suspend: set aside, given back
          | queue fifo [every ( expr )] ; | queue by ( expr (, expr)* ) [every ( expr )] ;
                                             -- waiting selection; every: admissions at the pool's ticks
          | reserve held ;                   -- a hold's unallocated reservation counts against later admissions
          | spill POOL via STAGE ( expr ) when ( expr ) ;  -- write evicted prefixes to a tier
kind     := fifo [ ( c ) ]                   -- c servers, one job each at rate 1
          | ps ( expr )                      -- throughput phi(present) shared equally; expr reads present
          | delay                            -- ps(present): every job at rate 1, no waiting
resource := NAME ( NAME , ... ) = expr ;           -- a time resource: a demand to time, read in `worker`
          | NAME cap expr ;                  -- a capacity, declared as a pool with `pool NAME on DEVICE`
eitem    := NAME [ '[' N ']' ] cap expr ;    -- a capacity the engine holds (`reqs cap max_seqs;`); `[N]`: N queues it admits
          | tokens cap expr ;                -- the tokens one iteration computes (required; `inf` for none)
          | granule expr ; | state NAME = expr ;
          | scheduler { (let NAME = expr ;)* sstmt+ }   -- what each iteration does (required)
          | worker { forward ( expr ) ; }    -- what the worker does with the batch: its forward's time (required)
          | connector QUEUE { [per_token ( expr ) ;] [load ( expr ) ;] [save ( expr ) ;] }
                                             -- its KV connector: to an `l2` queue, or from the source of
                                             -- its queue's KV relation (*An engine's request*)
sstmt    := advance running [only ( expr )] [admission | by ( expr , ... ) | decode first] [each at most ( expr )] ;
          | admit waiting [only ( expr )] [while ( expr )] [each at most ( expr )]
                [taking POOL ( expr ) [reserve ( expr )] , ... [reuse ( expr )]
                        [prefix ( expr : expr , ... )] [cache ( expr )]] ;
                                             -- what the admission takes of a request, reads of its
                                             -- cache, and what its release leaves cached
          | exclusive prefill [each at most ( expr )] ;
          | if ( expr ) { sstmt* } [else ( { sstmt* } | if … )]
          | set NAME = expr ;
wlitem   := arrive poisson ( rate ) ; | arrive renewal ( expr ) ; | arrive closed ( n ) ; | arrive batch ( n ) ; | arrive none ;
          | trace "file.csv" [ordered] ;      -- replay sessions from a trace
          | init block | turn block          -- Size / set / observe
          | send ( NAME [: expr] , … ) ;      -- what each request carries: the server's parameters
          | session block                    -- optional: the sequence of completed turns; reads the reply as reply.NAME
stmt     := turn ;                           -- draw attributes, submit, wait for the response (session only)
          | Size NAME = expr ;                -- workload request quantity
          | Cost NAME = expr ;                -- expression must have a resource Cost type
          | Cost NAME = { RESOURCE : expr [, RESOURCE : expr]* } ;
          | set NAME = expr ;
          | observe NAME = expr ;
          | hold POOL ( expr ) [reserve ( expr )] [, POOL ( expr ) [reserve ( expr )]]*
                 [prefix ( expr : expr , ... )]   -- the sequence as keyed segments
                 [reuse ( expr )] [at admission ( NAME = expr , ... )]
                 block [ cache ( expr ) ] [ lease POOL ( expr ) ] [ ; ]
                                             -- lease: that pool's allocation outlives the scope,
                                             -- until a release/transfer takes it, expr seconds, or the end
          | grow POOL ( expr ) ;             -- the enclosing hold's allocation on POOL grows, or the session's lease of it
          | shrink POOL ( expr ) ;           -- it gives expr back: the inverse of grow, a hold keeping what it computed
          | drop POOL ;                      -- discard the own cached prefix
          | release POOL ;                   -- give the enclosing hold's allocation on POOL back now, or end a lease of it
          | load POOL ( expr ) ;             -- the KV of expr tokens arrived: the enclosing hold's computed position advances
          | run STAGE [prefill | decode] ( expr ) [ growing POOL ] ;
                                             -- the mode: required on an engine, refused on any other stage
                                             -- growing: inside a hold of POOL, which grows with the tokens
          | run STAGE , STAGE [, STAGE]* ( expr , share = POLICY ) ;   -- one job holding every stage at once
                                             -- either: ( expr [, share = POLICY] [, latency = expr ] ), the wait before the work
          | run ( expr ) ;                   -- in a queue's entry: the queue's own stage
          | if ( expr ) block [ else ( block | if … ) ]   -- a test; a draw is if (~bernoulli( p ))
          | while ( expr ) block             -- test before each pass; continue after the block
          | loop block
          | fork block                       -- the block runs beside the session: a leg of the request
          | join ;                           -- wait until every leg forked so far has ended
          | choose NAME in expr by ( expr , ... ) ; -- NAME := argmin over 0..n, keys in order
          | end ;
          | QUEUE [ '[' expr ']' ] . VERB ( expr, ... ) [ from QUEUE [ '[' expr ']' ] ] [ to POOL ( expr ) ] ;
                                             -- a queue's entry, in its place (*Queues*)
          | mark NAME ;                        -- in an entry: the moment, read by the caller as QUEUE.NAME
          | ENGINE ( expr , expr ) ;         -- in a server: a request to the engine, its input and output tokens
          | serving                          -- the serving vocabulary, sugar for run
serving  := transfer [ '[' expr ']' | on STAGE [, STAGE]* ] expr from POOL to POOL ( expr ) ;
                                             -- the KV moves: run link; load; release
          | tool     [ '[' expr ']' | on STAGE [, STAGE]* ] expr ;
                                             -- transfer and tool on several stages: expr is ( expr , share = POLICY [, latency = expr ] )
POLICY   := share.maxmin | share.bottleneck   -- std/share: how the flows of a fabric divide it (below, *Stages*)
```

### Arrivals and the run

A model is a deployment and a workload, two files. Execution settings are supplied by
the CLI flags, an explicit instance file, or the host API; `run { ... }` is
not a model declaration. The resolved IR includes the execution settings.

`arrive renewal(e)` draws or gives each gap,
which must be positive and finite (a constant gap that is not does not
link; a drawn one stops the run), and the first renewal arrival comes after
one gap; `poisson(rate)` (a positive, finite constant) arrives at time 0 and
then after exponential gaps of mean `1 / rate`. `--arrivals N` runs exactly `N` open-workload arrivals and drains their
sessions; failing to by `horizon`, or draining at or before `warmup`, is an
error. A count is a whole number, or the program does not link: `closed(n)`
and `batch(n)` from 1 to a million sessions. The external `arrivals` setting
is a positive integer and `seed` is a nonnegative integer. The
report keeps `horizon` as configured and gives the time the run ended as
`end`; averages and rates are over `end - warmup`.
See [Workload](api/workload.md) and [A finite run](api/program.md#a-finite-run).

### Expressions

Arithmetic, comparisons (0/1), `&&`, `||`, `!` and
`c ? a : b` (a non-zero operand is true; `if` and `while` guards are held to 0
or 1); the draws `~exp`, `~det`, `~uniform`, `~erlang`, `~h2`,
`~bernoulli`; functions, observables of pools and stages, and aggregates
over an index, `max j in n (e)`, `min j in n (e)`, `sum j in n (e)`: `n` is
a number, a constant's name or a parenthesised constant expression, `j` is
a name the program does not already have, and the linker writes the terms
out with `j` = 0 … n-1 (`max k in 2 (used(kv[k]))` is
`max(used(kv[0]), used(kv[1]))`), at most 4 096 terms in a program, nested
ones included. The catalogue is the API reference: [functions](api/functions.md),
[distributions](api/distributions.md), [context
variables](api/context.md), [attributes](api/attributes.md).

The rules that are the language's, not the catalogue's:

- A **context variable** is available only at the evaluation moments that
  supply it; reading it elsewhere is a link error. For example, a session
  statement cannot read `tokens`. See [context variables](api/context.md)
  and [IR moments](ir.md#moments) for the complete table. `now` is not
  permitted in gauges, `given`, `only` predicates or iteration guards.
  `budget_left(step)` plans an iteration and cannot be read by that
  stage's own `budget` or `chunk`.
- **Names** do not collide: session attributes, `let` constants and the
  names the language supplies (the context variables, `inf`, an engine's
  lists `running`, `waiting` and `batch`) are kept apart, so
  `set present = …` does not link (a `ps` capacity reading `present` would
  read the attribute). Nor does a `def`, a parameter, an `observe`, a
  `stage`, a `pool`, a `device`, an `engine`, a `queue`, a `gauge` or a
  `claim` take a supplied name: `observe admitted = now;` does not link,
  nor an attribute's, built in or set (`stage think` does not link); a
  program may still `set` a built-in attribute. Two declarations of
  different kinds, a `let` among them, do not share a name either
  (`observe svc` or `let svc = 1;` beside `stage svc` does not link); an
  `observe` written again records the one statistic at another place. A
  queue's own pool or device (`P.kv`) shares its name only with what the
  queue's body reads bare: another of the same queue, a `def` and a
  `let`. `def kv` or `let kv = 1;` beside queue `P`'s `pool kv` does not
  link; a top-level `pool kv` or `stage kv` beside it links. A schedule's
  `let` is read only in its `each at most`, which reads the program's
  `def`s and `let`s, so it may not share its name with a `def` or a
  program's `let`, and may with a pool or a stage.
- **Constants** (a `let`, a `cap`, a `block`, a `fifo` count, the arrival
  rate or population) are numbers or `inf`; one that
  evaluates to NaN does not link.
- **Attributes.** Every name a `set` or a `choose` assigns is a session
  attribute. The built-in ones are `serial`, `turn_no`, `cached` (the prefix
  consumed at the last admission; 0 after a hold without `cache`),
  `computed` (the position a preempted hold had reached, 0 otherwise; [Semantics](#3-semantics)),
  and with a trace `new`, `out`, `think`, `more`, `forced`.

Workload assignments produce read-only-to-server `Size` attributes. A server
computes separate bookkeeping values and explicitly interprets quantities
with `cost(resource, expression)`. Resource primitives require this nominal
cost type; serving vocabulary performs the named conversion itself. The
contract applies to direct JSON IR as well as source. See
[attribute types](api/attributes.md#sizes-values-and-costs) and
[`cost`](api/functions.md#cost).

### Entry point and external inputs

Source programs declare exactly one `fn main()`. It constructs the deployment
once; each arriving session executes the session body of the workload file
the program runs with. Execution settings are supplied externally.
Only imports, definitions and fixed constants may precede `main`. Declarations
after it and nested or duplicate entry points are refused. A workload file
has no `fn main()`; its `args.number` inputs are top-level `let`s
([paired files](#paired-files)).

A plain `let` cannot be replaced externally. With `use "std/args";`, a
`let` inside `main` may declare an input using
`args.number("name", default)` as its entire initializer. CLI arguments
(`--name value`), instance bindings and API `sets` supply only those named
inputs. Defaults and supplied expressions are resolved at link time; the
result is the same IR as a program written with those values. An input that
would change a parse-time array size is refused. See [std/args](api/std/args.md)
for the full input contract. `use "std/kvt";` supplies the KV-transfer
vocabulary ([std/kvt](api/std/kvt.md)), `kvt.nixl_pull (…)` and `kvt.nixl_push (…)`
(below, *Queues*), and `use "std/share";` the sharing
policies ([std/share](api/std/share.md)), `share.maxmin` and
`share.bottleneck` (below, *Stages*).
Syntax snippets elsewhere may show just a main body or one of its nested
blocks.

### The serving vocabulary

`Size` and `Cost` declarations state attribute types explicitly. A composite
Cost evaluates each field once in written order and stores a separate cost
for that resource; `processing.mem` and `processing.svc` are scalar fields.
The source lowers to existing assignments and conversions, with no new
execution scope. See [attribute types](api/attributes.md#sizes-values-and-costs).

The statements above are about resources: `hold` a pool, `run` a stage.
The serving forms name a part of the request's lifecycle that is more than
one run (a KV transfer) or that is not the engine's (a tool call). They are
sugar: the parser rewrites each to the kernel statement it stands for, so
the IR (`serq ir` prints the kernel), the interpreter and the Lean model
know nothing of them. Prefill and decode are not forms: on a step engine
they are the mode of the run, written only where the engine's own work is.
An engine keeps a request's state and runs its prefill and decode itself
(*An engine's request*); a queue's entry runs its queue's engine as `run P
prefill (cost(P, T))`. The server, a gateway's `route` and the session
call the engine (`E (input, output);`) and do not run an engine themselves.

| Serving form | Kernel |
|---|---|
| `transfer (X) from P to Q (n);` | `run link (cost(link, X)); load Q (cost(Q, n)); release P;` — the KV of `n` tokens moves from the session's lease (or hold) on `P` to its hold on `Q`: the link takes the time, the tokens count as computed at `Q`, and `P` is given back (below, *A KV transfer*) |
| `tool Z;` | `run tool (cost(tool, Z));` |
| `tool[j] Z;` | `run tool[j] (cost(tool, Z));` (the index applies to the role's stage array) |
| `tool on S[j] (Z);` | `run S[j] (cost(S, Z));` |
| `transfer on egress[i], ingress[j] (X, share = s) from P to Q (n);` | `run egress[i], ingress[j] (cost(egress, ingress, X), share = s); load Q (cost(Q, n)); release P;` — one read that holds the sender's link and the receiver's at once (below, *Stages*); a link's `serve … latency x` puts `latency = x` on the `run` |

The argument is work in the unit of the stage it runs on: the time the job
takes alone on a `fifo`, `ps` or `delay` stage (seconds, when the program's
clock is seconds; a `ps` stage serves it at `φ(present)/present`).

#### Which stage

A form finds its stage among the stages declared above
it (declarations come first in every program here): the stage whose name
is the role's, `link` (or `transfer`), `tool`. Exactly one must qualify:
with none (`stage svc : fifo;` and `transfer (X) from P to Q (n);`, or
`tool Z;` in the server) or several (`link` and
`transfer`) the parser stops at the form and says so. `on STAGE` names the
stage explicitly; with several instances of a role, `choose j …; tool[j] Z;`
serves an array and `tool on S2 (Z);` stages that are not one.

A tool call in the workload needs no declaration. It is the client's time
between turns, not a resource of the deployment: when no stage `tool` is
declared above the workload's `tool Z;`, the parser supplies `stage tool :
delay;` after the declared stages, and the call runs on that delay of the
workload's own, an infinite server where no call waits for another. The IR
is the one the declaration gives, and the report shows the stage `tool`.
A tool server with capacity, which sessions share (a sandbox pool, a
rate-limited API), is declared in the workload file, which runs on its own
stages only: `stage sandbox[4] : fifo;` there, and name it,
`tool on sandbox[j] (Z);`; the workload naming a stage of the deployment's is
refused ("stage `sandbox` is the deployment file's"). A declared stage
`tool` of any kind, the deployment's included, is still the one `tool Z;`
uses; one declared below the workload's `tool Z;` is refused, since the
call has found no stage above it and the program would have two. A `tool` in the server, or indexed,
`tool[j] Z;`, finds a declared stage as above. The run is
plain: `transfer` and `tool` on a step engine are rejected by the linker as
`run E (cost(E, X))` would be. A linker error inside a form (an unknown name
in `Z`, say) speaks of the kernel statement.

#### A KV transfer

`transfer (X) from P to Q (n)` runs the link for `X`
units of work, loads `n` computed tokens into the destination hold on `Q`,
and releases the source hold or lease on `P`. Both allocations must exist
during the copy. A transfer without `from P to Q (n)` is a parse error;
use `run` directly to model link work without a KV copy.

`lease P (t)` names one of the hold's pools whose allocation stays the
session's after the scope's end, neither evictable nor a preemption
victim (unless the pool says `preempt leases by`, below), until the session's `release P` (a `transfer … from P` contains
one), `t` seconds, or the session's end, and then `cache` applies (`inf`
keeps the lease until an explicit release or the session's end). `release P` with a hold on `P` gives the innermost
enclosing hold's allocation there back now, caching per that hold's `cache`,
and the scope's end then has nothing left there. `load Q (cost(Q, n))` says the KV
of `n` tokens arrived from outside the engine: the enclosing hold's
computed position on `Q` advances by `n` (within its allocation), as a
`growing` run's would token by token, so `cache` and `cached` count them.
`transfer (X) from P to Q (n)` is the two around the link run.
See the [prefill/decode example](use-cases/pd.md) for a complete program
and its source correspondence.

The [vLLM correspondence](#7-vllm-v1-as-a-serq-program) maps serving
constructs to the pinned scheduler source.

### The two sides

A workload describes arrivals, turn attributes and how turns follow one
another. A server handles every turn. Omitting `session` means one turn
per arriving session; there is no call or termination statement to write.

For a conversation, `turn;` draws the next turn's attributes, submits it
to the server, and waits for its response. Statements after it see the
completed turn. The workload is a file of its own
([paired files](#paired-files)); from `examples/workloads/chat.sq` and the
deployment it runs on, `examples/multi-turn/vllm.sq`:

```serq
// examples/workloads/chat.sq
workload {
  arrive poisson(Lambda);
  init { set K = 0; }
  turn { … }
  send (input: K + n, output: o);
  session {
    turn;
    while (more) {
      set K = K + n + o;
      tool (~exp(Z));
      turn;
    }
  }
}

// examples/multi-turn/vllm.sq, in fn main()
server (input, hidden output) {
  vllm (input, output);
}
```

The report gives what the client saw of each turn, its time to first token,
time per output token and response time ([What the client
sees](#what-the-client-sees)); the program observes what else it needs.

The end of the session block ends the session. `end;` is only needed for
an early exit, including an exit from an unconditional `loop`.

#### What crosses

The server's signature is the deployment's interface, as an API's provider
writes one: `server (input, hidden output)` is what a request carries, and
`reply (R, …)` after it what its reply carries. The workload binds the
parameters once, `send (input: K + n, output: o);`, and each `turn;`
evaluates the binding after the `turn` block's draws; `send (s)` sends the
attribute `s` as it is. After `turn;` the workload reads the reply as
`reply.R`. Nothing else crosses:

- the server, its gateways' routes and the scheduler's expressions (queue,
  preempt and eviction keys, an engine's schedule) read the parameters,
  never an attribute the workload keeps (`K` above); a queue's entry reads
  what it is passed and the `hidden` parameters, as it read the hidden
  attributes;
- the workload reads its own attributes, those it sends as they are, and
  the reply after a `turn;`: never a parameter it binds to an expression,
  nor anything the server, the scheduler or an engine sets (`cached`,
  `vllm.first`);
- `serial`, the session's number, every request carries.

A parameter may be `hidden` ([`hidden`](api/workload.md#hidden)). The
server sets each reply attribute on every path that completes the
request. What the server sets stays the session's
from one request to the next (a sticky `home` in
`examples/multi-turn/routing.sq`); the parameters are bound anew at each.
`examples/pd-disaggregation/llmd_nixl_pull.sq` replies with the tokens the
decoder produced, which `max_model_len` can cut short, and its client
(`examples/workloads/chat_reply.sq`) adds them to its context:

```serq
send (input: K + n, output: o);
session { turn; while (more) { set K = K + n + reply.produced; … } }

server (input, hidden output) reply (produced) {
  if (input >= max_model_len) { end; }
  gw.route();
  set produced = D.produced;
}
```

A bare `server { … }` takes nothing and replies nothing: it reads no
attribute of the workload ([the design](design/request-interface.md)).

#### Paired files

Since the signature is all the two sides share, the program is the
deployment and the workload is a file of its own, given when the program
runs: `serq run DEPLOYMENT --workload WORKLOAD`, likewise `check`, `ir`,
`draw` and `target`, and pyserq's `compile(path, workload=…)`
([the design](design/paired-workload.md)). The deployment file is a
program with no `workload`: a `workload { … }` in its `fn main()` is a
compile error ("a workload is a file of its own, given when the program
runs"), and so is running its server with no workload given. The workload
file has no `fn main()`: its `use`s, its constants and inputs (`let`, an
`args.number` among them), its definitions (`def`), the stages it runs on
itself (`stage user : delay;`), the pools it holds itself (`pool live { cap
24; }`, no `on`: a cap on live sessions is the client's) and one
`workload { … }`. An `engine`, a `server` or a `device` in it is the
deployment file's and refused.

```serq
// examples/workloads/human_chat.sq (examples/multi-turn/vllm.sq runs it)
let Lambda = 0.3;      // conversations per second
let p = 0.8;           // the person asks again
let Z = 20.0;          // reading and typing time (s)

stage user : delay;

workload {
  arrive poisson(Lambda);
  …
  send (input: K + n, output: o);
  …
}
```

Each file reads its own names: no constant or definition of the workload
file is read by the deployment, none of the deployment's by the workload, and
both may declare `p`, each meaning its own. The workload runs on and holds
its own stages and pools, the server none of them; what the server does
to the session's memory it does on what the workload sends (a session's
last request sends `final`, and the server `if (final) { drop kv; }`,
`examples/multi-turn/replica.sq`). The inputs of both are the program's
(`--Lambda 0.5 --max_seqs 32`). A message names its file, at its own line.

The examples' deployments keep their paths; their workloads are in
`examples/workloads/` (the tutorial's in `docs/tutorial/programs/workloads/`),
and `examples/pairs.json` maps each program to its workload, or to the
workloads it runs under, the one it starts with first. One workload can run
on several deployments (`chat.sq` on `vllm.sq` and `vllm_lmcache.sq`), and
one deployment under several workloads (`vllm.sq` under a chat, a person's
chat, single turns and subagents).

The parser puts what `send` binds at the end of the `turn` block, a
hidden parameter in the IR's `hidden`; it expands
each source `turn;` into the IR's attribute-drawing `Turn` followed by the
server's statements. The interface is the frontend's: it adds no IR
construct, scope or dispatch, and a parameter sent as it is (`send (s)`)
leaves the IR as it was.

The IR retains statement sides and attribute types to enforce workload size
ownership. The parser and IR validator enforce these boundaries:

| | Session | Server |
|---|---|---|
| Turn lifecycle | `turn;`, optional early `end;` | `turn;` refused; completion is the end of its block; `end;` refuses the request and ends the session, outside a `hold` or a `fork` |
| Repetition | `while (condition) { … }`, `loop { … }` | The same, without session lifecycle statements |
| Queue entry calls | Refused | `gw.route();`, `D[j].decode (prompt);`, … |
| Admission | `hold … at admission (…) { … } cache (…)` | The same |

There is one `server` per workload. An explicit session must contain a
turn, and `session` belongs inside `workload`. Gateway selection and
routing belong in the server, for example `server { gw.route(); }`.
Several gateways may be declared, but none implicitly becomes the server.

### Queues

A request travels through stations, and each station is a queue: a waiting
line, an admission, a service, memory of its own.
`examples/pd-disaggregation/llmd_nixl_pull.sq` has a gateway and prefill and
decode instances, each instance with its NIC. A `queue` declares one station
whole:

```
queue P[NP] : prefill {
  device gpu { …; kv cap blocksP * bs; }
  engine on gpu { reqs cap max_seqsP; tokens cap B; scheduler { … } worker { forward (…); } }
  pool reqs on P { queue fifo; }
  pool kv on gpu { block bs; evict lru; preempt lifo; }
  prefill (prompt) {
    hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, reused + budget_left(P)))) reserve (cost(kv, prompt))
         at admission (reused = min(cachedin(kv), floor(prompt - 1, bs))) {
      set c = cached;
      run P prefill (cost(P, prompt - c)) growing kv;
    } cache (cost(reqs, kv, prompt)) lease kv (inf);
  }
}
```

The pools are the queue's (`P.kv` from outside, `kv` within), the stage is
named after the queue (`pool reqs on P`, `budget_left(P)`, `work(P[i])`), and
an *entry* — one per role the queue plays — holds what the station
does with one request, with the role's parameters (`prefill (prompt)`). A
queue declares its device and pools, then its stage (its engine, or a
`serve` of `fifo`, `ps` or `delay`), then its entries, each reading what is
above it; a pool `on` the queue's device or engine may follow the engine. The body is the server's statements; `run (cost(P, X))` with
no stage names the queue's own, and a step engine's run names it with its mode (`run P prefill (cost(P, T))`);
the stages that are not a step engine (a link's, a delay) the body may name
as a `server` does. `self` is the member's index in a family; nothing may be
declared `self`, which an entry reads before a parameter or local. A family's
size may be a `let` constant (`queue D[ND]`), and a family of one is still
a family: its entries are called `D[j].decode (…)` and its lease taken
`from D[j]`, as at any size. A reference to a member's pool or stage
(`holders(D[j].kv)`) is the kernel's array reference, which at size one
also takes `D.kv`.

Six roles are built into the parser, and a queue declares which it plays:

| Role | Entries | The queue |
|---|---|---|
| `gateway` | `route { … }` | `Q.route();` enters this queue's `route`; each gateway is a single queue |
| `prefill` | `prefill (prompt)` | computes the prompt; how it leaves the KV (`lease`, `cache`, a transfer) is the entry's |
| `agg` | `decode (prompt)` | aggregated: prefills the prompt and decodes on its own engine |
| `decode` | `decode (prompt) from Q` | disaggregated: decodes with the KV `Q`'s entry leased for this request |
| `link` | `transfer (n)`, or none | the NIC: the body is the time to read `n` tokens; without one, the `serve` is the cost, and its `latency` a wait before it |
| `l2` | none | an L2 KV cache (an LMCache server, a Mooncake store): one pool, the KV it holds, and a `serve`, the bandwidth its copies divide; an engine's `connector` copies through it (below) |

An editor reads this table, each role's entry verb, how many parameters
it takes and whether it is written `from Q`, from
`serq::frontend::highlight::queue_roles()` (the parser's `queue::ROLES`);
a test holds the two equal.

The server selects a gateway with `gw.route();`. The parser checks the
queue and its entry, then expands the route at the call site. Declarations
may follow the server and several gateways may coexist. Declaring a gateway
does not execute it or register a default server.

`queue`, `pool`, `serve`, `nic`, `turn` and `mark` are language
syntax. The role names and their entry signatures in the table are
predefined vocabulary; `gw`, `P` and `D` are names declared by this
program. User-defined roles are not supported.

An entry's verb and its `from` name its role: `decode (prompt)` is an
`agg` queue's, `decode (prompt) from P` a `decode` queue's. A pod that
does either, prefilling a short prompt itself and reading a long one's KV
from a prefiller (llm-d's decode pod), plays both: `queue D[ND] : agg,
decode`. Its callers call `D[j].decode (prompt)` either way, with `from`
or without.

The deployment calls an entry where the request goes: `P[i].prefill
(prompt);` and `D[j].decode (prompt) from P[i];`. The entry names the
queue its KV comes from, `decode (prompt) from P`: a queue declared above,
every entry of which leases its pool on every way through it, so that
whichever one the request went through left the KV; a `from` on a queue
that leases nothing, or on one not declared above, does not link, and a
call `from` another queue is refused. Inside the entry `P` is that lease,
the pool `P`'s entry leased for this request on the member `P[i]` the call
names (`transfer … from P to kv …`); it is not a number, not even in an
index.

A pod's NIC is the pod's: `nic ps(BwD);` in a queue is the stage
`D.nic`, one per member. A *pull relation* says which queue reads the KV
from which, and how the reads share the NICs:

```
queue P[NP] : prefill { … nic ps(BwP); prefill (prompt) { … } }
queue D[ND] : decode {
  engine on gpu {
    …
    connector P { load (request.preempted ? 0 : request.input - request.hit); }
    …
  }
  … nic ps(BwD);
  decode (prompt) from P {
    set produced = min(o, max_model_len - prompt);   // generation stops at max_model_len tokens (sched/utils.py:114-120)
    D (prompt, produced);
  }
}
kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);
```

`kvt.nixl_pull (P, D)` is one line for the topology, the mode and the
policy: the KV goes from `P` to `D`, the source first as the bytes go,
`D` starts the read (NIXL pull), `D` waits `x0` before each one (a number
or a constant over `let`s; `latency = …` may be left out), and concurrent
reads divide the two NICs by its `share` (*Stages*), which the relation
must name: it is not left to a default. The decoder's engine reads the KV
with its `connector P` (*An engine's request*): for a request its entry
was called `from P[i]` with, the kernel's `run P.nic[i], D.nic[j] (…,
latency = x0); load D.kv[j] (…); release P.kv[i];`, and nothing
for a request its entry took from no source. Inside an entry of `D` called
`from P[i]`, a `transfer` without `on` is the same read written by the
entry. Both queues
are declared above the relation and have a `nic`; a reader has one source;
an entry of `D` `from` another queue is an error, and so is a reader with
no entry `from` its source (the queues the wrong way round), and a
`transfer` without `on` in a queue that pulls from none. The relation is std/kvt's:
the program imports it with `use "std/kvt";`, and then declares nothing
named `kvt`.

`kvt.nixl_push (P, D, latency = x0);` is the same copy written by the
other side (NIXL push): the bytes cross the same two NICs, and the source's
worker posts the write, so the wait before each copy is `P`'s. A push says who moves
the bytes, not when the decoder is asked for its blocks: that is the
dispatch, written where it happens, with `fork` (below,
`examples/pd-disaggregation/vllm_nixl_push.sq`); the decoder's connector
waits for the dispatch's leg (`join`) before the copy. The wait is a delay,
so a push and a pull of the same constants run the same numbers. A queue waits
before the copies it posts for one relation: one `latency` per poster.

An L2 KV cache outside the engine, an LMCache server or a Mooncake store,
is a queue that plays `l2`: one pool, the KV it holds, and a `serve`,
the bandwidth the copies in and out of it divide. It has no engine, `nic`
or entry, and it is one queue, not a family. Any entry or engine may read
its pool (`cachedin(lmc.kv)`), and an engine copies through it with its
`connector` (*An engine's request*). From
`examples/multi-turn/vllm_lmcache.sq`:

```serq
queue lmc : l2 {
  device host { kv cap l2_cap; }
  pool kv on host { block chunk; evict lru; }
  serve ps(1);
}
…
engine vllm on gpu {
  …
  connector lmc {
    per_token (r);
    load (request.preempted ? 0 : beyond(request.known, request.hit));
    save (floor(request.input + request.output, chunk));
  }
  …
}
```

The copies run at the L2's `serve`, `per_token` seconds a token, and
keep their KV there: a load leaves the L2 its entry, and a save copies
only the part of the session's entry it lacks. See [the KV-transfer
library](https://github.com/servingQ/serQ/blob/main/docs/design/kv-transfer.md)
for why the entry is the session's (*Known limitations*).

`transfer on L[k], M[l] (n) from S to P (m)` names the stages itself, as
a `server` does: link queues, or any `ps` stages. `latency x` on a link's
`serve` is the wait before such a transfer: every `transfer on` naming the
link waits it first, at the stages it names and taking none of their
capacity, the links' waits summed in the order named. A number or a
`let` constant; only a link without an entry takes one. The kernel's
spelling is the run's own clause, `run a, b (cost(a, b, w), share = s,
latency = x)` or `transfer on a, b (w, share = s, latency = x) …`: a wait
read at the run's start, in seconds; the stage report counts it as the
run's `wait`, and the job as present. A transfer over a link with a
`latency` may not name one too. A link with a `transfer (n)` entry is called
instead, `nic[self].transfer (n) from P to kv (m);`, and runs its body on
its own stage. Arguments are substituted like an `at admission` binding, so
none may draw.

An entry sees its own. Its header — the units, `reserve`, `reuse`, the
`at admission` bindings, the `prefix` — reads the parameters, the queue's
pools and stage, the constants, `cached` and `computed`, and what the
entry sets above the
hold, as a server's header reads an attribute set before it (`set kept =
prompt;` before it, or in both arms of an `if`; not in a loop or `fork`
that may not have run); a `prefix` key also reads `serial`, the session's
number, so that an entry keys the session's own context (`prefix (system:
S, serial: inf)`) whose role fixes its parameters. A local set only in the hold's body or after it is
refused; one also set above is read at its latest value: after a
preemption, the one the body set (this is what `kept` relies on). Its body
also reads `now`, what it
sets itself and the request's `hidden` attributes, and nothing else the
session has set, nor another queue's
`Q.x`; every index it writes (`run nic[k]`) reads as the body does. Its
statements hold, grow, shrink, load and release only the queue's own pools and the
pool its `from` names. That is the `hidden` rule
at the queue's boundary: vLLM's scheduler knows `max_tokens`, not the length,
so `o` is not a parameter of `decode` and the body alone reads it. What the
body `set`s is the queue's (`D.known`); what it `observe`s is the program's;
a moment the caller needs is `mark prefill_done;`, read afterwards as
`P.prefill_done` (`examples/pd-disaggregation/pd_batching.sq`) — the
request's attribute, so without a member index. The gateway is the exception on the session's side: its
`route` reads the request's attributes as a `server` does and sets the ones
the session reads back (`prompt`).

Queues compile to named pools and stages; entry calls expand to their
bodies, and `mark` becomes `set`. They add no IR constructs.

## Semantics {#3-semantics}

### Configuration

Time; the live sessions with their attributes,
continuation (a stack of block frames), status (ready, queued at a pool,
at a stage, waiting to grow, at a `join`, ended), active holds and leases (allocations
kept past their scope, with an expiry); for each pool its
capacity, the allocations of its holders (in admission order), its cache
(a trie of keyed segments, each node a run of units with its release time
and release order and the holds that read it, or dead),
its admission queue and its growers; for each stage its jobs.

### Events and settling

Commands take no time; they run whenever a session is ready, in the
order sessions became ready. Flow lets time pass at the stages. After
every event the interpreter *settles*: it runs every ready session, then
retries growers and admissions at every pool not served by a stage, until
nothing changes; then it starts an iteration on every idle step stage that
has residents or a waiting queue it serves, provided no other event is
pending at the same instant (a scheduler step sees every arrival up to it).

### Legs

`fork { body }` starts a *leg* of the request: the body runs
beside the session from the same instant, with a copy of the session's
attributes. The leg's holds are its own (it acts on none of
the session's, and a `fork` stands in no hold that may be preempted, which
would run again and fork twice), its `set`s change only its copy, which
ends with it (a lease it leaves caches by that copy), and its draws read a
stream of its own; its observations are the program's. It
shares the session's cached prefix, which is the request's. When the leg
ends, what it leases passes to the session, whose `release` (a
`transfer … from`) takes it. `join;` waits until every leg the session has
forked has ended, and passes at once when none runs. A leg may not
`turn`, `end`, fork or `join`; a program that forks joins somewhere, and
a session may not end while a leg runs (a run-time error). A hold that can never fit refuses the request: the
session ends, a refused leg ends its session, and the legs of an ended
session run on and give back what they lease. A run that ends with
sessions and legs waiting only for each other, one at a `join` and none for
a lease that will expire, is an error: the leg waits for memory a lease holds, and the lease's session
waits inside a hold the leg's session needs (a hold-and-wait cycle; a
finite `lease` breaks it).

### Pools

`hold m₁(cost(m₁, u₁)) reserve(cost(m₁, r₁)), m₂(cost(m₂, u₂)) … reuse(cost(m₁, m₂, ρ)) { body } cache(cost(m₁, m₂, ℓ))`
joins the queue of `m₁`. The unit expressions are evaluated *when the
session is admitted* (observables such as the cache or an engine's budget
change while a session waits), and re-evaluated at every attempt, so the units, `reserve`
and `reuse` may not draw: a draw there would be a different number each
time the scheduler looked; sample into an attribute first, as for a queue
key. The head of a queue is admitted when every pool of its hold
has room for its `reserve` units next to the allocated units (`used + r ≤ cap`,
`r = max(u, reserve)`; cached prefixes never block). `reserve` is the clause
for "do not let me in until there is room for this", which is separate from
how much the hold then takes. On a pool marked `reserve held`, what a
hold reserved and has not allocated stays reserved while it lasts: a later
admission needs `used + Σ max(0, rᵢ − allocᵢ) + r ≤ cap`, and the holder
grows into its own reservation;
cached prefixes still never block. The first that does not
fit blocks the rest (head-of-line blocking). The `cache` clause is what
makes a hold take part in the prefix cache. With it, on admission the
session reads at most `ρ` units of the cached prefix of its hold's chain
(`cached :=` what it read) and keeps them allocated while the hold lasts;
what the session's entry held past them stays in the cache as a *dead*
entry with the same age, unusable, until evicted. A hold's chain is its
`prefix (k₁: n₁, …)`: the sequence as segments, each a key and a length,
and two chains share the cached units of their equal leading keys, so a
system prompt every session sends is computed once. A key is what it
reads (an attribute, or the expression) and its value; a segment of
length 0 is left out. A key names content: it and its length read
attributes and numbers, not the state, the clock, a draw or `cached`
(the linker refuses one). Without the clause the chain is `(serial: ∞)`,
the session's own; written out, that chain does not link, and a chain
whose other segments are of length 0 in a run is the session's own there
(`prefix (system: S, serial: inf)` with `S` 0). Only the last
segment may be endless, and a hold with `prefix` has `cache` (else it
reads and keeps nothing, and does not link). An engine's admission
writes its chain as `prefix (k: n, …)` after its `taking` (*An engine's
request*). A session's *entry* is what hangs under the key of its own
serial (`serial` is the run's number for the session, and no program sets it); the rest is shared, and no
session's `reuse`, `cache`, `drop` or consumption takes it. A hold with
`prefix` reads at most what it allocates; a hold without consumes its
entry whole, and what its allocation does not hold of it is gone. A
chain is cached on pools with `block`; on a slot pool the hold's chain is
the session's own, and a hold with `prefix` and no pool with `block`
does not link. A prefix several holds read is allocated once, and an
admission does not ask again for what another hold already reads; the
session's entry its own hold reads is consumed: the session's other holds
do not see it. Without
`cache` the hold is
memory alone: it leaves the session's cached blocks where they are,
evictable as before, and `cached` is 0 in its body (the linker rejects a
body that reads it there, and a `reuse` there; `cache (0)` is the hold
that consumes the prefix and keeps nothing). `reuse (0) cache (ℓ)` skips reuse while retaining caching on release.
Other entries are evicted in the
pool's order until allocations and cache fit; `u` units are allocated and
the body runs. At the end of the body the units are released and
`min(ℓ, computed)` units stay cached along the chain, rounded down to
blocks (`computed`
is the allocation, or the position a `growing` run or a `load` reached; a
`growing` run ends at the position it started at plus its work).
`release m` inside the body does the same for `m` alone, at that point:
the innermost enclosing hold on `m` gives its allocation there back,
caching per its clause, and holds `m` no longer. A hold's `lease m (t)`
keeps its allocation on `m` past the scope's end, neither evictable nor a
preemption victim (but see `preempt leases by` below), until the session's
`release m`, `t` seconds, or the session's end, and `cache` applies then; a `release m` outside any hold on
`m` ends the lease. A session that holds and leases nothing on `m` releases
nothing (a hold re-executed after a preemption reaches the statement
again). `load m (cost(m, n))` advances the innermost
enclosing hold's position on `m` by `n` tokens, which its allocation must
cover; the KV of a transfer counts as computed from then on. `grow`,
`shrink`, `growing`, `load` and `release` stand inside a hold of the same
pool reference, index included, as the linker resolves it (`kv[k]` with
`let k = 1` is `kv[1]`; `kv[i]` is not `kv[j]`), or `grow`, `shrink` and
`release` in a session that leases it, where outside every hold of the
pool they act on the session's lease of it, its earliest if it has
several; elsewhere the program does not link. A hold's index is read at
admission, at such a statement and after a preemption, and the readings
must name one member: the hold's body does not change an attribute its
index reads (`set`, `choose`, `turn`), and an index read again inside
reads attributes and numbers, not the state or the clock. A hold that a
pool of its own or of a hold around it may preempt runs again, admitted
anew, and reads its indices again: they read attributes and numbers, and
not `cached` or `computed`, which admission and the preemption set. A hold
names each pool once: the same reference twice does not link, and two
indices that name one member at run time fail the run. `end` releases every
hold but *keeps* the session's cached prefixes:
cached entries persist until explicitly dropped or evicted. A program that models dropping
them drops them in the server on the session's last request, which the
workload says it is (`send (…, final: more == 0)` and
`if (final) { drop kv; }`, `examples/multi-turn/replica.sq`): the KV pool is
the deployment's, which the workload does not hold. A pool of the
workload's own is dropped by `drop POOL;` before `end;`. Eviction takes leaves of the cache's
trie of segments, so no cached suffix outlives its prefix, and never one a
live hold reads; it is per leaf, or per block from the tail of the leaf when
the pool has `block b`; `evict lru` orders by release time and then release
order, `evict by (k₁, …)` by the keys and then release order. With every
chain `(serial: ∞)` a leaf is a session's entry. A request that can never fit — its units, or its
`reserve` when that is larger, above the cap, as they evaluate when the
session joins the queue — is rejected: the session ends, and the report
says how many did. Each of the units and the `reserve` that reads only
attributes and numbers is judged then; one that reads the deployment's
state (a pool or stage query, the clock, `budget_left`) asks for something
else at the next try,
so it is judged at every try and waits; a queue's head that, read as
the run ends, still asks a pool for more than its cap is named (`over:`,
the pool report's `over_cap`). One whose units or `reserve` are a constant does not
link: it would be rejected whenever it is reached. A pool an engine `S` admits
(`pool reqs on S` or `pool kv on S.gpu`) is not admitted at
settle time: its queue is served by `S`'s `admit waiting`, in an
iteration, after the running requests have taken their tokens, while the
iteration has budget left, and, under vLLM's schedule, not in an iteration
that preempted (vLLM's waiting loop, `scheduler.py:868-1128`). A
top-level engine is one engine on one device, with one pool per capacity.
A family of engines is a queue family, `queue S[N] : ROLE { device gpu { … }
engine on gpu { … } … }`: each member's engine serves the member's pools
and counts its own `kv` (`examples/pd-disaggregation/llmd_nixl_pull.sq` is
the xPyD case, `docs/use-cases/pd.md` §Writing xPyD). `engine S[N]` and an
engine's family of capacities (`reqs[N] cap c;`) are refused: an engine's
`taking` names one pool, and a call names no member (#539). An engine that serves several queues tries them in
the order their pools are declared, and the first head that does not fit
stops the iteration's admissions; `examples/pd-disaggregation/llmd_nixl_pull.sq` declares the
decoder's queue of requests whose KV has arrived before its queue of new
ones, as vLLM serves `skipped_waiting` before `waiting`
(`scheduler.py:2383-2385`). `budget_left(S)` then evaluates to the budget
left. Under an `exclusive prefill` schedule, a selected prefill ends admission;
otherwise a fitting waiting prefill can displace tentative decodes, and
its header sees the full budget. Until then a waiting session's cached
prefix is evictable: the *wait channel*.

`shrink m (cost(m, d))` is the inverse: the innermost hold on `m`, or
outside every hold the session's lease of it, gives back `d` units, down
to the nearest block above what remains. A hold keeps at least its
position on `m`; under `reserve held` what it gives back is no longer
reserved. A lease keeps something: a shrink that would leave it at
nothing fails the run, since a lease of nothing would read as one that
has ended (`allocated` 0) while it lasts on, and `release m` is what ends
it. The units go back at once, and a hold or a growth waiting for them
may be let in. Nothing held or leased there fails the run, as for `grow`.

`grow m (cost(m, d))` enlarges the innermost hold on `m` by `d` (rounded to
blocks). If it does not fit: with `preempt none` the session waits and
resumes where it was; with `preempt by (k₁, …)` a candidate is
preempted: the holders that are residents of the step stage the pool is the
memory of (a holder away from the engine — a prefiller's finished request
keeping its blocks leased, a decoder's request parked for a read — is in no
`running` list, vLLM `scheduler.py:742-813`; for a pool that is no engine's
memory, its holders in a scope), and of them the one with the least keys,
read for each with its attributes, `decoding`, `position` (its hold's
computed position on the pool) and `admission`, its place in the
candidates' admission order — by the session's latest admission, the
residents' serving order, or for a pool that is no engine's memory the
order the pool admitted its holders — ties to the one admitted last; not
`computed`, the position at the last preemption. `preempt lifo`
is `preempt by (-admission)`, vLLM's `running[-1]`, and the parser writes
it so; SGLang's retraction (from the decode batch, the fewest outputs, then
the longest prompt) is
`preempt by (1 - decoding, position - prompt, -prompt) requeue tail`. The victim's job
leaves its stage, its hold is released with its computed prefix cached (its
position: the cached prefix it consumed when no `growing` run or `load`
advanced it, not its allocation), and
it re-enters its queue with the hold statement to execute again: at the
head (vLLM's `prepend_request`, `requeue head`, the default), or with
`requeue tail` as a newcomer, at the back or where the queue's keys
place it, `waited` from 0, in the queue of the hold's first pool. The grower
itself can be the victim. The re-executed hold finds `computed` set to the
position the hold had computed (0 on a first execution and after a hold
completes), so a program can resume rather than restart: vLLM's
`_preempt_request` resets `num_computed_tokens` and keeps the request's
output tokens (`scheduler.py:1560-1561`), so the request is rescheduled with
`num_tokens = prompt + outputs`, reserves and recomputes that many
(`kv_cache_manager.py:515-531`) and generates only the rest. `computed` is
where the last preemption caught the hold, which a second one during the
re-prefill can put below what the first had reached; the tokens the request
has are the most any attempt had (vLLM's `num_tokens` is the length of its
token list, which a preemption does not shorten: `request.py:295-296`), and
that is the program's to carry. A preemption undoes the hold's allocation
and position, not what its body set: `kept` set at the top of the body is
there when the hold executes again. An engine carries it itself: its
request keeps the tokens it has, `request.known`, which after a
preemption is `max(known, computed + 1)` (the token sampled at `computed`
is the request's too), and generates only the rest (*An engine's
request*), a queue's engine too, which its entries call
(`examples/pd-disaggregation/`). A kernel hold that runs an engine carries
it in the program: `set kept = prompt;` above the hold, `known =
max(kept, computed + 1)` at admission and `set kept = known;` at the top
of the body; an entry's header reads what the entry sets above the hold
(#475). A program that recomputes from the prompt alone says so by not
reading `computed`.

`preempt leases by (k₁, …)` takes leases instead of holds. A growth that
does not fit ends the lease with the least keys among the sessions that
lease the pool and hold it in no scope, read for each as `preempt by`
reads them (its attributes, `admission` its place in the pool's order,
`position` the lease's computed position), ties to the one admitted
last. The lease's units go back and its `cache` applies; the session is
not interrupted, wherever it is (at a tool, in another queue, at an
engine), and no hold runs again. The grower is a candidate too: when its
own lease has the least keys, the growth ends it and goes on with nothing
to grow. What the session does next is the program's: `allocated (m)` is
what it has allocated there, 0 once its lease is gone, and a `grow` of `m`
with nothing held or leased there fails the run. A growth of a lease past
the pool's cap fails the run too, since no preemption makes room for it.
A session with several leases of one pool (a leg's, passed to it) has
`release`, `grow` and this rule act on the earliest. The rule does not
link on an engine's memory, where a lease is KV another instance has still
to read, nor on a pool no hold leases. The pool report's `preempt` counts
the leases it ended. `examples/multi-turn/thunderagent.sq` pauses an idle
program this way: its lease ends, and its next request is let in anew
([the ThunderAgent use case](use-cases/thunderagent.md)).

`preempt leases by (k₁, …) every (dt)` takes them at the pool's ticks
instead, at every multiple of `dt` seconds of the run (a constant, and
positive). A lease's growth is let through at once, past the cap if it
must: `free` reads below 0 until the tick. A hold that is admitted still
needs room, and a hold's growth takes a lease at once, as without
`every`. At each tick, while the pool is over its cap, the lease with
the least keys ends, as at a growth. With `suspend by (s₁, …)` too, a
lease taken, at a tick or by a hold's growth, is set aside instead: its
units are the pool's again, a cached prefix it read is let go as at a
release, `allocated` reads 0 and it is no victim, and at each later
tick, before the pool takes any, the leases set aside come back in the
order of the `s` keys (read as the victim's are), each that fits, to a
session that neither holds nor leases the pool again; one that does not
fit is passed over. A `release` of the pool, or the session's end, drops one; a hold
of it anew does not. `suspend` needs `every`, and `every` does not link
on a pool with `reserve held`, whose reservations a growth let past the
cap would take. A pool over its cap caches nothing until it is back
within it. ThunderAgent's router acts at a 5 s tick: it pauses then, and lets
a paused program in again at a tick before its next request when it
fits, the smallest first (`suspend by (prompt)`).

### Waiting selection

`queue by (k₁, …)` orders a pool's waiting holds
by keys re-read before every admission attempt (after each admission under
an engine, where `budget_left` is that attempt's budget); `waited` is
the seconds since the hold joined the queue, reset on re-entry. The chosen
head that does not fit blocks the rest, a preempted hold re-enters ahead of
the keys, a key may not draw or read a `hidden` attribute, and selection
sets no timer. See [Pool](api/pool.md).

`queue … every (dt)` admits only at the pool's ticks, every multiple of
`dt` seconds (a constant, and positive). At a tick the holds that waited
when it began are tried in the queue's order: each one admitted runs
until it blocks (its body, and what follows it, up to its next wait)
before the next is tried, and one that does not fit is passed over, not
waited behind. A hold queued during a tick waits for the next, so a
session is admitted to such a pool at most once an instant, and a `while`
around a hold on it links. It does not
link on a pool an engine admits. ThunderAgent's router lets its global
waiting queue in at its tick, those with a request pending first, the
smallest first in each, each placed before the next is tried
(`examples/multi-turn/thunderagent.sq`).

### Ties

Every order in the semantics is a declared key followed by a
declared number, the sequence number of a named event (for `choose`, the
index), so that two items with equal keys never fall to the order a data
structure happens to hold them in. A pool's queue:
the keys (`queue by`), compared in order and reevaluated before every
selection, then the order the sessions joined the queue (`fifo`
is that order alone); a preempted session re-enters at the head, ahead of
the key, unless its pool says `requeue tail`. Eviction: the keys (`evict by`) or the release time (`lru`), then
the order the entries were released. An engine's running requests: the
`advance running by` keys, then admission order. The preemption victim: the
`preempt by` keys, then the candidate admitted last (for a pool that is
no engine's memory, the holder the pool admitted last). `choose`: the keys, in order, then the smallest
index. A pool's growers: the order they stalled, the head blocking the
rest. Events at one instant: the order they were scheduled; sessions run
in the order they became ready; jobs of a `ps` stage with equal finish
tags finish in the order they started. The pools' ticks at an instant
come after its other events, in the order the pools are declared (a
pool's queue tick before its own preemption tick): a waiting queue
declared before the pools its programs enter lets them in before those
give back and pause. Each pool has one queue, and a hold
on several pools waits in its first pool's; where the heads of two queues
both wait for room in one pool, or one stage serves several queues, the
pool declared first is served first.

### Stages

`fifo(c)`: `c` servers, jobs in arrival order at rate 1.
`ps(φ)`: every job at once, each at `φ(present)/present`. `delay`: every job on its
own at rate 1, which is `ps(present)` and links to it. `run a, b (cost(a, b, w))` is one job that holds `a` and `b` from its
start to its end: a *flow*, whose work goes down at one rate at all its
stages, set by the run's `share` from their capacities:
`run a, b (cost(a, b, w), share = share.maxmin)`. The policies are
[std/share](api/std/share.md)'s. `share.maxmin`
is max-min fair: every flow's rate rises together until a stage fills,
the flows through it stop there, and the others go on. `share.bottleneck`
gives each flow its equal share at the tightest of its stages,
`min over s of φ_s / n_s`, and leaves what that does not use at the other
stages unused. Every stage of such a run is `ps(φ)` with a constant `φ` above 0
(a flow is not described by the `present` a capacity could read), a run
names each stage array once (an index is known only when the run starts),
and it names its `share`, which has no default; a run on one stage names
none. A stage array held by some run with another is *shared* for the
whole run: every job on it, a single-stage `run` included, is a flow, and
its utilisation is the capacity its flows carry, `Σ rate / φ`. The stages
runs join, however indirectly, are one *fabric*, whose flows are solved
together: a fabric has one policy, and runs that join their stages and
name two do not link, while two fabrics that share no stage may name two
(`examples/pd-disaggregation/pd_two_fabrics.sq`: NVLink inside a node,
RDMA between nodes).
Every other `ps` stage serves as above.

Every stage is some request's: the stage of a `run` (or one of its
stages, in a flow), of an engine's request, or the link a pool spills
`via`. A stage nothing runs — a queue whose entries no call reaches, a
`nic` no relation or `transfer` copies over, a `stage` left from an
earlier version of the program — does not link, the error pointing at its
declaration. No request reaches it, so it is a station with no arrow in
or out, and the deployment view, which draws the stations a request
reaches, would leave it out in silence; the report's row for it would be
zeros that read like an idle station.

### Engines

An [engine](api/engine.md) runs iterations. A plain `run`'s work is time
at rate 1, the clock's unit; an engine's `prefill` and `decode` work is in
tokens, the unit of its `tokens cap`. The clock itself has no unit: a
program whose costs are seconds runs in seconds, and
`examples/oracle/vllm_request.sq` runs on the step clock with `forward
(1)`, so its times are iterations.

An iteration serves the running requests in the order its schedule's
`advance running` names: `by (k₁, …)` (ascending keys evaluated for each
request with `decoding`, 1 for a decoding request, `admission`, its
admission sequence number, `remaining`, the tokens its run has left, and
`running.count`, `running.decoding`, `running.kv_decode`,
`running.kv_prefill`; ties in admission order; a key may not draw), or
`admission` (the order their sessions were admitted, vLLM's `running`
list; the default), which is `by` with no keys, where every request ties,
or `decode first`, which is `by (decoding ? 0 : 1)`; of the orders, the IR
knows only `by`. A scheduler that serves the shortest remaining run first
is `advance running by (remaining)`, the opposite `by (-remaining)`. One
token to a decoding run, up to `each at most` to a prefilling one (with a
`granule g`, short of its remainder, a multiple of `g`, none when that is
0: such a prefill is passed over, and the iteration admits no one after
it), until `tokens cap` is spent; a `growing` run first grows its hold to
the position it will reach (block by block, preempting if needed); a
victim the iteration has already served leaves it and its tokens return to
the budget (scheduler.py:779-797) and its hold caches its position, not the
chunk the iteration gave it ([vLLM correspondence](#7-vllm-v1-as-a-serq-program)); a grower that preempts itself ends the
iteration's serving (scheduler.py:807-813); then `admit waiting` admits
from the queues the engine admits. The iteration advances the clock by
`worker`'s expression in `batch.tokens`, `batch.decoding`,
`batch.prefilled`, `batch.kv_decode`, `batch.kv_prefill`,
`batch.attention` and `running.count`; its tokens are applied when it
ends. A run of zero work completes at once. An iteration that schedules no
token is not an iteration, unless it preempted: then it is the scheduler
step that only preempted (vLLM's `schedule()` admits nothing in a step
with `preempted_reqs`, `scheduler.py:869`, and the oracle driver counts
the step; the Lean model's `startIteration` gives that step its cost, and
`step` re-admits at the next event), and the next iteration re-admits the
victim. It lasts `worker`'s time at zero tokens, which is a modelling
choice: the real engine skips the forward pass of an empty step, so the
fixed part of the time overstates it. A hold whose body can never fit then
preempts itself forever (vLLM refuses such a KV cache at start-up, above);
the `stuck` counter below reports it. A hold that reserves what it will
need (`reserve (known)` after a preemption) is rejected instead, once the
reservation is above the cap.

`exclusive prefill` selects either one prefill alone or a decode-only
batch: a running prefill takes precedence, otherwise a waiting prefill that
fits displaces the tentative decodes and takes the full budget, and once a
prefill is selected nothing more is admitted in that iteration
([Engine](api/engine.md#exclusive-prefill)). Without a per-request chunk
cap, serving in admission order *is* serving decode-first
(`SerqLang.Serve.serve_eq_decode_first`; a cap breaks it,
`chunk_cap_breaks_shape`).

`only (p)` on `advance running` and `admit waiting` says which requests
the iteration serves; the order says in what order:

- `p` is read for each request when its turn comes, from the values a key
  reads, and may not draw or read `now` or `work(…)`.
- A key and `p` read `running.count`, `running.decoding` and
  `running.kv_…` as the running requests stand at that read. This counts a
  session the iteration admitted through `admit waiting` and leaves out one
  it preempted.
- A request served earlier in the iteration is not reconsidered.
- A request that `p` reads as 0 gets no token this iteration. It keeps
  what it holds and advances no computed KV, as a displaced decode does
  under `exclusive prefill`. An admitted session that `p` excludes waits as
  such a request. How many are admitted is the pool's `cap` and the hold's
  header, not `p`.
- An engine whose requests `p` all excludes runs no iteration. It waits
  for the next event of any kind, when `p` is read again; the clock moving
  is no event.
- The order that follows (`admission` when none is written) orders the
  rest.

FasterTransformer as Dai et al. model it (decode first, no mixed batching)
reads `only (in_phase())` in both statements with `def in_phase() {
running.decoding > 0 ? decoding : !decoding }`. Its opposite, prefills
alone (as many as the budget takes) while one is running, is
`running.decoding < running.count ? !decoding : decoding`. A waiting
prefill admitted after a decode was served still joins that decode. Taking
the served decode back is `exclusive prefill`'s admission rule; `only`
cannot be combined with it.

### The iteration as a program

vLLM's `schedule()` is the schedule `advance running; admit waiting while
(running.preempted == 0);`: serve the running requests, then, unless the
iteration preempted (`scheduler.py:869`), admit the waiting with the
budget left. Any other schedule is written the same way. Its statements
run once each in order: `advance running [only (p)] [order]` gives the
running requests not yet served their tokens, skipping and leaving
unserved those `p` excludes; `admit waiting [only (p)] [while (e)]` admits
the heads of the queues the engine admits one at a time, each served at
once unless `p` excludes it, while budget is left, the head fits and `e`
is 1; and `if (e) { … } else { … }`. A guard and a `while` read the
running requests as they stand, what the iteration has scheduled so far
(`batch.tokens`, `batch.prefilled`), `waiting.admitted` and
`running.preempted` (counts). See [Engine](api/engine.md#schedule) for
scheduling examples. A schedule with a path that neither advances nor
admits, or a guard that reads `now`, does not link (an engine that
schedules nothing waits for an event, and the clock moving is none); that
is necessary, not sufficient, and an engine the linker could not see stall
is named in the report when the run ends with its work unscheduled
(`idle: engine …`). `state NAME = c;` gives the engine a register its
schedule sets (`set NAME = e;`) and the scheduler's expressions read
between iterations; a set takes effect with its iteration (a try that
schedules, preempts and admits nothing is undone), and a register is read
only by its engine, the keys of a pool it admits, the header of a hold
whose first pool it admits, a gauge or a claim
([Engine](api/engine.md#registers)).

### Engines on devices

An engine is parse-time sugar for the IR's step stage
([the design](design/engine-device.md)): it lowers to a `step` stage and
its pools, so the IR, the interpreter and the Lean model see only the
kernel, whose names the IR and the claims over iterations read. From
`examples/multi-turn/vllm.sq`:

```serq
device gpu {
  compute (t) = t * a;
  hbm (k) = omega + beta * k;
  kv cap blocks * bs;
}
engine vllm on gpu {
  reqs cap max_seqs;
  tokens cap B;
  scheduler {
    let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
    advance running each at most (threshold);
    admit waiting while (running.preempted == 0) each at most (threshold);
  }
  worker {
    forward (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
  }
}
pool kv on gpu { block bs; evict lru; preempt lifo; }
pool reqs on vllm { queue fifo; }
```

| Engine form | IR step stage (`CStep`) |
|---|---|
| `device D { f (x) = e; X cap c; }` | no field: `f` is written out where `worker` reads it; `X cap c` is the `cap` of the `pool X on D` that must declare it |
| `pool X on D { rules }` | a pool, `cap c` and the rules, no `admit_via`: a hold waiting for it is admitted as soon as it fits; `memory` when `D` is the engine's device |
| `pool X on E.D { rules }` | the same, `admit_via` `E`: engine `E`, running on `D`, admits a hold waiting for it in an iteration |
| `pool X on E { rules }` | a pool, `cap c` of `X cap c` in engine `E` and the rules, `admit_via` `E` |
| `tokens cap B;` | `budget` `B` |
| `worker { forward (T); }` | `cost` `T`, the device's time resources written out |
| `each at most (e)` | `chunk` `e`: one cap per run for the whole iteration; `inf` is no cap, which the linker writes 0 as the kernel reads it |
| `state NAME = c;` | a `Register` of the stage, `init` `c` |
| `advance running [order]; admit waiting while (running.preempted == 0);` | `serve` `By([keys])` of the order (`By([])` for none, admission order), no `iteration` |
| the same with `only (p)` on both statements | `serve` as above, `iteration` `[Serve {only: p}, Admit {only: p, gate: !preempted}]` |
| `exclusive prefill; admit waiting while (running.preempted == 0);` | `serve` `ExclusivePrefill`, no `iteration` |
| any other `scheduler { … }` | `serve` `By([])`, `iteration` the body: `advance running` a `Serve`, `admit waiting` an `Admit`, `if` a `Branch`, `set` a `Set` |
| `running.count`, `running.decoding`, `running.kv_decode`, `running.kv_prefill`, `running.preempted`, `waiting.admitted` | `Nres`, `Ndec`, `Kvb`, `Kvp`, `Preempted`, `Admitted` |
| `batch.tokens`, `batch.prefilled`, `batch.decoding`, `batch.kv_decode`, `batch.kv_prefill`, `batch.attention` | `Ntok`, `Npre`, `Ndec`, `Kvb`, `Kvp`, `Attn` |
| `waiting.count` | `Queued` of each pool the engine admits, summed |

The schedule `advance running [only (p)] [order]; admit waiting [only (p)]
while (running.preempted == 0);` is vLLM's procedure: it lowers to no
iteration body, or with `only (p)` to the two-statement body above, when
both `p` are the same expression once each `def` is written out. A predicate both statements
read is named with a `def` (`def in_phase() { running.decoding > 0 ? decoding : !decoding }`, then
`only (in_phase())` twice), which is read where it stands, for each
request; a schedule's `let` is read once as the iteration starts and only
in `each at most`, so it cannot name one. `exclusive prefill; admit
waiting while (running.preempted == 0);` is the `ExclusivePrefill` order.
Any other schedule is the step's iteration body; `exclusive prefill`
appears only in the form above, since it takes back decodes a body cannot.
`each at most` chooses among constants (its condition may read the
iteration) or `max(k, e)`, `e` read as the condition is and `k` a
constant above 0, and one at or below 0 does not link: the kernel would
read 0 as no cap, so the program would mean one thing and run another;
`k` keeps a computed cap above 0 whatever `e` reads. An engine
names each value one way in all its clauses, and none by the kernel's
bare name: `running.…` is the residents as they stand, `waiting.…` the
queues, `batch.…` the iteration's batch.

| Value | `tokens cap` | `scheduler` | `worker` |
|---|---|---|---|
| `running.count`, `waiting.count` | yes | yes | no: the worker reads the batch |
| `running.decoding`, `running.kv_decode`, `running.kv_prefill` | yes | yes | no: the batch's is `batch.…` |
| `running.preempted`, `waiting.admitted` | no | yes | no |
| `batch.tokens`, `batch.prefilled` | no | yes: the batch so far | yes |
| `batch.decoding`, `batch.kv_decode`, `batch.kv_prefill`, `batch.attention` | no | no | yes |

`scheduler` is its statements: `if`, `while` and `set`, read as the
iteration is planned. A schedule's `only` and `by`, read for each resident
at its turn, and its `each at most` and the `let`s it reads, read as the
iteration starts, read what `tokens cap` reads: `running.count`,
`running.decoding`, `running.kv_…` and `waiting.count`.

`tokens cap` bounds the batch before it is formed, so it reads no
`batch.…`. The worker times the batch the scheduler formed, which a
budget or an `only` can leave short of the residents, so its `forward`
reads the batch, `batch.…`, and nothing of the residents or the queues. `running.preempted` and `waiting.admitted` say what the
schedule did, so only `scheduler` reads them. `engine` is a keyword: an
engine is named for what it models (`vllm`, `sglang`, `tgi`) or `llm`.
Inside a `queue`, `device gpu` is the member's and `engine on gpu` is the
queue's stage, named after the queue, so its pool is `pool reqs on Q`.

### An engine's request

An engine keeps a request's state, as vLLM's scheduler keeps a `Request`
([the design](design/scheduler-worker.md)), and the server calls it:
`vllm (input, output);` is a request of `input` tokens of prompt that
generates `output`. From `examples/multi-turn/vllm.sq`, its comments elided:

```serq
engine vllm on gpu {
  reqs cap max_seqs;
  tokens cap B;
  scheduler {
    let threshold = running.count + waiting.count > 1 ? chunk_cap : inf;
    advance running each at most (threshold);
    admit waiting while (running.preempted == 0) each at most (threshold)
      taking reqs (1), kv (min(request.known, request.hit + budget_left(vllm)))
        reserve (request.known)
      reuse (floor(request.known - 1, blocksize(kv)))
      cache (request.input + request.output);
  }
  worker {
    forward (c0 + max(hbm(batch.kv_decode + batch.kv_prefill), compute(batch.tokens)));
  }
}
```

The request waits in the queue of the first pool its admission takes. When
an `admit waiting` reaches it, the engine reads its `hit`, what it may read
of the cache in the engine's memory (`min(cachedin(P), reuse)`), then the
units and `reserve` of each pool it `taking`s; the admission reads the
cached prefix, at most `reuse`, as a hold with `cache` does. The engine
computes what of the tokens it has, `known`, the cache did not supply,
chunked by the scheduler as a prefill is, and samples a token when they
are all computed; it decodes one token an iteration until it has generated
`output`; the KV pool grows with the position, as a `growing` run's. A
request with no input decodes from its first iteration. A preempted request
gives its pools back, caching what it computed, keeps its tokens and what
it generated (`scheduler.py:1561`), and waits at the head of the queue (or
the tail, `requeue tail`); its `known` is then `max(known, computed + 1)`.
When it has generated `output` it gives its pools back with `cache`, and
the session goes on. Every `admit waiting` of the engine writes the same
`taking`, which names the engine's capacities and its device's; an engine
without `taking` takes nothing and admits a request as it is called.

| Value | What | Read in |
|---|---|---|
| `request.input`, `request.output` | the call's arguments | `taking`, `reuse`, `prefix`, `cache` |
| `request.known` | the tokens the request has, its input and what it has generated | `taking`, `reuse`, `prefix`, `cache` |
| `request.hit` | `min(cachedin(P), reuse)` on the engine's memory `P` | `taking`, a connector's `load` |
| `request.preempted` | 1 once the request has been preempted | `taking`, a connector's `load` |
| `request.loaded` | what the connector's `load` read | `taking` |
| `request.finished` | 1 at the release that ends the request, 0 at a preemption | `cache` |

The clauses also read the session's attributes, the engine's pools,
`cached`, `computed` and the constants, and `taking` the engine's `state`
registers. The input is the scheduler's (a `hidden` attribute may not be
it), and so is the output when the admission reads `request.output`. The
engine's records are attributes the caller reads after the call: `E.reused`
(the cached tokens its latest admission read), `E.prefill` (the tokens its
prefill computes), `E.admitted` (when) and `E.first` (when its first token
was sampled); a connector adds `E.loaded`, `E.saved` and `E.load_done`
(below). `E.reused`, `E.prefill` and `E.loaded` are observations of the
report too, one per admission.

The server, a gateway's `route` and the session may not run an engine's
prefill or decode, nor hold a pool an engine admits (`pool P on E`, `pool
kv on E.gpu`, an engine's capacity); each is refused with the `taking` and
the call to write instead (#539). A pool no engine admits (a top-level
pool, one `on DEVICE`) is held anywhere, around the call too, as
`examples/single-turn/fastertransformer.sq` holds its `reqs`. A queue's
entries run their own engine and no other. A queue's engine admits by
its `taking` as any engine does, and the queue's entries call it, `E
(input, output);` (`examples/multi-turn/thunderagent.sq`); the server and
the gateway call the entry (`E[j].decode (prompt);`), not the engine. A
family's `taking` names its pools without an index, and each member takes
its own. An entry may still run its engine in the kernel's words (`run E
prefill (…)`, the prefillers of `examples/pd-disaggregation/`).

An engine's `connector C { per_token (r); load (n); save (n); }` copies a
request's KV through the L2 queue `C` (vLLM's KV connector; *Queues*). At
the admission, after `hit`, it reads `load`, which `taking` reads as
`request.loaded`; a request that loads copies `n` tokens at `C`'s
bandwidth, `r` seconds a token, into its hold on the engine's memory, which
counts them as computed, before its prefill. When it has generated its
output it reads `save` (as `cache` is read) and, before it gives its pools
back, copies to its entry in `C` the part of `n` tokens the entry lacks
(LMCache's `request_finished` keeps the blocks until the save is done,
`lmcache_mp_connector.py:952`). `per_token` may not read `cached` or
`computed`, which during a copy are the L2 entry's.

A queue's engine's `connector P { load (n); }` reads instead from the
source of its queue's KV relation (`kvt.nixl_pull (P, D, …)`,
`kvt.nixl_push`; *Queues*): for a request its entry was called `from P[i]`
with, the `n` tokens' KV crosses `P`'s NIC and the decoder's, as the
relation says (in a push, once the dispatch's leg has joined), and the
lease the entry took ends; a request its entry took from no source loads
nothing. Such a connector has no `per_token` or `save`.

After any connector's load, a request that has KV for every token it has
computes its last again, to sample the next (`scheduler.py:3075-3076`),
and its admission is read again: a pool it took none of and takes now, it
waits for, as vLLM's request whose remote KV has arrived goes back to the
waiting queue (`scheduler.py:1264-1267`). So a decoder that reads its KV
holds blocks and no running slot while it reads:

```serq
          taking kv (request.loaded > 0 ? request.known : min(request.known, request.hit + budget_left(D)))
                   reserve (request.known),
                 reqs (request.loaded > 0 ? 0 : 1) reserve (1)
```

(`examples/pd-disaggregation/llmd_nixl_pull.sq`). The engine records
`E.loaded` for each admission and sets `E.loaded`, `E.saved` and
`E.load_done` (when its latest load ended) for the caller.

### `at admission`

Everything in a hold's header — the units, `reserve`,
`reuse` — is evaluated when the session is admitted, and a `set` above the
hold is not (`cache` is read when the session releases, `Serq/Exec.lean`'s
`release` and the interpreter agree). `at admission (x = e)` gives the
header a place to name what it is written in terms of, as the prefiller of
`examples/pd-disaggregation/vllm_nixl_push.sq` does:

```
hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, reused + budget_left(P)))) reserve (cost(kv, prompt))
     at admission (reused = min(cachedin(kv), floor(prompt - 1, bs))) { … }
```

The bindings are substituted into the header's expressions by the parser, so
the interpreter and the Lean model know nothing of them, and a program that
uses the clause has the IR of the one that inlines by hand. A
later binding sees the earlier ones. A binding may not draw (`~`): it is
substituted, so a name used twice would draw twice.

The body sees a binding too. One the body reads is set at its top, `set
x = e;`, which is its admission value because the body starts at the
admission's instant and `e` reads only attributes and constants; that `set`
is in the AST and the IR like any other. A binding that reads live state —
an observable such as `cachedin(kv)`, a context variable, or `cached`, which
the admission itself sets — has another value there, so the body reading it
is a parse error; the body reads `cached`, the units the admission consumed.
A binding's name is its own: no declaration anywhere in the program (a
`def`, an observation, a gauge, a pool, a stage, …) may share it, nor a
builtin attribute, a context variable, a `let` or an attribute the program
sets, and it is not read outside the holds that bind it. A binding reads only the ones
before it in its clause.

### `hidden`

In a `server`, hidden attributes may control run work, cache release and
observations. Decisions and allocations may read them only after a run
whose work reads them has ended. This also applies to attributes derived
from hidden ones; both branch arms must reveal an attribute before it is
known after the branch. See [the full rule](api/workload.md#hidden).

A `hidden` parameter, `server (input, hidden output)`, prevents scheduler
expressions, such as a hold's header, queue keys or a budget, from reading
`output`. Session statements,
run work, a hold's `cache` and a claim's `given` may read it. Attributes
set by the scheduler (`cached`, `computed`) cannot be hidden.
See [Workload](api/workload.md#hidden) for the permitted positions.

### Branching

`if (e)` takes the first block when `e` is 1 and the
second when it is 0; any other value (a fraction, a count, a negative
number, NaN) is a run-time error, since a guard is a test and a test has
two answers. A draw is written `if (~bernoulli(p))`: it takes the first
block with probability `p`, and the sample is a 0 or a 1 by the time the
guard sees it. `else if (f) { … }` is `else { if (f) { … } }`, so `f` is
read only when `e` is 0. A constant guard that is not 0 or
1 is refused at link time; a computed one is refused when it is evaluated.

### Amounts and indices

The work of a `run`, the units of a `hold`, a
`grow`, a `shrink` and a `load` are amounts, and an amount is not negative and not NaN;
an index names a member, a whole number from 0 below the array's count.
Anything else is the program's error, not a value to round: a constant
amount or index that is one is refused at link time, a computed one when it
is evaluated. Zero is an amount (a run of no work). The Lean model's
fragment computes over ℕ, where a subtraction stops at 0, so it agrees with
the interpreter on programs whose amounts are never negative.

### Workload

`init` runs at arrival. Each source `turn;` draws its attributes and waits
for the server; the `turn` block runs just before that submission.
With no session block, this happens once per arrival;
with a `trace`, it first loads the next turn's `new`, `out`, `think`,
`forced` and sets `more` (`ordered`: session `i` replays trace session `i`
modulo the trace's sessions).
Random draws read separate streams: the arrival law reads the run's
arrival stream and an unordered trace its trace stream; a session's `init`
and `turn` blocks read a stream of their own, seeded from (seed, session,
turn), and the session's statements another, seeded from (seed, session);
a draw the machine makes — an eviction key, a spill predicate, a `cost`, a
`budget`, a `ps` capacity — reads the interpreter's stream for it. So a
(session, turn) draws the same marks under every deployment run on one
seed, provided the draw expressions and their inputs are unchanged.
Changing a session's control flow or a distribution's parameters can still
change its workload; a shared seed does not make those changes equivalent.
Distinct seeds give unrelated streams for every session, so runs on seeds
1..N are independent replications.

### Every instant settles

A `loop` or `while` must let time pass on every pass
through its body: every path through it reaches a `run` (a constant zero
work does not count), a `hold` whose body does, or `end`; the linker
refuses a loop that does not (`loop { set w = w + 1; }`, or a `run` on one
arm of an `if` only). What the linker cannot see — a `run` whose
computed work is 0 on every pass, a `hold` that fits, grows past its pool
and preempts itself — the run time catches: a session that executes a
million statements without blocking, or that becomes ready a thousand
times at one instant, ends the run with an error naming it. Likewise an
iteration that schedules tokens lasts a positive time: a `cost` that
evaluates to 0 on it is an error (the step that only preempted may cost 0,
above).

### Lints

Linking rejects three programs that are well formed and almost
certainly not what their author meant: a `set` that reads live pool or
stage state (`cachedin`, `budget_left`, `used`, …) and is then used in a
hold's header, where the value is the one from before the session queued
(`at admission` is the clause for it); a hold without `cache` that reads
`cached` or writes `reuse`; and a constant `if` guard other than 0 or 1
(Branching, above). These are errors, not warnings.

### Statistics

`observe x = e` records a sample after warm-up with its
time, session and turn. `--dump DIR` exports these samples. The
[CLI reference](reference/cli.md) defines the reported statistics and
resource counters.

`stuck` counts sessions preempted again without advancing past their previous
preemption. Such sessions do not contribute completed response samples.
`growing_at_end` counts the holds still waiting to grow when the run ends
(`grow:` in the text report): under `preempt none`, or with nothing the
pool's `preempt` may take, a growth waits for room another holder frees.
When every session holding the pool is one of those waiting
(`growing_stalled`), none will: a hold around one on the same pool keeps
what it needs, or the holders wait on each other (#238).
A boolean observation that remains zero for at least 40 samples produces
a report note, not an error.

### What the client sees

Every turn is one request from a virtual client, and the report gives what
that client saw of it in its `client` section, whatever the program
observes ([the design](design/client-metrics.md)):

| Metric | Sample | Recorded |
|---|---|---|
| `ttft` | the turn's first output token less its start | when the first token is known |
| `tpot` | (the turn's answer less its first token) / the output tokens after the first | when the turn is answered |
| `response` | the turn's answer less its start | when the turn is answered |

A turn starts at its `turn;`, and is answered when the server's statements
for it are done. Its first output token is the one sampled by the prefill
that ended last before its first decode token, or that decode token when
no prefill ended before it; with no decode token, its last prefill's (a
request of one output token, which has a `ttft` and no `tpot`). Its output
tokens after the first are the decode tokens its engines commit and the
token a resumed prefill samples after a preemption. A turn that commits no
token, at stations with no engine, has a `response` only. These follow
vLLM's benchmark client and engine statistics (`vllm/benchmarks/serve.py`,
`vllm/v1/metrics/stats.py`), with the turn's start for vLLM's arrival. A
server that refuses a request ends the session there with `end;`, as
vLLM's input processor refuses a prompt it cannot hold: that turn is not
answered and the client records nothing of it, as vLLM's client counts
only the requests that succeed. A sample counts when it is recorded after the
warm-up, so a turn at the warm-up's start or at the horizon can have a
`ttft` and no `response`, or the opposite. A program's `observe` keeps its
own name beside them, in its own table.

### Gauges

`gauge x = e;` declares a function of the deployment's state
and the report gives its time average over `[warmup, end]`, with a
batch-means 95% CI over 20 windows, and the least and greatest value held for
a positive time. An `observe` is a sample a session takes when it gets
there; a gauge is a signal in time, read at the end of every instant (the
state the instant's last event leaves, which is the state until the next
one; what the instant passes through on the way is not read), so
`gauge u = used(kv);` is the pool's time-average `used`. The expression has
no session and is held constant between events: it reads pool and stage
observables and constants, and an attribute, a draw, `cachedin` (the
session's own prefix), `now` or `work(…)` (both move between events), or
`budget_left(…)` (it plans an iteration, which may draw) is a link error. A
pool or stage it names has a number for its index (`kv[0]`, or the `kv[k]`
an aggregate writes out), so reading a gauge cannot fail the run. What to call an imbalance is the program's: the time fraction some
decoder is full is `gauge full = max j in N (free(reqs[j]) == 0);`, the
spread `max j in N (used(kv[j])) - min j in N (used(kv[j]))`. `--dump DIR`
writes each gauge's change points as `gauge/NAME.csv` (`time,value`).

### Claims

A claim states a property of the program's paths. It reads and does not act: a program runs the same
with its claims removed. The interpreter checks each claim on the path it
runs and the report says what it found; the same IR is the source of the
claim's statement in Lean. There are three forms:

```
claim work_conserving: every iteration of llm (demand < bmax || tokens == bmax);
claim starved: some iteration of llm (demand >= bmax && tokens < bmax);
claim mean_ok: at end (total(response) <= 1000000 * count(response));
```

`every iteration of S (e)` says `e` is non-zero at every iteration of the
step stage `S` (a member of an array is named by a constant index,
`E[0]`); `some iteration of S (e)` says it is at one of them at least; `at
end (e)` says it is when the run ends. A claim over iterations is read when
an iteration starts, after its batch is scheduled, where the cost is read:
it reads the cost's variables (`tokens`, `prefilled`, `decoders`,
`residents`, `kv_decode`, `kv_prefill`, `attention`), `now` (the start),
the observables `queue`, `busy`, `used`, `free`, `holders`, `queued` (a
pool or stage named by a number, as in a gauge) and three variables of its
own. `demand` is the tokens the stage's residents could take in this
iteration if the budget were unlimited: one for a decode with work left,
the remaining prompt (up to the `chunk`) for a prefill, summed over the
residents after the batch is scheduled, the ones it admitted and the ones
an `only` leaves out included. `served` is the tokens the stage
scheduled in its earlier iterations, from the start of the run. `arrived`
is the sessions the workload has started by the iteration's start, one
arriving at that instant included, so `arrived * W - served` is the work
that has arrived and not been scheduled when every session brings `W`
tokens. A claim
`at end` reads the constants, `now` (the end) and the aggregates of the
run's observations, every value observed under `o` from time 0, warm-up
included: `total(o)`, `count(o)`, `largest(o)` and `smallest(o)` (0 when
there is none), and `prefix_total(o)`, `Σ_k (v_1 + … + v_k)` over the
values sorted ascending, the least total completion time of jobs of those
sizes served one at a time. Neither form reads a session attribute, draws,
or reads `work(…)`, `budget_left(…)` or `cachedin(…)`; each is a link
error. Warm-up does not apply: a claim is about the whole path.

`given (e)` restricts the claim to the paths whose every session
satisfies `e`. It is read for each session once its `init` block has run,
from the session's attributes and the constants (no `now`, no draw, no
observable). A session that fails it puts the claim out of the run's
scope, and the claim is checked no further.

A claim over iterations holds, or fails at the start of its first failing
iteration; a `some` claim is witnessed at the first iteration that
satisfies it, or not witnessed. A claim `at end` holds or fails, and is
not evaluated when sessions are still live at the end, since the run did
not reach the end the claim is about (`--arrivals N` drains them).
A claim a session put out of scope says which session; the JSON fields are
in the [CLI reference](reference/cli.md). A path that holds a claim is
evidence, not a proof: the proof is the Lean statement's.

For a program inside the Lean fragment, `scripts/gen_lean_claims.py`
writes each claim as a statement about the executable semantics below
(`lean/Serq/Claims.lean`): for every workload of the program's family
(any number of sessions up to 500, a drawn attribute any natural number,
the arrival times any under `poisson`, the program's under a constant
`renewal`, 0 under `batch`, the sessions that satisfy `given`), every machine of every path
of the program satisfies the claim (`every iteration`), some machine of
some path does (`some iteration`, whose workload must not draw), or every
machine at which every session has ended does (`at end`). The proofs are
Lean files, and the build fails when a claim has none or the program has
changed what it claims (`examples/papers/ClaimsProved.lean`). The programs under
`examples/papers/` are written this way: three papers' propositions, each
stated in the program that is the paper's serving system and proved about
that program's paths ([use cases](use-cases/index.md), `docs/lean.md`).

## Programs {#5-programs}

Each program under `examples/` is a deployment; it runs with the workload
file `examples/pairs.json` gives it, from `examples/workloads/`
(`serq run examples/multi-turn/vllm.sq --workload examples/workloads/chat.sq`).

| Program | Deployment | Checked by |
|---|---|---|
| `single-turn/mg1.sq`, `single-turn/ps.sq`, `multi-turn/closed.sq` | M/G/1 FIFO, M/G/1-PS, M/M/1//N | a run against the closed form, by hand, in [getting started](getting-started.md) and the [tutorial](tutorial/01-a-queue.md); no test |
| `multi-turn/replica.sq` | the paper's two-resource replica on the open-session scenario (`advance running decode first`; the server drops a session's KV after the request its workload sends as `final`, before a last tool call, where libqueuingsim drops it after: fewer evictions, a higher hit rate) | — |
| `multi-turn/routing.sq` | four replicas, five routing policies | — |
| `multi-turn/thunderagent.sq` | ThunderAgent's router in TR mode in front of two vLLM replicas, a fixed number of agents at once ([use case](use-cases/thunderagent.md)) | the source (ThunderAgent at 7ddc861), read; `tests/thunderagent.rs` against the router's default mode; no oracle |
| `multi-turn/vllm.sq` | vLLM v1's engine, under a multi-turn agent workload and, from `examples/pairs.json`, a person's chat, single turns and an approximated subagent workload ([use case](use-cases/workloads.md)) | `tests/paired.rs` |
| `multi-turn/vllm_lmcache.sq` | `vllm.sq` with an LMCache server as its L2 (`queue lmc : l2`, the engine's `connector lmc { … }`) | the source (the LMCache connector at vLLM v0.31.0, the scheduler at 0c87a197); `tests/connector.rs`; no scheduler oracle |
| `multi-turn/vllm_system_prompt.sq` | `vllm.sq` serving the sessions of `M` products, each with its own system prompt, shared across sessions (its admission's `prefix (product: floor(S, bs), serial: inf)`) | the source (vLLM at 0c87a197); `tests/shared_prefix.rs`; no scheduler oracle |
| `oracle/vllm_shared_prompt.sq` | `vllm_replay.sq` with a system prompt every session shares (its admission's `prefix (system: floor(S, bs), serial: inf)`), on the scenario `tools/oracle/shared_trace` | the upstream oracle, `tests/vllm_cache.rs`; no Lean theorem |
| `oracle/vllm_request.sq` | one vLLM v1 request on the step clock, compiled per scenario to `tools/oracle/{alone,chunked,hol,longchunk,mixed,preempt,seqcap}.ir.json` | the upstream oracle ([vLLM correspondence](#7-vllm-v1-as-a-serq-program)), `tests/vllm_oracle.rs`, the Lean theorems generated from the same IR |
| `replay/vllm_replay.sq` | vLLM v1 on the A100 testbed replaying the short-context trace ([A100 testbed](#8-vllm-on-the-a100-testbed)) | the prefix-cache oracle `tools/oracle/cache_trace` (`tests/vllm_cache.rs`, theorem `vllm_cache_trace`) |
| `pd-disaggregation/llmd_nixl_pull.sq` | llm-d's prefill/decode disaggregation with the NIXL connector, two prefill and two decode instances ([use case](use-cases/pd.md)) | the source (llm-d at 8a2f37d, the router at 13eebdb, vLLM at 0c87a197), `tests/pd_semantics.rs`; no scheduler oracle |
| `pd-disaggregation/vllm_nixl_push.sq` | vLLM's push proxy in front of one prefill and one decode instance with the NIXL connector's push mode, on the A6000 testbed, replaying the short-context trace ([the push mode](https://github.com/servingQ/serQ/blob/main/docs/design/push-mode.md)) | the source (vLLM at 0c87a197), `tests/fork_join.rs`; no scheduler oracle |
| `pd-disaggregation/pd_two_fabrics.sq` | prefill/decode on two nodes, the KV read over NVLink inside a node and over RDMA between them, each fabric under its own `share` | `make check` links and draws it; neither policy is measured ([bandwidth sharing](https://github.com/servingQ/serQ/blob/main/docs/design/bandwidth-sharing.md)) |
| `papers/*.sq` | three scheduling papers' serving systems | their claims, proved in Lean ([use cases](use-cases/index.md)) |
| `engines/sglang.sq`, `engines/tensorrt_llm.sq`, `engines/tgi.sq` | SGLang, TensorRT-LLM and TGI with their defaults, as close as serQ writes them today ([engine neutrality](https://github.com/servingQ/serQ/blob/main/docs/design/engine-neutrality.md)) | the source, read; `make check` links and draws them; no oracle |
| `single-turn/fastertransformer.sq`, `single-turn/separate_phases.sq`, `vendors/*.sq`, the other `pd-disaggregation/*.sq` | the [use cases](use-cases/index.md) that describe them | `make check` links and draws them |

## vLLM v1 as a serQ program {#7-vllm-v1-as-a-serq-program}

The table is `examples/replay/vllm_replay.sq` (and
`examples/oracle/vllm_request.sq`, its one-request form) against vLLM's
scheduler at the pinned revision (`ref/vllm` at 0c87a197; the A100 testbed
runs vLLM 0.30.0, whose scheduler gives the same answers on the first six
scenarios below). It is a correspondence under synchronous scheduling at that
revision, not to the latest vLLM ([the vLLM use case](use-cases/vllm.md)).
The admission of `examples/multi-turn/vllm.sq`'s engine, the engine of the
workload examples, is a simpler one: it reserves `request.known` as this one does, but takes its hit
from the cache alone rather than from the previous prompt, and caches `request.input + request.output`.

| vLLM | serQ | Where |
|---|---|---|
| a token budget per step, running requests first in `running` order, then waiting requests with the budget left | `engine vllm on gpu { tokens cap B; scheduler { advance running; admit waiting while (running.preempted == 0); } … }`, residents in admission order; `pool reqs on vllm` | `scheduler.py:577, 624-823, 868-1128` |
| `max_num_seqs` | `reqs cap max_seqs;` in `engine vllm`, held as `pool reqs on vllm` | `scheduler.py:877-879` |
| FCFS, head-of-line blocking (`if new_blocks is None: break`) | pool queue `fifo`; the first request that does not fit blocks | `scheduler.py:1228-1235` |
| admission needs blocks for the whole prompt (`scheduler_reserve_full_isl = True`), but only the first chunk is allocated | `kv (request.hit + min(request.known − request.hit, budget_left(vllm))) reserve (request.known)` | `kv_cache_manager.py:515-531`, `config/scheduler.py:191` |
| a waiting request's prefix is looked up and its blocks touched only when it is scheduled | units evaluated at admission; the queue served by the engine | `scheduler.py:932-939`, `block_pool.py:754-770` |
| chunked prefill, `long_prefill_token_threshold` | the engine's prefill, `each at most (threshold)` with `let threshold = running.count + waiting.count > 1 ? c : inf;`: the cap only while more than one request is running or waiting; `waiting.count` counts the queues the engine admits, as vLLM counts `waiting`, whose `skipped_waiting` these programs leave empty; a constant cap is not vLLM's, and `serq target` refuses it | `scheduler.py:606-616, 675-676, 1115-1128` |
| `allocate_slots` block by block as the request advances | the memory pool grows with the request's position | `kv_cache_manager.py:371-608` |
| preemption of `running[-1]`, `waiting.prepend_request`, `num_computed_tokens = 0`, no admission in a step that preempted | `preempt lifo`, re-queued at the head, admitted again; `admit waiting while (running.preempted == 0)` | `scheduler.py:742-813, 869, 1539-1582` |
| a preempted request keeps its output tokens: it is rescheduled with `num_tokens = prompt + outputs`, reserves and recomputes that many, and generates the rest | the engine keeps the request's tokens: `request.known` is `max(known, computed + 1)` after a preemption, it computes what the cache does not hold of them and generates the rest | `scheduler.py:1560-1561`, `request.py:295-296`, `kv_cache_manager.py:515-531` |
| the scheduler reserves by `num_tokens` (prompt and generated so far), never by the final length: it knows `max_tokens` and learns the length when `check_stop` sees EOS or the cap | `server (input, hidden output)`: no header, key or budget reads `output` | `kv_cache_manager.py:517, 533-534`, `config/scheduler.py:191`, `scheduler.py:639, 2426`, `sched/utils.py:98-119` |
| the prefix cache holds every *computed* full block, generated tokens included; a hit is the longest run of cached full blocks, at most `num_tokens − 1` | `cache (request.input + request.output − 1)`; `reuse (floor(min(prev prompt, prompt − 1), bs))`; the unmatched blocks stay cached, dead | `kv_cache_manager.py:289-300, 602-606`, `single_type_kv_cache_manager.py:743-838` |
| the free queue: freed blocks appended tail first (LRU), in the order requests finish | `evict lru` per block from the tail, ties by release order | `block_pool.py:776-805`, `single_type_kv_cache_manager.py:557-585` |
| a finished session's blocks stay in the free queue | `end` keeps the cache | `block_pool.py:776-805` |
| a forced miss (a nonce at the head of the prompt) matches nothing; the old blocks stay | `reuse (0)` | trace |

Not modelled, on purpose: a victim the PRIORITY path takes out of a step that
had scheduled it leaves the full blocks of that step's chunk hashed in the
prefix cache (`allocate_slots` cached them, kv_cache_manager.py:602-606, and
the freed blocks return with their hashes, block_pool.py:776-805), though
the step never computes their KV; serQ caches what was computed, the
position ([Semantics](#3-semantics)).

The adaptive long-prefill threshold is off by default; it is expressible as
`n > 1 ? max(threshold, floor(budget / n)) : inf`, with `n` =
`running.count + waiting.count` and `budget` the engine's `tokens cap`
(`scheduler.py:609-622`, [Engine](api/engine.md)), but no program here
writes it. `serq target` exports that cap as `long_prefill_token_threshold:
threshold` with `long_prefill_token_threshold_adaptive: true`
(`config/scheduler.py:87-91`), and refuses a cap that computes another
share.

Not modelled: the watermark (0 by default), encoder inputs, speculative decoding, sliding
window, the PRIORITY policy (its victim, the largest `(priority,
arrival_time)`, `scheduler.py:761-765`, put back into a heap ordered by
the same, `request_queue.py:159-164`, is `preempt by (-priority, -t0)
requeue tail` beside `queue by (priority, t0)`, but no program here writes
the policy and no oracle scenario checks it), the deferred free of in-flight
blocks, asynchronous scheduling ([A100 testbed](#8-vllm-on-the-a100-testbed)), and cross-session prefix sharing
in these programs (their chains are per session; a hold's `prefix` shares
segments by key, [limitations](#9-known-limitations)).

### Admission

The admission of the request in `examples/multi-turn/vllm.sq`'s engine is the
prefix-cache lookup and the allocation of the first chunk, and both happen
when the scheduler admits the request, not when it queues. `known` is
every token the request has: the prompt, or after a preemption the tokens
it had computed plus the one sampled there, never fewer than an earlier
attempt had (`known = max(kept, computed + 1)`, vLLM's
`request.num_tokens`, `request.py:295-296`). `reused`, the prefix hit, is
the cached *full blocks* of those, never all of them, since the last token is
recomputed for its logits: `floor((known − 1) / bs) · bs`
(`kv_cache_manager.py:289-300`). `cachedin(kv)` is read in the header, so
it is read in the waiting loop (`scheduler.py:932-939`), and until then a
waiting request's prefix is still evictable: the wait channel ([Semantics](#3-semantics)). The
units are `reused` plus the chunk the budget the running requests
leave can take now, `min(known, reused + budget_left(vllm))`
(`scheduler.py:1078-1128, 1214-1226`): every known token, or as far as the
reused tokens and the budget reach, whichever is less. The rest is allocated as the
request runs (`growing kv`). The admission still needs room for every known
token, `reserve (cost(kv, known))`: vLLM
checks the whole sequence before it allocates the first chunk
(`scheduler_reserve_full_isl`, on by default, `config/scheduler.py:191`,
`kv_cache_manager.py:515-531`, passed at `scheduler.py:1223`).

### How the correspondence is checked



1. *Deterministic scenarios* (`tools/oracle/*.json`): the real scheduler
   driven by a fake model runner (`tools/vllm_oracle.py`), the real A100
   engine with Qwen3-8B stepped by hand (`tools/oracle/a100_engine.json`,
   `scripts/check_oracle_gpu.py`), the serQ program (`tests/vllm_oracle.rs`)
   and the Lean executable semantics (`Serq/Oracle.lean`, one theorem per
   scenario, `decide +kernel`) give the same first-token and last-token
   step for every request, and the same number of preemptions (7 scenarios:
   self-preemption, chunked prefill sharing the budget, the request cap,
   head-of-line blocking, the chunk cap, six mixed requests with staggered
   arrivals on 39 blocks, and the chunk cap lifted for a request alone; the
   A100 engine ran the first six). None of them uses the prefix cache.
2. *The prefix cache* (`tools/oracle/cache_trace.*`): three sessions of the
   short-context trace with prefix hits, replayed by the real scheduler and
   KV-cache manager (`tools/vllm_replay_oracle.py`), by
   `vllm_replay.sq` (`tests/vllm_cache.rs`) and in Lean (theorem
   `vllm_cache_trace`).
3. *The trace at full scale*: the same replay of all 333 sessions gave the
   same send time, first-token time and cached tokens for 3 321 of 3 321
   requests. That run was made in `serving-queue-theory`; this repository
   checks the excerpt of 2, and `scripts/lean_bench.py` checks that Lean
   and the interpreter agree on the full trace.
4. *A shared system prompt* (`tools/oracle/shared_trace.*`, written by
   `tools/oracle/shared_trace.py`): three sessions of three turns whose
   prompts all begin with one system prompt of 64 tokens, replayed by the
   real scheduler and by `oracle/vllm_shared_prompt.sq` (`tests/vllm_cache.rs`)
   on 30 blocks: the same first-token and last-token step and cached tokens
   for all nine requests, the later sessions' first turns reading the
   prompt another session computed, and eviction taking the sessions' own
   tails before it. The sessions start after the earlier ones' first turns
   end ([limitations](#9-known-limitations)). No Lean theorem: a chain is
   outside the model's fragment.

These comparisons check individual requests and steps, rather than only
aggregate statistics.

## vLLM on the A100 testbed {#8-vllm-on-the-a100-testbed}

The [measured replay example](use-cases/vllm.md#measured-replay) documents
the A100 cost fit, calibration, comparison runs and their limitations.

## Known limitations {#9-known-limitations}

* Continuous work and fluid rates at `fifo`/`ps`/`delay` stages; an
  engine is discrete. A fluid server is an engine whose `tokens cap` is
  filled to the memory time; at ω = 0.2 ms that is 5 000 iterations
  per simulated second, so long horizons can be slow.
* Cross-session prefix sharing is what a hold's `prefix` keys say, not
  content: a program names who shares what (a system prompt's key), and
  segments with one key are one content. A prefix is cached when its hold
  releases it, not while it is being computed: vLLM caches a request's
  blocks as it schedules them (`kv_cache_manager.py:602-606`), so a request
  admitted after another has scheduled the same system prompt reads it,
  and serQ's computes it again, and holds its own copy while it does. A
  segment is shared in whole blocks only where the program says so, as
  `floor(n, bs)`: vLLM's block that mixes the system prompt with a
  session's text is the session's. A chain cannot be spilled to a tier yet.
  The Lean model's cache is per session: a program with `prefix` is outside
  its fragment.
* A leg's lease does not expire when its session is slow to ask for it:
  vLLM's prefiller frees a pushed request's blocks after its lease
  duration when no decoder has registered, and the request fails; the
  program's lease is one number, and a copy after it reads nothing
  ([the push mode](https://github.com/servingQ/serQ/blob/main/docs/design/push-mode.md)).
* A lease's bound is one number; the heartbeat that renews it is not a
  construct ([the KV transfer](https://github.com/servingQ/serQ/blob/main/docs/design/pd-transfer.md)).
* One eviction order per pool; a priced order uses `price(stage, …)` with
  the stage's online estimates.
* The Lean model covers a fragment, and there is no proof that the
  interpreter implements it ([the Lean model](lean.md#what-is-not-covered-yet)).
