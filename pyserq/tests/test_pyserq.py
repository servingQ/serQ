"""pyserq gives what the CLI gives: the same report for the same program,
overrides and seed, and the samples `--dump` writes.

    python pyserq/tests/test_pyserq.py        # with pyserq installed and target/release/serq built
"""

import csv
import json
import math
import os
import subprocess
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from itertools import groupby
from pathlib import Path

import pyserq

ROOT = Path(__file__).resolve().parents[2]
CLI = os.environ.get("SERQ_CLI", str(ROOT / "target" / "release" / "serq"))
MG1 = ROOT / "examples" / "single-turn" / "mg1.sq"
REPLAY = ROOT / "examples" / "replay" / "vllm_replay.sq"


def cli(path, *args):
    d = tempfile.mkdtemp(prefix="pyserq-")
    out = subprocess.run([CLI, "run", str(path), *args, "--json", "--dump", d],
                         capture_output=True, text=True, check=True)
    return json.loads(out.stdout), Path(d)


def test_a_file_with_numbers_and_a_seed():
    r = pyserq.run(pyserq.compile(MG1, sets={"lam": 0.8, "law": 1}, seed=10))
    want, dump = cli(MG1, "--set", "lam=0.8", "--set", "law=1", "--seed", "10")
    assert json.loads(r.json()) == want
    for name in want["observes"]:
        o = r.observes[name]
        rows = list(csv.reader(open(dump / f"{name}.csv")))[1:]
        assert [(float(t), int(s), int(u), float(v)) for t, s, u, v in rows] == list(zip(o.times, o.sessions, o.turns, o.samples))


def same(x, j):
    """An attribute and its JSON field: JSON writes a NaN or an infinity as null."""
    return (j is None and not math.isfinite(x)) or x == j


def attrs(x):
    return {a for a in dir(x) if not a.startswith("_")}


SAMPLES = {"samples", "times", "sessions", "turns"}
LOOKUPS = {"json", "observes", "stages", "pools", "observe", "stage", "stages_named", "pool"}


def test_the_report_is_its_json_by_name():
    for r in [pyserq.run(pyserq.compile(MG1, seed=4, horizon=20000.0, warmup=1000.0)),
              pyserq.run(pyserq.compile(REPLAY, sets={"N": 40}))]:
        js = json.loads(r.json())
        scalars = {k: v for k, v in js.items() if k not in ("observes", "stages", "pools")}
        # every field, both ways: nothing JSON has is missing, nothing it lacks is added
        assert attrs(r) == set(scalars) | LOOKUPS
        assert all(same(getattr(r, k), v) for k, v in scalars.items())
        assert list(r.observes) == list(js["observes"])
        for name, o in js["observes"].items():
            ob = r.observe(name)
            assert ob.name == name and attrs(ob) == set(o) | {"name"} | SAMPLES
            assert all(same(getattr(ob, k), v) for k, v in o.items())
            assert len(ob.samples) == ob.count
        for rows, want in [(r.stages, js["stages"]), (r.pools, js["pools"])]:
            assert len(rows) == len(want)
            for row, w in zip(rows, want):
                assert attrs(row) == set(w)
                assert all(same(getattr(row, k), v) for k, v in w.items())
        for s in r.stages:
            assert r.stage(s.name).name == s.name and s.name in {t.name for t in r.stages_named(s.name)}
        for p in r.pools:
            assert r.pool(p.name).name == p.name
        assert r.observe("nope") is None and r.stage("nope") is None and r.pool("nope") is None
    assert type(r.pools[0]).__module__ == "pyserq" and type(r.observe("ttft")).__module__ == "pyserq"
    assert pyserq.run(pyserq.compile(MG1, horizon=5.0, warmup=0.0)).observe("sojourn").ci == math.inf


def test_program_text_with_run_options():
    src = MG1.read_text()
    r = pyserq.run(pyserq.compile(source=src, sets={"lam": 0.5}, seed=3, horizon=20000.0, warmup=1000.0))
    want, _ = cli(MG1, "--set", "lam=0.5", "--seed", "3", "--horizon", "20000", "--warmup", "1000")
    assert json.loads(r.json()) == want


def test_a_trace_read_next_to_the_program_and_an_expression():
    r = pyserq.run(pyserq.compile(REPLAY, sets={"N": 40, "spacing": "2 * 1.75"}))
    want, _ = cli(REPLAY, "--set", "N=40", "--set", "spacing=2 * 1.75")
    assert json.loads(r.json()) == want


def test_a_trace_given_as_a_path():
    trace = REPLAY.parent / "data" / "short_base.csv"
    r = pyserq.run(pyserq.compile(REPLAY, sets={"N": 40}, trace=trace))
    want, _ = cli(REPLAY, "--set", "N=40", "--trace", str(trace))
    assert json.loads(r.json()) == want
    assert pyserq.REPORT_VERSION >= 1


def test_a_trace_read_as_a_replay_reads_it():
    # short_m10 has forced turns (a sixth column of 1s)
    path = REPLAY.parent / "data" / "short_m10.csv"
    rows = [r for r in csv.reader(open(path)) if r and not r[0].startswith(("#", "session"))]
    want = [[tuple(map(float, r[2:6])) for r in g] for _, g in groupby(rows, key=lambda r: r[0])]
    assert any(t[3] == 1.0 for s in want for t in s)
    assert pyserq.read_trace(path) == want


def test_the_ir_round_trips():
    p = pyserq.compile(MG1)
    ir = json.loads(p.to_json())
    assert ir["version"] == pyserq.IR_VERSION
    q = pyserq.Program.from_json(p.to_json())
    assert pyserq.run(q).json() == pyserq.run(p).json()


def test_an_infinity_is_inf():
    for x, e in [(math.inf, "inf"), (-math.inf, "-inf")]:
        assert pyserq.compile(MG1, sets={"cv2": x}).to_json() == pyserq.compile(MG1, sets={"cv2": e}).to_json()


def test_errors_are_value_errors():
    for call in [
        lambda: pyserq.compile(ROOT / "nowhere.sq"),
        lambda: pyserq.compile(source="session { run nowhere (1); end; }"),
        lambda: pyserq.compile(MG1, sets={"nope": 1}),
        lambda: pyserq.compile(MG1, source="x"),
        lambda: pyserq.compile(MG1, sets={"lam": float("nan")}),
        lambda: pyserq.compile(MG1, sets={"not a name": 1}),
        lambda: pyserq.compile(),
        lambda: pyserq.read_trace(ROOT / "nowhere.csv"),
        lambda: pyserq.read_trace(MG1),
    ]:
        try:
            call()
        except ValueError:
            continue
        raise AssertionError("no ValueError")


def test_runs_in_threads_are_the_runs_alone():
    ps = [pyserq.compile(MG1, seed=s, horizon=1e6) for s in range(4)]
    t0 = time.time()
    alone = [pyserq.run(p).json() for p in ps]
    serial = time.time() - t0
    t0 = time.time()
    threaded = [r.json() for r in ThreadPoolExecutor(4).map(pyserq.run, ps)]
    parallel = time.time() - t0
    assert threaded == alone
    print(f"  4 runs: {serial:.2f}s one after another, {parallel:.2f}s in 4 threads")


if __name__ == "__main__":
    for name, f in list(globals().items()):
        if name.startswith("test_"):
            f()
            print("ok", name)
    print(f"OK: pyserq {pyserq.__version__} (IR {pyserq.IR_VERSION}) agrees with {CLI}")
