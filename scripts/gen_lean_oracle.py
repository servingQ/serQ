#!/usr/bin/env python3
"""Generate lean/Serq/Oracle.lean from the IR of the vLLM scheduler
scenarios in tools/oracle/, and lean/Serq/Regress.lean from the regressions
in tests/lean-regress/ (their IR and the interpreter's answers, see
scripts/lean_regress.py). The oracle scenarios are:

- <name>.ir.json for the six single-request scenarios:
  examples/oracle/vllm_request.sq compiled with the scenario's engine, the
  requests as explicit sessions; answers in <name>.out.json.
- cache_trace.ir.json for the multi-turn prefix-cache scenario:
  examples/replay/vllm_replay.sq on a unit step clock with the trace inlined as
  explicit sessions with turns; answers in cache_trace.out.csv.

Every program, deployment and workload in the Lean file is translated from
that IR, not written by hand: the Lean theorems are about the same IR the
serQ tests run. The translation accepts the fragment of the IR that the
executable semantics (lean/Serq/Exec.lean) covers and fails on anything else.
`--check` fails if the committed file is stale."""
import json
import math
import os
import sys

ROOT = os.path.join(os.path.dirname(__file__), "..")
ODIR = os.path.join(ROOT, "tools", "oracle")
OUT = os.path.join(ROOT, "lean", "Serq", "Oracle.lean")
# 5 added the statements `Release` and `Load` (a KV transfer between two pools),
# which are outside the fragment: a program that uses them fails below.
# 6 adds renewal arrivals and finite open runs, outside explicit-session semantics.
# 7 makes `choose` compare a tuple of keys. 8 lets a run hold several stages
# at once (`Run.also`, under `Program.share`), outside the fragment.
# 9 reevaluates non-FIFO queue keys and supplies Waited, outside this fragment.
# 10 makes a hold's `cache` clause what consumes the session's own prefix
# (serQ #234): a hold without it leaves the entry where it is, and the
# fragment's `admit` (lean/Serq/Exec.lean) does the same; same shape, and no
# oracle program holds a pool with entries without `cache`.
# The translation is one, with 10's meaning: a 7-9 file reads the same
# only where no hold without `cache` meets a pool with entries, which holds
# for every file the generator has read. 11 changed what a `seed` names
# (per-session random streams), which the fragment never reads: its
# programs draw nothing, so a 10 file and an 11 file translate alike. The
# pinned corpus (IR 11) is FIFO and stays inside the fragment.
IR_VERSION = 11
SUPPORTED_IR_VERSIONS = (7, 8, 9, 10, IR_VERSION)


class Fragment(Exception):
    """An IR construct outside the Lean executable fragment."""


# `preempt lifo`, which the parser writes as `preempt by (-admission)`: the
# latest admitted resident, back at the head (`Exec.victim`). The fragment
# knows no other victim order and no `requeue tail`.
LIFO_KEYS = [{"Unary": ["Neg", {"Ctx": "Admission"}]}]


def is_lifo(preempt):
    """Whether a pool's `preempt` is `lifo`: the keys `[-admission]`, back
    at the head (`tail` absent or false)."""
    by = preempt.get("By") if isinstance(preempt, dict) else None
    return by is not None and by.get("keys") == LIFO_KEYS and not by.get("tail", False)


NOT_PREEMPTED = {"Unary": ["Not", {"Ctx": "Preempted"}]}


def only_body(body):
    """The `p` of a step stage's body, when the body is the stage's `serve
    only (p)` (`[Serve {only: p}, Admit {only: p, gate: !preempted}]`, which
    the linker writes), the fragment's `only`; None for no body. Any other
    body is outside the fragment."""
    if body is None:
        return None
    if (len(body) == 2 and "Serve" in body[0] and "Admit" in body[1]
            and body[0]["Serve"].get("by") is None
            and body[0]["Serve"].get("only") is not None
            and body[1]["Admit"].get("only") == body[0]["Serve"]["only"]
            and body[1]["Admit"].get("gate") == NOT_PREEMPTED):
        return body[0]["Serve"]["only"]
    raise Fragment("iteration: the fragment runs vLLM's procedure or a stage's `serve only`, "
                   "not another body")


def nat(v, what):
    if not (isinstance(v, (int, float)) and v >= 0 and float(v).is_integer()):
        raise Fragment(f"{what}: {v} is not a natural number")
    return int(v)


def one_ref(r, what):
    if r["count"] != 1 or r["index"] is not None:
        raise Fragment(f"{what}: families of pools/stages are outside the fragment")
    return r["base"]


def fold(e):
    """The value of an expression that does not depend on the session or the
    context, else None: constants, products with a zero constant, and the
    arithmetic, division and rounding of constants, as the interpreter
    computes them (a negative difference stays negative, and `nat` refuses
    a negative result: a budget or a chunk folds to the interpreter's
    value, not to ℕ's)."""
    if "Num" in e:
        return e["Num"]
    if "Binary" in e:
        op, a, b = e["Binary"]
        x, y = fold(a), fold(b)
        if op == "Mul" and (x == 0 or y == 0):
            return 0.0
        if x is None or y is None:
            return None
        if op == "Div":
            return x / y if y != 0 else None
        return {"Add": x + y, "Sub": x - y, "Mul": x * y}.get(op)
    if "Call" in e:
        f, args = e["Call"]
        vals = [fold(a["Expr"]) if "Expr" in a else None for a in args]
        if any(v is None for v in vals):
            return None
        if f == "Ceil" and len(vals) == 1:
            return float(math.ceil(vals[0]))
        if f == "Floor" and len(vals) == 1:
            return float(math.floor(vals[0]))
        if f in ("Min", "Max") and len(vals) == 2:
            return min(vals) if f == "Min" else max(vals)
    return None


REL = {"Lt": "<", "Le": "≤", "Gt": ">", "Ge": "≥", "Eq": "=", "Ne": "≠"}
ARITH = {"Add": "+", "Sub": "-", "Mul": "*"}


def subtracts(e):
    """Whether an expression has a difference anywhere in it."""
    if isinstance(e, dict):
        if "Binary" in e and e["Binary"][0] == "Sub":
            return True
        return any(subtracts(v) for v in e.values())
    if isinstance(e, list):
        return any(subtracts(v) for v in e)
    return False


class Expr:
    """An IR expression as a Lean term over natural numbers: `nat` gives a
    term of type ℕ (a boolean is 1 or 0), `prop` a proposition (the
    expression is not 0). The arithmetic, comparisons, conditions and
    rounding are the same at every moment; `leaf` translates what is
    specific to one (an attribute, a context variable, an observable).
    `sub=False` refuses a difference where ℕ, which stops at 0, would part
    from the interpreter, which goes negative."""

    def __init__(self, leaf, sub=True):
        self.leaf, self.sub = leaf, sub

    def nat(self, e):
        if not self.sub and subtracts(e):
            raise Fragment("a difference: ℕ stops at 0 where the interpreter goes negative")
        v = fold(e)
        if v is not None:
            return str(nat(v, "constant"))
        if "Binary" in e:
            op, a, b = e["Binary"]
            if op in ARITH:
                return f"({self.nat(a)} {ARITH[op]} {self.nat(b)})"
            if op in REL or op in ("And", "Or"):
                return f"(if {self.prop(e)} then 1 else 0)"
            raise Fragment(f"operator {op} (a division must be under floor, or exact between constants)")
        if "Unary" in e:
            op, a = e["Unary"]
            if op == "Not":
                return f"(if {self.prop(a)} then 0 else 1)"
            raise Fragment(f"unary {op}")
        if "Cond" in e:
            c, a, b = e["Cond"]
            return f"(if {self.prop(c)} then {self.nat(a)} else {self.nat(b)})"
        if "Call" in e:
            f, args = e["Call"]
            if f == "Floor" and len(args) == 1 and "Binary" in args[0].get("Expr", {}):
                op, a, b = args[0]["Expr"]["Binary"]
                if op == "Div":
                    return f"({self.nat(a)} / {self.nat(b)})"
            if f == "Ceil" and len(args) == 1 and "Binary" in args[0].get("Expr", {}):
                op, a, b = args[0]["Expr"]["Binary"]
                if op == "Div" and fold(b) is not None:
                    k = nat(fold(b), "ceil divisor")
                    if k == 0:
                        raise Fragment("ceil of a division by 0")
                    return f"(({self.nat(a)} + {k - 1}) / {k})"
            if f in ("Min", "Max") and len(args) == 2 and all("Expr" in a for a in args):
                return f"({f.lower()} {self.nat(args[0]['Expr'])} {self.nat(args[1]['Expr'])})"
        return self.leaf(e)

    def prop(self, e):
        if "Binary" in e:
            op, a, b = e["Binary"]
            if op in REL:
                return f"({self.nat(a)} {REL[op]} {self.nat(b)})"
            if op == "And":
                return f"({self.prop(a)} ∧ {self.prop(b)})"
            if op == "Or":
                return f"({self.prop(a)} ∨ {self.prop(b)})"
        if "Unary" in e and e["Unary"][0] == "Not":
            return f"(¬ {self.prop(e['Unary'][1])})"
        return f"({self.nat(e)} ≠ 0)"

    def top(self, e):
        """An expression in statement position: drop one pair of outer parentheses."""
        t = self.nat(e)
        return t[1:-1] if t.startswith("(") and t.endswith(")") else t


def chunk_leaf(e):
    """What a step's `chunk` reads as an iteration starts (`Exec.ChunkEnv`)."""
    if "Ctx" in e:
        if e["Ctx"] == "Nres":
            return "c.residents"
        raise Fragment(f"chunk reads {e['Ctx']}: `residents` and `queued(p)` only")
    if "Call" in e:
        f, args = e["Call"]
        if f == "Queued" and len(args) == 1 and "Pool" in args[0]:
            return f"(c.queued {one_ref(args[0]['Pool'], 'queued')})"
        raise Fragment(f"chunk calls {f}")
    raise Fragment(f"chunk {e}")


def chunk_rule(e):
    """A step's chunk as `Exec.Deployment`'s `chunk` and `chunkAt`: a
    constant cap, or the program's expression over what an iteration's
    start reads, `residents` and `queued(p)`, as a Lean function of
    `Exec.ChunkEnv` (vLLM's `residents + queued(p) > 1 ? c : 0`,
    scheduler.py:606-616, is one)."""
    c = fold(e)
    if c is None and "Cond" in e:
        # a rule whose two outcomes are one constant is that constant
        a, b = fold(e["Cond"][1]), fold(e["Cond"][2])
        c = a if a is not None and a == b else None
    if c is not None:
        return c, None
    return 0, f"some fun c => {Expr(chunk_leaf, sub=False).top(e)}"


# the context variables an iteration cost may read, and the field of
# `Exec.IterStats` each one is
COST_VARS = {"Ntok": "tokens", "Npre": "prefilled", "Ndec": "decoders", "Kvb": "kvDecode"}


def affine(e):
    """An expression as {variable or None (the constant): coefficient}, if it
    is affine in the context variables, else None."""
    if "Num" in e:
        return {None: e["Num"]}
    if "Ctx" in e:
        return {e["Ctx"]: 1.0}
    if "Binary" in e:
        op, a, b = e["Binary"]
        x, y = affine(a), affine(b)
        if x is None or y is None:
            return None
        if op in ("Add", "Sub"):
            sign = 1.0 if op == "Add" else -1.0
            out = dict(x)
            for k, v in y.items():
                out[k] = out.get(k, 0.0) + sign * v
            return out
        if op == "Mul":
            if set(x) == {None}:
                return {k: x[None] * v for k, v in y.items()}
            if set(y) == {None}:
                return {k: y[None] * v for k, v in x.items()}
    return None


def cost_fn(e):
    """`Exec.Deployment.cost` of a step engine's `cost`: natural
    coefficients on tokens, prefilled, decoders, kv_decode and attention and
    a constant term of at least 1 (the interpreter lets an iteration last 0;
    `Exec` lasts at least one clock unit, so the two agree only when the
    constant is at least 1). `IterStats` holds twice the attention work, so
    the coefficient of `attention` must be even; anything else is outside
    the fragment."""
    a = affine(e)
    if a is None:
        raise Fragment("the iteration cost must be affine in the context variables")
    if a.get(None, 0.0) < 1:
        raise Fragment("iteration cost: the constant term must be at least 1 clock unit "
                       "(the interpreter allows an iteration of length 0, the fragment does not)")
    terms = []
    for k, v in a.items():
        if v == 0:
            continue
        if v < 0 or v != int(v):
            raise Fragment(f"iteration cost coefficient {v}: clock units are natural numbers")
        if k is None:
            terms.append(str(int(v)))
        elif k in COST_VARS:
            terms.append(f"{int(v)} * st.{COST_VARS[k]}")
        elif k == "Attn":
            if int(v) % 2:
                raise Fragment(f"attention coefficient {v}: attention is a multiple of 1/2, "
                               "so an odd coefficient leaves the clock's natural numbers")
            terms.append(f"{int(v) // 2} * st.attention2")
        else:
            raise Fragment(f"iteration cost reads {k}, which the fragment's IterStats does not have")
    return "fun _ => 1" if terms == ["1"] else f"fun st => {' + '.join(terms) or '0'}"


class Lean:
    """IR (a serQ program as JSON) to the Lean surface syntax `[route| … ]`
    of `Route Exec.Env ℕ`. Attribute slots are the IR's; the built-in
    `cached` and `serial` read the environment."""

    def __init__(self, ir):
        self.ir = ir
        self.builtin = {ir["slot_cached"]: "x.cached", ir["slot_serial"]: "x.serial"}

    def leaf(self, e):
        """What a session statement reads: attributes, `now`, the engine's
        budget left and a pool's cached prefix."""
        if "Attr" in e:
            k = e["Attr"]
            return self.builtin.get(k, f"(x.attr {k})")
        if "Ctx" in e:
            if e["Ctx"] == "Now":
                return "x.now"
            raise Fragment(f"context variable {e['Ctx']}")
        if "Call" in e:
            f, args = e["Call"]
            if f == "BudgetLeft" and len(args) == 1 and "Stage" in args[0]:
                if one_ref(args[0]["Stage"], "budget_left") != 0:
                    raise Fragment("budget_left of a stage other than the engine")
                return "x.budgetLeft"
            if f == "CachedIn" and len(args) == 1 and "Pool" in args[0]:
                return f"(x.cachedIn {one_ref(args[0]['Pool'], 'cachedin')})"
            raise Fragment(f"call {f}")
        raise Fragment(f"expression {e}")

    def top(self, e):
        """An expression in statement position: drop one pair of outer parentheses."""
        return Expr(self.leaf).top(e)

    def block(self, b, ind):
        pad = "  " * ind
        out = []
        for st in self.ir["blocks"][b]:
            if st == "End":
                out.append(pad + "stop")
                return "\n".join(out)
            if st == "Turn":
                out.append(pad + "turn;")
                continue
            (kind, v), = st.items()
            if kind == "Set":
                slot, e = v
                if slot in self.builtin:
                    raise Fragment(f"set of the built-in `{self.ir['attrs'][slot]}`")
                out.append(f"{pad}set {slot} = {self.top(e)};")
            elif kind == "Observe":
                k, e = v
                out.append(f"{pad}observe {k} = {self.top(e)};")
            elif kind == "Run":
                if v.get("also"):
                    raise Fragment("a run over several stages (`also`)")
                s = one_ref(v["stage"], "run")
                mode = {"Plain": "", "Prefill": " prefill", "Decode": " decode"}[v["mode"]]
                if (s == 0) == (mode == ""):
                    raise Fragment("prefill/decode run the engine, plain runs a delay")
                g = ""
                if v["growing"]:
                    gp = one_ref(v["growing"], "growing")
                    if gp != self.ir["stages"][0]["kind"]["Step"]["memory"]:
                        raise Fragment(f"growing pool {gp}, not the engine's memory (the fragment's "
                                       "preemption victim is the engine's rule)")
                    g = f" growing {gp}"
                out.append(f"{pad}run {s}{mode} ({self.top(v['work'])}){g};")
            elif kind == "Hold":
                if v.get("lease") is not None:
                    raise Fragment("a hold with a lease is outside the fragment")
                ps = []
                for r, u, fits in v["pools"]:
                    f = f" fits ({self.top(fits)})" if fits is not None else ""
                    ps.append(f"{one_ref(r, 'hold')} ({self.top(u)}){f}")
                ru = f" reuse ({self.top(v['reuse'])})" if v["reuse"] is not None else ""
                ca = f" cache ({self.top(v['cache'])})" if v["cache"] is not None else ""
                out.append(f"{pad}hold {', '.join(ps)}{ru} {{")
                out.append(self.block(v["body"], ind + 1))
                out.append(f"{pad}}}{ca};")
            elif kind == "Branch":
                c, t, f = v
                out.append(f"{pad}branch ({self.top(c)}) {{")
                out.append(self.block(t, ind + 1))
                out.append(f"{pad}}} else {{")
                out.append(self.block(f, ind + 1))
                out.append(f"{pad}}};")
            elif kind == "Loop":
                out.append(f"{pad}loop {{")
                out.append(self.block(v, ind + 1))
                out.append(f"{pad}}}")
                return "\n".join(out)
            else:
                raise Fragment(f"statement {kind}")
        out.append(pad + "done")
        return "\n".join(out)

    def deployment(self):
        ir = self.ir
        st = ir["stages"]
        if not st or "Step" not in st[0]["kind"] or any(s["kind"] != "Delay" for s in st[1:]):
            raise Fragment("stages must be one step engine (stage 0) and delays")
        step = st[0]["kind"]["Step"]
        cost = cost_fn(step["cost"])
        if step["serve"] != {"By": []}:
            raise Fragment(f"serve {step['serve']}: the fragment serves residents in admission order (`By([])`)")
        if step.get("iteration") is not None:
            raise Fragment("iteration: the oracle fragment runs vLLM's procedure, not a body "
                           "(a stage's `serve only` is one)")
        pools = []
        for i, p in enumerate(ir["pools"]):
            if p.get("reserve_held"):
                raise Fragment(f"pool {p['name']}: reserve held")
            if p["evict"] != "Lru" or p["queue"] is not None or p["spill"] is not None:
                raise Fragment(f"pool {p['name']}: only LRU eviction, FIFO queue, no spill")
            via = p["admit_via"] is not None
            if via and p["admit_via"] != 0:
                raise Fragment(f"pool {p['name']}: admitted by a stage other than the engine")
            if not (p["preempt"] == "None" if via else is_lifo(p["preempt"])):
                raise Fragment(f"pool {p['name']}: preemption {p['preempt']}")
            if not via and step["memory"] != i:
                raise Fragment(f"pool {p['name']}: not the engine's memory")
            pools.append(f"⟨{nat(p['cap'], 'cap')}, {nat(p['block'] or 1, 'block')}, {'true' if via else 'false'}, none⟩")
        chunk, at = chunk_rule(step["chunk"])
        return (f"⟨[{', '.join(pools)}], {nat(fold(step['budget']), 'budget')}, "
                f"{nat(chunk, 'chunk')}, "
                f"{'none' if step['memory'] is None else 'some ' + str(step['memory'])}, {cost}, none, "
                f"{at or 'none'}⟩")

    def workload(self):
        """`Exec.Workload` of the IR's explicit sessions."""
        ir = self.ir
        ss = ir["arrival"].get("Sessions") if isinstance(ir["arrival"], dict) else None
        if not ss:
            raise Fragment("the workload must be explicit sessions (`CArrival::Sessions`)")
        if ir["trace"] is not None:
            raise Fragment("a trace file: inline it (`serq ir --inline-trace`)")
        for st in ir["blocks"][ir["init"]]:
            if "Set" not in st or any(st["Set"][0] not in {a for a, _ in s["attrs"]} for s in ss):
                raise Fragment("`init` does more than every session's presets override")
        if ir["blocks"][ir["turn"]]:
            raise Fragment("a `turn` block (the fragment reads turns from the workload only)")

        def pairs(xs):
            return "[" + ", ".join(f"({slot}, {nat(v, 'attribute value')})" for slot, v in xs) + "]"

        init = "[" + ", ".join(pairs(s["attrs"]) for s in ss) + "]"
        if any(s.get("turns") for s in ss):
            turns = "[" + ", ".join("[" + ", ".join(pairs(t) for t in s.get("turns", [])) + "]" for s in ss) + "]"
            return f"⟨{init}, {turns}, some {ir['slot_turn']}, {ir['slot_more']}, some {ir['slot_computed']}, none⟩"
        return f"⟨{init}, [], none, 0, some {ir['slot_computed']}, none⟩"


def load(name):
    with open(os.path.join(ODIR, name + ".ir.json")) as source:
        ir = json.load(source)
    if ir["version"] not in SUPPORTED_IR_VERSIONS:
        raise Fragment(f"{name}: IR version {ir['version']} (this generator reads {SUPPORTED_IR_VERSIONS})")
    if ir.get("share") is not None:
        raise Fragment(f"{name}: shared multi-stage execution is outside the fragment")
    if ir.get("arrivals") is not None:
        raise Fragment(f"{name}: finite arrival limits are outside the fragment")
    return ir, Lean(ir)


def doc_tables(ir):
    pools = ", ".join(f"{i} = {p['name']}" for i, p in enumerate(ir["pools"]))
    stages = ", ".join(f"{i} = {s['name']}" for i, s in enumerate(ir["stages"]))
    obs = ", ".join(f"{i} = {n}" for i, n in enumerate(ir["observes"]))
    skip = {ir["slot_cached"], ir["slot_serial"]}
    attrs = ", ".join(f"{i} = {n}" for i, n in enumerate(ir["attrs"]) if i not in skip)
    return f"Attributes: {attrs}. Observations: {obs}. Pools: {pools}. Stages: {stages}."


def program(defname, names, source):
    """A Lean program from the IR session program of every named scenario; the programs
    must be the same program (only constants in the deployment and the
    workload differ)."""
    progs = {}
    for name in names:
        ir, lean = load(name)
        progs[name] = (lean.block(ir["session"], 1), doc_tables(ir))
    body, tables = next(iter(progs.values()))
    for name, p in progs.items():
        if p != (body, tables):
            raise Fragment(f"{name}: its session program differs from the other scenarios'")
    return f"""/-- {source}, translated from its IR. {tables} -/
def {defname} : Prog := [route|
{body}]

theorem {defname}_wf : {defname}.wf = true := by decide
"""


HEAD = '''/-
# vLLM scheduler scenarios as theorems about serQ programs

Generated by `scripts/gen_lean_oracle.py` from the IR of serQ's oracle
scenarios (`tools/oracle/*.ir.json`) and the real vLLM v1 scheduler's
answers (`*.out.json`, `cache_trace.out.csv`: serQ `tools/vllm_oracle.py`
and `tools/vllm_replay_oracle.py`, upstream `ref/vllm` at 0c87a197).
Do not edit. The programs, the deployments and the workloads below are
translations of that IR, the same IR the serQ tests run.

Six scenarios run one request program, `vllmRequest` (serQ
`examples/oracle/vllm_request.sq`): wait until the arrival, hold a slot and the
KV blocks of the chunk the engine's budget leaves (admission needs room for
the whole prompt), prefill growing the hold, decode growing it. The
theorems say that the executable semantics gives, for every request, the
step of its first token and of its last token, and the number of
preemptions, that the real scheduler gives. The seventh, `vllmTurn` (serQ
`examples/replay/vllm_replay.sq` on a unit step clock, the trace inlined as the
sessions' turns), is a multi-turn replay with a prefix cache: hits, a
partial hit and misses caused by eviction; its theorem gives, for every
turn, the send step, the time to first token, the latency and the cached
tokens that the real scheduler and KV-cache manager give. All are checked
by evaluation in the kernel (`decide +kernel`: no axiom beyond the
standard three).
-/
import Serq.Exec

set_option maxRecDepth 100000

namespace SerqLang
namespace Oracle

open Exec

{REQUEST}
/-- (first-token steps, last-token steps, preemptions) of `vllmRequest`. -/
def outcome (D : Deployment) (horizon : ℕ) (w : Workload) :
    List (ℕ × ℕ) × List (ℕ × ℕ) × ℕ :=
  let m := Exec.runW D horizon w vllmRequest
  (observed m 0, observed m 1, m.preempts)
'''


def request_scenarios():
    return sorted(f[:-9] for f in os.listdir(ODIR) if f.endswith(".out.json") and os.path.exists(os.path.join(ODIR, f[:-9] + ".json")))


def gen():
    names = request_scenarios()
    for n in names:
        if not os.path.exists(os.path.join(ODIR, n + ".ir.json")):
            raise Fragment(f"{n}: no IR file ({n}.ir.json)")
    out = [HEAD.replace("{REQUEST}", program("vllmRequest", names, "The vLLM request program (serQ `examples/oracle/vllm_request.sq`)"))]
    for name in names:
        sc = json.load(open(os.path.join(ODIR, name + ".json")))
        ans = json.load(open(os.path.join(ODIR, name + ".out.json")))
        ir, lean = load(name)
        n = len(ir["arrival"]["Sessions"])
        if n != len(sc["requests"]):
            raise Fragment(f"{name}: {n} sessions in the IR, {len(sc['requests'])} requests in the scenario")
        first = sorted((int(k), v) for k, v in ans["first"].items())
        done = sorted((int(k), v) for k, v in ans["done"].items())
        horizon = max([v for _, v in done] + [0]) + 5
        fs = ", ".join(f"({k}, {v})" for k, v in first)
        ds = ", ".join(f"({k}, {v})" for k, v in done)
        obs = ir["observes"]
        if obs[:2] != ["first", "done"]:
            raise Fragment(f"{name}: observations {obs} (outcome reads 0 = first, 1 = done)")
        out.append(f'''
/-- serQ `tools/oracle/{name}.ir.json`: {n} requests, {sc["num_blocks"]} blocks of {sc["block_size"]}, budget {sc["budget"]}, {sc["max_seqs"]} slots, chunk {sc.get("chunk", 0)}; the deployment and the workload are the IR's. -/
theorem vllm_{name} :
    outcome {lean.deployment()} {horizon}
      {lean.workload()} =
    ([{fs}], [{ds}], {ans["preemptions"]}) := by
  decide +kernel
''')
    out.append(gen_cache())
    out.append("\nend Oracle\nend SerqLang\n")
    return "".join(out)


def gen_cache():
    ir, lean = load("cache_trace")
    rows = {}
    for l in open(os.path.join(ODIR, "cache_trace.out.csv")).read().splitlines()[1:]:
        s, k, sent, first, done, prompt, cached, o = l.split(",")
        rows[(int(s), int(k))] = tuple(nat(float(x), "answer") for x in (sent, first, done, cached))
    keys = sorted(rows)
    horizon = max(r[2] for r in rows.values()) + 5
    ob = {n: i for i, n in enumerate(ir["observes"])}
    for n in ("sent", "ttft", "latency", "cached_tokens"):
        if n not in ob:
            raise Fragment(f"cache_trace: no observation `{n}`")

    def col(f):
        return "[" + ", ".join(f"({s}, {f(rows[(s, k)])})" for s, k in keys) + "]"

    return "\n/-! ### A multi-turn scenario with a prefix cache -/\n\n" + program(
        "vllmTurn", ["cache_trace"], "The vLLM replay program (serQ `examples/replay/vllm_replay.sq`) on a unit step clock"
    ) + f'''
/-- serQ `tools/oracle/cache_trace.ir.json`: {len(ir["arrival"]["Sessions"])} sessions of the inlined trace
`cache_trace.csv`; the deployment and the workload are the IR's. Per turn:
send step, time to first token, latency, cached tokens at admission. -/
theorem vllm_cache_trace :
    let m := Exec.runW {lean.deployment()} {horizon}
      {lean.workload()} vllmTurn
    (observed m {ob["sent"]}, observed m {ob["ttft"]}, observed m {ob["latency"]}, observed m {ob["cached_tokens"]}) =
      ({col(lambda r: r[0])},
       {col(lambda r: r[1] - r[0])},
       {col(lambda r: r[2] - r[0])},
       {col(lambda r: r[3])}) := by
  decide +kernel
'''


RDIR = os.path.join(ROOT, "tests", "lean-regress")
ROUT = os.path.join(ROOT, "lean", "Serq", "Regress.lean")

RHEAD = """/-
# Regressions: the Lean semantics against the interpreter

Generated by `scripts/gen_lean_oracle.py` from `tests/lean-regress/`: each
program's IR (`<name>.ir.json`, its sessions explicit) and the answer
`serq run` gives on it (`<name>.out.json`, recorded by
`scripts/lean_regress.py`, which `make drt` checks against the current
interpreter). Do not edit. Each theorem runs the program in the executable
semantics and states every observation, in `Exec.observed` order, and the
number of preemptions. These are the cases differential testing found, or
constructs its random cases do not reach.
-/
import Serq.Exec

set_option maxRecDepth 100000

namespace SerqLang
namespace Regress

open Exec
"""


def camel(name):
    head, *rest = name.split("_")
    return head + "".join(w.capitalize() for w in rest)


def gen_regress():
    out = [RHEAD]
    names = sorted(f[:-len(".ir.json")] for f in os.listdir(RDIR) if f.endswith(".ir.json"))
    for name in names:
        ir = json.load(open(os.path.join(RDIR, name + ".ir.json")))
        ans = json.load(open(os.path.join(RDIR, name + ".out.json")))
        if ir["version"] not in SUPPORTED_IR_VERSIONS:
            raise Fragment(f"{name}: IR version {ir['version']}")
        lean = Lean(ir)
        obs = ir["observes"]
        lhs = ", ".join(f"observed m {i}" for i in range(len(obs)))
        rhs = ", ".join("[" + ", ".join(f"({s}, {v})" for s, v in ans["observes"].get(n, [])) + "]" for n in obs)
        horizon = nat(ir["horizon"], "horizon")
        out.append(f'''
/-- `tests/lean-regress/{name}.sq`, translated from its IR. {doc_tables(ir)} -/
def {camel(name)} : Prog := [route|
{lean.block(ir["session"], 1)}]

/-- `serq run`: {", ".join(obs)} per session, and {ans["preemptions"]} preemptions. -/
theorem regress_{name} :
    let m := Exec.runW {lean.deployment()} {horizon}
      {lean.workload()} {camel(name)}
    ([{lhs}], m.preempts) =
    ([{rhs}], {ans["preemptions"]}) := by
  decide +kernel
''')
    out.append("\nend Regress\nend SerqLang\n")
    return "".join(out)


if __name__ == "__main__":
    try:
        files = [(OUT, gen()), (ROUT, gen_regress())]
    except Fragment as e:
        print("FAIL: outside the Lean fragment:", e)
        sys.exit(1)
    for path, txt in files:
        rel = os.path.relpath(path, ROOT)
        if "--check" in sys.argv:
            if open(path).read() != txt:
                print(f"STALE: {rel}; run scripts/gen_lean_oracle.py")
                sys.exit(1)
            print(f"{rel} is current")
        else:
            open(path, "w").write(txt)
            print("wrote", rel)
