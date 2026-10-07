# serQ: the text syntax, the semantics, and the vLLM correspondence

This reference defines the text syntax and its semantics. Text compiles to
[the IR](ir.md), which the interpreter runs and the Lean generators read.
For individual constructs and examples, use the [API reference](api/index.md).

## Overview {#1-what-serq-is-for}

A serving deployment is a program. The program names the resources of the
deployment (memory pools, stages), says how sessions arrive and how a
session's turns evolve (the workload), and gives the path every session
runs. One program is simulated (`serq run`), stated about and proved
about in Lean (a fragment of the IR, `docs/lean.md`), and drawn
(`serq draw`).

The organising idea: a deployment does four things to a request. It makes
it **wait for a resource**, **runs** it on a stage, **frees** the resource
(possibly keeping a prefix cached), and **sends it somewhere next**. The
first three are one scoped statement (`hold`); a resource is a pool,
whether it is KV memory or request slots; and the colocated engine that
prefills in the compute its decode step leaves is a stage kind of its own
(`step`).

## Syntax {#2-syntax}

```
program  := (constant | definition | import)* fn main ( ) { item* }
constant := let NAME = expr ;
definition := def NAME ( NAME , ... ) ( { expr } | block )
import   := use "file.sq" ; | use "std/args" ;
item     := let NAME = expr ;
          | let NAME = args.number ( "argument_name" , expr ) ;
          | use "file.sq" ;                  -- the definitions of a library, next to this file
          | def NAME ( NAME , ... ) { expr }   -- a name for an expression: NAME ( arg , ... )
          | def NAME ( NAME , ... ) block      -- a name for statements: NAME ( arg , ... ) ;
          | pool NAME [ '[' N ']' ] { poolopt* }
          | stage NAME [ '[' N ']' ] : kind ;
          | workload { wlitem* }
          | server block                      -- request handling; the session lives inside workload
          | queue NAME [ '[' expr ']' ] [ : ROLE [, ROLE]* ] { qitem* }   -- a station: its pools, stage and entries (below, *Queues*)
          | share maxmin ; | share bottleneck ;   -- how a run over several stages divides them
          | QUEUE pull QUEUE [ latency expr ] share ( maxmin | bottleneck ) ;   -- the reader, its source, the read (below, *Queues*)
          | QUEUE push QUEUE [ latency expr ] share ( maxmin | bottleneck ) ;   -- the source, its reader, the write
          | gauge NAME = expr ;              -- the time average of a function of the state (Gauges)
          | claim NAME [given ( expr )] : every iteration of STAGE ( expr ) ;   -- a proposition about every path (Claims)
          | claim NAME [given ( expr )] : some iteration of STAGE ( expr ) ;
          | claim NAME [given ( expr )] : at end ( expr ) ;
qitem    := pool NAME { poolopt* }              -- the queue's own; only its entries hold it
          | serve kind [ latency expr ] ;       -- the queue's stage, named after the queue; `latency` a link's
          | nic kind ;                          -- the queue's NIC, the stage `QUEUE.nic`
          | VERB [ ( NAME, ... ) ] [ from NAME ] block   -- an entry of one of the queue's roles
poolopt  := cap expr ;                       -- capacity in units (default inf)
          | block expr ;                     -- allocate and cache in blocks
          | evict lru ; | evict by ( expr , ... ) ;   -- eviction order (ascending keys)
          | preempt none ; | preempt lifo ;  -- what a failed growth does
          | preempt by ( expr , ... ) [requeue head | requeue tail] ;   -- the victim: least keys
          | queue fifo ; | queue by ( expr (, expr)* ) ; -- waiting selection
          | admit via STAGE ;                -- the queue is served by a step stage's scheduler
          | reserve held ;                   -- a hold's unallocated reservation counts against later admissions
          | spill POOL via STAGE ( expr ) when ( expr ) ;  -- write evicted prefixes to a tier
kind     := fifo [ ( c ) ]                   -- c servers, one job each at rate 1
          | ps ( expr )                      -- throughput phi(present) shared equally; expr reads present
          | delay                            -- ps(present): every job at rate 1, no waiting
          | step { cost expr ; [budget expr ;] [chunk expr ;] [granule c ;]     -- options in any order; budget inf by default
                   [serve admission ; | serve by ( expr , ... ) ; | serve decode first ;
                    | serve exclusive prefill ;
                    | serve only ( expr ) [admission | by ( expr , ... ) | decode first] ;]
                   [memory POOL ;] [state NAME = expr ;]* [iteration { istmt* }] }
istmt    := serve [only ( expr )] [admission | by ( expr , ... ) | decode first] ;   -- the residents not yet served
          | admit [only ( expr )] [while ( expr )] ;           -- the waiting, one at a time, each served
          | branch ( expr ) { istmt* } [else { istmt* }]
          | set NAME = expr ;                  -- one of the stage's registers (`state NAME = c ;` among its options)
wlitem   := arrive poisson ( rate ) ; | arrive renewal ( expr ) ; | arrive closed ( n ) ; | arrive batch ( n ) ; | arrive none ;
          | trace "file.csv" [ordered] ;      -- replay sessions from a trace
          | init block | turn block          -- Size / set / observe
          | session block                    -- optional: the sequence of completed turns
          | hidden NAME [, NAME]* ;           -- the scheduler may not read these
stmt     := turn ;                           -- draw attributes, submit, wait for the response (session only)
          | Size NAME = expr ;                -- workload request quantity
          | Cost NAME = expr ;                -- expression must have a resource Cost type
          | Cost NAME = { RESOURCE : expr [, RESOURCE : expr]* } ;
          | set NAME = expr ;
          | observe NAME = expr ;
          | hold POOL ( expr ) [reserve ( expr )] [, POOL ( expr ) [reserve ( expr )]]*
                 [reuse ( expr )] [at admission ( NAME = expr , ... )]
                 block [ cache ( expr ) ] [ lease POOL ( expr ) ] [ ; ]
                                             -- lease: that pool's allocation outlives the scope,
                                             -- until a release/transfer takes it, expr seconds, or the end
          | grow POOL ( expr ) ;             -- the enclosing hold's allocation on POOL grows
          | drop POOL ;                      -- discard the own cached prefix
          | release POOL ;                   -- give the enclosing hold's allocation on POOL back now, or end a lease of it
          | load POOL ( expr ) ;             -- the KV of expr tokens arrived: the enclosing hold's computed position advances
          | run STAGE [prefill | decode] ( expr ) [ growing POOL ] ;
                                             -- the mode: required on a step stage, refused on any other
                                             -- growing: inside a hold of POOL, which grows with the tokens
          | run STAGE , STAGE [, STAGE]* ( expr ) ;   -- one job holding every stage at once
          | run ( expr ) ;                   -- in a queue's entry: the queue's own stage
          | branch ( expr ) block [ else block ]          -- a test
          | branch with ( expr ) block [ else block ]     -- a draw, w.p. expr
          | while ( expr ) block             -- test before each pass; continue after the block
          | loop block
          | fork block                       -- the block runs beside the session: a leg of the request
          | join ;                           -- wait until every leg forked so far has ended
          | choose NAME in expr by ( expr , ... ) ; -- NAME := argmin over 0..n, keys in order
          | end ;
          | QUEUE [ '[' expr ']' ] . VERB ( expr, ... ) [ from QUEUE [ '[' expr ']' ] ] [ to POOL ( expr ) ] ;
                                             -- a queue's entry, in its place (*Queues*)
          | mark NAME ;                        -- in an entry: the moment, read by the caller as QUEUE.NAME
          | serving                          -- the serving vocabulary, sugar for run
serving  := transfer [ '[' expr ']' | on STAGE [, STAGE]* ] expr from POOL to POOL ( expr ) ;
                                             -- the KV moves: run link; load; release
          | tool     [ '[' expr ']' | on STAGE [, STAGE]* ] expr ;
```

### Arrivals and the run

A model defines the deployment and workload. Execution settings are supplied by
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
`c ? a : b` (a non-zero operand is true; `branch` and `while` guards are held to 0
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
  names the language supplies (the context variables, `inf`) are kept
  apart, so `set present = …` does not link (a `ps` capacity reading
  `present` would read the attribute).
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
once; each arriving session executes its session body. Execution settings are supplied externally.
Only imports, definitions and fixed constants may precede `main`. Declarations
after it and nested or duplicate entry points are refused. Libraries provide
definitions and imports, never an executable entry point.

A plain `let` cannot be replaced externally. With `use "std/args";`, a
`let` inside `main` may declare an input using
`args.number("name", default)` as its entire initializer. CLI arguments after
`--`, `--set`, instance bindings and API `sets` supply only those named
inputs. Defaults and supplied expressions are resolved at link time; the
result is the same IR as a program written with those values. An input that
would change a parse-time array size is refused. See [Program](api/program.md)
for the full input contract. Syntax snippets elsewhere may show just a main
body or one of its nested blocks.

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
they are the mode of the run, `run E prefill (cost(E, T));` and
`run E decode (cost(E, T));`, which names the engine the request uses.

| Serving form | Kernel |
|---|---|
| `transfer (X) from P to Q (n);` | `run link (cost(link, X)); load Q (cost(Q, n)); release P;` — the KV of `n` tokens moves from the session's lease (or hold) on `P` to its hold on `Q`: the link takes the time, the tokens count as computed at `Q`, and `P` is given back (below, *A KV transfer*) |
| `tool Z;` | `run tool (cost(tool, Z));` |
| `tool[j] Z;` | `run tool[j] (cost(tool, Z));` (the index applies to the role's stage array) |
| `tool on S[j] (Z);` | `run S[j] (cost(S, Z));` |
| `transfer on egress[i], ingress[j] (X) from P to Q (n);` | `run egress[i], ingress[j] (cost(egress, ingress, X)); load Q (cost(Q, n)); release P;` — one read that holds the sender's link and the receiver's at once (below, *Stages*) |

The argument is work in the unit of the stage it runs on: the time the job
takes alone on a `fifo`, `ps` or `delay` stage (seconds, when the program's
clock is seconds; a `ps` stage serves it at `φ(present)/present`).

#### Which stage

A form finds its stage among the stages declared above
it (declarations come first in every program here): the stage whose name
is the role's, `link` (or `transfer`), `tool`. Exactly one must qualify:
with none (`stage svc : fifo;` and `tool Z;`) or several (`link` and
`transfer`) the parser stops at the form and says so. `on STAGE` names the
stage explicitly; with several instances of a role, `choose j …; tool[j] Z;`
serves an array and `tool on S2 (Z);` stages that are not one. The run is
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
victim, until the session's `release P` (a `transfer … from P` contains
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
completed turn. From `examples/multi-turn/vllm.sq`:

```serq
workload {
  arrive poisson(Lambda);
  hidden o;
  init { set K = 0; }
  turn { … }
  session {
    turn;
    while (more) {
      set K = K + n + o;
      tool (~exp(Z));
      turn;
    }
  }
}

server {
  set t0 = now;
  set prompt = K + n;
  vllm_request(reqs, kv, engine, prompt, o, t0);
  observe response = now - t0;
}
```

The end of the session block ends the session. `end;` is only needed for
an early exit, including an exit from an unconditional `loop`.

The parser expands each source `turn;` into the IR's attribute-drawing
`Turn` followed by the server's statements. Client and server attributes
remain shared; this expansion introduces no extra scope or dispatch.

The IR retains statement sides and attribute types to enforce workload size
ownership. The parser and IR validator enforce these boundaries:

| | Session | Server |
|---|---|---|
| Turn lifecycle | `turn;`, optional early `end;` | Refused; completion is the end of its block |
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
  pool reqs { cap max_seqsP; admit via P; }
  pool kv { cap blocksP * bs; block bs; evict lru; preempt lifo; }
  serve step { budget B; cost …; memory kv; }
  prefill (prompt) {
    hold reqs (cost(reqs, 1)), kv (cost(kv, min(prompt, hit + budget_left(P)))) reserve (cost(kv, prompt))
         at admission (hit = min(cachedin(kv), reusable(prompt, bs))) {
      set c = cached;
      run P prefill (cost(P, prompt - c)) growing kv;
    } cache (cost(reqs, kv, prompt)) lease kv (inf);
  }
}
```

The pools are the queue's (`P.kv` from outside, `kv` within), the stage is
named after the queue (`admit via P`, `budget_left(P)`, `work(P[i])`), and
an *entry* — one per verb of the queue's roles — holds what the station
does with one request, with the role's parameters (`prefill (prompt)`). A
queue declares its pools, then its `serve`, then its entries, each reading
what is above it. The body is the server's statements; `run (cost(P, X))` with
no stage names the queue's own, and a step engine's run names it with its mode (`run P prefill (cost(P, T))`);
the stages that are not a step engine (a link's, a delay) the body may name
as a `server` does. `self` is the member's index in a family. A family's
size may be a `let` constant (`queue D[ND]`), and a family of one is still
a family: its entries are called `D[j].decode (…)` and its lease taken
`from D[j]`, as at any size. A reference to a member's pool or stage
(`holders(D[j].kv)`) is the kernel's array reference, which at size one
also takes `D.kv`.

Four roles are built into the parser, and a queue declares which it plays:

| Role | Entries | The queue |
|---|---|---|
| `gateway` | `route { … }` | `Q.route();` enters this queue's `route`; each gateway is a single queue |
| `prefill` | `prefill (prompt)` | computes the prompt; how it leaves the KV (`lease`, `cache`, a transfer) is the entry's |
| `decode` | `decode (prompt)`, `decode (prompt) from Q` | a local prefill, or with the KV `Q`'s entry leased for this request |
| `link` | `transfer (n)`, or none | the NIC: the body is the time to read `n` tokens; without one, the `serve` is the cost, and its `latency` a wait before it |

The server selects a gateway with `gw.route();`. The parser checks the
queue and its entry, then expands the route at the call site. Declarations
may follow the server and several gateways may coexist. Declaring a gateway
does not execute it or register a default server.

`queue`, `pool`, `serve`, `nic`, `pull`, `turn` and `mark` are language
syntax. The role names and their entry signatures in the table are
predefined vocabulary; `gw`, `P` and `D` are names declared by this
program. User-defined roles are not supported.

The deployment calls an entry where the request goes: `P[i].prefill
(prompt);` and `D[j].decode (prompt) from P[i];`. `from P[i]` is the pool
`P`'s entry leases, and every entry of `P` must lease it on every way
through it, so that whichever one the request went through left the KV; a `from` on a queue that leases
nothing does not link.
Inside the entry the `from` name is that pool, and in an index it is
the source member's index.

A pod's NIC is the pod's: `nic ps(BwD);` in a queue is the stage
`D.nic`, one per member. A *pull relation* says which queue reads the KV
from which, and how the reads share the NICs:

```
queue P[NP] : prefill { … nic ps(BwP); prefill (prompt) { … } }
queue D[ND] : decode {
  … nic ps(BwD);
  decode (prompt) from src {
    …
    transfer (prompt - c) from src to kv (prompt - 1 - c);   // over P's NIC and this pod's
  }
}
D pull P latency x0 share maxmin;
```

`D pull P` is one line for the topology and the mode: the KV goes from `P`
to `D`, `D` starts the read (NIXL pull), `D` waits `x0` before each one
(a number or a constant over `let`s), and concurrent reads divide the two
NICs by the policy, which is the program's `share` and is written here, not
left to a default. Inside an entry of `D` called `from P[i]`, a `transfer`
without `on` is the read: the kernel's `run D.nic.latency[j] (x0); run
P.nic[i], D.nic[j] (…); load D.kv[j] (…); release P.kv[i];`. Both queues
are declared above the relation and have a `nic`; a reader has one source;
an entry of `D` called `from` another queue is an error, and so is a
`transfer` without `on` in a queue that pulls from none. A program with a
relation does not also write `share` on its own.

`P push D latency x0 share maxmin;` is the same copy written by the other
side (NIXL push): the bytes cross the same two NICs, and the source's
worker posts the write, so the wait before each copy is `P`'s, the delay
stage `P.nic.latency`, indexed by the source member. A push says who moves
the bytes, not when the decoder is asked for its blocks: that is the
dispatch, written where it happens, with `fork` (below,
`examples/pd-disaggregation/vllm_nixl_push.sq`). The wait is a delay, so a
push and a pull of the same constants run the same numbers. A queue waits
before the copies it posts for one relation: one `latency` per poster.

`transfer on L[k], M[l] (n) from S to P (m)` names the stages itself, as
a `server` does: link queues, or any `ps` stages. `latency x` on a link's
`serve` is the wait before such a transfer: a delay stage
`L.latency` of the link's own, which every `transfer on` naming the link
runs first, one wait per link named, in the order named. A number or a
`let` constant; only a link without an entry takes one. A link with a `transfer (n)` entry is called
instead, `nic[self].transfer (n) from src to kv (m);`, and runs its body on
its own stage. Arguments are substituted like an `at admission` binding, so
none may draw.

An entry sees its own. Its header — the units, `reserve`, `reuse`, the
`at admission` bindings — reads the parameters, the queue's pools and stage,
the constants, and `cached` and `computed`; its body also reads `now`, what
it sets itself and the request's `hidden` attributes, and nothing else the
session has set, nor another queue's
`Q.x`; every index it writes (`run nic[k]`) reads as the body does. Its
statements hold, grow, load and release only the queue's own pools and the
pool its `from` names. That is the `hidden` rule
at the queue's boundary: vLLM's scheduler knows `max_tokens`, not the length,
so `o` is not a parameter of `decode` and the body alone reads it. What the
body `set`s is the queue's (`D.known`); what it `observe`s is the program's;
a moment the caller needs is `mark first_token;`, read afterwards as
`D.first_token` — the request's attribute, so without a member index. The gateway is the exception on the session's side: its
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
(entries of units, release time and release order, per session or dead),
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
session consumes at most `ρ` units of its own cached prefix (`cached :=`
what it consumed); the rest of that entry stays in the cache as a *dead*
entry with the same age, unusable, until evicted. Without it the hold is
memory alone: it leaves the session's cached blocks where they are,
evictable as before, and `cached` is 0 in its body (the linker rejects a
body that reads it there, and a `reuse` there; `cache (0)` is the hold
that consumes the prefix and keeps nothing). `reuse (0) cache (ℓ)` skips reuse while retaining caching on release.
Other entries are evicted in the
pool's order until allocations and cache fit; `u` units are allocated and
the body runs. At the end of the body the units are released and
`min(ℓ, computed)` units stay cached, rounded down to blocks (`computed`
is the allocation, or the position a `growing` run or a `load` reached).
`release m` inside the body does the same for `m` alone, at that point:
the innermost enclosing hold on `m` gives its allocation there back,
caching per its clause, and holds `m` no longer. A hold's `lease m (t)`
keeps its allocation on `m` past the scope's end, neither evictable nor a
preemption victim, until the session's `release m`, `t` seconds, or the
session's end, and `cache` applies then; a `release m` outside any hold on
`m` ends the lease. A session that holds and leases nothing on `m` releases
nothing (a hold re-executed after a preemption reaches the statement
again). `load m (cost(m, n))` advances the innermost
enclosing hold's position on `m` by `n` tokens, which its allocation must
cover; the KV of a transfer counts as computed from then on. `grow`,
`growing`, `load` and `release` stand inside a hold of the same pool
reference, index included, as the linker resolves it (`kv[k]` with `let k
= 1` is `kv[1]`; `kv[i]` is not `kv[j]`), or `release` in a session that
leases it; elsewhere the program does not link. A hold's index is read at
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
them writes `drop POOL;` before `end;`. Eviction is per entry, or per block
from the tail of the entry when the pool has `block b`; `evict lru` orders
by release time and then release order, `evict by (k₁, …)` by the keys and
then release order. A request that can never fit — its units, or its
`reserve` when that is larger, above the cap, as they evaluate when the
session joins the queue — is rejected: the session ends, and the report
says how many did. Each of the units and the `reserve` that reads only
attributes and numbers is judged then; one that reads the deployment's
state (a pool or stage query, the clock, `budget_left`) asks for something
else at the next try,
so it is judged at every try and waits; a queue's head that, read as
the run ends, still asks a pool for more than its cap is named (`over:`,
the pool report's `over_cap`). One whose units or `reserve` are a constant does not
link: it would be rejected whenever it is reached. A pool marked `admit via S` is not admitted at settle time: its queue is
served by step stage `S`, at the start of an iteration, after the
residents have taken their tokens, while the iteration has budget left, and
not in an iteration that preempted (vLLM's waiting loop,
`scheduler.py:868-1128`). Families are joined member for member: `pool
q[N] { admit via S; }` next to `stage S[N]` serves `q[i]` by `S[i]`, and
`stage E[N] : step { memory kv; }` next to `pool kv[N]` counts `kv[i]` for
`E[i]`; next to a family of one, every member gets that one, and any other
pair of counts is a link error (`examples/pd-disaggregation/llmd_nixl_pull.sq` is the xPyD case,
`docs/use-cases/pd.md` §Writing xPyD). A stage that serves several queues tries them in
the order their pools are declared, and the first head that does not fit
stops the iteration's admissions; `examples/pd-disaggregation/llmd_nixl_pull.sq` declares the
decoder's queue of requests whose KV has arrived before its queue of new
ones, as vLLM serves `skipped_waiting` before `waiting`
(`scheduler.py:2383-2385`). `budget_left(S)` then evaluates to the budget
left. Under `serve exclusive prefill`, a selected prefill ends admission;
otherwise a fitting waiting prefill can displace tentative decodes, and
its header sees the full budget. Until then a waiting session's cached
prefix is evictable: the *wait channel*.

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
(`kv_cache_manager.py:515-531`) and generates only the rest. The vLLM
programs write `known = computed < prompt ? prompt : computed + 1` (the token
sampled at `computed` is the request's too), `run engine prefill (cost(engine, known - c))` and
`run engine decode (cost(engine, o - 1 - (known - prompt)))`. A program that recomputes from the
prompt alone says so by not reading `computed`.

### Waiting selection

`queue by (k₁, …)` orders a pool's waiting holds
by keys re-read before every admission attempt (after each admission under
`admit via`, where `budget_left` is that attempt's budget); `waited` is
the seconds since the hold joined the queue, reset on re-entry. The chosen
head that does not fit blocks the rest, a preempted hold re-enters ahead of
the keys, a key may not draw or read a `hidden` attribute, and selection
sets no timer. See [Pool](api/pool.md).

### Ties

Every order in the semantics is a declared key followed by a
declared number, the sequence number of a named event (for `choose`, the
index), so that two items with equal keys never fall to the order a data
structure happens to hold them in. A pool's queue:
the keys (`queue by`), compared in order and reevaluated before every
selection, then the order the sessions joined the queue (`fifo`
is that order alone); a preempted session re-enters at the head, ahead of
the key, unless its pool says `requeue tail`. Eviction: the keys (`evict by`) or the release time (`lru`), then
the order the entries were released. A step stage's residents: the
`serve by` keys, then admission order. The preemption victim: the
`preempt by` keys, then the candidate admitted last (for a pool that is
no engine's memory, the holder the pool admitted last). `choose`: the keys, in order, then the smallest
index. A pool's growers: the order they stalled, the head blocking the
rest. Events at one instant: the order they were scheduled; sessions run
in the order they became ready; jobs of a `ps` stage with equal finish
tags finish in the order they started. Each pool has one queue, and a hold
on several pools waits in its first pool's; where the heads of two queues
both wait for room in one pool, or one stage serves several queues, the
pool declared first is served first.

### Stages

`fifo(c)`: `c` servers, jobs in arrival order at rate 1.
`ps(φ)`: every job at once, each at `φ(present)/present`. `delay`: every job on its
own at rate 1, which is `ps(present)` and links to it. `run a, b (cost(a, b, w))` is one job that holds `a` and `b` from its
start to its end: a *flow*, whose work goes down at one rate at all its
stages, set by the program's `share` from their capacities. `share maxmin`
is max-min fair: every flow's rate rises together until a stage fills,
the flows through it stop there, and the others go on. `share bottleneck`
gives each flow its equal share at the tightest of its stages,
`min over s of φ_s / n_s`, and leaves what that does not use at the other
stages unused. Every stage of such a run is `ps(φ)` with a constant `φ` above 0
(a flow is not described by the `present` a capacity could read), a run
names each stage array once (an index is known only when the run starts),
and a program with one declares its `share`, which has no default. A
stage array held by some run with another is *shared* for the whole run:
every job on it, a single-stage `run` included, is a flow of the policy,
and its utilisation is the capacity its flows carry, `Σ rate / φ`. Every
other `ps` stage serves as above. `step { budget B; cost C; }`: an engine that runs
iterations. A plain `run`'s work is time at rate 1, the clock's unit; a step
engine's `prefill` and `decode` work is in the unit of `B`, tokens. The
clock itself has no unit: a program whose costs are seconds runs in seconds,
and `examples/oracle/vllm_request.sq` runs on the step clock with `cost 1`, so
its times are iterations. The residents are served the way `serve` names, said once per
stage: an order, `by (k₁, …)` (ascending keys evaluated for each resident
with `decoding`, 1 for a decoding resident, `admission`, its admission
sequence number, `remaining`, the tokens its run has left, and the
totals `residents`, `decoders`, `kv_decode`, `kv_prefill`; ties in admission order; a key may
not draw), or the rule `exclusive prefill`, below, which is not an order and
so cannot be combined with one. `admission` (the order their sessions were
admitted, vLLM's `running` list; the default) is `by` with no keys, where
every resident ties, and `decode first` is `by (decoding ? 0 : 1)`; of the
orders, the IR knows only `by`. A scheduler that serves the shortest
remaining run first is `serve by (remaining)`, the opposite `serve by
(-remaining)`. One token to a decoding job, up to `chunk` to a prefilling
one (with a `granule g`, short of its remainder, a multiple of `g`, none
when that is 0: such a prefill is passed over, and the iteration admits no
one after it),
until the budget is spent; a `growing` job first grows its hold to the
position it will reach (block by block, preempting if needed); a victim
the iteration has already served leaves it and its tokens return to the
budget (scheduler.py:779-797) and its hold caches its position, not the
chunk the iteration gave it ([vLLM correspondence](#7-vllm-v1-as-a-serq-program)); a grower that preempts itself ends the
iteration's serving (scheduler.py:807-813); then the stage admits from the
queues it serves. The iteration advances the clock by
`C`, an expression in `tokens`, `decoders`, `prefilled`, `residents`, `kv_decode`, `kv_prefill`, `attention`; its
tokens are applied when it ends. A run of zero work completes at once. An
iteration that schedules no token is not an iteration, unless it preempted:
then it is the scheduler step that only preempted (vLLM's `schedule()`
admits nothing in a step with `preempted_reqs`, `scheduler.py:869`, and the
oracle driver counts the step; the Lean model's `startIteration` gives that
step its cost, and `step` re-admits at the next event), and the next
iteration re-admits the victim. It lasts `C` at zero tokens, which is a
modelling choice: the real engine skips the forward pass of an empty step,
so the fixed part of `C` overstates it. A hold whose body can never fit
then preempts itself forever (vLLM refuses such a KV cache at start-up,
above); the `stuck` counter below reports it. A hold that reserves what it
will need (`reserve (known)` after a preemption) is rejected instead, once
the reservation is above the cap.
`serve exclusive prefill` selects either one prefill alone or a decode-only
batch: a resident prefill takes precedence, otherwise a waiting prefill that
fits displaces the tentative decodes and takes the full budget, and once a
prefill is selected nothing more is admitted in that iteration
([Stage](api/stage.md)). Without
a per-request chunk cap, serving in admission order *is* serving
decode-first (`SerqLang.Serve.serve_eq_decode_first`; a cap breaks it,
`chunk_cap_breaks_shape`).
`serve only (p)` says which residents the iteration serves; `by` says in
what order:

- `p` is read for each resident when its turn comes, from the variables a
  key reads, and may not draw or read `now` or `work(…)`.
- A key and `p` read the totals (`residents`, `decoders`, `kv_decode`,
  `kv_prefill`) as the residents stand at that read. This counts a session
  that the iteration admitted through `admit via` and leaves out one it
  preempted.
- A resident served earlier in the iteration is not reconsidered.
- A resident that `p` reads as 0 gets no token this iteration. It keeps
  what it holds and advances no computed KV, as a displaced decode does
  under `exclusive prefill`. An admitted session that `p` excludes waits as
  such a resident. How many are admitted is the pool's `cap` and the hold's
  header, not `p`.
- An engine whose residents `p` all excludes runs no iteration. It waits
  for the next event of any kind, when `p` is read again; the clock moving
  is no event.
- The order that follows (`admission` when none is written) orders the
  rest.

FasterTransformer as Dai et al. model it (decode first, no mixed batching) is
`serve only (decoders > 0 ? decoding : !decoding);`. Its opposite, prefills
alone (as many as the budget takes) while one is resident, is
`serve only (decoders < residents ? !decoding : decoding);`. A waiting
prefill admitted after a decode was served still joins that decode. Taking
the served decode back is `exclusive prefill`'s admission rule. `only` cannot be combined with `exclusive prefill`.
A stage's `serve only (p)` is the iteration body `serve only (p); admit only
(p) while (!preempted);` (below), which the linker writes.

### The iteration as a program

Everything above but `serve only` is one procedure, vLLM's
`schedule()`: serve the residents, then, unless the iteration preempted
(`scheduler.py:869`), admit the waiting with the budget left. `iteration { … }`
on a step stage supplies an explicit body. Its statements run once each
in order: `serve [only (p)] [order]` gives the residents not
yet served their tokens, skipping and leaving unserved those `p` excludes;
`admit [only (p)] [while (e)]` admits the heads of the queues the stage
serves one at a time, each served at once unless `p` excludes it, while
budget is left, the head fits and `e` is 1; and `branch (e) { … } else { …
}`. A guard and a `while` read the residents' totals, what the iteration
has scheduled so far (`tokens`, `prefilled`), `admitted` and `preempted`
(counts). The procedure above is the body `serve; admit while
(!preempted);`. A stage's `serve only (p)` expands to `serve only (p);
admit only (p) while (!preempted);`. A body cannot be combined with `exclusive prefill` or a stage-level
`only`. See [Stage](api/stage.md#iteration) for scheduling examples.
A body with a path that neither serves nor admits, or a guard that reads
`now`, does not link (an engine that schedules nothing waits for an event,
and the clock moving is none); that is necessary, not sufficient, and an
engine the linker could not see stall is named in the report when the run
ends with its work unscheduled (`idle: stage …`). `state NAME = c;` gives
the stage a register its body sets (`set NAME = e;`) and the scheduler's
expressions read between iterations; a set
takes effect with its iteration (a try that schedules, preempts and admits
nothing is undone), and a register is read only by its stage, the keys of a
pool it admits, the header of a hold whose first pool it admits, a gauge
or a claim ([Stage](api/stage.md#registers)).

### `at admission`

Everything in a hold's header — the units, `reserve`,
`reuse` — is evaluated when the session is admitted, and a `set` above the
hold is not (`cache` is read when the session releases, `Serq/Exec.lean`'s
`release` and the interpreter agree). `at admission (x = e)` gives the
header a place to name what it is written in terms of, as `lib/vllm.sq`
does:

```
hold reqs (cost(reqs, 1)), kv (cost(kv, min(known, hit + budget_left(engine))))
     at admission (known = computed < prompt ? prompt : computed + 1,
                   hit = min(cachedin(kv), reusable(known, blocksize(kv)))) { … }
```

The bindings are substituted into the header's expressions by the parser, so
the interpreter and the Lean model know nothing of them, and a program that
uses the clause has the IR of the one that inlines by hand. A
later binding sees the earlier ones. A binding may not draw (`~`): it is
substituted, so a name used twice would draw twice.

The body sees a binding too. One the body reads is set at its top, `set
known = e;`, which is its admission value because the body starts at the
admission's instant and `e` reads only attributes and constants; that `set`
is in the AST and the IR like any other. A binding that reads live state —
an observable such as `cachedin(kv)`, a context variable, or `cached`, which
the admission itself sets — has another value there, so the body reading it
is a parse error; the body reads `cached`, the units the admission consumed.
The name of a binding the body reads is its own: not a builtin attribute, a
pool or stage, a context variable, a `let` or an attribute the program sets,
and not read outside the holds that bind it. A binding reads only the ones
before it in its clause.

### `hidden`

In a `server`, hidden attributes may control run work, cache release and
observations. Decisions and allocations may read them only after a run
whose work reads them has ended. This also applies to attributes derived
from hidden ones; both branch arms must reveal an attribute before it is
known after the branch. See [the full rule](api/workload.md#hidden).

`hidden o;` in a workload prevents scheduler expressions, such as a hold's
header, queue keys or a budget, from reading `o`. Session statements,
run work, a hold's `cache` and a claim's `given` may read it. Attributes
set by the scheduler (`cached`, `computed`) cannot be hidden.
See [Workload](api/workload.md#hidden) for the permitted positions.

### Branching

`branch (e)` takes the first block when `e` is 1 and the
second when it is 0; any other value (a fraction, a count, a negative
number, NaN) is a run-time error, since a guard is a test and a test has
two answers. `branch with (p)` takes the first block with probability `p`,
and is sugar the parser rewrites to `branch (~bernoulli(p))` — the IR, the
interpreter and the Lean model know only the one form, and the draw is a
0 or a 1 by the time the guard sees it. A constant guard that is not 0 or
1 is refused at link time; a computed one is refused when it is evaluated.

### Amounts and indices

The work of a `run`, the units of a `hold`, a
`grow` and a `load` are amounts, and an amount is not negative and not NaN;
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

### Every instant settles

A `loop` or `while` must let time pass on every pass
through its body: every path through it reaches a `run` (a constant zero
work does not count), a `hold` whose body does, or `end`; the linker
refuses a loop that does not (`loop { set w = w + 1; }`, or a `run` on one
arm of a `branch` only). What the linker cannot see — a `run` whose
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
`cached` or writes `reuse`; and a constant `branch` guard other than 0 or 1
(Branching, above). These are errors, not warnings.

### Statistics

`observe x = e` records a sample after warm-up with its
time, session and turn. `--dump DIR` exports these samples. The
[CLI reference](reference/cli.md) defines the reported statistics and
resource counters.

`stuck` counts sessions preempted again without advancing past their previous
preemption. Such sessions do not contribute completed response samples.
A boolean observation that remains zero for at least 40 samples produces
a report note, not an error.

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
claim work_conserving: every iteration of engine (demand < bmax || tokens == bmax);
claim starved: some iteration of engine (demand >= bmax && tokens < bmax);
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
`serve only` leaves out included. `served` is the tokens the stage
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

| Program | Deployment | Checked by |
|---|---|---|
| `single-turn/mg1.sq`, `single-turn/ps.sq`, `multi-turn/closed.sq` | M/G/1 FIFO, M/G/1-PS, M/M/1//N | a run against the closed form, by hand, in [getting started](getting-started.md) and the [tutorial](tutorial/01-a-queue.md); no test |
| `multi-turn/replica.sq` | the paper's two-resource replica on the open-session scenario (`serve decode first`, `drop kv` before `end`) | — |
| `multi-turn/routing.sq` | four replicas, five routing policies | — |
| `multi-turn/vllm.sq` | vLLM v1's engine (`lib/vllm.sq`) under a multi-turn agent workload | its engine is the other vLLM workloads' (`tests/workloads.rs`) |
| `single-turn/vllm_single_turn.sq`, `multi-turn/vllm_chat.sq`, `subagent/vllm_subagents.sq` | the same engine under a single-turn, a chat and an approximated subagent workload ([use case](use-cases/workloads.md)) | `tests/workloads.rs` |
| `oracle/vllm_request.sq` | one vLLM v1 request on the step clock, compiled per scenario to `tools/oracle/{alone,chunked,hol,longchunk,mixed,preempt,seqcap}.ir.json` | the upstream oracle ([vLLM correspondence](#7-vllm-v1-as-a-serq-program)), `tests/vllm_oracle.rs`, the Lean theorems generated from the same IR |
| `replay/vllm_replay.sq` | vLLM v1 on the A100 testbed replaying the short-context trace ([A100 testbed](#8-vllm-on-the-a100-testbed)) | the prefix-cache oracle `tools/oracle/cache_trace` (`tests/vllm_cache.rs`, theorem `vllm_cache_trace`) |
| `pd-disaggregation/llmd_nixl_pull.sq` | llm-d's prefill/decode disaggregation with the NIXL connector, two prefill and two decode instances ([use case](use-cases/pd.md)) | the source (llm-d at 8a2f37d, the router at 13eebdb, vLLM at 0c87a197), `tests/pd_semantics.rs`; no scheduler oracle |
| `pd-disaggregation/vllm_nixl_push.sq` | vLLM's push proxy in front of one prefill and one decode instance with the NIXL connector's push mode, on the A6000 testbed, replaying the short-context trace ([the push mode](https://github.com/servingQ/serQ/blob/main/docs/design/push-mode.md)) | the source (vLLM at 0c87a197), `tests/fork_join.rs`; no scheduler oracle |
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
`lib/vllm.sq`'s `vllm_request`, the engine of the workload examples, is a
simpler one: it does not `reserve`, takes its hit from the cache alone
rather than from the previous prompt, and caches `prompt + o`.

| vLLM | serQ | Where |
|---|---|---|
| a token budget per step, running requests first in `running` order, then waiting requests with the budget left | `step { budget B }`, residents in admission order; `pool reqs { admit via engine; }` | `scheduler.py:577, 624-823, 868-1128` |
| `max_num_seqs` | `pool reqs { cap max_seqs }` in the hold | `scheduler.py:877-879` |
| FCFS, head-of-line blocking (`if new_blocks is None: break`) | pool queue `fifo`; the first request that does not fit blocks | `scheduler.py:1228-1235` |
| admission needs blocks for the whole prompt (`scheduler_reserve_full_isl = True`), but only the first chunk is allocated | `kv (hit + min(prompt − hit, budget_left(engine))) reserve (prompt)` | `kv_cache_manager.py:515-531`, `config/scheduler.py:191` |
| a waiting request's prefix is looked up and its blocks touched only when it is scheduled | units evaluated at admission; the queue served by the engine | `scheduler.py:932-939`, `block_pool.py:754-770` |
| chunked prefill, `long_prefill_token_threshold` | `run engine prefill (cost(engine, n)) growing kv`, `chunk long_prefill(reqs, c)`: the cap only while more than one request is running or waiting (`lib/vllm.sq`); a constant cap is not vLLM's, and `serq target` refuses it | `scheduler.py:606-616, 675-676, 1115-1128` |
| `allocate_slots` block by block as the request advances | `growing kv` | `kv_cache_manager.py:371-608` |
| preemption of `running[-1]`, `waiting.prepend_request`, `num_computed_tokens = 0`, no admission in a step that preempted | `preempt lifo`, re-queued at the head, hold re-executed; `admit via` skips preempting iterations | `scheduler.py:742-813, 869, 1539-1582` |
| a preempted request keeps its output tokens: it is rescheduled with `num_tokens = prompt + outputs`, reserves and recomputes that many, and generates the rest | `computed` read by the re-executed hold: `known = computed < prompt ? prompt : computed + 1`, `run engine prefill (cost(engine, known - c))`, `run engine decode (cost(engine, o - 1 - (known - prompt)))` | `scheduler.py:1560-1561`, `kv_cache_manager.py:515-531` |
| the scheduler reserves by `num_tokens` (prompt and generated so far), never by the final length: it knows `max_tokens` and learns the length when `check_stop` sees EOS or the cap | `hidden o;`: no header, key or budget reads `o` | `kv_cache_manager.py:517, 533-534`, `config/scheduler.py:191`, `scheduler.py:639, 2426`, `sched/utils.py:98-119` |
| the prefix cache holds every *computed* full block, generated tokens included; a hit is the longest run of cached full blocks, at most `num_tokens − 1` | `cache (prompt + out − 1)`; `reuse (floor(min(prev prompt, prompt − 1)/bs)·bs)`; the unmatched blocks stay cached, dead | `kv_cache_manager.py:289-300, 602-606`, `single_type_kv_cache_manager.py:743-838` |
| the free queue: freed blocks appended tail first (LRU), in the order requests finish | `evict lru` per block from the tail, ties by release order | `block_pool.py:776-805`, `single_type_kv_cache_manager.py:557-585` |
| a finished session's blocks stay in the free queue | `end` keeps the cache | `block_pool.py:776-805` |
| a forced miss (a nonce at the head of the prompt) matches nothing; the old blocks stay | `reuse (0)` | trace |

Not modelled, on purpose: a victim the PRIORITY path takes out of a step that
had scheduled it leaves the full blocks of that step's chunk hashed in the
prefix cache (`allocate_slots` cached them, kv_cache_manager.py:602-606, and
the freed blocks return with their hashes, block_pool.py:776-805), though
the step never computes their KV; serQ caches what was computed, the
position ([Semantics](#3-semantics)).

Not modelled: the watermark (0 by default), the adaptive long-prefill
threshold (off by default), encoder inputs, speculative decoding, sliding
window, the PRIORITY policy (its victim, the largest `(priority,
arrival_time)`, `scheduler.py:761-765`, put back into a heap ordered by
the same, `request_queue.py:159-164`, is `preempt by (-priority, -t0)
requeue tail` beside `queue by (priority, t0)`, but no program here writes
the policy and no oracle scenario checks it), the deferred free of in-flight
blocks, asynchronous scheduling ([A100 testbed](#8-vllm-on-the-a100-testbed)), and cross-session prefix sharing
(cache entries are per session, [limitations](#9-known-limitations)).

### Admission

The header of the `hold` in `lib/vllm.sq`'s `vllm_request` is the
prefix-cache lookup and the allocation of the first chunk, and both happen
when the scheduler admits the request, not when it queues. `known` is
every token the request has: the prompt, or after a preemption the tokens
it had computed plus the one sampled there (`known = computed < prompt ?
prompt : computed + 1`, vLLM's `request.num_tokens`). The hit is the
cached *full blocks* of those, never all of them, since the last token is
recomputed for its logits: `floor((known − 1) / bs) · bs`
(`kv_cache_manager.py:289-300`). `cachedin(kv)` is read in the header, so
it is read in the waiting loop (`scheduler.py:932-939`), and until then a
waiting request's prefix is still evictable: the wait channel ([Semantics](#3-semantics)). The
units are the hit plus the chunk the budget the running requests
leave can take now, `min(known, hit + budget_left(engine))`
(`scheduler.py:1078-1128, 1214-1226`): every known token, or as far as the
hit and the budget reach, whichever is less. The rest is allocated as the
request runs (`growing kv`).

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

These comparisons check individual requests and steps, rather than only
aggregate statistics.

## vLLM on the A100 testbed {#8-vllm-on-the-a100-testbed}

The [measured replay example](use-cases/vllm.md#measured-replay) documents
the A100 cost fit, calibration, comparison runs and their limitations.

## Known limitations {#9-known-limitations}

* Continuous work and fluid rates at `fifo`/`ps`/`delay` stages; the
  `step` stage is discrete. A fluid server is the step stage with the
  budget filled to the memory time; at ω = 0.2 ms that is 5 000 iterations
  per simulated second, so long horizons can be slow.
* Cache entries are per session; cross-session prefix sharing (a common
  system prompt) needs a content-addressed cache.
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
