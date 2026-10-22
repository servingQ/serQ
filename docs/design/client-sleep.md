# The client's own time

A workload's time between turns is `sleep (Z);`, whatever the client waits
for: a person reading and typing, a tool call, the subagents a turn
delegated to, a trace's gap between a session's requests. Every `sleep` of
the workload runs on one delay the parser supplies, named `sleep`; what
it waits for is written in a comment beside it. `tool Z;`, the form it
replaces, is a retired word. Issue #508.

## The problem

Before this change one meaning had three spellings (criterion 0). The
client's time between turns is a delay of the workload's own, an infinite
server where no client waits for another, and the examples wrote it as:

- `tool (~exp(Z));`, which ran on a delay the parser supplied when no
  stage `tool` was declared (#500, #506);
- a stage the workload declared and ran with the kernel, `stage user :
  delay;` and `run user (cost(user, ~exp(Z)));`;
- the same kernel on a stage named after what is waited for, `delegate`
  for the subagents, `gate` for a trace's spacing.

`tool on user (~exp(Z));` and `run user (cost(user, ~exp(Z)));` give the
same IR byte for byte, and `tool (~exp(Z));` without the declaration
differs from them in the stage's name alone (`serq ir
examples/multi-turn/vllm.sq --workload examples/workloads/human_chat.sq`).
So the name of a stage said what the client waited for, which is a comment's
job, and a workload that waited for two things declared two stages:
`subagents.sq` declared `stage tool` although its `tool` needed none, only
to keep `tool`'s index before `delegate`'s (#506's ordering).

## Before and after

`examples/workloads/human_chat.sq:9`, `:27`, before:

```serq
stage user : delay;
…
      run user (cost(user, ~exp(Z)));
```

after:

```serq
      sleep (~exp(Z));  // the person reads and types
```

`examples/workloads/subagents.sq:21-22`, `:48`, before:

```serq
stage tool : delay;      // declared, though a workload's `tool` needs none: it keeps its IR index before `delegate`
stage delegate : delay;
…
        if (fan) { run delegate (cost(delegate, W)); } else { tool (~exp(Z)); }
```

after:

```serq
        if (fan) {
          sleep (W);  // the subagents
        } else {
          sleep (~exp(Z));  // a tool call
        }
```

`examples/workloads/short_base.sq:7`, `:16`, `:19` (and `shared_trace.sq`,
`vllm_replay.sq`, `one_request.sq`), before:

```serq
stage gate : delay;
…
    run gate (cost(gate, serial * spacing));
    turn;
    while (more) {
      tool think;
      turn;
    }
```

after:

```serq
    sleep (serial * spacing);  // the session's first turn, `spacing` after the previous session's
    turn;
    while (more) {
      sleep (think);  // the trace's gap
      turn;
    }
```

`docs/use-cases/workloads.md:22`, before and after:

```text
| between turns | — | a person, `run user (cost(user, ~exp(Z)))` | a tool, `tool (~exp(Z))` | a tool, or waiting for the subagents |
| between turns | — | a person, `sleep (~exp(Z))` | a tool call, `sleep (~exp(Z))` | a tool call, or the subagents, `sleep (W)` |
```

`tool` in a program now says what to write:

```text
`tool Z;` is gone: the client's own time between turns is the workload's `sleep (Z);`, on a delay of its own the parser supplies, whatever it waits for (a tool call, a person, a trace's gap); a stage the program declares (a tool server with a capacity) runs with `run S (cost(S, Z));`
```

## The form

`sleep (Z);` is `run sleep (cost(sleep, Z));` on `stage sleep : delay;`,
which the parser supplies after every stage either file declares, as it
supplied `tool`. No `CStmt`, `CExpr` or field moves, so `IR_VERSION`, the
Lean generator and the interpreter do not. Programs' IR moves as data:

- where one wait was `tool` or `user` (the 12 `tool` workloads,
  `human_chat.sq`, `closed.sq`), the stage's name, one line;
- where two waits were two stages (`subagents.sq`'s `tool` and `delegate`;
  `short_base.sq`, `shared_trace.sq` and `vllm_replay.sq`'s `gate` and
  `tool`), one stage fewer and the second's runs renumbered;
- `one_request.sq`'s `gate`, the oracle's arrival time, by name: the seven
  `tools/oracle/*.ir.json` of `vllm_request.sq` and the four
  `tools/oracle/serve/*.ir.json`; `cache_trace.ir.json` loses one stage, and
  `lean/Serq/Oracle.lean`'s `vllmTurn` runs `sleep` as stage 2 where it ran
  `gate` as 2 and `tool` as 3.

A delay serves every job at once at rate 1, so a job's time there is its
work, whoever else is there: moving waits from two delays onto one changes
no session's time. Every report line outside the stage table is the same,
for each of the 42 pairs of `examples/pairs.json` (seed 1, horizon 200 s,
against origin/main's binary on origin/main's files), and
`merged_waits_change_only_the_stage_table` (`tests/ir.rs`) holds it for
`subagents.sq`. The stage table, `vllm.sq` with `subagents.sq`, seed 1,
horizon 300 s, before:

```text
stage     number   util  done    thru    wait  service   iters
--------  ------  -----  ----  ------  ------  -------  ------
vllm       0.324  0.256  2143  7.1433  0.0000   0.0454  180951
tool       6.828  0.986   705  2.3500  0.0000   2.8643       0
delegate   3.999  0.928    73  0.2433  0.0000  15.4000       0
```

after:

```text
stage  number   util  done    thru    wait  service   iters
-----  ------  -----  ----  ------  ------  -------  ------
vllm    0.324  0.256  2143  7.1433  0.0000   0.0454  180951
sleep  10.828  0.986   778  2.5933  0.0000   4.0406       0
```

The merged row is the two rows' jobs together: `number` and `done` add
(6.828 + 3.999, 705 + 73), and `service` is their mean weighted by `done`.
`util` does not add: it is the time either row had a job, at least the
larger of the two (`merged_waits_change_only_the_stage_table` checks all
four). Here the separate rows said nothing the program does not state:
`W` is an input, and the tool time's mean is `Z`.

A trace replay is different. Its workload sleeps `serial * spacing` before
a session's first turn and the trace's `think` between its turns, and
before this change ran the first on `gate` and the second on `tool`:
`examples/replay/vllm_replay.sq` with `vllm_replay.sq`, seed 1, horizon
200 s, before:

```text
stage   number   util  done    thru    wait   service  iters
-----  -------  -----  ----  ------  ------  --------  -----
vllm     0.424  0.337   907  4.5350  0.0000    0.0935   2457
front    0.096  0.092   482  2.4100  0.0000    0.0400      0
gate   303.928  1.000    57  0.2850  0.0000  101.5000      0
tool    12.560  0.999   424  2.1200  0.0000    5.5292      0
```

after:

```text
stage   number   util  done    thru    wait  service  iters
-----  -------  -----  ----  ------  ------  -------  -----
vllm     0.424  0.337   907  4.5350  0.0000   0.0935   2457
front    0.096  0.092   482  2.4100  0.0000   0.0400      0
sleep  316.487  1.000   481  2.4050  0.0000  16.9021      0
```

The `sleep` row counts the sessions not yet sent with those between
turns, and the first are most of it: 304 of its 316 at 200 s (the 333
sessions all arrive at 0 and wait their offset there). The time between
turns, 12.56 sessions over 5.53 s, is no longer read off the report.
`vllm_nixl_push.sq` with `short_base.sq` is the same (`gate` 62.165 and
`tool` 14.569 become `sleep` 76.734). The before was run on this change's
binary with the kernel spelling, `stage gate : delay; stage tool :
delay;` in the workload and run with `run`: the stages origin/main ran,
in its order, and origin/main's binary prints the same two rows.

## Why `sleep`

It is the name upstream gives the client's waits. vLLM's benchmark client
sleeps until a trace request's timestamp before sending it
(`benchmarks/serve.py:508-515`), the trace workloads' `gate`, and its
multi-turn client sleeps an exponential time between the requests it
sends, which go round-robin over its active conversations
(`poisson_sleep`,
`multi_turn/benchmark_serving_multi_turn.py:561-568, 690-691, 799, 801-803`).
That sleep is not one conversation's time between its turns, as the chat
workloads' is, unless the client keeps one conversation active; but
neither sleep is the server's: both are the client's time. A serving
engineer reads `sleep (~exp(Z));` as that client (criterion 1).

## The rules it keeps, and the ones that went

`sleep` is the workload session's alone. In the server, a queue's entry,
or the workload's `init` and `turn` it is refused with the reason: the
server's time runs on a stage it declares, and `init` and `turn` draw
attributes, which take no time. The supplied stage is a declaration, so a
declaration named `sleep` beside the form, a stage in either file, above
the form or below it, or a pool, is refused naming its place. A queue may
not be named `sleep`, whose `sleep.verb ();` would read as the statement,
nor any word of the language: only a queue with an engine was checked
against `KEYWORDS`, so `queue run : prefill { … }` linked and its call
`run.prefill (…);` failed as a `run` ("expected identifier, found Dot").
The supplied stage joins the program's declarations, so an
attribute named `sleep` is refused by #470's rule, as beside a declared
stage. The old `tool` missed that: on origin/main, `init { set tool = 1; }`
and `tool (tool);` linked, the one word an attribute and a stage. `sleep`
also takes its time in parentheses, and `sleep 3;` says so: `tool Z;` took
it bare. The stage is the workload's (`prog.paired` lists it), so the
deployment reading it is refused as for any stage of the workload's.

That replaces three rules of `tool`: a stage `tool` declared above the
call was used whatever its kind, the deployment's included; one declared
below was refused; and the linker's `paired_station` let the workload run
on the deployment's `tool` as its one exception (#723's fix was a pool
named `tool` slipping through it). The exception is gone, and so is the
server's `tool`, `tool on S (Z);` and `tool[j] Z;`, which only tests used.
A wait with a row of its own, or with a capacity clients share (a sandbox
pool, a rate-limited API), is a stage the workload declares and runs with
the kernel: `stage sandbox : fifo(4);`, `run sandbox (cost(sandbox, Z));`.

`tool` is a retired word, as `mark` became in #724: it leaves `KEYWORDS`,
the highlighter's `STATEMENTS` and `docs/hooks/serq_lexer.py`, and `sleep`
takes its place (121 keywords). A statement beginning with `tool`, however
it goes on, gets the message above; a name `tool` set or defined is a
retired word. Stage names are read as identifiers and not checked against
the retired words, so `stage tool : delay; run tool (cost(tool, Z));`
still links, as in `tests/lean-regress/preempt_delay.sq`.

The companion programs pinned at v0.1.5 migrate on their own:
`serving-queue-theory/programs/open_vllm.sq:104`, `replay_vllm.sq:103` and
`lecture_pd.sq:44` write the form; the ones that declare `stage tool` and
run it with the kernel keep linking.

## Self-critique

**More role keywords (`user`, `delegate`, `gate`), #508's option 2.** It
keeps a row per kind of wait with no declaration. But the kinds of client
wait are the program's to name, not the language's: a keyword per kind is a
rule written in the language that a program could not state otherwise
(criterion 2), and every new kind of client would ask for another.

**Keeping `tool` beside `sleep`.** On the one supplied stage, `tool (Z)`
and `sleep (Z)` give byte-identical IR: two spellings of one meaning. On
two stages it is option 2 again. After this change no example, tutorial
or deployment writes `tool`, so the rule that a construct some program
uses stays would retire it anyway.

**`tool` not retired, only removed.** A program written before would get
"unknown statement `tool`" and no direction. `mark`'s precedent (#724) is
to say what replaced it; stage names are unaffected.

**`wait (Z)`, the issue's spelling.** The report's `wait` column is time
queued before service (#611), and `examples/single-turn/mg1.sq:11`
observes `wait = now - t0 - s`, the queueing delay: a delay station's time
is the opposite. `think` is a built-in attribute, the trace's column that
`sleep (think);` reads. `delay (Z)` reads as a role form on a stage named
`delay`, which is also the stage kind. `pause` and `idle` (a step-table
column) say less than the upstream name.

**Refusing a workload's declared delay run with the kernel** (the old
`user` spelling) as a second spelling of `sleep`. Every sugar keeps its
kernel spelling (`run link …` beside `transfer`), and a program that wants
a row of its own for one wait needs it. Nothing about it produces a wrong
answer, so no check was added.

**The lost rows.** `delegate` and `gate` had rows of their own. The
subagents' wait is an input, `W`, and its row said nothing the program
does not state. The trace's rows did: in a replay the merged `sleep` row
is mostly sessions not yet sent (above), and the time between turns,
which `tool`'s row measured, is no longer in the report. A program that
wants the rows apart declares the stage and runs it with the kernel,
`stage gate : delay;` and `run gate (cost(gate, serial * spacing));`, as
`docs/use-cases/vllm.md` says of the measured replay. Making the offset a
form of its own (the session's start, not a wait) would be a second
spelling of the client's time for one report row; a stage the program
declares already gives it.

**A `sleep`-only refusal of a queue's name.** The first draft refused
`queue sleep` alone, since `sleep.verb ();` would read as the statement.
`queue run` and `queue observe` failed at their calls the same way, and a
queue with an engine was already checked against `KEYWORDS`: one rule in
two places, each covering part of the class. The check now reads
`KEYWORDS` for every queue, at its declaration. It also refuses `queue
nic`, `queue engine` and `queue first`, which tests used and no program
does. Letting the call win instead, `run.prefill (…);` read as the queue
`run` because one is declared, was rejected: the one word would name a
statement and a queue (criterion 0).
