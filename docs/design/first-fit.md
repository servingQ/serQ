# First fit: a waiter that does not fit, passed over

Issue #65. A pool's queue admits the waiter its order selects, while it
fits. When the selected waiter does not fit, the admission stops and every
waiter behind it waits too: head-of-line blocking. #8 found this to fail
criterion 2: it was a rule of the language, and a program could not state
the opposite, "pass over a waiter that does not fit and admit the next one
that does". `queue … first fit` is that opposite. Head-of-line blocking stays
the default, because every engine and paper program here keeps it.

## Before

Three places held the rule, at `9730799`. A pool's own admission,
`try_admit` (`src/engine/interp.rs:3486-3502`), stopped at the selected
waiter:

```rust
    /// Admit the selected waiting hold while every pool of its hold
    /// has room; a selection that does not fit blocks the rest. A pool whose
    /// queue is served by a stage (`admit_via`) is admitted from there.
    fn try_admit(&mut self, pl: usize) {
        // an engine admits its queue; a pool under `queue … every`, its ticks
        if self.p.pools[pl].admit_via.is_some() || self.p.pools[pl].queue_every.is_some() {
            return;
        }
        while let Some((index, sid)) = self.next_waiter(pl) {
            let pending = self.pending_now(sid);
            if !self.fits_all(&pending) {
                break;
            }
            self.pools[pl].queue.remove(index);
            self.admit(sid, pending);
        }
    }
```

An engine's iteration, `admit_bound` (`:6062-6066`), returned `false` at
the same place:

```rust
            let pending = self.pending_now(sid);
            self.admit_budget = None;
            if !self.fits_all(&pending) {
                return false;
            }
```

A pool under `queue … every (dt)` (#591), at its tick, `queue_tick`
(`:4759-4775`), did the opposite:

```rust
    /// A tick of `pl` under `queue … every`: the holds that waited when it
    /// began are tried in the queue's order, each admitted one running
    /// until it blocks before the next is tried, one that does not fit
    /// passed over. A hold queued during the tick waits for the next.
    …
            let pending = self.pending_now(sid);
            if !self.fits_all(&pending) {
                continue;
            }
```

So the fit rule was tied to whether the pool ticks: a pool without ticks
could not pass over, and a pool with ticks could not block. The language
said both (`docs/language.md`, Waiting selection): "The chosen head that
does not fit blocks the rest", and of the tick "one that does not fit is
passed over, not waited behind".

What a program could do was imitate first fit with a key that reads the
pool's free units. In the tutorial's memory program
(`docs/tutorial/programs/02-memory.sq`) this links, since the keys are
read again before every attempt:

```serq
pool mem { cap C; queue by (items > free(mem) ? 1 : 0); }
```

The key restates the hold's units. It is exact for that pool only because
the hold is one pool with no `block`, no `reserve` and no cached prefix.
Each of those the key would have to compute again, which puts one rule in
two places and has the program say how instead of what (criterion 1).

## After

A modifier on the queue clause, after the order and before `every`:

```serq
pool mem { cap C; queue fifo first fit; }
pool mem { cap C; queue by (k₁, …) first fit; }
pool gate { queue by (k₁, …) first fit every (dt); }
```

`docs/tutorial/programs/02-first-fit.sq` is the tutorial's memory program
with that one line changed. The other order is refused with the fix:

```text
`first fit` says which waiter the queue admits, at a tick as at any other time, so it comes with the order, before `every`: `queue … first fit every (dt);`
```

The interpreter has one helper, `first_fitting`, which the pool's own
admission and an engine's iteration both call. It selects as before. When
the waiter does not fit, without `first fit` it returns nothing, as the
`break` and the `return false` did. With `first fit` it passes the waiter
over and selects again from the rest, the keys read again. A tick keeps
its own list of the waiters it has tried, and the `continue` became a
`break` unless the pool says `first fit`.

## Semantics

- **Each attempt reads the queue from its head.** An admission attempt
  (each `try_admit` round, each admission of an engine's iteration) selects
  from the whole queue, keys read again and a preempted hold first, as
  without `first fit`. What it passed over it forgets when it returns.
  This is the smallest rule, and it is the rule the keys already follow,
  which is why the key imitation above and the clause give the same run.
  vLLM keeps a per-step list of the requests it skipped
  (`step_skipped_waiting`, `scheduler.py:911-924`). The two differ only
  when a waiter's need shrinks within one iteration, as units that read
  `budget_left` with no `reserve` do, and no program here writes `first
  fit` on a pool an engine admits.
- **Every pool the hold names.** A waiter is passed over when it does not
  fit every pool its hold names (`fits_all`), as it blocks without the
  clause. The clause is on the queue's pool: the pool whose queue the hold
  waits in.
- **A tick.** Without `first fit`, the first that does not fit ends the
  tick, as it stops an admission at any other time. With it, it is passed
  over, and each waiter is tried once a tick, as #591 wrote it.
  `examples/multi-turn/thunderagent.sq`'s `waiting_queue` has no cap, so
  its holds always fit and its runs do not move.
- **An engine.** `pool kv on llm.gpu { queue fifo first fit; }` is allowed.
  A queue none of whose waiters fits is passed over for the stage's next
  pool. `serq target` refuses it: vLLM stops at the first that does not fit.
- **A waiter that never fits.** One whose units read the deployment's state
  is not rejected when it joins (#364). Under head-of-line blocking it is
  the head the others wait behind, and the report names the head that asks
  more than the cap (`over_cap`). Under `first fit` the others pass it, and
  it would wait to the end unseen, so the report reads every waiter of such
  a queue. No static check can tell, since its units read the state.

## The evidence: what engines do

Every engine and paper program here stops at the first that does not fit:

| System | At a request that does not fit | Where |
|---|---|---|
| vLLM | `if new_blocks is None: … break` | `scheduler.py:1228-1235` |
| vllm-ascend `v0.27.1rc1`, the recompute scheduler | the same `break` | [`recompute_scheduler.py` L808-L815](https://github.com/vllm-project/vllm-ascend/blob/v0.27.1rc1/vllm_ascend/core/recompute_scheduler.py#L808-L815) |
| TensorRT-LLM `bf414e37`, `GUARANTEED_NO_EVICT` | "If one requests fails to be scheduled, break" | [`capacityScheduler.cpp` L442-L446](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L442-L446) |
| TensorRT-LLM, `MAX_UTILIZATION` | pauses a started request and tries again; with none to pause, `break` | [L591-L616](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L591-L616) |
| DeepSpeed-MII `8abdd98` | `if req_tokens < len(r.input_tokens): break` | [`ragged_batching.py` L306-L307](https://github.com/microsoft/DeepSpeed-MII/blob/8abdd987421988a9d50d8b1dfa71ca6283a30f6c/mii/batching/ragged_batching.py#L306-L307) |
| MC-SF (arXiv 2502.07115) | "else Break the for loop" | its Algorithm 1 |
| Kong's SVF (`examples/papers/kong_svf.sq`) | the proof's Lemma A.2: every step a request waits, the head does not fit | `examples/papers/Kong.lean`, the header |

Passing over happens in three places, and none is a single pool's fit:

- vLLM passes over a request whose LoRA would exceed `max_loras`
  (`scheduler.py:911-924`); serQ does not model LoRA.
- TensorRT-LLM's prefix-aware scheduling, on by default, passes over a
  context whose first blocks a context admitted in the same pass will
  contribute ([L367-L376](https://github.com/NVIDIA/TensorRT-LLM/blob/bf414e37291b9d15a5328af99e349db8dedf7a4d/cpp/tensorrt_llm/batch_manager/capacityScheduler.cpp#L367-L376),
  `docs/design/engine-neutrality.md`): a skip for the shared cache, not
  for room.
- ThunderAgent's resume passes over a program that does not fit the total
  capacity or the largest replica
  ([`router.py` L872-L876, L901-L904](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L872-L876)
  at `7ddc861`). Its fit spans the replicas, so `thunderagent.sq` writes it
  in the program: a program that finds no room queues again for the next
  tick.

So no engine program will need `first fit`, and waiting for one, as the
issue last proposed, would leave the rule standing. Criterion 2 asks that
it be the program's: the default stays vLLM's, and the opposite is one
clause.

## The program that uses it

The tutorial's memory chapter, §Head-of-line blocking
(`docs/tutorial/02-memory.md`), runs the two programs at `cap 6`, where the
chapter's sweep shows the most waiting (seed 1, horizon 100000, warm-up
5000):

| `cap 6` | admit wait | its p99 | response |
|---|---|---|---|
| `02-memory.sq`, the head blocks | 1.5808 | 10.9918 | 2.5805 |
| `02-first-fit.sq`, `queue fifo first fit;` | 1.0687 | 8.7344 | 2.0684 |

The key imitation gives the clause's run report for report
(`tests/first_fit.rs`, `the_tutorial_runs_as_the_key_that_imitates_it`).

On this load the largest requests gain too. Split by an observation of
their own (`if (items > 4) { observe big_wait = now - t0; }`), the 5-item
requests wait 1.8453 under head-of-line blocking and 1.7726 under first
fit, the others 1.4929 and 0.8349: the small ones that pass a large one
drain the queue faster. The rule bounds nothing, though. Each attempt
starts again from the head, and a large request is passed over for as
long as smaller ones keep fitting, whatever the keys: one they put first
is passed over too. A program that wants a bound keeps head-of-line
blocking and writes the age into the key, which with the free units gives
first fit until a waiter is old:

```serq
pool mem { cap C; queue by (waited > 5 ? 0 : (items > free(mem) ? 2 : 1)); }
```

At `cap 6` this gives 1.1913 overall, the 5-item requests 1.7846 with a
p99 of 10.1664 against first fit's 10.8318. It is a key, with the key's
limits above; no program here asks for it.

## The price

- **IR 16.** `CPool.first_fit: bool`, omitted when false. An old reader
  that ignored it would run another program, and `queue_every` changes
  meaning under the same shape: its tick no longer passes over by
  itself. 15 is tagged (`v0.1.7-alpha.2`), so the version moves. Every
  committed IR file moves its version only.
- **The Lean generators** pin 16 and refuse the field: the fragment's
  `admitHeads` stops at the head (`lean/Serq/Exec.lean`). The two
  generators now read one list of the pool features outside the fragment,
  `pool_outside_fragment` in `scripts/gen_lean_oracle.py`. The claims
  generator had its own, which lacked `queue_every`: #591 taught the
  oracle generator alone, so a claim of a ticking pool would have been
  stated about a pool that admits at once. The Lean files regenerate the
  same, since no oracle or claims program writes either.
- **`serq target`** refuses `first fit` on vLLM's pools.
- **The frontend**: one keyword, `fit` (122), in `KEYWORDS`, the
  highlighter's options and the docs lexer, and one branch of the queue
  clause. `fit` was a word #152 retired, the condition of `admit if …
  fit`. It comes back with one meaning, a queue's; where it stood as a
  condition or a name it is still refused, and the message now says
  where the word lives (`tests/first_fit.rs`,
  `the_retired_fit_says_where_the_word_lives_now`). No program's report moves: every pair of `examples/pairs.json`
  gives the same JSON report as before (seed 1).

## Self-critique

- **`pass blocked;`, the issue's spelling**, as a statement of its own:
  two new keywords instead of one, and it stands apart from the queue
  whose rule it changes. `first` was a keyword already.
- **A word that was never retired.** `fit` meant a hold's condition
  before #152, so a reader of an old design record meets it twice. The
  two meanings never stand in one language, the old one is refused with
  a message that names the new, and "first fit" is the scheduling term a
  serving engineer reads without serQ (criterion 1), which a fresh word
  would not be.
- **The default written out.** `queue fifo head blocks;` or the like
  would make a second spelling of the default, which CLAUDE.md counts as
  two spellings of one meaning. The default has none.
- **A lookahead window**, `lookahead (n)`: its default, `(0)`, could be
  written, which is two spellings again, and no program wants a window.
- **The rule on each refusing pool.** vLLM's one loop passes over on LoRA
  and stops on KV, so a rule per pool that refuses would be closer to it.
  serQ does not model LoRA, and no program here has two pools with two
  rules. When LoRA is modelled this is the place to look again.
- **Refusing `first fit` on a pool an engine admits**, as `queue … every`
  is refused there: `every` is refused because two admission clocks would
  conflict, not because no engine wants it. A clause that linked on some
  pools and not others would move the criterion-2 rule rather than remove
  it.
- **Keeping the tick's pass-over fixed.** Then `first fit` on a ticking
  pool would say nothing, and a ticking pool still could not block. The
  tick's rule follows the clause; no program depended on it.
- **vLLM's per-step list of skipped requests.** It would be state in the
  interpreter that no program needs; it differs from the rule above only
  when a waiter's need shrinks within an iteration.
- **Waiting for an engine program**, the issue's own last condition. No
  engine here passes over for room (above), so it would never come, and
  the rule would stay the language's.
- **Leaving the key imitation as the answer.** It is exact only for one
  pool without `block`, `reserve` or a cached prefix, and it restates the
  units.
- **The growers' order and a suspended lease's give-back** keep their
  fixed rules: a pool's stalled growers are retried in order and the
  first that does not fit stops them (`retry_growers`), and a tick's
  give-back passes over a lease that does not fit
  (`docs/design/thunderagent.md`, self-critique). No program here asks
  for the opposite of either, so no clause selects them.
