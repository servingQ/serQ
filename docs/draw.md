# `seq-lang draw`: a program as a figure

**Status: experimental.** The command works and every program in `examples/`
and every IR file in `tools/oracle/` renders under `make check`, but the
notation, the flags and the output are not stable, and `--format svg`/`tikz`
output may change between releases. §5 lists what is not done. Design
discussion: [RFC #1](https://github.com/vrvrv/seQ/issues/1).

```
seq-lang draw FILE [--format tikz|svg] [--out PATH] [--set name=expr]...
```

`FILE` is program text (`.seq`) or IR (`.json`), as for `run`, `check` and
`ir`. Output goes to stdout unless `--out` names a file. `--set` applies to
program text and is rejected on `.json`, where the constants are already
folded.

## 1. Why the figure is generated

`pyncd` (MIT Zardini Lab) writes a deep learning model as an algebraic term
and derives the PyTorch code, the backward pass and the diagram from it. The
diagram cannot drift from the model, because it is not a second description of
the model: it is the same term in a second notation.

`docs/ir.md` makes the same argument for seQ, and names the failure it fixes:
before the IR, the vLLM request program had three hand-kept copies. A figure
drawn by hand would be a fourth. `fig:deployment` in Lecture 1 §2 of
`serving-queue-theory` is exactly that — hand-written TikZ for a deployment
that `examples/pd-disaggregation/lecture_pd.seq` already defines. `tests/draw.rs` asserts that
the generated figure has that figure's topology.

The view is a pure function of `ir::Program`. It runs no simulation, draw
no measurements, and are deterministic: the same IR gives the same bytes.
It adds no a type to `src/ir.rs`, so `IR_VERSION` is unaffected.

## 2. The deployment view

The program as a queueing network.

Pools and stages are declared, but the arrows are not — the flow is a property
of the session program. `deployment::project` walks it carrying a hold stack:

| | |
|---|---|
| **Nodes** | one per stage a `Run` reaches; a `CRef` with `count > 1` is one node labelled `[N]` |
| **Edges** | the successor relation on `Run`s in session order, threaded through `Branch` (both arms) and `Loop` (a back edge to the body's first station) |
| **Enclosure** | every `Run` is tagged with the `Hold`s around it; a group of stations sharing a hold on pool `p` becomes `p`'s dashed box — **the lecture's "instance" boundary**. A leased pool stays on the stations after its hold and a `Release` takes it off, so a KV transfer draws two boxes that cross at the link station (`examples/pd-disaggregation/llmd_pd.seq`) |
| **Edge labels** | a `Branch` guard, via `Program::show_expr` |
| **Ends** | `CArrival` labels the in-arrow, `End` the out-arrow |

Two runs at the same stage in a row are two visits, not a flow, and are not
drawn. A chain of guards that moves nobody (`routing.seq` has five sibling
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
| a pool enclosing a station | dashed rounded box, with its options stacked in the column at its left |
| finite `cap` | a slot grid: `cap` cells when `cap ≤ 32`, schematic above that — `kv` at 160 000 is not 160 000 squares |
| a pool a hold caches in | a grey strip along the bottom of its box |
| the pool's queue | the queue glyph ahead of the box |
| `admit via S` | a dashed edge from the queue to `S` |

A pool's eviction order is drawn only where something is cached in it: an
order over an empty cache says nothing.

Which pool a `cache` clause leaves units in follows `interp.rs::release_hold`: a
hold with a `growing` run caches in that pool alone, and one without caches in
all of its pools. `replica.seq` is the case that makes the difference visible
— its `hold batch (1), kv (…)` really does keep a unit of `batch` cached.

### Against the lecture's figure

`tests/draw.rs::lecture_pd_has_the_topology_of_fig_deployment` is the
acceptance test, and it is the one place in this feature with an
independently hand-drawn answer key. Four differences are expected:

1. **Geometry.** The lecture places the tool call below centre by hand; the
   generated layout puts feedback edges in lanes below the station row.
2. **`link` moves inside the prefill box.** The figure draws it outside both
   dashed boxes; the program — and the lecture's own `L1:ex:program` listing,
   where `run link X` stands between `admit mem_P c` and
   `free mem_P cache κT` — holds `memP` across the transfer. The lecture says
   as much: *"The figure is a picture, not a definition: it does not say when
   a waiting request is admitted, what happens to its KV memory afterwards, or
   who decides a hit."* A figure read out of the holds says all three.
3. **`step` needs a glyph** the lecture has none for.
4. **Cost labels are the real expressions** — `min(n, 16)` where the lecture
   writes `φ(m)`, since the IR holds the folded expression, not the symbol.

## 3. Formats

`--format tikz` (default) writes a `tikzpicture` that needs `\usepackage{tikz}`
and nothing else, with the colours it uses defined above it. It is the default
because the figure it replaces is TikZ source inside a LaTeX document, and a
generated artefact substitutes for that only if the document can `\input` it:
an image inherits neither the document's fonts nor its rules, and does not
diff.

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
(`tests/golden/`, `make draw-golden`) guarding the writers. `make check` draws
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
