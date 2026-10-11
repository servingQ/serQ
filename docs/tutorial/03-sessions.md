# 3. Sessions and turns

A session can make several requests, with tool calls or thinking time between
turns. This chapter models the conversation and the context it accumulates.

## The program

```serq title="docs/tutorial/programs/03-sessions.sq"
--8<-- "docs/tutorial/programs/03-sessions.sq"
```

## What is new

### `while` and `turn`

```serq
session {
  turn;
  observe context = K + n + o;
  while (~bernoulli(p)) {
    set K = K + n + o;
    run tool (cost(tool, ~exp(Z)));
    turn;
    observe context = K + n + o;
  }
}
```

`turn;` draws the next turn's attributes and waits for its response.
`while` tests its guard before each pass and continues after the loop when
it is zero. Here `~bernoulli(p)` draws 1 with probability `p = 0.8`:
after each response, the client either thinks and sends another turn or
finishes. No explicit server call or final `end;` is needed.

A guard must be 0 or 1. Write a state condition such as `while (more)`
for trace continuation, or an explicit draw as above for random continuation.

### Attributes carry state across turns

```serq
init { set K = 0; }
…
set K = K + n + o;
```

`init` runs once when the session arrives; `set` assigns a **session
attribute**, which survives from turn to turn. `K` is the context the session
has accumulated. It grows with each turn, increasing the memory the session needs.

### `delay`

```serq
stage tool : delay;
```

A `delay` stage is an infinite-server station: every job runs immediately at
rate 1, nobody waits. It is how you model something outside your system — a
tool call, a human reading, a downstream API.

## Running it

```bash
serq run docs/tutorial/programs/03-sessions.sq --horizon 20000 --warmup 2000 --seed 1
```

```text
run: horizon 20000 end 20000 warmup 2000 seed 1 events 97969 arrivals 9905 ended 8905 turns 43952 mean live 6.295

observe   count       mean    95% CI    cv2         p99
--------  -----  ---------  --------  -----  ----------
response  43953     0.1784   ±0.0049  0.465      0.6027
context   43953  4962.0292  ±93.0588  0.460  17100.8941

stage   number   util   done    thru    wait  service  iters
------  ------  -----  -----  ------  ------  -------  -----
engine   0.436  0.330  43953  2.4418  0.0434   0.1351      0
tool     5.859  0.999  35049  1.9472  0.0000   3.0091      0
```

With continuation probability $p = 0.8$, a complete session has an expected
$1/(1-p) = 5$ turns. The report counts activity within the run's measurement
window; it is not a sample of complete session lengths.

The tool stage accounts for 5.859 of the 6.295 live sessions on average.
Keeping their context between turns uses memory even while the engine is
not serving them. The mean observed context is 4 962 tokens.

## What to try

Turn the thinking time down and watch the sessions pile back in:

```bash
for Z in 0.5 1 3 10; do
  serq run docs/tutorial/programs/03-sessions.sq --horizon 20000 --warmup 2000 --seed 1 --set Z=$Z
done
```

---

Next: keep the context between turns. → [The prefix cache](04-prefix-cache.md)
