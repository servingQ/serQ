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
| **Nodes** | one per stage a `Run` reaches; a `CRef` with `count > 1` is one node labelled `[N]` |
| **Edges** | the successor relation on `Run`s in session order, threaded through `Branch` (both arms) and `Loop` (a back edge to the body's first station) |
| **Enclosure** | every `Run` is tagged with the `Hold`s around it; a group of stations sharing a hold on pool `p` becomes `p`'s dashed box — an instance's boundary. A leased pool stays on the stations after its hold and a `Release` takes it off, so a KV transfer draws two boxes that cross at the link station (`examples/pd-disaggregation/llmd_pd.seq`) |
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
