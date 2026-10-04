"""pyserq gives what the CLI gives: the same report for the same program,
overrides and seed, the samples `--dump` writes, and the figure `serq draw`
prints.

    maturin develop -m pyserq/Cargo.toml      # pyserq from this checkout
    python pyserq/tests/test_pyserq.py        # with pyserq installed and target/release/serq built
"""

import csv
import importlib.metadata
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
ARRAYS = """pool kv[2] { cap 10; } pool reqs { cap 4; } stage svc[2] : fifo;
workload { arrive poisson(1); }
session { set j = ~bernoulli(0.5); hold reqs (1) { hold kv[j] (1) { run svc[j] (~exp(0.5)); } } end; }
run { horizon 100; }"""


def cli(path, *args):
    d = tempfile.mkdtemp(prefix="pyserq-")
    out = subprocess.run([CLI, "run", str(path), *args, "--json", "--dump", d],
                         capture_output=True, text=True, check=True)
    return json.loads(out.stdout), Path(d)


def cli_source(src, *args):
    f = Path(tempfile.mkdtemp(prefix="pyserq-")) / "program.sq"
    f.write_text(src)
    return cli(f, *args)


def test_a_file_with_numbers_and_a_seed():
    r = pyserq.run(pyserq.compile(MG1, sets={"lam": 0.8, "law": 1}, seed=10))
    want, dump = cli(MG1, "--set", "lam=0.8", "--set", "law=1", "--seed", "10")
    assert json.loads(r.json()) == want
    for name in want["observes"]:
        o = r.observes[name]
        rows = list(csv.reader(open(dump / f"{name}.csv")))[1:]
        assert [(float(t), int(s), int(u), float(v)) for t, s, u, v in rows] == list(zip(o.times, o.sessions, o.turns, o.samples))


GAUGED = """
pool kv[2] { cap 100; }
stage svc[2] : fifo;
workload { arrive poisson(1.5); }
session {
  choose j in 2 by (holders(kv[j]));
  set u = ~uniform(1, 20);
  hold kv[j] (u) { run svc[j] (~exp(0.5)); }
  end;
}
gauge spread = max k in 2 (used(kv[k])) - min k in 2 (used(kv[k]));
run { horizon 2000; warmup 100; seed 3; }
"""


def test_a_gauge_is_its_json_and_its_dump():
    r = pyserq.run(pyserq.compile(source=GAUGED))
    want, dump = cli_source(GAUGED)
    assert json.loads(r.json()) == want
    g = r.gauge("spread")
    assert list(r.gauges) == ["spread"] and g.name == "spread"
    assert attrs(g) == set(want["gauges"]["spread"]) | {"name", "times", "values"}
    assert all(same(getattr(g, k), v) for k, v in want["gauges"]["spread"].items())
    rows = list(csv.reader(open(dump / "gauge" / "spread.csv")))[1:]
    assert [(float(t), float(v)) for t, v in rows] == list(zip(g.times, g.values))


def same(x, j):
    """An attribute and its JSON field: JSON writes a NaN or an infinity as null."""
    return x == j or (j is None and x is not None and not math.isfinite(x))


def attrs(x):
    return {a for a in dir(x) if not a.startswith("_")}


SAMPLES = {"samples", "times", "sessions", "turns"}
LOOKUPS = {"json", "observes", "gauges", "stages", "pools", "observe", "gauge", "stage", "stages_named", "pool", "pools_named"}


def test_the_report_is_its_json_by_name():
    arrays = pyserq.run(pyserq.compile(source=ARRAYS))
    assert [p.index for p in arrays.pools_named("kv")] == [0, 1] and arrays.pool("reqs").index is None
    for r in [pyserq.run(pyserq.compile(MG1, seed=4, horizon=20000.0, warmup=1000.0)),
              arrays,
              pyserq.run(pyserq.compile(REPLAY, sets={"N": 40}))]:
        js = json.loads(r.json())
        scalars = {k: v for k, v in js.items() if k not in ("observes", "gauges", "stages", "pools")}
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
        # an array's members are its rows of one name, indexed in order; a single one has no index
        for rows, named in [(r.stages, r.stages_named), (r.pools, r.pools_named)]:
            for x in rows:
                members = [m.index for m in named(x.name)]
                assert members in ([None], list(range(len(members))))
        assert list(r.gauges) == list(js["gauges"])
        assert r.observe("nope") is None and r.stage("nope") is None and r.pool("nope") is None
        assert r.gauge("nope") is None
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


def test_defs_is_the_program_written_with_that_body():
    src = ("def service() = ~exp(1);\nstage svc : fifo;\nworkload { arrive poisson(0.5); }\n"
           "session { run svc (service()); observe s = now; end; }\nrun { horizon 1000; seed 2; }\n")
    given = pyserq.compile(source=src, defs={"service": "~erlang(4, 1)"})
    written = pyserq.compile(source=src.replace("~exp(1)", "~erlang(4, 1)"))
    assert given.to_json() == written.to_json() != pyserq.compile(source=src).to_json()
    want, _ = cli_source(src, "--def", "service=~erlang(4, 1)")
    assert json.loads(pyserq.run(given).json()) == want


def test_rng_is_the_stream_a_run_draws_from():
    # the arrivals of a run seeded 11 are the gaps Rng(11) draws, exactly
    src = ("let lam = 0.5;\nworkload { arrive poisson(lam); }\n"
           "session { observe t = now; end; }\nrun { horizon 100; warmup 0; seed 11; }\n")
    got = pyserq.run(pyserq.compile(source=src)).observe("t").samples
    rng, t, want = pyserq.Rng(11), 0.0, []
    while t < 100:
        want.append(t)
        t += -math.log(1.0 - rng.random_f64()) / 0.5
    assert len(got) > 20 and got == want[: len(got)]
    # rand 0.9's StdRng: seed_from_u64(7), as serving-queue-theory's port draws it
    r = pyserq.Rng(7)
    assert (r.next_u64(), r.random_f64(), r.range_u32(1, 6)) == (559256596868823998, 0.3070862833742408, 1)
    a, b = pyserq.Rng(3), pyserq.Rng(3)
    assert [a.range_f64(-1.0, 1.0) for _ in range(5)] == [b.range_f64(-1.0, 1.0) for _ in range(5)]
    assert all(2 <= pyserq.Rng(s).range_u64(2, 9) <= 9 for s in range(50))


def test_draw_is_serq_draw():
    # every example, both formats; a program split into a workload and a
    # server draws what one request runs, which a compiled Program does not say
    for f in sorted((ROOT / "examples").rglob("*.sq")):
        for fmt in ["svg", "tikz"]:
            want = subprocess.run([CLI, "draw", str(f), "--format", fmt],
                                  capture_output=True, text=True, check=True).stdout
            assert pyserq.draw(f, format=fmt) == want, f
    want = subprocess.run([CLI, "draw", str(MG1), "--set", "lam=0.8", "--def", "service=~exp(1)"],
                          capture_output=True, text=True, check=True).stdout
    assert pyserq.draw(MG1, sets={"lam": 0.8}, defs={"service": "~exp(1)"}) == want
    assert pyserq.draw(source=MG1.read_text()) == pyserq.draw(MG1)
    ir = Path(tempfile.mkdtemp(prefix="pyserq-")) / "mg1.json"
    ir.write_text(pyserq.compile(MG1).to_json())
    assert pyserq.draw(ir) == subprocess.run([CLI, "draw", str(ir)], capture_output=True, text=True, check=True).stdout


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
        lambda: pyserq.Rng(1).range_u64(5, 4),
        lambda: pyserq.Rng(1).range_f64(0.0, math.inf),
        lambda: pyserq.compile(MG1, defs={"nope": "1"}),
        lambda: pyserq.draw(MG1, format="png"),
        lambda: pyserq.draw(),
        lambda: pyserq.draw(MG1, sets={"nope": 1}),
    ]:
        try:
            call()
        except ValueError:
            continue
        raise AssertionError("no ValueError")


def test_an_override_on_ir_says_what_it_overrides():
    # IR has its constants folded and its definitions expanded; the refusal
    # names the `let` or the `def`, not the CLI's flag
    ir = Path(tempfile.mkdtemp(prefix="pyserq-")) / "mg1.json"
    ir.write_text(pyserq.compile(MG1).to_json())
    for call, what in [
        (lambda: pyserq.compile(ir, sets={"lam": 1}), "a `let` override"),
        (lambda: pyserq.compile(ir, defs={"service": "~exp(1)"}), "a `def` override"),
        (lambda: pyserq.draw(ir, sets={"lam": 1}), "a `let` override"),
        (lambda: pyserq.draw(ir, defs={"service": "~exp(1)"}), "a `def` override"),
    ]:
        try:
            call()
        except ValueError as e:
            assert str(e).startswith(what) and "--" not in str(e), e
            continue
        raise AssertionError("no ValueError")


def test_an_override_error_names_the_let_or_def():
    # sets= and defs= meet the library's errors; they speak of what is
    # overridden, not of the CLI's --set and --def
    for call, what in [
        (lambda: pyserq.compile(MG1, sets={"nope": 1}), "unknown `let` override `nope`"),
        (lambda: pyserq.compile(MG1, sets={"x y": 1}), "invalid `let` override name `x y`"),
        (lambda: pyserq.compile(MG1, sets={"lam": "~"}), "invalid expression in the `let` override"),
        (lambda: pyserq.compile(MG1, defs={"nope": "1"}), "unknown `def` override `nope`"),
        (lambda: pyserq.compile(MG1, defs={"x y": "1"}), "invalid `def` override name `x y`"),
        (lambda: pyserq.compile(MG1, defs={"service": "~exp("}), "invalid expression in the `def` override"),
    ]:
        try:
            call()
        except ValueError as e:
            assert what in str(e) and "--set" not in str(e) and "--def" not in str(e), e
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


def test_one_version():
    # the wheel's version is the crate's (scripts/version.py), as pip writes it
    assert importlib.metadata.version("pyserq") == pyserq.__version__.replace("-dev.", ".dev")


def test_the_wheel_carries_its_license():
    # pyserq/LICENSE is a link to the repository's; maturin reads LICEN[CS]E*
    meta = importlib.metadata.metadata("pyserq")
    assert meta["License-Expression"] == "Apache-2.0"
    assert any(f.name == "LICENSE" for f in importlib.metadata.files("pyserq"))


if __name__ == "__main__":
    for name, f in list(globals().items()):
        if name.startswith("test_"):
            f()
            print("ok", name)
    print(f"OK: pyserq {pyserq.__version__} (IR {pyserq.IR_VERSION}) agrees with {CLI}")
