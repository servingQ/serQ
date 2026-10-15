# The client, the server and the gateway in the figure

The deployment figure drew a request arriving from the figure's edge and
leaving every station that ended it for `out`. The decoder seemed to answer
someone outside the picture, and the gateway the server routes through was
not drawn at all (#705, #706). serq-lar's Flow view patched both on the page
(`web/src/flow/graph.ts` `CLIENT`, `exitId`; `EditView.tsx` `ServerNode`),
and so inherited what the figure got wrong. This document draws the client,
the server and the gateway in the figure. For the gateway it adds one IR
field, which a reader may ignore, so `IR_VERSION` does not move.

## Before

`serq draw examples/pd-disaggregation/llmd_nixl_pull.sq --workload
examples/workloads/chat_reply.sq --format svg` at v0.1.6, `d=` cut:

```
<g data-node="#0"> <polygon class="station" … />
<text …>choose j, i</text>
<path … data-role="arrival" data-to="#0" …/>
<path … data-role="exit" data-from="#0" …/>
<path … data-role="exit" data-from="D[0]" …/>
<path … data-role="exit" data-from="D[1]" …/>
<text … class="dim">out</text>
```

The diamond `#0` mixed two things: the server's `if (input >=
max_model_len) { end; }` and the route's `choose j` and `choose i`.
`vllm_nixl_push.sq`, whose route does not branch, had no gateway drawn at
all. `routing.sq`, which has no gateway, had a diamond of the same kind.

## After

```
<rect class="server" … data-role="server" …/>
<g data-node="#0" data-role="decision">…</g>
<g data-node="gw.route" data-role="gateway">…</g>
<g data-node="#client" data-role="client">…</g>
<path … data-role="arrival" data-from="#client" data-to="#0" …/>
<path … data-role="exit" data-from="#0" data-to="#client" …/>
<path … data-role="flow" data-from="#0" data-to="gw.route" …/>
<path … data-role="reply" data-from="D[0]" data-to="gw.route" …/>
<path … data-role="reply" data-from="D[1]" data-to="gw.route" …/>
<path … data-role="exit" data-from="gw.route" data-to="#client" …/>
```

- **The client** is a node left of everything else. A program has one
  workload (`duplicate workload`), so the figure has one client. Its tool
  and think times are its own and are not drawn.
- **The server** is a frame around everything but the client. A program
  has one `server`. The arrival crosses the frame's wall, and so does every
  exit on its way back to the client, along a lane of its own under the
  row. `out` is gone.
- **The gateway** is a hexagon named after its queue. The route's `choose`s
  are written in it, and its branches leave it. A station the route ends at
  replies to it (`reply`), and the request leaves by it. The server's
  statements before `gw.route();` keep their own diamond. A program with no
  gateway draws as before. A node that is no stage says which kind it is:
  `decision`, `tick`, `gateway` or `client`.

## Where the route is: `Program::routes`

The parser expands a gateway's `route` in place of the server's
`gw.route();` (`queue::expand`), so the IR had no trace of it. Yet the
figure is drawn from the IR, and `docs/visualization/deployment.md`
promises that a program, its JSON IR and a `pyserq.Program` draw the same
figure. So the IR says where the route is:

```rust
pub routes: Vec<Route>,   // serde: default, skipped when empty
pub struct Route { pub gateway: String, pub block: BlockId, pub from: usize, pub to: usize }
```

`blocks[block][from..to]` are the route's statements, and the blocks nested
in them are the route's too. The server is spliced at every `turn`, so one
gateway can have several routes. The interpreter does not read the field.
It is added and may be ignored, so `IR_VERSION` stays at 14 (`docs/ir.md`,
Stability). `Program::validate` refuses a range its block does not have.
That check is stricter but changes nothing a correct program does.

Inside the parser, `expand` puts a `Stmt::Route(Some(gw))` … `Stmt::Route(None)`
pair around the route's body, next to the `Side` pair it already put there.
The linker turns that pair into the range. `deployment::request`, which
copies the server's statements into a block of their own, moves a route
with them.

## Names

A page finds a node by `data-node`, so a name must mean one node (criterion
0).

- The client is `#client`, not `client`. A stage may be called `client`,
  and then two `<g data-node="client">` would stand in one figure (found in
  review of #707). A name that starts with `#` is no stage's, as with `#0`.
  The box itself still reads `client`.
- The gateway is `gw.route`, not `gw`. A gateway queue that also plays a
  role with an engine has a stage `gw`. No stage is called `gw.route`,
  because a queue's stages are its engine `Q` and its NIC `Q.nic`. And
  `gw.route` is what the server writes.

## Self-critique

- **A `Side::Gateway`.** Rejected. A new variant of a field the interpreter
  reads is a version bump, and it would have to say which gateway as well.
- **The figure from the text alone.** Rejected. A program and its JSON IR
  would then draw two different figures.
- **A gateway tag on every statement, aligned with `blocks` as `sides`
  is.** Rejected. It writes the gateway's name once per statement, where a
  range writes it once per call.
- **`data-node="gw"` and `data-node="client"`, as the issues wrote them.**
  Rejected for the collisions above. serq-lar must read `gw.route` and
  `#client`, and drop its own `CLIENT` node, `ServerNode` frame and
  `exitId` (its `docs/upstream.md` U20).
- **An `end` in a route replying through the gateway.** Written, then
  removed. An entry does not refuse a request; the server does (`` `end` in
  a queue's entry `` does not link). So no program reaches that branch.
- **The route's `choose`s under the hexagon.** Moved inside it. The arrows
  that leave and enter the gateway from below crossed the note.
- **What it still does not draw.** After `gw.route();` the server may go on
  (`set produced = D.produced;`). The reply is drawn leaving by the gateway,
  not passing back through the server's statements: they are no station.
