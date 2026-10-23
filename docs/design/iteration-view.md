# The iteration view

An engine of a run, drawn iteration by iteration: across, its iterations;
down, the requests in them; a cell the tokens a request took, coloured by
whether it prefills or decodes; under them, the iteration's tokens against
its `tokens cap` and the engine's memory against its cap, with the
preemptions and the evictions marked. `serq draw --log LOG` draws it from
the log `serq run … --log LOG` wrote (#22). No IR change; the log gains
fields a reader may ignore, so `LOG_VERSION` stays 0.

## Before

What the tutorial could show of the cliff was six numbers per rate
(`docs/tutorial/06-the-cliff.md`, two rows of its table):

```
| sessions/s | from cache | TTFT (s) | preemptions | `stuck` | utilisation |
| 1.9 | 0.150 | 1.532 | 3,659 | 0 | 0.895 |
| 2.0 | 0.032 | 25.124 | 6,484 | 0 | 1.000 |
```

They say that the engine fell over, not why: whether its batch empties,
which requests are preempted, whether it runs out of tokens or of blocks.
The deployment view could not answer either: it is a function of the IR
and runs nothing ("The deployment view does not show an
iteration-by-iteration trace", `docs/visualization/index.md` before this
change). The log of #602 had an `iter` per iteration, and #728 gave it each
request's tokens, the budget and the memory's `used` and `cached`; what an
iteration evicted and the cap the memory is measured against were in
neither the log nor anything a reader of it had.

## After

`docs/assets/cliff-2.0.iterations.svg` and `cliff-1.5.iterations.svg`, in
the tutorial's new section and on `docs/visualization/iterations.md`. At
2.0 sessions/s the 120 iterations from t = 591 take 249 of their 2048
tokens on average, hold at most 9 requests of 16, and keep the KV pool
allocated to 3381–4000 of its 4000 blocks with at most 618 cached; half
the columns evict, a request is preempted, and the 17 prefill columns,
which recompute what the cache lost, take 95 % of the time. At 1.5 the
same window is three requests and a pool of cache. The engine at the
cliff is short of blocks, not tokens: the answer the table could not give.

## Decisions

**serQ draws it, not serq-lar.** serq-lar replays the log already, but its
rule is that a figure is serQ's (serq-lar `docs/upstream.md` U2: "One
figure, the same in the docs and the app"), and the tutorial needs the
figure as a committed asset a test regenerates (criterion 3), which a page
outside this repository cannot be. The view is `view::iterations::figure(&Log,
…)`, which serq-lar's wasm crate can call on the log it holds.

**Its input is the log, not the program.** The deployment view is a
function of the IR; this one is a function of a run. Drawing it by running
the program again (`run --draw`) would make the run exist to draw; drawing
it from the file `run --log` wrote keeps one run, one record, and any
number of figures of it, as #22 asked: the figure reads the record
`run --log` wrote and does not run the simulation.

**`serq draw --log LOG`, with no FILE.** Rejected: a positional `FILE.json`
told apart by its contents (an IR and a log both have a top-level
`version`); a `--view` flag on `draw FILE` (the figure is not a view of
FILE); `run --draw` (above). Its options `--engine`, `--from` and
`--iterations` are read only on this path and are not reserved in
`args::OPTIONS`: that list reserves names because a program's inputs share
the command line with the options, and no program shares this one, as
`fmt --check` does not.

**The name is the iteration view.** "Trace" already names the workload's
arrival trace (`--trace F`, `--inline-trace`); the log's event is `iter`,
and the axis is iterations.

**The log carries what the figure needs, and nothing it can compute.**
`iter.evicted` is the memory pool's `evicted_units` so far: a counter, not
an event per eviction, since the run at 2.0 evicts 21.9 million units in
600 s (1.4 million blocks), and a counter needs no rule for which iteration
an eviction belongs to. It is the report's figure, which a warm-up does
not reset. The header gives each engine stage its `memory` pool and each
pool its `cap`, so the strip's cap comes from the same file as its `used`
and `cached` and cannot be read against another program (criterion 0).
`tokens cap` was already the log's `budget` (#728): it is an expression a
reader could not evaluate. The three are in `Memory`, one value with
`used` and `cached`: present together, absent together for an engine
with no memory pool.

**The mode and the preemptions come from events that were there.** A
cell's colour is the mode of its session's last `run`, the rule
`tools/serq_iter_trace.py` reads (a run's mode is fixed from its start,
and a `run E decode (…)` outside any engine request has no `status`); on
the cliff run at 2.0 it agrees with the `status` events' mode for every one
of 1 178 655 cells. A cross is a `preempted` status at this engine, or a
`preempt` from its memory pool of a request in its last batch (an engine
whose entries take no request), placed at the engine's next iteration:
in that run every `preempted` status comes just before an `iter` of its
engine at the same instant.

**A cell is the share of the cap.** Its height is its tokens over the
iteration's `tokens cap`, at least a hairline: a decode is a line and a
prefill chunk a block, one meaning for both colours. Rejected: decodes as
full cells, which would make height mean tokens in one colour and nothing
in the other. Without a cap, the window's largest iteration is the scale,
and the figure says so.

**The window is a count of iterations from a time.** `--from T` and
`--iterations N` (120): the issue asks for iterations across, and their
rate varies (276 per second at 2.0, 820 at 1.5 on the cliff's program), so
a count gives figures of one width where a span of time would not. A
window past the end is an error naming the last iteration, not an empty
figure; a truncated log is drawn as far as it goes, and says so.

**The reader lives beside the writer.** `Log::from_json` reads typed
records, refuses another `version`, passes over an event it does not know,
and names statuses, transfers and modes back through the tables the writer
uses (`Status::ALL`, `Transfer::ALL`, `Moved::ALL`, the modes): one list,
one place. A log written before `each`, `evicted` or a pool's `cap` is
refused with the advice to run again. Every example's log reads back equal
to what was written (`tests/log.rs`).

**A program's figure does not move.** The new colours are defined only in
a figure that paints one (`Figure::of_a_run`), so every
`docs/assets/*.deployment.svg` stays byte for byte as it was.

## Self-critique

- The axis counts iterations, not seconds: a prefill of 40 ms is as wide
  as a decode of 0.3 ms. At 2.0 the 17 prefill columns are 95 % of the
  window's time and 14 % of its width. The times under the strips say so,
  every twentieth column; an axis in time would make the decodes invisible
  instead.
- A row is a session, as the log's `reqs` names it, not an engine call: two
  calls of one session in the window share a row.
- The queue that makes the TTFT 25 s is not drawn: the figure is the
  batch. Nor is `reqs cap`, the slots, which the reader takes from the
  program.
- An eviction is a mark of presence, not of size.
- `--log` means write in `run` and read in `draw`: one file, two
  directions. The file is the same; the alternative names (`--from-log`)
  would be a second word for it.
- The vLLM overlay the issue names (serQ's iterations and vLLM's
  scheduler steps on one axis, the divergence coloured) is not here: it
  needs `tools/vllm_oracle.py` to record each step's batch in `iter`'s
  shape (it records a count of steps, `"steps": step`), and that needs a
  run of vLLM's scheduler, which this change could not make. It is a
  follow-up.
- A log is large (41 MB for 600 s at 2.0, 88 MB at 1.5); the figure reads
  all of it. A columnar log, or a window read lazily, would be the fix if
  that bites.
