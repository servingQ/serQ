"""vLLM v1's scheduler with the serving order of a serQ program: the
programmable half of `serq target` (docs/design/serving-specification-language.md,
§5).

`serq target` compiles a program whose policies are all vLLM's to a
configuration. A program that serves its residents by keys (`serve by
(k1, ...)`) is vLLM but for the order of the running loop, and this class
supplies that order: before each step it sorts `running` by the program's
keys, ties in admission order. Everything else is vLLM's own code.

One thing has to be kept apart that vLLM's FCFS path keeps together. Its
running loop visits `running` in order and preempts `running[-1]`
(scheduler.py:767, 799), so the order of the list is both the serving order
and the victim. serQ's victim is the most recently admitted resident
whatever the serving order (`src/engine/interp.rs`, `lifo_victim`).
vLLM's PRIORITY path picks the victim apart from the list order, as
`max(running, key=(priority, arrival_time))` (scheduler.py:761-797). This
class runs that path and gives each request the number of its latest
admission as its priority. The waiting queue is then a priority queue
(request_queue.py:202), so priorities are chosen to keep it FCFS. A new
request's priority is the order it reached the scheduler (`add_request`),
not its `arrival_time`, which is when the request object was made. A
preempted one gets a
decreasing negative number, so the latest preempted is first, as FCFS's
`prepend_request` would put it.

The keys are the IR's `CExpr`s, as `serq target` prints them. The context a
key may read here is what vLLM's scheduler can observe of a running
request: `Decoding` (it has computed every token it has but the last) and
`Admission` (the number of its latest admission). `serq target` refuses any
other.
"""

from vllm.v1.core.sched.request_queue import SchedulingPolicy
from vllm.v1.core.sched.scheduler import Scheduler


def _eval(e, req, adm):
    if isinstance(e, str):
        raise ValueError(f"unsupported key {e!r}")
    (tag, v), = e.items()
    if tag == "Num":
        return float(v)
    if tag == "Ctx":
        if v == "Decoding":
            # decoding: one token left to compute, and it is not the prompt's
            return 1.0 if (req.num_output_tokens > 0
                           and req.num_tokens - req.num_computed_tokens <= 1) else 0.0
        if v == "Admission":
            return float(adm)
        raise ValueError(f"vLLM's scheduler does not observe `{v}`")
    if tag == "Unary":
        op, a = v
        x = _eval(a, req, adm)
        return {"Neg": -x, "Not": 0.0 if x != 0 else 1.0}[op]
    if tag == "Binary":
        op, a, b = v
        x, y = _eval(a, req, adm), _eval(b, req, adm)
        return {
            "Add": x + y, "Sub": x - y, "Mul": x * y, "Div": x / y,
            "Lt": float(x < y), "Le": float(x <= y), "Gt": float(x > y),
            "Ge": float(x >= y), "Eq": float(x == y), "Ne": float(x != y),
            "And": float(x != 0 and y != 0), "Or": float(x != 0 or y != 0),
        }[op]
    if tag == "Cond":
        c, a, b = v
        return _eval(a, req, adm) if _eval(c, req, adm) != 0 else _eval(b, req, adm)
    raise ValueError(f"unsupported key {tag}")


class SerqScheduler(Scheduler):
    """`Scheduler` whose running loop serves in the order of `serve_by`."""

    def __init__(self, *args, serve_by=(), **kwargs):
        super().__init__(*args, **kwargs)
        self.serq_init(serve_by)

    def serq_init(self, serve_by):
        """Make a constructed `Scheduler` this one (the oracle builds the
        scheduler with vLLM's test helper and then calls this)."""
        if self.waiting or self.running:
            raise RuntimeError("serq_init on a scheduler with requests")
        from vllm.v1.core.sched.request_queue import create_request_queue

        self.serve_by = list(serve_by)
        self.policy = SchedulingPolicy.PRIORITY
        self.waiting = create_request_queue(self.policy)
        self.skipped_waiting = create_request_queue(self.policy)
        self.serq_admissions = 0
        self.serq_preemptions = 0
        self.serq_arrivals = 0
        self.serq_admitted = {}

    def add_request(self, request):
        # the order of arrival at the scheduler, not `arrival_time`: the
        # priority queue breaks ties by `arrival_time`, which is when the
        # request object was made, and FCFS is the order of `add_request`
        self.serq_arrivals += 1
        request.priority = self.serq_arrivals
        super().add_request(request)

    def _preempt_request(self, request, timestamp, drop_stale_output=False):
        # before the call: it puts the request back in the waiting queue
        # (scheduler.py:1580-1581), and the heap orders it by its priority then
        self.serq_preemptions += 1
        request.priority = -self.serq_preemptions
        self.serq_admitted.pop(request.request_id, None)
        super()._preempt_request(request, timestamp, drop_stale_output)

    def schedule(self, throttle_prefills=False):
        adm = self.serq_admitted
        self.running.sort(
            key=lambda r: tuple(_eval(k, r, adm[r.request_id]) for k in self.serve_by)
            + (adm[r.request_id],)
        )
        out = super().schedule(throttle_prefills)
        # what this step admitted, in the order it admitted them
        for r in self.running:
            if r.request_id not in adm:
                self.serq_admissions += 1
                adm[r.request_id] = self.serq_admissions
                r.priority = self.serq_admissions
        return out
