# What a pool's tick counts

ThunderAgent's `--use-acting-token-decay` (7ddc861) counts a program that
is calling a tool for `2^-t` of its tokens, `t` seconds since its
response, where the router's tick lets programs in, and in full where it
pauses them and where it places a new program at once. The paper's runs
have it on. `examples/multi-turn/thunderagent.sq` could not write it.
#591's pool gave a set-aside lease back at a tick "each that fits", judged
by the pool's own count, every lease for its units. A program could not
state the opposite (criterion 2), and ThunderAgent's code has two counts.

This change makes the count the program's: `suspend by (s, …) counting
(c)`, and `counted(m)` reads what a pool counts as taken under it. It is
IR 16 (#587). The result is a finding. Written as the code writes it, the
decay makes TR mode lose its gain at the paper's scale, where the paper
reports its best numbers with it on. Before this change the docs said the
decay "cannot be read off the program"; now it is written and measured.

## Before and after

`examples/multi-turn/thunderagent.sq` before (`9730799`), the replica's
pool and the waiting path:

```serq
  pool prog[J] {
    cap tr ? room : inf;
    preempt leases by (reasoning, prompt) every (dt) suspend by (prompt);
  }
  // --8<-- [end:prog]
  // --8<-- [start:helpers]
  def no_room(x) { max r in J (free(prog[r])) < x }
```

```serq
      set t_q = now;
      set waits = tr && (K > 0 || queued(waiting_queue) > 0 || no_room(charged));
      while (waits) {
        hold waiting_queue (cost(waiting_queue, 1)) { }
        set waits = no_room(charged) && now - t_q < force_resume;
      }
      choose j in J by (tr ? (free(prog[j]) >= charged ? 0 : 1) : 0,
                        tr ? used(prog[j]) : holders(prog[j]));
```

`docs/language.md` before: at each tick "the leases set aside come back
in the order of the `s` keys (read as the victim's are), each that fits,
to a session that neither holds nor leases the pool again; one that does
not fit is passed over."

After:

```serq
  let decay = args.number("decay", 0);       // --use-acting-token-decay: its base, 2 (0: off)
```

```serq
  def tokens_at_tick() {
    decay > 0 && !reasoning
      ? buffer + (total_tokens - buffer) * decay ^ (acting_since - now)
      : allocated(prog[home])
  }
  pool prog[J] {
    cap tr ? room : inf;
    preempt leases by (reasoning, prompt) every (dt)
      suspend by (prompt) counting (tokens_at_tick());
  }
  // --8<-- [end:prog]
  // --8<-- [start:helpers]
  def room_of(r) { ticked ? room - counted(prog[r]) : free(prog[r]) }
  def no_room(x) { max r in J (room_of(r)) < x }
```

```serq
      set t_q = now;
      set ticked = 0;
      set waits = tr && (K > 0 || queued(waiting_queue) > 0 || no_room(charged));
      while (waits) {
        hold waiting_queue (cost(waiting_queue, 1)) { }
        set ticked = 1;
        set waits = no_room(charged) && now - t_q < force_resume;
      }
      choose j in J by (tr ? (room_of(j) >= charged ? 0 : 1) : 0,
                        tr ? -room_of(j) : holders(prog[j]));
```

```serq
    if (allocated(prog[home]) > 0) { count_as(usage); }
    set total_tokens = allocated(prog[home]);
    set acting_since = now;
```

With `decay` at 0, the code's default, the reports are those of `9730799`
byte for byte. Eight runs were compared: the paper preset at N 96 (TR
and default mode), N 192 with `dt` 2, and N 144 with `w` 0.5, 25 000 s;
the example at N 16 (default mode), 32, 48, and 32 with `est` 0.2. Every
test and committed number stays as it was. With `decay` 2, the paper
preset at N 96, seed 1, serves 39 298 turns in 20 000 s after warm-up
(118 steps/min), where `decay` 0 serves 105 125 (315).

## The rule

At each tick of a pool under `suspend by (s, …) counting (c)`, before it
pauses:

1. The pool takes its room once, as the tick begins: its cap less
   `counted(m)`. `counted(m)` is the pool's units, with each lessee's
   allocation replaced by `c` read for it. A lessee is a session that
   leases the pool and holds it in no scope, and `c` is read for it as a
   victim key is (`Moment::Victim`).
2. The leases set aside are tried in the order of the `s` keys. One whose
   units fit what is left of the room comes back and takes its units from
   it. One that does not fit is passed over.
3. What comes back may take the pool past its cap. The same tick's pause,
   which reads the units, then sets the excess aside again.

A `c` read below 0, or not a number, fails the run. A count below 0 would
make room the pool does not have, and a NaN would let every lease back,
since the comparison would be false. `inf` is allowed: such a lease lets
no other back. `counting (allocated(m))` is #591's rule exactly: the room
is `cap - used`, and each lease that comes back fits the cap. Since
`counted` subtracts each lessee's own allocation (`allocated_by`), that
holds bit for bit, prefix sharing included.

This is ThunderAgent's tick, read at
[7ddc861](https://github.com/ThunderAgent-org/ThunderAgent/tree/7ddc8610270e56d3b109eed8796b3a4360fc67c9):

- `_scheduled_check` runs `_greedy_resume`, then `_pause_until_safe` on
  each backend whose `remaining_capacity()` is below 0
  ([`scheduler/router.py#L759-L771`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L759-L771)).
- `_greedy_resume` takes each backend's room once, by
  `remaining_capacity_with_decay()` when the flag is on
  ([`#L833-L837`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L833-L837)).
  Each program it places then takes its `total_tokens` and a buffer from
  that room
  ([`#L889-L926`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L889-L926)).
- `remaining_capacity_with_decay` counts each ACTING program's
  `total_tokens` times `2^-(now - acting_since)`, the reasoning programs'
  tokens whole, and a buffer for each program
  ([`backend/state.py#L196-L214`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/backend/state.py#L196-L214)).
  `acting_since` is set at the response
  ([`scheduler/router.py#L475-L477`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L475-L477)).
- A new program placed at once is judged by `remaining_capacity()`, the
  full count
  ([`#L330-L362`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L330-L362)).

`_greedy_resume` judges the whole global waiting queue by the decayed
room. That queue holds the paused programs with a request pending and the
new programs, as well as the paused ones without
([`#L851-L866`](https://github.com/ThunderAgent-org/ThunderAgent/blob/7ddc8610270e56d3b109eed8796b3a4360fc67c9/ThunderAgent/scheduler/router.py#L851-L866)).
So the example reads the decayed room in two places. The pool's `counting`
judges the comeback of a paused program with no request pending. A
program that a tick has let through `waiting_queue` (`ticked`) reads
`room - counted(prog[r])`. A program placed at once reads
`free(prog[r])`. `tokens_at_tick()` counts `total_tokens`, the lease
before the acting weight `w` shrinks it, because ThunderAgent weighs
`state.total_tokens` and not the weighted count.

## The checks

- `suspend by (…)` without `counting (…)` is a parse error that gives its
  reason. A clause whose default could be left out is two spellings.
- `counting` is checked as a victim key is: read at `Moment::Victim`, no
  draw, no `computed`, an ordinary quantity and not a resource cost. The
  three checks over a pool's expressions now read one list,
  `Preempt::exprs()` (the victim keys, the `suspend` keys, the count).
  #591 added its `suspend` keys to two of the three lists and not to
  `declaration_types`. Because of that, `suspend by (cost(prog, prompt))`
  linked and ran on `9730799`, while the same expression as a victim key
  was refused. `tests/lease_ticks.rs`
  (`a_counting_is_written_and_read_as_a_key_is`) reproduces the gap as it
  was found.
- `counted(m)` does not link in three places:
  - on a pool without `counting`, where it would be `used(m)`;
  - in a pool's keys or `counting`, where a `counting` would read itself;
  - in a gauge, which has no session and is held between instants, while
    a `counting` may read `now`.
- An engine's `only` or its scheduler's guards may not read `counted(m)`
  of a pool whose `counting` reads `now` or `work(…)`. Through `counted`
  they would read the clock, which #263 refuses (`reads_clock` follows
  the call).

## Evidence

All runs use the paper preset, 25 000 s with 5 000 s of warm-up, 4 seeds,
`tools/thunderagent/sweep.py --preset paper decay=…`.

| `decay` | TR steps/min, N = 96, 144, 192 | ratio | TR token hit | resumed/turn | paused/turn | prefilled/turn |
|---|---|---|---|---|---|---|
| 0 (off) | 319.7, 319.1, 318.5 | 3.18-3.21 | 0.93 | 0.05-0.06 | 0.07-0.08 | 1 260-1 295 |
| 2 | 113.8, 102.6, 102.6 | 1.03-1.13 | 0.24-0.34 | 0.63-0.81 | 1.26-1.40 | 12 007-13 920 |
| 4 | 111.1, 101.1, 101.3 | 1.02-1.10 | 0.24-0.32 | 0.65-0.83 | 1.38-1.49 | 12 406-14 077 |
| 6 | 109.7, 101.3, 100.8 | 1.01-1.09 | 0.23-0.32 | 0.66-0.83 | 1.42-1.51 | 12 564-14 121 |
| 8 | 109.8, 101.0, 100.7 | 1.01-1.09 | 0.24-0.32 | 0.67-0.83 | 1.43-1.52 | 12 566-14 119 |

At N 96 with base 2, `dt` 2, 5 and 10 s serve 111.8, 113.8 and 130.8
steps/min. The paper's Fig. 6b reports 363, 378 and 343, and 373, 382,
360 and 333 at bases 2, 4, 6 and 8. At the example's configuration,
where a tool takes 3 s on average, base 2 costs less: 27.0, 26.0, 24.5
and 23.1 steps/min at 8, 16, 32 and 48 agents, against 28.9, 28.8, 27.7
and 27.0.

The effect is in the waiting path, not in the comeback. Two variants
show it, each at base 2 for N 96, 144 and 192:

- With the decay only in the pool's `counting`, `room_of(r)` being
  `free(prog[r])`, TR mode serves 319.9, 319.2 and 318.8.
- With `counting` at 0 as well, so that every set-aside lease that fits
  the cap comes back, it serves 319.9, 319.2 and 318.7.

Decaying the waiting path as well lowers it to 113.8, 102.6 and 102.6.
A program a few seconds into its tool counts for almost nothing, so each
tick lets in nearly every program that waits. That is more than the
replica's KV holds, and the pause, which counts in full, sets the excess
aside at once. Turns from a paused program then find their prefix
evicted.

This change does not settle why the paper's runs differ. The candidates
are what the program does not write: ThunderAgent's placement at a tick
(largest first, onto the backend with the most room, and moving a paused
program), the reasoning programs it marks for a pause and counts until
their responses ([ThunderAgent](thunderagent.md#what-the-program-still-does-not-write),
items 1 and 2), and how long the paper's tools take.

## The IR

`Preempt::Leases.suspend` changes from `[CExpr]` to `Suspend { keys, count
}`, and `Fun::Counted` is new: IR 16. The field is retyped, and an old
reader would have to honour the count, which it cannot read. A pool whose
preempt is not `lifo` is outside the Lean fragment, and so is a call the
generator does not know. No oracle program has either, so a 15 file and
a 16 file translate alike (`scripts/gen_lean_oracle.py`).

## Self-critique

**(a) `shrink` the acting programs' leases by the decay.** `shrink`
lowers the count everywhere, at the pause and at placement too. That is
`w`, `--acting-token-weight`, which #574 already measured: a ratio of 2.6
at `w` 0.5, through churn. It is not the decay, which only the tick's
let-in reads.

**Give back every set-aside lease, then let the pause trim.** This is
the issue's reading of item 4. It would be a second fixed rule, and a
program could not state its opposite (criterion 2). It is also not the
code's rule: under the decay, a replica full of reasoning programs
resumes nothing. For the comeback, the variant above with `counting` at
0 serves the same as the decayed count.

**A room expression over a replica's sessions.** A program would sum
its own weights over the pool's lessees. No expression can aggregate over
sessions, so this needs a new `CExpr`, which every consumer pays for.
`counted` is a function of a pool, whose argument the per-lessee
expression already is.

**`free(m)` reading the count on such a pool.** Then `free` would mean
two things (criterion 0). Placing a program at once still needs the full
count (`remaining_capacity()`).

**An optional `counting`, defaulting to the units.** A clause whose
default can be written or left out is two spellings. Only
`thunderagent.sq` and tests wrote `suspend`, and each now writes
`counting (allocated(m))` or its own count.

**A weight fixed when the lease is written.** The decay is a function of
the tick's time, not of the moment the program leased, so the count is
read at the tick.

**Counts below 0.** They would make room the pool does not have, so the
run fails. A count above the units is allowed: it is a pessimistic
policy whose opposite can be written.

**Re-reading the room after each comeback.** A lease that comes back is
a lessee, and a re-read would count it as `c` reads for it, its decayed
count. ThunderAgent subtracts the full `total_tokens` and buffer from
the room it took once, so a comeback takes its units.

**(c) Closing the issue without writing the decay.** The docs' question,
whether the decay explains the paper's numbers, would stay unanswerable.
Written, it has an answer: not as the code writes it, here.

`counted(m)` equals `used(m)` when the count is `allocated(m)`. That is
two values coinciding, not two spellings. `counted` reads what the
program's count says, and a program that counts each lease for its units
reads `used` by its own choice. `counted` on a pool with no `counting`,
where the two would mean the same, does not link.
