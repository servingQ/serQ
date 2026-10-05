#!/usr/bin/env python3
"""Generate lean/Serq/Claims.lean, the claims of the programs under
examples/papers/ as Lean statements about the executable semantics, and
examples/papers/ClaimsProved.lean, which checks that examples/papers/ proves
every one of them.

The input is the IR of each program (tools/claims/<name>.ir.json, written
by `make claims-ir`, checked current by `cargo test --test claims`). For
each program the generator writes, from that IR and nothing else:

- `prog`: the session program, preceded by what the workload does before
  it: under `renewal` or `poisson` arrivals a delay until the session's
  arrival time (attribute `arrive`), then the `init` block's sets that do
  not draw;
- `deployment`: the step engine and the pools;
- `family_<claim>`: the workloads a claim quantifies over. Every number of
  sessions up to the fragment's bound; each session's drawn `init`
  attributes any natural numbers (a superset of their support, so a claim
  proved over it holds for the program's distribution); the arrival times
  any natural numbers under `poisson`, `(i + 1) * gap` under a constant
  `renewal`, 0 under `batch`; and the claim's own `given` for each
  session (one family per claim: a `given` restricts its claim only). An existential claim (`some iteration`) needs
  the family to be the support exactly, so it is refused when `init` draws;
- one `Prop` per claim, `Exec.EveryIteration`, `Exec.SomeIteration` or
  `Exec.AtEnd` of the claim's expression.

The translation reads natural numbers: subtraction truncates and a
division must be under `floor` or between constants that divide exactly.
Anything else is outside the fragment and fails. `--check` fails if a
committed file is stale."""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from gen_lean_oracle import (  # noqa: E402
    Expr, Fragment, Lean, affine, chunk_rule, fold, nat, one_ref, COST_VARS, LIFO,
)

ROOT = os.path.join(os.path.dirname(__file__), "..")
CDIR = os.path.join(ROOT, "tools", "claims")
OUT = os.path.join(ROOT, "lean", "Serq", "Claims.lean")
PROVED = os.path.join(ROOT, "examples", "papers", "ClaimsProved.lean")
IR_VERSION = 11
# the fragment's bound on the number of sessions, under which the fuel of
# `settleLoop`, `drain` and `admitHeads` is shown to suffice (examples/papers/Kong.lean)
MAX_SESSIONS = 500


def camel(name):
    return "".join(w.capitalize() for w in name.split("_"))


def has_sample(e):
    if isinstance(e, dict):
        if "Sample" in e:
            return True
        return any(has_sample(v) for v in e.values())
    if isinstance(e, list):
        return any(has_sample(v) for v in e)
    return False


def iter_leaf(e):
    if "Ctx" in e:
        v = {"Ntok": "r.stats.tokens", "Npre": "r.stats.prefilled", "Ndec": "r.stats.decoders",
             "Kvb": "r.stats.kvDecode", "Nres": "r.residents", "Demand": "r.demand",
             "Served": "r.served", "Arrived": "r.arrived", "Now": "r.start"}.get(e["Ctx"])
        if v:
            return v
        raise Fragment(f"an iteration claim reads {e['Ctx']}")
    if "Call" in e:
        f, args = e["Call"]
        if f in ("Queued", "Holders", "Used") and len(args) == 1 and "Pool" in args[0]:
            p = one_ref(args[0]["Pool"], f)
            return f"(r.{f.lower()}.getD {p} 0)"
    raise Fragment(f"an iteration claim reads {e}")


def end_leaf(e):
    if "Agg" in e:
        kind, o = e["Agg"]
        if kind == "Total":
            return f"(Exec.total m {o})"
        if kind == "Count":
            return f"(Exec.count m {o})"
        if kind == "PrefixTotal":
            return f"(Exec.prefixTotal (Exec.values m {o}))"
        raise Fragment(f"aggregate {kind}")
    if "Ctx" in e and e["Ctx"] == "Now":
        return "m.now"
    raise Fragment(f"an end claim reads {e}")


def serve_leaf(e):
    if "Ctx" in e:
        v = {"Decoding": "e.decoding", "Admission": "e.admission", "Remaining": "e.remaining",
             "Nres": "e.residents", "Ndec": "e.decoders"}.get(e["Ctx"])
        if v:
            return v
    if "Attr" in e:
        return f"(e.attr {e['Attr']})"
    raise Fragment(f"serve only reads {e}")


class Program:
    def __init__(self, name, ir):
        self.name, self.ir = name, ir
        if ir["version"] != IR_VERSION:
            raise Fragment(f"{name}: IR version {ir['version']}")
        for what in ("share", "arrivals", "trace"):
            if ir.get(what) is not None:
                raise Fragment(f"{name}: {what}")
        self.lean = Lean(ir)
        self.arr_slot = len(ir["attrs"])  # a fresh slot for the arrival time
        arr = ir["arrival"]
        (self.akind, self.aval), = arr.items() if isinstance(arr, dict) else ((arr, None),)
        if self.akind not in ("Poisson", "Renewal", "Batch"):
            raise Fragment(f"{name}: arrivals {self.akind}")
        self.init = ir["blocks"][ir["init"]]
        if ir["blocks"][ir["turn"]]:
            raise Fragment(f"{name}: a turn block")
        self.drawn = set()
        self.defs = {}  # attribute -> its init expression, for `given`
        for st in self.init:
            if "Set" not in st:
                raise Fragment(f"{name}: init does more than set")
            slot, e = st["Set"]
            if has_sample(e):
                self.drawn.add(slot)
            else:
                self.defs[slot] = e

    def gap(self):
        if self.akind != "Renewal":
            return None
        g = fold(self.aval)
        if g is None:
            raise Fragment(f"{self.name}: a renewal gap that is not a constant")
        return nat(g, "renewal gap")

    def prog(self):
        lean = self.lean
        pre = []
        if self.akind != "Batch":
            pre.append(f"  run 1 (x.attr {self.arr_slot});")
        for st in self.init:
            slot, e = st["Set"]
            if slot in self.drawn:
                continue
            pre.append(f"  set {slot} = {lean.top(e)};")
        body = lean.block(self.ir["session"], 1)
        return "[route|\n" + "\n".join(pre + [body]) + "]"

    def deployment(self):
        ir = self.ir
        st = ir["stages"]
        if not st or "Step" not in st[0]["kind"] or any(s["kind"] != "Delay" for s in st[1:]):
            raise Fragment("stages must be one step engine (stage 0) and delays")
        step = st[0]["kind"]["Step"]
        if step["serve"] not in ({"By": []}, "DecodeFirst"):
            raise Fragment(f"serve {step['serve']}: the fragment serves residents in admission order")
        cost = cost_fn(step["cost"])
        only = "none"
        if step.get("only") is not None:
            only = f"some fun e => {Expr(serve_leaf).nat(step['only'])}"
        grown = self.grown_pools()
        pools = []
        for i, p in enumerate(ir["pools"]):
            if p["evict"] != "Lru" or p["spill"] is not None or p["admit_via"] is not None:
                raise Fragment(f"pool {p['name']}: LRU eviction, no spill, no admit via")
            if i in grown and (p["preempt"] != LIFO or step["memory"] != i):
                raise Fragment(f"pool {p['name']}: a grown pool is the engine's memory under preempt lifo")
            key = "none"
            if p["queue"] is not None:
                if i in grown:
                    raise Fragment(f"pool {p['name']}: queue keys on a pool that may preempt")
                if len(p["queue"]) != 1:
                    raise Fragment(f"pool {p['name']}: one queue key")
                key = f"some fun x => {self.lean.top(p['queue'][0])}"
            pools.append(f"⟨{nat(p['cap'], 'cap')}, {nat(p['block'] or 1, 'block')}, false, {key}⟩")
        chunk, at = chunk_rule(step["chunk"])
        return (f"⟨[{', '.join(pools)}], {nat(fold(step['budget']), 'budget')}, "
                f"{nat(chunk, 'chunk')}, "
                f"{'none' if step['memory'] is None else 'some ' + str(step['memory'])}, {cost}, {only}, "
                f"{at or 'none'}⟩")

    def grown_pools(self):
        out = set()

        def walk(b):
            for st in self.ir["blocks"][b]:
                if not isinstance(st, dict):
                    continue
                (k, v), = st.items()
                if k == "Run" and v["growing"]:
                    out.add(one_ref(v["growing"], "growing"))
                if k == "Hold":
                    walk(v["body"])
                if k == "Branch":
                    walk(v[1]); walk(v[2])
                if k == "Loop":
                    walk(v)
        walk(self.ir["session"])
        return out

    def given_expr(self, claims):
        """The conjunction of the claims' `given`, over a session's preset
        attributes `a` (and its arrival time), with init's sets substituted."""
        gs = [c["given"] for c in claims if c.get("given") is not None]
        if not gs:
            return "True"
        prog = self

        def subst(e):
            if isinstance(e, dict):
                if "Attr" in e and e["Attr"] in prog.defs:
                    return subst(prog.defs[e["Attr"]])
                if "Ctx" in e and e["Ctx"] == "Now":
                    return {"Attr": prog.arr_slot} if prog.akind != "Batch" else {"Num": 0.0}
                return {k: subst(v) for k, v in e.items()}
            if isinstance(e, list):
                return [subst(v) for v in e]
            return e

        def leaf(e):
            if "Attr" in e:
                return f"(a {e['Attr']})"
            raise Fragment(f"given reads {e}")
        return " ∧ ".join(Expr(leaf).prop(subst(g)) for g in gs)

    def family(self, claims):
        conds = [f"w.init.length ≤ {MAX_SESSIONS}", "w.turns = []", "w.turnSlot = none",
                 f"w.computedSlot = some {self.ir['slot_computed']}"]
        if self.akind != "Batch":
            conds.append(f"w.arriveSlot = some {self.arr_slot}")
        g = self.given_expr(claims)
        per = [] if g == "True" else [f"({g})".replace("(a ", "(w.attr i ")]
        if self.gap() is not None:
            per.append(f"w.attr i {self.arr_slot} = (i + 1) * {self.gap()}")
        if per:
            conds.append(f"∀ i < w.init.length, {' ∧ '.join(per)}")
        return " ∧\n    ".join(conds)

    def claim(self, c):
        k = c["kind"]
        if isinstance(k, dict) and ("EveryIteration" in k or "SomeIteration" in k):
            stage = k.get("EveryIteration", k.get("SomeIteration"))
            if stage != 0:
                raise Fragment("a claim over a stage other than the engine")
            body = Expr(iter_leaf).prop(c["expr"])
            if "SomeIteration" in k:
                if self.drawn:
                    raise Fragment(f"{c['name']}: an existential claim over a workload that draws")
                return f"SomeIteration deployment family_{c['name']} prog fun r => {body}"
            return f"EveryIteration deployment family_{c['name']} prog fun r => {body}"
        if k == "AtEnd":
            return f"AtEnd deployment family_{c['name']} prog fun m => {Expr(end_leaf).prop(c['expr'])}"
        raise Fragment(f"claim kind {k}")


def cost_fn(e):
    """`Exec.Deployment.cost`: the oracle generator's affine costs, plus
    terms `k * ceil(tokens / b)`."""
    terms = []

    def split(e):
        # an affine sum whose ceil terms are pulled out
        if "Binary" in e and e["Binary"][0] == "Add":
            return split(e["Binary"][1]) + split(e["Binary"][2])
        return [e]

    rest = []
    for t in split(e):
        c = None
        if "Binary" in t and t["Binary"][0] == "Mul":
            _, a, b = t["Binary"]
            for k, x in ((a, b), (b, a)):
                if fold(k) is not None and "Call" in x and x["Call"][0] == "Ceil":
                    c = (fold(k), x)
        elif "Call" in t and t["Call"][0] == "Ceil":
            c = (1.0, t)
        if c is None:
            rest.append(t)
            continue
        k, call = c
        arg = call["Call"][1][0]["Expr"]
        if not ("Binary" in arg and arg["Binary"][0] == "Div" and arg["Binary"][1] == {"Ctx": "Ntok"}
                and fold(arg["Binary"][2]) is not None):
            raise Fragment("a ceil term of the cost must be ceil(tokens / b)")
        b = nat(fold(arg["Binary"][2]), "ceil divisor")
        terms.append(f"{nat(k, 'cost coefficient')} * ((st.tokens + {b - 1}) / {b})")
    a = {}
    for t in rest:
        x = affine(t)
        if x is None:
            raise Fragment("the iteration cost must be affine in the context variables and ceil(tokens / b)")
        for k, v in x.items():
            a[k] = a.get(k, 0.0) + v
    for k, v in a.items():
        if v == 0:
            continue
        if k is None:
            terms.insert(0, str(nat(v, "cost constant")))
        elif k in COST_VARS:
            terms.append(f"{nat(v, 'cost coefficient')} * st.{COST_VARS[k]}")
        else:
            raise Fragment(f"iteration cost reads {k}")
    body = ' + '.join(terms) or '0'
    return f"fun st => {body}" if "st." in body else f"fun _ => {body}"


HEAD = '''/-
# The claims of the paper programs, as statements about the executable semantics

Generated by `scripts/gen_lean_claims.py` from the IR of the programs under
`examples/papers/` (`tools/claims/*.ir.json`). Do not edit. For each
program: its session program (after the workload's arrival delay and the
`init` sets that do not draw), its deployment, the family of workloads its
claims quantify over, and each claim as an `Exec.EveryIteration`,
`Exec.SomeIteration` or `Exec.AtEnd` statement (`Serq/Claim.lean`). The
proofs are in `examples/papers/`; `examples/papers/ClaimsProved.lean` checks that every
claim has one.
-/
import Serq.Claim

set_option maxRecDepth 100000

namespace SerqLang
namespace Claims

open Exec
'''


def names():
    return sorted(f[:-len(".ir.json")] for f in os.listdir(CDIR) if f.endswith(".ir.json"))


def gen():
    out = [HEAD]
    proved = ['''/-
# Every claim of the paper programs is proved

Generated by `scripts/gen_lean_claims.py`. Do not edit. Each line states a
claim of `Serq/Claims.lean` and names its proof in `examples/papers/`: the build
fails if a claim has no proof, or if a proof proves something else than
what the program now claims.
-/
import Serq.Claims
{IMPORTS}

namespace SerqLang
namespace ClaimsProved
''']
    papers = os.path.join(ROOT, "examples", "papers")
    stems = sorted(f[:-len(".lean")] for f in os.listdir(papers)
                   if f.endswith(".lean") and f != "ClaimsProved.lean")
    proved[0] = proved[0].replace("{IMPORTS}", "\n".join(f"import papers.{s}" for s in stems))
    for name in names():
        ir = json.load(open(os.path.join(CDIR, name + ".ir.json")))
        p = Program(name, ir)
        claims = ir.get("claims", [])
        ns = camel(name)
        tables = (f"Attributes: {', '.join(f'{i} = {a}' for i, a in enumerate(ir['attrs']))}"
                  + (f", {p.arr_slot} = arrive" if p.akind != "Batch" else "")
                  + f". Observations: {', '.join(f'{i} = {o}' for i, o in enumerate(ir['observes']))}.")
        out.append(f'''
/-! ## `examples/papers/{name}.sq` -/

namespace {ns}

/-- The session program of `examples/papers/{name}.sq`. {tables} -/
def prog : Prog := {p.prog()}

def deployment : Deployment :=
  {p.deployment()}
''')
        for c in claims:
            out.append(f'''
/-- The workloads `claim {c["name"]}` quantifies over: its own `given` only. -/
def family_{c["name"]} (w : Workload) : Prop :=
    {p.family([c])}

/-- `claim {c["name"]}` of `{name}.sq`. -/
def {c["name"]} : Prop :=
  {p.claim(c)}
''')
            proved.append(f"example : Claims.{ns}.{c['name']} := Papers.{ns}.{c['name']}\n")
        out.append(f"\nend {ns}\n")
    out.append("\nend Claims\nend SerqLang\n")
    proved.append("\nend ClaimsProved\nend SerqLang\n")
    return "".join(out), "".join(proved)


if __name__ == "__main__":
    try:
        claims, proved = gen()
    except Fragment as e:
        print("FAIL: outside the Lean fragment:", e)
        sys.exit(1)
    for path, txt in ((OUT, claims), (PROVED, proved)):
        rel = os.path.relpath(path, ROOT)
        if "--check" in sys.argv:
            if open(path).read() != txt:
                print(f"STALE: {rel}; run scripts/gen_lean_claims.py")
                sys.exit(1)
            print(f"{rel} is current")
        else:
            open(path, "w").write(txt)
            print("wrote", rel)
