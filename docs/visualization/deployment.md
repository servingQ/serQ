# The deployment view

The program as a queueing network.

![llm-d prefill/decode over NIXL](../assets/llmd_nixl_pull.deployment.svg)

That is `examples/pd-disaggregation/llmd_nixl_pull.sq`: two prefill and two decode
instances, the KV read over NIXL. The server first refuses a prompt longer
than `max_model_len` (the diamond), then calls the gateway (`gw.route();`,
the hexagon). Each filled panel is an instance: what the gateway's
`choose` picks, a pod with its engine, its NIC and its pools; the
engine's slots and KV blocks are drawn in the engine's frame. The gateway
picks one of `NP` prefillers and one of `ND` decoders, and the figure draws
all of them, a panel per member stacked down from the first (`P[0]`,
`P[1]`), the gateway's arrow turning down to each. The decoder replies to
the gateway, and the reply leaves by it for the client. The
read crosses from a prefiller's box to a decoder's, from each to each: it
holds the prefiller's NIC and the decoder's at once, and what it moves
(`P.kv[i] → D.kv[j]`: the prefiller's blocks stay leased until it ends, the
decoder's are allocated before it starts) and the link's latency are written
over the arrows. It is a `pull`: the decoder's worker starts each read, so a
dot stands over each `D.nic[j]`. In `vllm_nixl_push.sq` the same copy is a
`push`, its two NICs in no panel and bracketed as one job, the dot on the
bracket over `P.nic`.

## Request flow

For a program with a `server` or gateway, the figure shows one request's
path. Workload tool calls, subsequent turns and session exits are omitted,
but holds surrounding the workload's `request` remain visible. The figure
reads the request from the IR (`sides`), and a gateway's route from it too
(`routes`), so a program, its JSON IR and a compiled `pyserq.Program` draw
the same figure. Two requests are the same when they run the same
statements inside workload holds of the same pools, whatever the units;
when the session's requests differ, the figure shows the full session.
A pool the request holds encloses the stations it reaches while it holds
it: in a hold's scope, past it while a `lease` lasts, until the `release`
that ends it, and after a `grow` of a pool held in no scope, which grows
the session's lease. After a branch or a loop a pool stays held where
every way through holds it, whichever hold took it on each.

| Element | Meaning |
|---|---|
| Client | The rounded box at the left, outside the server: the workload, which sends a request and waits for its reply. Its tool and think times are its own, so they are not drawn. A program has one workload, so the figure has one client. |
| Server frame | The frame around everything but the client: the one `server` that takes a request and gives its reply. The arrival leaves the client and crosses the server's wall; every exit crosses it back to the client. |
| Station | A stage reached by a `run`; an array is labelled with its size. Consecutive prefill and decode runs at one engine share a station. |
| Arrow | Request flow between stages, including branches and loop returns. Guards label alternatives. An exit is the reply going back to the client, along a lane under the row; one that ends a session inside the server carries its guard. A station a gateway's route ends at replies to the gateway, along a lane of its own, and the reply leaves by the gateway. Every station has an arrow in and one out: a stage nothing runs does not link, and a request goes on from every station it reaches. `serq check` and `serq draw` check it on every program (`deployment::checked_figure`) and refuse a figure that cuts a station off, naming it, as the view's fault; `tests/draw.rs` checks it over every example and tutorial program. |
| Decision diamond | A branch or routing decision before the first station or gateway: the server's own (`if (input >= max_model_len) { end; }` before `gw.route();`), or, with no gateway, a server's `choose` (`routing.sq`). |
| Gateway hexagon | A gateway (`queue gw : gateway`), named above it, where the server calls `gw.route();`: the route's `choose`s are written in it, its branches leave it, and the stations it ends at reply to it. A gateway the route passes through without branching is drawn all the same (`vllm_nixl_push.sq`). |
| Filled panel | An instance selected by `choose`, grouping the engine, NIC and pools indexed by exactly its variable. One the session also indexes otherwise (`E[j]`, `E[0]`) is in no panel; a panel takes in a dashed enclosure only when all of it stands inside. An instance's pool is drawn only inside its panel, so a station outside it does not show that hold; a run between stations the panels would pull apart is drawn outside them. |
| Stacked panels | An instance's members, one panel each (`choose i in NP`: `NP` of them, its families' smallest size), stacked down from the first; past eight, the figure says how many it drew. An arrow into the instance turns down beside the stack to each member, one out of it from each; an arrow between two stacked instances goes from each member to each. |
| Dashed enclosure | A pool held across several stations. |
| Unfilled frame | Pools held at one station, listed below its glyph. |
| Transfer bracket | A job using both endpoints' NICs, an arrow inside it from each station to the next the run names; its label names the source and destination KV. |
| `pull` / `push`, a dot on the bracket | Who posts a KV relation's copy (`kvt.nixl_pull`, `kvt.nixl_push`): the dot stands over the poster's NIC, and the word leads the copy's label. A pull is posted at the arrow's head, where the bytes arrive; a push at its tail. Between two stacked instances the copy has no bracket, and a dot stands over each member's NIC. A `transfer on` written by hand names no poster and has neither. |
| Latency label | A delay before a single transfer. A delay leading to several transfers remains a station. |

The diagram simplifies control flow. Consecutive visits to one station do
not add an arrow, and guard chains with no intervening station collapse.
A guard fixed by earlier constant assignments can omit an arm with no
station; parameter-controlled alternatives remain visible. Inspect the
program when you need the exact execution path.

![vLLM v1: one engine](../assets/vllm.deployment.svg)

That is `examples/multi-turn/vllm.sq`. Its request slot (`reqs`) and its
KV blocks (`kv`) are held only at `vllm`, so they are drawn in its frame.
The tool call and the next turn are the workload's, so the figure has
neither: a request comes from the client to `vllm`, and its reply goes back.

## Addressing the figure

The SVG is a picture that a page can also address. Each station's shapes
are grouped under the name the arrows call it by, `<g data-node="E"
data-stage="E">`: an array's members share the name (`E[0]` and `E[1]`
are one station, `E`), except in a stacked instance, whose stations and
pools are named by their member, `<g data-node="P[1]" data-stage="P[1]">`
(the run log's `name` and `index`, written `P[1]`); a queue glyph and a
drum carry their pool and which of the two they are, `<g
data-pool="P.reqs[0]" data-role="queue">`. A
decision diamond, a pool's tick, a gateway and the client are no stage:
they have a node name and no `data-stage`, and say which they are, `<g
data-node="#0" data-role="decision">`, `<g data-node="#1"
data-role="tick">`, `<g data-node="gw.route" data-role="gateway">`, `<g
data-node="#client" data-role="client">`. That is what
[serq-lar](https://github.com/servingQ/serq-lar) animates a
[run log](../reference/log.md) over: it finds a station by what the
program calls it, so a change of layout moves nothing but the picture. The
TikZ writer ignores the ids.

| `data-role` | on | means |
|---|---|---|
| `queue` | `<g data-pool>` | the queue glyph where holds of the pool wait |
| `drum` | `<g data-pool>` | the pool's drum |
| `decision` | `<g data-node>` | a decision diamond |
| `tick` | `<g data-node>` | a pool's queue tick (`queue every`) a hold waits for |
| `gateway` | `<g data-node="gw.route">` | a gateway, where the server calls its route |
| `client` | `<g data-node="#client">` | the client, where sessions come from and replies go back to |
| `server` | a `<rect class="server">` | the server's frame, around all but the client |
| `arrival` | a `<path>` | the arrow sessions arrive by, `data-from="#client"`; `data-to` names the node it reaches |
| `flow` | a `<path>` | an arrow sessions take from `data-from` to `data-to`: the next station, a loop's return, a transfer between two NICs |
| `reply` | a `<path>` | the reply, from `data-from`, a station a gateway's route ended at, back to the gateway `data-to` |
| `exit` | a `<path>` | the reply, from `data-from` back to the client, `data-to="#client"` |
| `budget` | a `<rect>` inside `<g data-stage>` | the used part of an engine's budget bar |
| `bracket` | a `<rect>` | the stations one job holds at once (a transfer over two NICs); `data-nodes` lists them, and a `flow` arrow joins each to the next in the run's order |
| `instance` | a `<rect class="instance">` | an instance's panel, what one `choose` picks: `data-nodes` lists the stations in it, in no order, a stacked instance's member's by the member (`P[1] P.nic[1]`). An editor reads what is in a panel here, not from where things stand |

A KV relation's copy also says who posts it, on its bracket and on its
`flow` arrow:
`data-copy="pull"` or `"push"`, and `data-poster` the poster's node
(`D.nic[1]`). The dot is a `<circle class="poster">`.

An arrow's ends name a station by its stage, and a decision or a tick, which
is no stage, by its place among the figure's nodes: `#0` is the first. The
place is the layout's, so the name holds within one figure, not across
programs. A page can so walk a request from one station to the next along
the arrows it would take, through the decision between them, and across a
bracket as one place. A loop's return among the flows is the one painted
`class="back"`, which the layout decides (`a >= b` in the row). The client,
no stage either, is `#client` (its box reads `client`): a name that starts
with `#` is no stage's, so a stage called `client` is another node. A
gateway is named by the call the server makes, `gw.route`, the same in
every figure of the program: a gateway queue with an engine has a stage
`gw`, and no stage is called `gw.route`.

## Glyphs

| IR | Glyph |
|---|---|
| `Fifo(c)` | circle, `FIFO`, the server count when `c ≠ 1` |
| `Ps(φ)` | circle, `PS`, with `φ` beneath |
| `Delay` | rounded box with a duration-density glyph: decaying for `~exp` and `~h2`, a hump for `~erlang`, flat for `~uniform`, a spike for a deterministic duration. Other or long expressions use a row of small circles for infinite servers |
| `Step { … }` | rounded box with a token-budget bar: an iterating engine, not a queueing station. It is labelled `engine` and `tokens cap B` in the engine form's words, the residents' totals as `running.count`, `running.decoding` and `running.kv_decode`; the IR keeps no form, so a step stage read from IR is drawn the same |
| a pool | a drum, its capacity and options written beside it (`cap 8192 · block 16`) |
| a pool held at one station alone | a row in the station's unfilled frame, under its glyph: drum, name, options |
| a pool held across several stations | dashed rounded box, the drum and options in the column at its left |
| a pool a hold caches in | a grey strip under its row, or along the bottom of its box |
| the queue a hold waits in | ahead of a dashed box, one per hold rather than per pool: a hold of several pools joins the queue of its first. A frame draws none: its pools are taken at its entrance |
| a step stage's `memory` | `pool X on S's device` as the pool's title: the engine form's pool on the device, which the IR does not name; `admitted by T` among its options when a stage `T` admits it |
| a pool `S` admits (`admit_via` `S`: `pool X on S`, `on S.D`) | `pool X on S` as the pool's title, unless it is `S`'s memory; from a dashed box, also a dashed edge from the queue to `S` |

A pool's eviction order is drawn only where something is cached in it: an order
over an empty cache says nothing.

### Which pool holds the cache

A hold with a `growing` run caches in that pool alone. A hold without one
caches in all its pools. The cache strip follows this behavior, rather
than assuming every cached pool represents KV memory.

## Nesting

![The paper's two-resource replica](../assets/replica.deployment.svg)

`replica.sq`'s workload holds `live` for a session's whole conversation,
and its server holds `reqs` and `kv` only around the engine. A request is
served inside the workload's hold, so `live` is drawn too. All three are
held at `llm` alone, so all three are rows in its frame.

## A program that holds nothing

![Four replicas and a migration link](../assets/routing.deployment.svg)

`routing.sq` has no pools at all. The figure therefore has no pool enclosures. The `choose j of 4` under `rep[j]` is the routing policy.
