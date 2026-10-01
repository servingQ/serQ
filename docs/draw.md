# `serq draw`: a program as a figure

**Status: experimental.** The command works and every program in `examples/`
and every IR file in `tools/oracle/` renders under `make check`, but the
notation, the flags and the output are not stable, and `--format svg`/`tikz`
output may change between releases. §5 lists what is not done. Design
discussion: [RFC #1](https://github.com/vrvrv/serQ/issues/1).

```
serq draw FILE [--format tikz|svg] [--out PATH] [--set name=expr]... [--def name=expr]...
```

`FILE` is program text (`.sq`) or IR (`.json`), as for `run`, `check` and
`ir`. Output goes to stdout unless `--out` names a file. `--set` and `--def` apply to
program text and are rejected on `.json`, where the constants are already
folded and the definitions expanded.

## 1. Why the figure is generated

`pyncd` (MIT Zardini Lab) writes a deep learning model as an algebraic term
and derives the PyTorch code, the backward pass and the diagram from it. The
diagram cannot drift from the model, because it is not a second description of
the model: it is the same term in a second notation.

`docs/ir.md` makes the same argument for serQ, and names the failure it fixes:
before the IR, the vLLM request program had three hand-kept copies. A figure
drawn by hand would be a fourth.

The view is a pure function of `ir::Program`. It runs no simulation, draw
no measurements, and are deterministic: the same IR gives the same bytes.
It adds no a type to `src/ir.rs`, so `IR_VERSION` is unaffected.

## 2. The deployment view

The program as a queueing network.

Pools and stages are declared, but the arrows are not — the flow is a property
of the session program. `deployment::project` walks it carrying a hold stack:

| | |
|---|---|
| **Nodes** | one per stage a `Run` reaches (`Node::stage` is `Some`); a `CRef` with `count > 1` is one node labelled `[N]`. A loop that decides before its first station adds a decision ◇, a node with no stage (`Node::stage` is `None`, `kind` is `Decision`) |
| **Edges** | the successor relation on `Run`s in session order, threaded through `Branch` (both arms) and `Loop` (a body that decides before its first station - several first stations, or an `end` before any - starts at a decision ◇, named by the `choose`s it makes, which every turn comes back to and which an `end` before any station leaves from; any other body is walked twice, so its last stations lead back to the station it starts at). An arrow forward past other stations, and an entry past the first, run in a lane below the row rather than through them |
| **Enclosure** | every `Run` is tagged with the `Hold`s around it; a group of stations sharing a hold on pool `p` becomes `p`'s dashed box: the units it holds there. A leased pool stays on the stations after its hold and a `Release` takes it off. A `choose` picks an instance, drawn as a solid box around what the session indexes by its variable; inside one only its own pools are boxed, and a run between two instances (`examples/pd-disaggregation/llmd_nixl_pull.sq`'s read) is an arrow between their boxes carrying what it moves and the link's `latency`, where pool boxes would cross. A hold whose units are the constant 0 only reserves (`reqs (0) reserve (1)`, a request parked without a running slot), occupies nothing, and draws no box. A pool held at one station alone - at it and at neither station beside it - is that station's: the station is drawn in a solid frame with the pool's slot grid and options under its glyph, and no dashed box (`Net::resident_pools`) |
| **Edge labels** | a `Branch` guard, via `Program::show_expr` |
| **Ends** | `CArrival` labels the in-arrow, `End` the out-arrow |

Two runs at the same stage in a row are two visits, not a flow, and are not
drawn. A chain of guards that moves nobody (`routing.sq` has five sibling
`branch (policy == k)` blocks) collapses to one edge rather than multiplying
out. A `choose` annotates the station whose reference reads the attribute it
names — `rep[j]`, not whatever station happens to come next.

### Glyphs

| IR | Glyph |
|---|---|
| `Fifo(c)` | circle, `FIFO`, the server count when `c ≠ 1` |
| `Ps(φ)` | circle, `PS`, `φ` beneath |
| `Delay` | rounded box of small circles — infinitely many servers |
| `Step { … }` | a rounded box with a token-budget bar. The lecture has no glyph for this one: `docs/language.md` §4 calls the colocated engine the one stage kind the lecture could not express |
| a pool held at one station alone | a row in the station's solid frame, under its glyph: slot grid, name, options |
| a pool enclosing several stations | dashed rounded box, with its options stacked in the column at its left |
| finite `cap` | a slot grid: `cap` cells when `cap ≤ 32`, schematic above that — `kv` at 160 000 is not 160 000 squares |
| a pool a hold caches in | a grey strip under its row, or along the bottom of its box |
| the queue a hold waits in | one queue glyph per hold, not per pool: a hold of several pools joins the queue of its first (`interp.rs` `enqueue_hold`). At a frame's entrance, named by the pools it takes there (`reqs + kv`); otherwise ahead of the first pool's box |
| `admit via S` | `admit via S` among the pool's options; from a dashed box, also a dashed edge from the queue to `S` |

A pool's eviction order is drawn only where something is cached in it: an
order over an empty cache says nothing.

Which pool a `cache` clause leaves units in follows `interp.rs::release_hold`: a
hold with a `growing` run caches in that pool alone, and one without caches in
all of its pools. `replica.sq` is the case that makes the difference visible
— its `hold batch (1), kv (…)` really does keep a unit of `batch` cached.

## 3. Formats

`--format tikz` (default) writes a `tikzpicture` that needs `\usepackage{tikz}`
and nothing else, with the colours it uses defined above it. It is the default
because a figure in a LaTeX document is best TikZ source the document can
`\input`: an image inherits neither the document's fonts nor its rules, and
does not diff.

`--format svg` writes a standalone SVG. Colours are written as presentation
attributes with a `prefers-color-scheme` override, not as CSS custom
properties, because `librsvg` and `cairosvg` ignore `var()` and paint the
result black.

`tsncd`, the renderer behind `pyncd`, has no TikZ backend: it draws SVG into
the DOM with KaTeX and captures with `html-to-image`, and its figures reach
documents as images. That is right for notebooks and web pages. The TikZ
backend here is a divergence justified by the target artefact, not an
imitation. What is copied from `tsncd` is the layer that makes a second writer
cheap — one `Figure`, two writers, and geometry tested instead of bytes.

## 4. Layout and tests

```
src/view/figure.rs     the geometry a view produces and a writer consumes
src/view/deployment.rs ir::Program -> Figure   the network projection
src/view/tikz.rs       Figure -> String
src/view/svg.rs        Figure -> String
```

`Figure` is the test surface; no writer decides a coordinate. `tests/draw.rs`
asserts on rectangles and on the projected `Net`, with golden files
(`tests/golden/`, `make draw-golden`) guarding the writers; the figures the
site shows (`docs/assets/NAME.deployment.svg`) must be what their program
(the one `NAME.sq` under `examples/*/` or `docs/tutorial/programs/`) draws now, and
`make draw-golden` rewrites them too. `make check` draws
every program in both formats, and every IR file in
`tools/oracle/`.

## 5. Not done

* **A pool held in two places gets two enclosures.** That is correct — a box
  spanning both would swallow the stations between them — but a reader may
  want to see that the two boxes are the same pool, and nothing says so beyond
  the name.
* **One station row.** A program with many stages runs off to the right
  instead of wrapping.
* **Long expressions are elided** with `~` rather than wrapped or footnoted.
* **`let` names are gone**: `cap blocks * bs` prints as `160000`. The IR folds
  constants, and the folded value is what the run uses.
* **No step-trace figure.** The figure with iterations across and residents
  down — where magnitude and the invariant `allocated + cached ≤ cap` live —
  needs a structured trace that `--dump` does not emit. It is the natural next
  one.
