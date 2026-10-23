#!/usr/bin/env python3
"""Collapse and recovery of vLLM's prefix cache (#120): run
examples/multi-turn/vllm.sq under examples/workloads/closed_burst.sq and
open_burst.sq over loads, starts, bursts, eviction orders, interventions
and seeds, and write what the runs show in windows of 25 s.

    cargo build --release
    tools/metastability/study.py              # about three minutes on 56 cores; writes raw.jsonl (not committed), results.csv, summary.md
    tools/metastability/study.py --summary    # rewrites results.csv and summary.md from raw.jsonl, without running

Every number is a simulation of the committed programs, not a measurement.
An eviction order or an intervention is a line of the program the study
states otherwise (`evict lru;` read as `evict by (waiting);`), or an input
(`--blocks`, `--max_live`); the gauges the windows read are lines the
study adds before the server, which observe and change nothing. Each edit
must occur exactly once in the program.

A run of horizon T is the first T seconds of the run of a longer horizon
with the same seed (the interpreter draws nothing ahead of the clock), so
the rows of the shorter horizons are read from the longest run;
test_study.py reruns one of them at its own horizon.
"""

import concurrent.futures
import csv
import json
import math
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SERQ = os.environ.get("SERQ", str(ROOT / "target/release/serq"))
sys.path.insert(0, str(ROOT / "scripts"))
import pairs  # noqa: E402  (each program's workloads: examples/pairs.json)

PROGRAM = ROOT / "examples/multi-turn/vllm.sq"
CLOSED = ROOT / "examples/workloads/closed_burst.sq"
OPEN = ROOT / "examples/workloads/open_burst.sq"
SEEDS = [1, 2, 3, 4, 5]
WINDOW = 25.0  # s
# The workloads' constants the measures read, checked against their text.
K0 = 16000  # the context a full hit reuses
TURNS = 10  # a conversation's mean turns, 1 / (1 - p) with p = 0.9
Z_OPEN = 4.0  # an open conversation's think time (s)
BLOCKS = 34000  # num_gpu_blocks: 544 000 tokens, 32.6 contexts of K0 + 500 + 200.5
BS = 16
SLO = 2.0  # the TTFT (s) a turn of goodput meets
PRE = 500.0  # pre-burst windows start here, after the start's transient (s)
BURST_START = 1000.0
CLOSED_PEAK = 1.5  # the closed loop's think time in its burst (s)
OPEN_BURST, OPEN_PEAK = 60, 1.5  # the open burst: 60 s of 1.5 conversations/s
CONTEXT = 16000 + 500.5 + 200.5  # tokens a conversation's turn computes or caches: K0 + E[n] + E[o]
# 4 windows (100 s) in a row decide a collapse or a recovery
RUN_OF = 4
COLLAPSED = 0.1  # a window's full-hit fraction below which it is collapsed

LRU = "evict lru;"
ANCHOR = "  // --8<-- [start:server]\n"
# Arms: (what it is, edits of the program's lines, inputs). The first three
# are eviction orders; the others are the interventions of the issue's Q3.
ARMS = {
    "lru": ("vLLM's free queue, `evict lru;` (as committed)", [], {}),
    "full": ("a waiting turn's prefix evicted last, `evict by (waiting);`", [(LRU, "evict by (waiting);")], {}),
    "partial": (
        "the same for 10 s after its release, `evict by (waiting * (age < 10));`",
        [(LRU, "evict by (waiting * (age < 10));")],
        {},
    ),
    "compute2x": ("half the compute per scheduled token, `let a = 1e-5;`", [("let a = 2e-5;", "let a = 1e-5;")], {}),
    "kv1.5x": ("1.5 times the KV blocks, `--blocks 51000`", [], {"blocks": 51000}),
    "live30": ("at most 30 conversations in, `--max_live 30`", [], {"max_live": 30}),
    "live25": ("at most 25 conversations in, `--max_live 25`", [], {"max_live": 25}),
}
PROTECTION = ["lru", "full", "partial"]
STARTS = {"default": {}, "warm": {"Z_warm": 50, "warm_until": 600}, "flushed": {"warm_until": 1}}
LAMBDAS = [round(0.20 + 0.05 * i, 2) for i in range(15)]  # 0.20 .. 0.90
# Student t, two-sided 95 %, by degrees of freedom
T95 = {1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571, 6: 2.447, 7: 2.365, 8: 2.306, 9: 2.262}


def cells():
    """Every cell of the study: a workload, an arm, a load, a start, a
    burst, the horizons read; the seeds run each."""
    # E1: a closed loop's burst, and how long it takes to recover
    for burst in [60, 300]:
        for arm in PROTECTION:
            yield cell("E1", CLOSED, arm, 4.0, "default", burst, CLOSED_PEAK, [2000])
    # E2: one load, two starts: does the long run depend on where it began?
    for arm in ["lru", "full"]:
        for z in [2.0, 2.5, 3.0, 4.0]:
            for start in ["warm", "flushed"]:
                yield cell("E2", CLOSED, arm, z, start, 0, 0, [2000, 8000])
    # E3: up and down: warm, a burst, then the load E2 ran from each start
    for z in [2.0, 2.5, 3.0, 4.0]:
        yield cell("E3", CLOSED, "lru", z, "warm", 300, CLOSED_PEAK, [2000, 8000])
    # E4: open conversations, every arm, with and without a burst
    for arm in ARMS:
        for lam in LAMBDAS:
            for burst in [0, OPEN_BURST]:
                yield cell("E4", OPEN, arm, lam, "default", burst, OPEN_PEAK, [2000, 8000])


def cell(experiment, workload, arm, load, start, burst, peak, horizons):
    """`load` is the think time Z of the closed loop or the arrival rate
    Lambda of the open conversations; `peak` is its value in the burst."""
    sets = {"blocks": BLOCKS, **ARMS[arm][2], **STARTS[start]}
    name = "Z" if workload == CLOSED else "Lambda"
    sets[name] = load
    if burst:
        sets.update({"burst_start": BURST_START, "burst_len": burst, f"{name}_burst": peak})
    return {
        "experiment": experiment,
        "workload": workload,
        "arm": arm,
        "load": load,
        "start": start,
        "burst": burst,
        "edits": ARMS[arm][1],
        "sets": sets,
        "horizons": horizons,
    }


def program_text(workload, edits):
    """vllm.sq with the arm's edits and the gauges the windows read."""
    text = PROGRAM.read_text()
    gauges = "  gauge q = queued(reqs);\n  gauge kv_used = used(kv);\n"
    if workload == OPEN:
        # the conversations waiting to enter: a gauge reads any pool
        gauges += "  gauge door = queued(live);\n"
    for old, new in [*edits, (ANCHOR, gauges + ANCHOR)]:
        assert text.count(old) == 1, f"{old!r} occurs {text.count(old)} times in {PROGRAM.name}"
        text = text.replace(old, new)
    return text


def check_workloads():
    assert {CLOSED, OPEN} <= set(pairs.workloads(PROGRAM)), "examples/pairs.json"
    for w in [CLOSED, OPEN]:
        text = w.read_text()
        assert f"let K0 = {K0};" in text, w.name
    assert "let p = 0.9;" in OPEN.read_text() and f"let Z = {Z_OPEN};" in OPEN.read_text()


def run(workload, edits, sets, seed, horizon):
    """One run, read into windows: (the report, the window series)."""
    with tempfile.TemporaryDirectory() as tmp:
        program = Path(tmp) / PROGRAM.name
        program.write_text(program_text(workload, edits))
        dump = Path(tmp) / "dump"
        cmd = [SERQ, "run", str(program), "--workload", str(workload), "--horizon", str(horizon), "--warmup", "0", "--seed", str(seed), "--json", "--dump", str(dump)]
        for k, v in sets.items():
            cmd += [f"--{k}", str(v)]
        rep = json.loads(subprocess.run(cmd, check=True, capture_output=True, text=True).stdout)
        return rep, windows(dump, horizon, sets.get("blocks", BLOCKS) * BS)


def samples(path):
    with open(path) as f:
        return [(float(r["time"]), float(r["value"])) for r in csv.DictReader(f)]


def windows(dump, horizon, capacity):
    """Per window of WINDOW s: admissions, full hits (`vllm.reused` >= K0),
    prefill tokens, completions, turns whose TTFT met the SLO, the TTFT and
    response sums, and the gauges' time averages."""
    n = int(horizon // WINDOW)
    s = {k: [0.0] * n for k in ["adm", "full", "prefill", "first", "good", "ttft", "done", "resp"]}

    def add(key, t, v):
        k = int(t // WINDOW)
        if k < n:
            s[key][k] += v

    for t, v in samples(dump / "vllm.reused.csv"):
        add("adm", t, 1)
        add("full", t, 1 if v >= K0 else 0)
    for t, v in samples(dump / "vllm.prefill.csv"):
        add("prefill", t, v)
    for t, v in samples(dump / "client/ttft.csv"):
        add("first", t, 1)
        add("good", t, 1 if v <= SLO else 0)
        add("ttft", t, v)
    for t, v in samples(dump / "client/response.csv"):
        add("done", t, 1)
        add("resp", t, v)
    for name in ["q", "kv_used", "door"]:
        path = dump / "gauge" / f"{name}.csv"
        if path.exists():
            g = average(samples(path), n, horizon)
            s[name] = [x / capacity for x in g] if name == "kv_used" else g
    return s


def average(points, n, horizon):
    """A piecewise-constant signal's time average in each window."""
    out = [0.0] * n
    for i, (t, v) in enumerate(points):
        end = min(points[i + 1][0] if i + 1 < len(points) else horizon, n * WINDOW)
        while t < end:
            k = int(t // WINDOW)
            e = min(end, (k + 1) * WINDOW)
            out[k] += v * (e - t) / WINDOW
            t = e
    return out


# ---- measures of a window series (pure: test_study.py feeds them series)


def hit(s, k):
    return s["full"][k] / s["adm"][k] if s["adm"][k] else math.nan


def backlog(s, k):
    """Turns waiting for the engine, plus conversations waiting to enter."""
    return s["q"][k] + (s["door"][k] if "door" in s else 0.0)


def collapse_time(s, n):
    """When the first run of RUN_OF windows below COLLAPSED full hits
    starts (s), or None. A window without admissions breaks a run: it
    shows nothing."""
    run = 0
    for k in range(n):
        h = hit(s, k)
        run = run + 1 if h < COLLAPSED else 0
        if run == RUN_OF:
            return (k - RUN_OF + 1) * WINDOW
    return None


def collapsed_at_end(s, n, span=500.0):
    """Every window of the last `span` seconds is below COLLAPSED."""
    ks = range(n - int(span // WINDOW), n)
    return all(hit(s, k) < COLLAPSED for k in ks)


def growing(s, n):
    """The backlog's mean over the last quarter of the run exceeds the
    third quarter's by more than 5 and by more than a fifth: a run that
    falls behind, not one near saturation that diverges slowly."""
    a, b, c = n // 2, (3 * n) // 4, n
    q3 = sum(backlog(s, k) for k in range(a, b)) / (b - a)
    q4 = sum(backlog(s, k) for k in range(b, c)) / (c - b)
    return q4 - q3 > max(5.0, 0.2 * q3)


def recovery_time(s, n, burst_end, start=PRE, until=BURST_START):
    """Seconds from the burst's end to the first run of RUN_OF windows
    whose full hits are within 0.1 of the pre-burst level and whose
    backlog is within 2 of it; inf when none comes before the run ends;
    None when the pre-burst windows were already collapsed."""
    k0, k1 = int(start // WINDOW), int(until // WINDOW)
    if collapse_time(_slice(s, k0, k1), k1 - k0) is not None:
        return None
    adm = sum(s["adm"][k0:k1])
    base_hit = sum(s["full"][k0:k1]) / adm
    base_q = sum(backlog(s, k) for k in range(k0, k1)) / (k1 - k0)
    run = 0
    for k in range(math.ceil(burst_end / WINDOW), n):
        ok = hit(s, k) >= base_hit - 0.1 and backlog(s, k) <= base_q + 2
        run = run + 1 if ok else 0
        if run == RUN_OF:
            return (k - RUN_OF + 1) * WINDOW - burst_end
    return math.inf


def _slice(s, a, b):
    return {k: v[a:b] for k, v in s.items()}


def span(s, a, b):
    """The window series' totals over windows [a, b), as the row's means."""
    t = (b - a) * WINDOW
    adm, first, done = sum(s["adm"][a:b]), sum(s["first"][a:b]), sum(s["done"][a:b])
    out = {
        "hit": sum(s["full"][a:b]) / adm if adm else math.nan,
        "recompute": sum(s["prefill"][a:b]) / adm if adm else math.nan,
        "turns_per_s": done / t,
        "goodput": sum(s["good"][a:b]) / t,
        "ttft": sum(s["ttft"][a:b]) / first if first else math.nan,
        "response": sum(s["resp"][a:b]) / done if done else math.nan,
        "q": sum(s["q"][a:b]) / (b - a),
        "kv_used": sum(s["kv_used"][a:b]) / (b - a),
    }
    out["door"] = sum(s["door"][a:b]) / (b - a) if "door" in s else None
    return out


def rows(c, seed, s, horizons=None):
    """The rows of one run: one per horizon read, the means over its last
    half."""
    out = []
    for h in horizons or c["horizons"]:
        n = int(h // WINDOW)
        assert n <= len(s["adm"]), (h, len(s["adm"]))
        r = {
            "experiment": c["experiment"],
            "workload": c["workload"].stem,
            "arm": c["arm"],
            "load": c["load"],
            "start": c["start"],
            "burst": c["burst"],
            "horizon": h,
            "seed": seed,
            **span(s, n // 2, n),
            "collapse_s": collapse_time(s, n),
            "collapsed_at_end": int(collapsed_at_end(s, n)),
            "growing": int(growing(s, n)) if c["workload"] == OPEN else None,
            "recovery_s": None,
            "burst_hit": None,
        }
        if c["burst"]:
            end = BURST_START + c["burst"]
            r["burst_hit"] = span(s, int(BURST_START // WINDOW), int(end // WINDOW))["hit"]
            # the pre-burst windows are at the cell's load only from the
            # default start: a warm start's light load lasts until 600 s
            if c["start"] == "default":
                r["recovery_s"] = recovery_time(s, n, end)
        out.append(r)
    return out


def job(args):
    i, c, seed = args
    h = max(c["horizons"])
    _rep, s = run(c["workload"], c["edits"], c["sets"], seed, h)
    return i, seed, s


def main():
    check_workloads()
    grid = list(cells())
    if sys.argv[1:] == ["--summary"]:
        # rewrite results.csv and summary.md from raw.jsonl, without running
        series = {}
        with open(HERE / "raw.jsonl") as raw:
            for i, line in zip(range(len(grid) * len(SEEDS)), raw):
                rec = json.loads(line)
                series[(i // len(SEEDS), rec["seed"])] = rec["series"]
        assert len(series) == len(grid) * len(SEEDS), "raw.jsonl is not this grid's: run study.py"
    else:
        jobs = [(i, c, seed) for i, c in enumerate(grid) for seed in SEEDS]
        series = {}
        with concurrent.futures.ProcessPoolExecutor(max_workers=os.cpu_count()) as pool:
            for done, (i, seed, s) in enumerate(pool.map(job, jobs, chunksize=1), 1):
                series[(i, seed)] = s
                if done % 100 == 0:
                    print(f"{done}/{len(jobs)} runs", file=sys.stderr)
        with open(HERE / "raw.jsonl", "w") as raw:
            for i, c in enumerate(grid):
                for seed in SEEDS:
                    params = {k: (v.stem if k == "workload" else v) for k, v in c.items()}
                    raw.write(json.dumps({"params": params, "seed": seed, "window": WINDOW, "series": series[(i, seed)]}) + "\n")
    out = []
    for i, c in enumerate(grid):
        for seed in SEEDS:
            out += rows(c, seed, series[(i, seed)])
    keys = list(out[0])
    with open(HERE / "results.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=keys)
        w.writeheader()
        w.writerows({k: cell_text(v) for k, v in r.items()} for r in out)
    (HERE / "summary.md").write_text(summary(out, grid, series))


def cell_text(v):
    if v is None:
        return ""
    if isinstance(v, float):
        return repr(v)
    return v


# ---- summary.md


def ci(xs):
    xs = [x for x in xs if x is not None and not math.isnan(x)]
    n = len(xs)
    if n == 0:
        return math.nan, math.nan
    m = sum(xs) / n
    if n < 2:
        return m, math.nan
    sd = math.sqrt(sum((x - m) ** 2 for x in xs) / (n - 1))
    return m, T95[n - 1] * sd / math.sqrt(n)


def fmt(xs, digits=2):
    xs = [x for x in xs if x is not None and not math.isnan(x)]
    if not xs:
        return "–"
    m, h = ci(xs)
    return f"{m:.{digits}f}" if math.isnan(h) else f"{m:.{digits}f} ± {h:.{digits}f}"


def times(xs):
    """Seconds over seeds: the finite ones' mean ± CI, and how many never came."""
    fin = [x for x in xs if x is not None and math.isfinite(x)]
    never = sum(1 for x in xs if x is not None and math.isinf(x))
    head = fmt(fin, 0) if fin else "–"
    return head + (f" ({never} never)" if never else "")


def count(rs, f):
    return f"{sum(1 for r in rs if f(r))}/{len(rs)}"


def summary(rows_, grid, series):
    g = {}
    for r in rows_:
        g.setdefault((r["experiment"], r["arm"], r["load"], r["start"], r["burst"], r["horizon"]), []).append(r)

    def get(e, arm, load, start, burst, h):
        return g.get((e, arm, load, start, burst, h), [])

    out = [
        "# Collapse and recovery of vLLM's prefix cache: the study",
        "",
        "Generated by `tools/metastability/study.py`; simulation of `examples/multi-turn/vllm.sq` as committed, not a measurement.",
        f"Mean ± 95 % CI over seeds {SEEDS[0]}–{SEEDS[-1]} (Student t). {BLOCKS} blocks of {BS} tokens: about 32.6 contexts of K0 + n + o = 16 700 tokens. "
        f"Windows of {WINDOW:.0f} s. A *full hit* is an admission that reused at least K0 = {K0} tokens; a window is *collapsed* below {COLLAPSED} full hits, "
        f"and a collapse is the first {RUN_OF} collapsed windows in a row. *Recompute* is the prefill tokens an admission computes. *Goodput* counts the turns whose TTFT is at most {SLO:.0f} s. "
        "A row's means are over the last half of its horizon.",
        "",
        "## E1: a burst in a closed loop (`closed_burst.sq`, N = 40, Z = 4 s, the burst Z = 1.5 s from 1000 s)",
        "",
        f"Recovery: from the burst's end to the first {RUN_OF} windows in a row within 0.1 full hits and 2 waiting turns of the pre-burst level ({PRE:.0f}–{BURST_START:.0f} s), in steps of {WINDOW:.0f} s.",
        "",
        "| burst (s) | arm | hit before | hit in the burst | recovery (s) | hit, last half | turns/s | TTFT (s) |",
        "|---|---|---|---|---|---|---|---|",
    ]
    for burst in [60, 300]:
        for arm in PROTECTION:
            rs = get("E1", arm, 4.0, "default", burst, 2000)
            before = [pre_hit(series, grid, "E1", arm, 4.0, "default", burst, seed) for seed in SEEDS]
            out.append(
                f"| {burst} | `{arm}` | {fmt(before)} | {fmt([r['burst_hit'] for r in rs])} | {times([r['recovery_s'] for r in rs])} | "
                f"{fmt([r['hit'] for r in rs])} | {fmt([r['turns_per_s'] for r in rs])} | {fmt([r['ttft'] for r in rs])} |"
            )
    for burst in [60, 300]:
        out += [
            "",
            f"Windows of 50 s around the {burst} s burst, mean over seeds: full hits, waiting turns, TTFT (s).",
            "",
            "| from (s) | " + " | ".join(f"`{a}` hit | `{a}` waiting | `{a}` TTFT" for a in PROTECTION) + " |",
            "|---|" + "---|" * (3 * len(PROTECTION)),
        ]
        t0, t1 = BURST_START - 100, BURST_START + burst + 300
        t = t0
        while t < t1:
            cells_ = []
            for arm in PROTECTION:
                ss = [series_of(series, grid, "E1", arm, 4.0, "default", burst, seed) for seed in SEEDS]
                a, b = int(t // WINDOW), int((t + 50) // WINDOW)
                sp = [span(s, a, b) for s in ss]
                cells_ += [f"{mean([x['hit'] for x in sp]):.2f}", f"{mean([x['q'] for x in sp]):.1f}", f"{mean([x['ttft'] for x in sp]):.2f}"]
            mark = " (burst)" if BURST_START <= t < BURST_START + burst else ""
            out.append(f"| {t:.0f}{mark} | " + " | ".join(cells_) + " |")
            t += 50
    out += [
        "",
        "## E2: two starts at one load (`closed_burst.sq`, N = 40, no burst)",
        "",
        "*Warm*: a think time of 50 s until 600 s, which fills the cache one turn at a time. *Flushed*: all 40 first turns at once into an empty cache. "
        "`collapsed at end` counts the seeds whose last 500 s are all collapsed windows.",
        "",
        "| arm | Z (s) | start | horizon (s) | hit | recompute (tokens) | turns/s | TTFT (s) | waiting | collapsed at end |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for arm in ["lru", "full"]:
        for z in [2.0, 2.5, 3.0, 4.0]:
            for start in ["warm", "flushed"]:
                for h in [2000, 8000]:
                    rs = get("E2", arm, z, start, 0, h)
                    out.append(
                        f"| `{arm}` | {z} | {start} | {h} | {fmt([r['hit'] for r in rs])} | {fmt([r['recompute'] for r in rs], 0)} | "
                        f"{fmt([r['turns_per_s'] for r in rs])} | {fmt([r['ttft'] for r in rs])} | {fmt([r['q'] for r in rs], 1)} | {count(rs, lambda r: r['collapsed_at_end'])} |"
                    )
    after = BURST_START + 300 + 200
    out += [
        "",
        "## E3: up and down (`lru`; warm, then the burst Z = 1.5 s for 300 s from 1000 s, then Z)",
        "",
        f"Full hits from {after:.0f} s (200 s after the burst) to the horizon, next to E2's over the same windows from each start at the same Z.",
        "",
        "| Z (s) | horizon (s) | after the burst | E2 warm | E2 flushed | collapsed at end |",
        "|---|---|---|---|---|---|",
    ]
    for z in [2.0, 2.5, 3.0, 4.0]:
        for h in [2000, 8000]:
            a, b = int(after // WINDOW), int(h // WINDOW)
            late = {
                (e, start, burst): fmt([span(series_of(series, grid, e, "lru", z, start, burst, seed), a, b)["hit"] for seed in SEEDS])
                for e, start, burst in [("E3", "warm", 300), ("E2", "warm", 0), ("E2", "flushed", 0)]
            }
            rs = get("E3", "lru", z, "warm", 300, h)
            out.append(
                f"| {z} | {h} | {late[('E3', 'warm', 300)]} | {late[('E2', 'warm', 0)]} | {late[('E2', 'flushed', 0)]} | "
                f"{count(rs, lambda r: r['collapsed_at_end'])} |"
            )
    out += [
        "",
        "## E4: open conversations (`open_burst.sq`, p = 0.9, Z = 4 s; the burst 1.5 conversations/s for 60 s from 1000 s)",
        "",
        "The arms:",
        "",
    ]
    out += [f"- `{a}`: {title}" for a, (title, _e, _s) in ARMS.items()]
    out += [
        "",
        "`collapsed` counts the seeds with a collapse anywhere in the run, `at end` those whose last 500 s are all collapsed, `growing` those whose "
        "backlog (waiting turns plus conversations waiting to enter) grew from the third quarter of the run to the fourth by more than 5 and a fifth. "
        "`door` is the conversations waiting to enter (`--max_live` only). Means over the last half, horizon 8000 s. "
        "The `burst:` columns are the runs with the burst: `collapsed` counts the seeds whose first collapse comes at or after the burst's start "
        "(a seed already collapsed before it is not counted), `at end` and `recovery` as above.",
        "",
        "| arm | Λ | collapsed | first collapse (s) | at end | growing | hit | turns/s | goodput | TTFT (s) | door | KV used | burst: collapsed | burst: at end | burst: recovery (s) |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for arm in ARMS:
        for lam in LAMBDAS:
            rs = get("E4", arm, lam, "default", 0, 8000)
            bs = get("E4", arm, lam, "default", OPEN_BURST, 8000)
            onset = [r["collapse_s"] for r in rs if r["collapse_s"] is not None]
            out.append(
                f"| `{arm}` | {lam:.2f} | {count(rs, lambda r: r['collapse_s'] is not None)} | {fmt(onset, 0) if onset else '–'} | "
                f"{count(rs, lambda r: r['collapsed_at_end'])} | {count(rs, lambda r: r['growing'])} | {fmt([r['hit'] for r in rs])} | "
                f"{fmt([r['turns_per_s'] for r in rs])} | {fmt([r['goodput'] for r in rs])} | {fmt([r['ttft'] for r in rs], 2)} | "
                f"{fmt([r['door'] for r in rs], 1)} | {fmt([r['kv_used'] for r in rs])} | "
                f"{count(bs, lambda r: r['collapse_s'] is not None and r['collapse_s'] >= BURST_START)} | {count(bs, lambda r: r['collapsed_at_end'])} | {times([r['recovery_s'] for r in bs])} |"
            )
    out += [
        "",
        "### Two horizons (`lru`)",
        "",
        "The seeds collapsed by each horizon, and those collapsed through its last 500 s; with the burst, those collapsed through the last 500 s.",
        "",
        "| Λ | collapsed by 2000 s | at end, 2000 s | collapsed by 8000 s | at end, 8000 s | burst: at end, 2000 s | burst: at end, 8000 s |",
        "|---|---|---|---|---|---|---|",
    ]
    for lam in LAMBDAS:
        a, b = get("E4", "lru", lam, "default", 0, 2000), get("E4", "lru", lam, "default", 0, 8000)
        ba, bb = get("E4", "lru", lam, "default", OPEN_BURST, 2000), get("E4", "lru", lam, "default", OPEN_BURST, 8000)
        out.append(
            f"| {lam:.2f} | {count(a, lambda r: r['collapse_s'] is not None)} | {count(a, lambda r: r['collapsed_at_end'])} | "
            f"{count(b, lambda r: r['collapse_s'] is not None)} | {count(b, lambda r: r['collapsed_at_end'])} | "
            f"{count(ba, lambda r: r['collapsed_at_end'])} | {count(bb, lambda r: r['collapsed_at_end'])} |"
        )
    out += [
        "",
        "## Q3: the arms ranked (open conversations, 8000 s)",
        "",
        "*Holds to*: the largest Λ of the grid up to which no seed collapses or grows. *Recovers to*: the largest Λ up to which every seed "
        "recovers from the burst and does not grow. Goodput and TTFT at Λ = 0.5 and 0.7; recovery from the burst at Λ = 0.25, and the seeds that "
        "never recover at 0.5.",
        "",
        "| arm | holds to | recovers to | goodput, Λ 0.5 | TTFT (s), Λ 0.5 | goodput, Λ 0.7 | TTFT (s), Λ 0.7 | door, Λ 0.7 | recovery (s), Λ 0.25 | never recovers, Λ 0.5 |",
        "|---|---|---|---|---|---|---|---|---|---|",
    ]
    for arm in ARMS:
        at = lambda lam, burst=0: get("E4", arm, lam, "default", burst, 8000)  # noqa: E731
        never = at(0.5, OPEN_BURST)
        out.append(
            f"| `{arm}` | {upto(g, arm, holds)} | {upto(g, arm, recovers)} | "
            f"{fmt([r['goodput'] for r in at(0.5)])} | {fmt([r['ttft'] for r in at(0.5)])} | "
            f"{fmt([r['goodput'] for r in at(0.7)])} | {fmt([r['ttft'] for r in at(0.7)])} | {fmt([r['door'] for r in at(0.7)], 1)} | "
            f"{times([r['recovery_s'] for r in at(0.25, OPEN_BURST)])} | {count(never, lambda r: r['recovery_s'] is None or math.isinf(r['recovery_s']))} |"
        )
    out += band(g, grid, series)
    return "\n".join(out) + "\n"


def holds(r):
    """A run without a burst that neither collapses nor grows."""
    return r["collapse_s"] is None and not r["growing"]


def recovers(r):
    """A run with the burst that recovers from it and does not grow."""
    return r["recovery_s"] is not None and math.isfinite(r["recovery_s"]) and not r["growing"]


def upto(g, arm, ok):
    """The largest Λ of the grid up to which every seed's run is `ok`."""
    burst = OPEN_BURST if ok is recovers else 0
    best = None
    for lam in LAMBDAS:
        rs = g.get(("E4", arm, lam, "default", burst, 8000), [])
        if not rs or not all(ok(r) for r in rs):
            break
        best = lam
    return f"{best:.2f}" if best is not None else f"< {LAMBDAS[0]:.2f}"


def first_load(g, arm, burst, bad):
    """The smallest Λ of the grid where some seed's run is `bad`."""
    for lam in LAMBDAS:
        if any(bad(r) for r in g.get(("E4", arm, lam, "default", burst, 8000), [])):
            return f"{lam:.2f}"
    return "–"


def series_of(series, grid, e, arm, load, start, burst, seed):
    for i, c in enumerate(grid):
        if (c["experiment"], c["arm"], c["load"], c["start"], c["burst"]) == (e, arm, load, start, burst):
            return series[(i, seed)]
    raise KeyError((e, arm, load, start, burst))


def pre_hit(series, grid, e, arm, load, start, burst, seed):
    s = series_of(series, grid, e, arm, load, start, burst, seed)
    return span(s, int(PRE // WINDOW), int(BURST_START // WINDOW))["hit"]


def mean(xs):
    xs = [x for x in xs if x is not None and not math.isnan(x)]
    return sum(xs) / len(xs) if xs else math.nan


def poisson_above(m, c):
    """P(X > c) for X Poisson of mean m: the conversations in at once
    exceed c contexts."""
    term, below = math.exp(-m), 0.0
    for j in range(math.floor(c) + 1):
        below += term
        term *= m / (j + 1)
    return 1.0 - below


def band(g, grid, series):
    """The two-number prediction of the open model's band, per arm.
    Λ_low = μ_cold / E[turns]: below it a collapsed engine serves the turns
    that arrive and drains; above it a collapse lasts. μ_cold is one turn
    of K0 + n new tokens and o decodes, all computed, at `a` a token:
    1 / (a (K0 + E[n] + E[o])), and also measured. Λ_high = C / ((E[turns]
    − 1) Z + E[turns] R_warm): above it the conversations in at once, by
    Little's law, do not fit the cache's C contexts, or the cap."""
    out = [
        "",
        "## The band",
        "",
        "Two numbers per arm, from the program's constants. *Λ_low* = μ_cold / E[turns], with μ_cold = 1 / (a (K0 + E[n] + E[o])) turns/s, "
        "a turn computed whole at `a` seconds a token: above it a collapsed engine cannot serve the turns that arrive, so a collapse lasts. "
        "*Λ_high* = C / ((E[turns] − 1) Z + E[turns] R_warm), C the contexts the KV holds (or the cap on conversations in, if smaller) and "
        f"R_warm the response at Λ = {LAMBDAS[0]:.2f}: above it the conversations in at once (Little's law) do not fit the cache, or the cap, "
        "so the warm state cannot last. Against them, what the runs show: μ_cold measured over every collapsed window of the arm's runs "
        "without the burst; the largest Λ up to which every seed recovers from the burst and the first where one does not; the first Λ "
        "where a seed collapses or grows without a burst. Last, the chance that more than C conversations are in at once at that Λ when "
        "they are Poisson with mean Λ D, D = (E[turns] − 1) Z + E[turns] R_warm the time one is in (M/G/∞: without a cap none waits to enter).",
        "",
        "| arm | μ_cold, model | μ_cold, runs | Λ_low | recovers to | first not recovered | C | R_warm (s) | Λ_high | first collapse or growth | P(in > C) there |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for arm, (_title, edits, sets) in ARMS.items():
        a = 1e-5 if ("let a = 2e-5;", "let a = 1e-5;") in edits else 2e-5
        contexts = min(sets.get("blocks", BLOCKS) * BS / CONTEXT, sets.get("max_live", math.inf))
        capped = "max_live" in sets
        cold, r_warm = [], []
        for i, c in enumerate(grid):
            if c["experiment"] != "E4" or c["arm"] != arm or c["burst"]:
                continue
            for seed in SEEDS:
                s = series[(i, seed)]
                n = len(s["adm"])
                cold += [s["done"][k] / WINDOW for k in range(n) if hit(s, k) < COLLAPSED]
                if c["load"] == LAMBDAS[0]:
                    r_warm.append(span(s, n // 2, n)["response"])
        mu_model = 1 / (a * CONTEXT)
        rw = mean(r_warm)
        d = (TURNS - 1) * Z_OPEN + TURNS * rw
        high = contexts / d
        not_recovered = lambda r: r["recovery_s"] is not None and math.isinf(r["recovery_s"])  # noqa: E731
        low = "–" if capped else f"{mu_model / TURNS:.3f}"
        measured = "–" if capped else f"{mean(cold):.2f}"
        broke = first_load(g, arm, 0, lambda r: not holds(r))
        tail = "–" if capped or broke == "–" else f"{poisson_above(float(broke) * d, contexts):.2g}"
        out.append(
            f"| `{arm}` | {'–' if capped else f'{mu_model:.2f}'} | {measured} | {low} | {upto(g, arm, recovers)} | "
            f"{first_load(g, arm, OPEN_BURST, not_recovered)} | {contexts:.1f} | {rw:.3f} | {high:.3f} | {broke} | {tail} |"
        )
    out += [
        "",
        "A `max_live` arm never holds more contexts than its cap, so it does not collapse; the conversations its cap keeps out wait to "
        "enter, and that queue is what grows past Λ_high or does not drain after the burst.",
    ]
    return out


if __name__ == "__main__":
    main()
