/-
# Regressions found by comparing the Lean semantics with the interpreter

Each theorem runs a program of `examples/oracle/` (translated from its IR by
`scripts/gen_lean_oracle.py`'s translator, the batch arrival written as
explicit sessions) and states the answer `serq run` gives.

* `preempt_in_delay` (`examples/oracle/preempt_delay.sq`): a growth on the
  engine's memory while the other holder is in a delay inside its hold. The
  victim is the engine resident admitted last, here the grower itself, and
  its hold resumes with `computed` (`Exec.victim`, `Exec.preemptLast`).
-/
import Serq.Exec

set_option maxRecDepth 100000

namespace SerqLang
namespace Regress

open Exec

/-- `examples/oracle/preempt_delay.sq`, translated from its IR. Attributes: 2 = turn_no, 3 = new, 4 = out, 5 = think, 6 = more, 7 = forced, 8 = computed, 9 = d. Observations: 0 = done. Pools: 0 = kv, 1 = slots. Stages: 0 = engine, 1 = tool. -/
def preemptDelay : Prog := [route|
  hold 1 (1), 0 (16) {
    run 0 prefill (16) growing 0;
    run 1 (x.attr 9);
    run 0 decode (20) growing 0;
    observe 0 = x.now;
    done
  };
  stop]

/-- The interpreter: `done` = 65 for session 0 and 43 for session 1, after
8 preemptions. -/
theorem preempt_in_delay :
    let m := Exec.runW ⟨[⟨48, 16, false⟩, ⟨4, 1, true⟩], 64, 0, some 0, fun _ => 1⟩ 100 ⟨[[(9, 1)], [(9, 10)]], [], none, 0, some 8⟩ preemptDelay
    (observed m 0, m.preempts) = ([(0, 65), (1, 43)], 8) := by
  decide +kernel

end Regress
end SerqLang
