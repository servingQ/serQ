"""pyserq gives what the CLI gives: the same report for the same program,
overrides and seed, and the samples `--dump` writes.

    python pyserq/tests/test_pyserq.py        # with pyserq installed and target/release/serq built
"""

import csv
import json
import os
import subprocess
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
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
        values, times, sessions, turns = r.observe(name)
        rows = list(csv.reader(open(dump / f"{name}.csv")))[1:]
        assert [(float(t), int(s), int(u), float(v)) for t, s, u, v in rows] == list(zip(times, sessions, turns, values))


def test_program_text_with_run_options():
    src = MG1.read_text()
    r = pyserq.run(pyserq.compile(source=src, sets={"lam": 0.5}, seed=3, horizon=20000.0, warmup=1000.0))
    want, _ = cli(MG1, "--set", "lam=0.5", "--seed", "3", "--horizon", "20000", "--warmup", "1000")
    assert json.loads(r.json()) == want


def test_a_trace_read_next_to_the_program_and_an_expression():
    r = pyserq.run(pyserq.compile(REPLAY, sets={"N": 40, "spacing": "2 * 1.75"}))
    want, _ = cli(REPLAY, "--set", "N=40", "--set", "spacing=2 * 1.75")
    assert json.loads(r.json()) == want


def test_the_ir_round_trips():
    p = pyserq.compile(MG1)
    ir = json.loads(p.to_json())
    assert ir["version"] == pyserq.IR_VERSION
    q = pyserq.Program.from_json(p.to_json())
    assert pyserq.run(q).json() == pyserq.run(p).json()


def test_errors_are_value_errors():
    for call in [
        lambda: pyserq.compile(ROOT / "nowhere.sq"),
        lambda: pyserq.compile(source="session { run nowhere (1); end; }"),
        lambda: pyserq.compile(MG1, sets={"nope": 1}),
        lambda: pyserq.compile(MG1, source="x"),
        lambda: pyserq.run(pyserq.compile(MG1)).observe("nope"),
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
