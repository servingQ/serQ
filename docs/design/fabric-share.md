# A policy per fabric

How concurrent flows divide the stages they hold at once is a property of
the fabric they cross: the NICs of a prefiller and a decoder, NVLink inside
a node, the network between nodes. Until this change it was the program's:
one `share maxmin;` for every run over several stages. This document moves
the policy to the run, so two fabrics that share no stage may divide their
capacity two ways, and names the policies in a library, `std/share`, beside
`std/kvt`. Issue #559; `Run.share` replaces `Program.share` within the
untagged IR 13.

## Before

`examples/pd-disaggregation/llmd_nixl_pull.sq`, lines 176-177:

```serq
  share maxmin;
  kvt.nixl_pull (P, D, latency = x0);
```

`share maxmin;` stands alone, two lines from the relation it is about, and
it is the only one a program has: [The pull relation](pull-relation.md)
noted that "NVLink one way and RDMA another is a policy per relation, and
an IR change". `maxmin` and `bottleneck` were keywords of the language.

## After

```serq
use "std/kvt";
use "std/share";
…
  kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);
```

and in the kernel, a run or a `transfer on` over several stages:

```serq
run egress[i], ingress[j] (cost(egress, ingress, n), share = share.maxmin);
transfer on egress[i], ingress[j] (n, share = share.bottleneck) from P to kv (m);
```

The relation's IR is the program before it, with `share` moved from the
program to each run that has an `also`: the three programs that wrote
`share maxmin;` (`pd_batching.sq`, `llmd_nixl_pull.sq`,
`vllm_nixl_push.sq`) give the same IR but for that move, and the same
report.

## The design

**A policy is a value of std/share.** `share.maxmin` and
`share.bottleneck` are read only after `use "std/share";`, from one table
(`Share::ALL`, `src/ir.rs`) that the parser's check, its error messages
and the policy's name all read. `share`, `maxmin` and `bottleneck` are no
longer keywords: `share` is the module's name, refused as a declaration
beside the import, as `kvt` and `args` are. A policy is a mechanism the
program selects (criterion 2); a third policy is a row of the table and a
solver arm, not a word of the grammar.

**A job over several stages names its policy.** The relation takes
`share = …` beside `latency = …`, in either order, each once; a run or a
`transfer on` over several stages takes it after its work, inside the
parentheses, `(w, share = …)`. It has no default. A job on one stage names
none: a policy divides what is held at once, and a single-stage job on a
shared stage is a flow of its fabric's policy.

**A fabric has one policy.** The stages that runs join, however
indirectly, are one fabric (`Program::stage_shares`, a union of each run's
stage arrays). The flow solver already solves one connected component of
flows at a time; a component is inside one fabric, so its flows are all of
one policy, and the solver runs each policy over its own flows
(`flows_solve`). Two runs that join stages and name two policies do not
link: max-min fairness over flows some of which are capped at their
bottleneck share has no definition here (criterion 0). A program with one
fabric, which is every program so far, is unchanged bit for bit: the flows
and stages the solver visits are the same, in the same order.

**The IR.** `CStmt::Run` gains `share: Option<Share>`, present exactly
when `also` is not empty, and `Program.share` goes. IR 13 is untagged, so
the version does not move; the change goes in the coming tag's message.
`Program::validate` checks the presence rule and the fabric rule, so IR
from a tool meets them too. The Lean generator refuses a program with a
`Run.share`, as it refused one with `Program.share`: shared multi-stage
execution is outside its fragment.

## Checks

- a relation without `share = …`, and an argument given twice;
- `share.…` without `use "std/share";`, a policy the library does not
  have, and a policy written without its module (`share = maxmin`);
- a run or `transfer on` over several stages without `share`, and a run
  on one stage with one;
- runs that join stages and name two policies, with the stages named;
- a declaration named `share` beside the import;
- `share maxmin;` at the top of `main`, which says where it went.

## Self-critique

**The program's one `share`.** The old form, kept by
[The KV-transfer library](kv-transfer.md), which rejected `share = …` on
the call because it set the program's one policy from inside a call and
two connectors would each have had to name it and agree. Both reasons
were about a program-wide value. With a policy per fabric the call sets
its own fabric's, and two connectors agree only when their copies cross
one NIC, which is then a fact about the fabric, checked at link time.

**`share maxmin;` kept as a default.** A default beside the argument is
two spellings of one policy, and a program that left a relation's
`share` out would take a rule from a line elsewhere.

**`network.share.maxmin`.** A two-level module says where the policy
applies, but it needs a namespace inside a namespace, which no library has,
for one member; and the mechanism is not the network's: `run a, b (…)`
divides any `ps` stages held at once (an L2 cache's bandwidth and a
decoder's NIC, say).

**A policy on the stage.** `nic ps(BwD) share maxmin;` puts the policy
where the capacity is, but a fabric joins several stages, and each would
repeat the policy and have to agree. The run is where stages are joined.

**`share = maxmin`, without the module.** Shorter, and it reads `share`
once; the name would be read from `Share::ALL` only in the argument's
place, so it needs no import. It was weighed against a policy that names
where it comes from: `share.maxmin` is the library's value, the way
`kvt.nixl_pull` is the library's function, so a reader sees that the
language supplies the mechanism and the program picks from it, and a bare
`maxmin` would be the only library name written without its module.

**A clause after the work.** `run a, b (w) share share.maxmin;` reads as
`share` twice in a row; inside the parentheses it reads as the relation's
keyword argument does.
