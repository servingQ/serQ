#!/usr/bin/env python3
"""Generate lean/Serq/Oracle.lean from the IR of the vLLM scheduler
scenarios in tools/oracle/, and lean/Serq/Regress.lean from the regressions
in tests/lean-regress/ (their IR and the interpreter's answers, see
scripts/lean_regress.py). The oracle scenarios are:

- <name>.ir.json for the single-request scenarios:
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
# at once (`Run.also`, under its `Run.share`), outside the fragment.
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
# pinned corpus (IR 12) is FIFO and stays inside the fragment.
# 12 adds resource cost conversions, erased without changing numeric work.
# 13 removes the `Delay` stage kind: a delay is `Ps` of `present`, every job
# at rate 1, and `is_delay` reads either spelling. It also reseeds the
# session streams (serQ #297), which the fragment never reads, as for 11.
# 14 adds `Run.latency`, a wait before a run's work at its stages, which was
# a delay stage of its own run before it; outside the fragment (below, with
# `also`), and no oracle program has one. Untagged, 14 also gains
# `Run.poster`, which side of a copy posts it: only on a run with `also`,
# already outside the fragment, and the run is the same whichever it is.
# 15 (docs/design/request-state.md) makes a connector's `load` and `save` a
# `CTransfer` each where they were blocks, adds `CRequest.send` (a KV relation's
# source keeps a request's blocks for the reader's load) and removes the
# statement `Join`. A connector was outside the fragment and stays so (below),
# a send is, and no oracle program had a `Join`: a 14 file and a 15 file
# translate alike.
# 16 (docs/design/tick-count.md) makes `Preempt::Leases.suspend` a
# `{keys, count}`, the count a tick judges a set-aside lease's comeback by,
# and adds the call `Counted`. A pool whose preempt is not lifo is outside
# the fragment (below), as is a call it does not know, and no oracle program
# has either: a 15 file and a 16 file translate alike.
IR_VERSION = 16
SUPPORTED_IR_VERSIONS = (7, 8, 9, 10, 11, 12, 13, 14, 15, IR_VERSION)


def is_delay(kind):
    """A delay stage: `Delay` up to IR 12, `ps(present)` from 13."""
    return kind == "Delay" or kind == {"Ps": {"Ctx": "N"}}


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


def fragment_step(ir):
    """The step engine of a program in the fragment, which both generators
    read (this one and gen_lean_claims.py): stage 0 is the step engine and
    every other stage a delay; a prefill gets any amount (no `granule`); the
    residents are served in admission order, `serve` `By([])` (`decode
    first` and `by (…)` lower to `By` with keys, `exclusive prefill` to
    `ExclusivePrefill`: `src/frontend/link.rs`, `serve`)."""
    st = ir["stages"]
    if not st or "Step" not in st[0]["kind"] or any(not is_delay(s["kind"]) for s in st[1:]):
        raise Fragment("stages must be one step engine (stage 0) and delays")
    step = st[0]["kind"]["Step"]
    if step.get("granule") is not None:
        raise Fragment("granule: the fragment gives a prefill any amount")
    if step["serve"] != {"By": []}:
        raise Fragment(f"serve {step['serve']}: the fragment serves residents in admission order (`By([])`)")
    return step


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
    if "Cost" in e:
        return fold(e["Cost"][1])
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


def erase_costs(e):
    """Conversions preserve values, including under syntax-sensitive rounding."""
    if isinstance(e, dict):
        if "Cost" in e:
            return erase_costs(e["Cost"][1])
        return {k: erase_costs(v) for k, v in e.items()}
    if isinstance(e, list):
        return [erase_costs(v) for v in e]
    return e


def unsafe_division(e):
    """A divisor without a positive constant natural value is not proven
    safe in Nat: Rust yields infinity/NaN at zero, whereas Nat yields 0."""
    if isinstance(e, dict):
        if "Binary" in e and e["Binary"][0] == "Div":
            divisor = fold(e["Binary"][2])
            if (divisor is None or not math.isfinite(divisor)
                    or divisor <= 0 or not float(divisor).is_integer()):
                return True
        return any(unsafe_division(v) for v in e.values())
    if isinstance(e, list):
        return any(unsafe_division(v) for v in e)
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
        e = erase_costs(e)
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

    def __init__(self, ir, observed=None):
        # its own list of attributes, which a request's state extends
        self.ir = {**ir, "attrs": list(ir["attrs"])}
        # An engine's records are written where something reads them: an
        # observation its theorem compares (`observed`, its slots; None for
        # every one), an attribute an expression reads. One nothing reads
        # changes nothing the theorem states, and the papers' proofs follow
        # the program's text.
        self.observed = observed

        def attrs_read(x):
            if isinstance(x, dict):
                return ({x["Attr"]} if "Attr" in x and isinstance(x["Attr"], int) else set()).union(
                    *(attrs_read(v) for v in x.values()))
            if isinstance(x, list):
                return set().union(*(attrs_read(v) for v in x))
            return set()
        self.read = attrs_read(ir)
        self.empty_turns = False
        self.builtin = {ir["slot_cached"]: "x.cached", ir["slot_serial"]: "x.serial"}
        self.request_slots = {}
        # the attributes a request's input reads that the translation takes
        # to be at least 1 (`request_untaken`): its caller's workload must be
        self.positive = set()

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

    def boolean_guard(self, e):
        """Only translate guards whose range is 0/1: Exec has no runtime
        error for Rust's invalid-guard case. This is deliberately conservative,
        not a general dataflow proof for mutable attributes."""
        if "Num" in e:
            return e["Num"] in (0, 1)
        if "Binary" in e:
            return e["Binary"][0] in (*REL, "And", "Or")
        if "Unary" in e:
            return e["Unary"][0] == "Not"
        if "Cond" in e:
            return all(self.boolean_guard(x) for x in e["Cond"][1:])
        if e != {"Attr": self.ir["slot_more"]}:
            return False
        # The trace sets more to 0/1, but source assignments and explicit
        # session/turn presets can overwrite it. Inspect all of them, even
        # in a nested body or before the first Turn.
        slot = self.ir["slot_more"]
        if any(slot == self.ir[k] for k in
               ("slot_cached", "slot_serial", "slot_turn", "slot_computed")):
            return False
        arrival = self.ir["arrival"]
        sessions = arrival.get("Sessions") if isinstance(arrival, dict) else None
        if not sessions or self.ir["trace"] is not None:
            return False
        if any(isinstance(st, dict) and "Set" in st and st["Set"][0] == slot
               for block in self.ir["blocks"] for st in block):
            return False
        return all(value in (0, 1)
                   for session in sessions
                   for attrs in [session["attrs"], *session.get("turns", [])]
                   for key, value in attrs if key == slot)

    def while_guard(self, e):
        if not self.boolean_guard(e):
            raise Fragment("while guard is not guaranteed to be 0 or 1; "
                           "the Lean fragment does not model invalid-guard errors")
        # A boolean result does not make Nat arithmetic agree with Rust:
        # prompt - 2 < 0 can be true in Rust and false in Lean. Check both
        # the guard and every assignment feeding it, including alias chains
        # and loop-carried values. A visited set terminates cyclic aliases.
        pending, seen = [e], set()

        def attributes(value):
            if isinstance(value, dict):
                if "Attr" in value:
                    yield value["Attr"]
                for child in value.values():
                    yield from attributes(child)
            elif isinstance(value, list):
                for child in value:
                    yield from attributes(child)

        while pending:
            value = pending.pop()
            if subtracts(value):
                raise Fragment("while guard depends on subtraction: Lean naturals truncate "
                               "at 0 where Rust can go negative")
            if unsafe_division(value):
                raise Fragment("while guard depends on division without a positive constant "
                               "natural divisor: Lean division by 0 differs from Rust")
            for slot in attributes(value):
                if slot not in seen:
                    seen.add(slot)
                    pending.extend(st["Set"][1]
                                   for block in self.ir["blocks"] for st in block
                                   if isinstance(st, dict) and "Set" in st
                                   and st["Set"][0] == slot)
        return self.top(e)

    def block(self, b, ind):
        pad = "  " * ind
        out = []
        for st in self.ir["blocks"][b]:
            if st == "End":
                out.append(pad + "stop")
                return "\n".join(out)
            if st == "Turn":
                if self.empty_turns:
                    continue
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
                if v.get("latency") is not None:
                    raise Fragment("a run with a wait before its work (`latency`)")
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
            elif kind == "Request":
                out.extend(self.request(v, pad, ind))
            elif kind == "Hold":
                if v.get("lease") is not None:
                    raise Fragment("a hold with a lease is outside the fragment")
                if v.get("prefix") is not None:
                    raise Fragment("a hold with a prefix chain: the model's cache is per "
                                   "session, not a trie of shared segments")
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
            elif kind == "While":
                c, body = v
                out.append(f"{pad}while ({self.while_guard(c)}) {{")
                out.append(self.block(body, ind + 1))
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

    def fresh(self, name):
        """A slot past the IR's attributes, for the state the interpreter
        keeps of a request; `Exec` reads an unset attribute as 0."""
        self.ir["attrs"].append(name)
        return len(self.ir["attrs"]) - 1

    def request(self, v, pad, ind):
        """`E (input, output);`, a request to the engine, as the kernel text
        it means (docs/design/scheduler-worker.md, Lean): the call's values
        and `kept`, the tokens it has, in attributes; a hold of what the
        admission takes, whose clauses read `request.known` as `max(kept,
        computed + 1)` and `request.hit` as `min(cachedin(P), reuse)` on the
        engine's memory `P`; in it `known` and `kept` set, the engine's
        records of the admission, the prefill of what the hold did not read
        from the cache, the first token's time when the prefill ends on the
        prompt, and the decode of the rest. Preempted, the hold runs again,
        `kept` where it had got to, as the interpreter's request keeps its
        tokens (scheduler.py:1561)."""
        e = one_ref(v["stage"], "request")
        if e != 0:
            raise Fragment("a request to an engine other than stage 0")
        step = self.ir["stages"][e]["kind"]["Step"]
        spec = step.get("request")
        if step.get("connector") is not None:
            raise Fragment("an engine's connector")
        if spec is None or not spec["pools"]:
            return self.request_untaken(v, e, spec, pad, ind)
        if spec.get("prefix") is not None:
            raise Fragment("a request's prefix chain: the model's cache is per session")
        if spec.get("send") is not None:
            raise Fragment("an engine whose requests are sent is outside the fragment")
        mem = step["memory"]
        if mem is None or all(one_ref(r, "taking") != mem for r, _, _ in spec["pools"]):
            raise Fragment("a request that does not take the engine's memory")
        # one session has one request at a time: every call shares the slots
        if e not in self.request_slots:
            name = self.ir["stages"][e]["name"]
            self.request_slots[e] = tuple(
                self.fresh(f"{name}.{w}") for w in ("input", "output", "kept", "known", "again"))
        inp, outp, kept, known, again = self.request_slots[e]
        computed = self.ir["slot_computed"]

        def attr(k):
            return {"Attr": k}

        def call(f, *xs):
            return {"Call": [f, [{"Expr": x} for x in xs]]}

        # the tokens it has: its input at the first admission, and after a
        # preemption the position it had reached and the token sampled there
        # (`Interp::preempt`), never fewer than before
        at_admission = {"Cond": [
            attr(again),
            call("Max", attr(kept), {"Binary": ["Add", attr(computed), {"Num": 1.0}]}),
            attr(kept),
        ]}

        def subst(x, known_is, hit_is=None):
            """`x` with the request's context variables written as the
            kernel's reads."""
            if isinstance(x, list):
                return [subst(y, known_is, hit_is) for y in x]
            if not isinstance(x, dict):
                return x
            if "Ctx" in x and isinstance(x["Ctx"], str):
                c = x["Ctx"]
                if c == "Input":
                    return attr(inp)
                if c == "Output":
                    return attr(outp)
                if c == "Known":
                    if known_is is None:
                        raise Fragment("`request.known` in `cache`: the kernel's hold reads the "
                                       "admission's")
                    return known_is
                if c == "Hit":
                    if hit_is is None:
                        raise Fragment("`request.hit` outside `taking`")
                    return hit_is
                if c in ("Finished", "Loaded", "Requeued"):
                    raise Fragment(f"`{c}`, which the kernel's hold has no word for")
                return x
            return {k: subst(y, known_is, hit_is) for k, y in x.items()}

        cached_in = {"Call": ["CachedIn", [{"Pool": {"base": mem, "count": 1, "index": None}}]]}
        reuse = None if spec.get("reuse") is None else subst(spec["reuse"], at_admission)
        hit = cached_in if reuse is None else call("Min", cached_in, reuse)
        ps = []
        for r, u, fits in spec["pools"]:
            u = self.top(subst(u, at_admission, hit))
            f = f" fits ({self.top(subst(fits, at_admission, hit))})" if fits is not None else ""
            ps.append(f"{one_ref(r, 'taking')} ({u}){f}")
        ru = f" reuse ({self.top(reuse)})" if reuse is not None else ""
        cache = spec.get("cache")
        ca = f" cache ({self.top(subst(cache, None))})" if cache is not None else ""
        rec = spec["records"]
        inner = "  " * (ind + 1)
        n, c = f"(x.attr {known})", "x.cached"
        return self.records([
            f"{pad}set {inp} = {self.top(v['input'])};",
            f"{pad}set {outp} = {self.top(v['output'])};",
            f"{pad}set {kept} = x.attr {inp};",
            f"{pad}set {again} = 0;",
            f"{pad}hold {', '.join(ps)}{ru} {{",
            f"{inner}set {known} = {self.top(at_admission)};",
            f"{inner}set {kept} = x.attr {known};",
            f"{inner}set {again} = 1;",
            f"{inner}observe {rec['reused']} = {c};",
            f"{inner}observe {rec['prefill']} = {n} - {c};",
            f"{inner}set {rec['admitted']} = x.now;",
            f"{inner}set {rec['last_reused']} = {c};",
            f"{inner}set {rec['last_prefill']} = {n} - {c};",
            f"{inner}run {e} prefill ({n} - {c}) growing {mem};",
            f"{inner}branch (if ({n} = (x.attr {inp})) then 1 else 0) {{",
            f"{inner}  set {rec['first']} = x.now;",
            f"{inner}  done",
            f"{inner}}} else {{",
            f"{inner}  done",
            f"{inner}}};",
            f"{inner}run {e} decode (((x.attr {outp}) - 1) - ({n} - (x.attr {inp}))) growing {mem};",
            f"{inner}done",
            f"{pad}}}{ca};",
        ], rec)

    def records(self, lines, rec):
        """`lines` without the engine's records nothing reads (`__init__`)."""
        if rec is None:
            return lines
        drop = {f"observe {rec[k]} =" for k in ("reused", "prefill")
                if self.observed is not None and rec[k] not in self.observed}
        drop |= {f"set {rec[k]} =" for k in ("admitted", "first", "last_reused", "last_prefill")
                 if rec[k] not in self.read}
        return [l for l in lines if not any(l.strip().startswith(d) for d in drop)]

    def request_untaken(self, v, e, spec, pad, ind):
        """A request to an engine that takes nothing, admitted as it is
        called: its records, a prefill of its input when it has one, which
        samples its first token, and the decode of the rest."""
        inner = "  " * (ind + 1)
        inp = self.top(v["input"])
        outp = self.top(v["output"])
        less_one = self.top({"Binary": ["Sub", v["output"], {"Num": 1.0}]})
        out = []
        rec = spec["records"] if spec is not None else None
        if rec is not None:
            out += [
                f"{pad}observe {rec['reused']} = 0;",
                f"{pad}observe {rec['prefill']} = {inp};",
                f"{pad}set {rec['admitted']} = x.now;",
                f"{pad}set {rec['last_reused']} = 0;",
                f"{pad}set {rec['last_prefill']} = {inp};",
            ]
        first = [f"set {rec['first']} = x.now;"] if rec is not None else []
        prefilled = [f"run {e} prefill ({inp});", *first, f"run {e} decode ({less_one});"]
        alone = [f"run {e} decode ({outp});"]
        n = fold(erase_costs(v["input"]))
        if n is not None:
            return self.records(out + [pad + st for st in (prefilled if n > 0 else alone)], rec)
        # An input that is an attribute is a prefill of it and a decode of
        # the rest, unless the first token's time is read: the papers'
        # proofs (`Slot.lean`, `Bari.lean`) follow a program without a
        # branch. It is the interpreter's request for an input of 1 or more,
        # and the caller's workload must have one (`positive`): a claim's
        # family holds it, where the program's `init` draws only such.
        inp_attr = erase_costs(v["input"]).get("Attr")
        if (rec is None or rec["first"] not in self.read) and isinstance(inp_attr, int):
            self.positive.add(inp_attr)
            return self.records(out + [f"{pad}run {e} prefill ({inp});",
                                       f"{pad}run {e} decode ({less_one});"], rec)
        return self.records(out + [
            f"{pad}branch (if ({inp}) > 0 then 1 else 0) {{",
            *(inner + st for st in prefilled),
            f"{inner}done",
            f"{pad}}} else {{",
            *(inner + st for st in alone),
            f"{inner}done",
            f"{pad}}};",
        ], rec)

    def deployment(self):
        ir = self.ir
        step = fragment_step(ir)
        cost = cost_fn(step["cost"])
        if step.get("iteration") is not None:
            raise Fragment("iteration: the oracle fragment runs vLLM's procedure, not a body "
                           "(a stage's `serve only` is one)")
        pools = []
        for i, p in enumerate(ir["pools"]):
            if p.get("reserve_held"):
                raise Fragment(f"pool {p['name']}: reserve held")
            if p.get("queue_every") is not None:
                raise Fragment(f"pool {p['name']}: queue every, admissions at ticks")
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
    # a shared stage makes every run on it a flow, a single-stage one too
    if any(
        isinstance(st, dict) and "Run" in st and st["Run"].get("share") is not None
        for block in ir["blocks"]
        for st in block
    ):
        raise Fragment(f"{name}: shared multi-stage execution is outside the fragment")
    if ir.get("arrivals") is not None:
        raise Fragment(f"{name}: finite arrival limits are outside the fragment")
    return ir, Lean(ir)


def explicit_positive(ir, lean, name):
    """The explicit sessions give every request input the translation takes
    to be at least 1 (`Lean.positive`) such a value."""
    for s in (ir["arrival"].get("Sessions") or []) if isinstance(ir["arrival"], dict) else []:
        vals = dict((slot, v) for slot, v in s["attrs"])
        for k in lean.positive:
            if vals.get(k, 0) < 1:
                raise Fragment(f"{name}: a session's request input `{ir['attrs'][k]}` is 0")


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
        ir, _ = load(name)
        # every record: the differential test (`lean_drt.py`) compares every
        # observation of the program the Lean executable runs
        lean = Lean(ir)
        body = lean.block(ir["session"], 1)
        explicit_positive(ir, lean, name)
        progs[name] = (body, doc_tables(lean.ir))
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

The request scenarios run one request program, `vllmRequest` (serQ
`examples/oracle/vllm_request.sq`): wait until the arrival, hold a slot and the
KV blocks of the chunk the engine's budget leaves (admission needs room for
the whole prompt), prefill growing the hold, decode growing it. The
theorems say that the executable semantics gives, for every request, the
step of its first token and of its last token, and the number of
preemptions, that the real scheduler gives. The last, `vllmTurn` (serQ
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
/-- serQ `tools/oracle/{name}.ir.json`: {n} requests, {sc["num_blocks"]} blocks of {sc["block_size"]}, budget {sc["budget"]}, {sc["max_seqs"]} slots, chunk {sc.get("chunk", 0)}{", adaptive" if sc.get("adaptive") else ""}; the deployment and the workload are the IR's. -/
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
    # the cached tokens are what the engine records of each admission; the
    # trace preempts nothing, so each turn has one
    ob["cached_tokens"] = ob.get("vllm.reused")
    for n in ("sent", "ttft", "response", "cached_tokens"):
        if ob.get(n) is None:
            raise Fragment(f"cache_trace: no observation `{n}`")

    def col(f):
        return "[" + ", ".join(f"({s}, {f(rows[(s, k)])})" for s, k in keys) + "]"

    return "\n/-! ### A multi-turn scenario with a prefix cache -/\n\n" + program(
        "vllmTurn", ["cache_trace"], "The vLLM replay program (serQ `examples/replay/vllm_replay.sq`) on a unit step clock",
    ) + f'''
/-- serQ `tools/oracle/cache_trace.ir.json`: {len(ir["arrival"]["Sessions"])} sessions of the inlined trace
`cache_trace.csv`; the deployment and the workload are the IR's. Per turn:
send step, time to first token, latency, cached tokens at admission. -/
theorem vllm_cache_trace :
    let m := Exec.runW {lean.deployment()} {horizon}
      {lean.workload()} vllmTurn
    (observed m {ob["sent"]}, observed m {ob["ttft"]}, observed m {ob["response"]}, observed m {ob["cached_tokens"]}) =
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
        body = lean.block(ir["session"], 1)
        explicit_positive(ir, lean, name)
        obs = ir["observes"]
        lhs = ", ".join(f"observed m {i}" for i in range(len(obs)))
        rhs = ", ".join("[" + ", ".join(f"({s}, {v})" for s, v in ans["observes"].get(n, [])) + "]" for n in obs)
        horizon = nat(ir["horizon"], "horizon")
        out.append(f'''
/-- `tests/lean-regress/{name}.sq`, translated from its IR. {doc_tables(lean.ir)} -/
def {camel(name)} : Prog := [route|
{body}]

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
