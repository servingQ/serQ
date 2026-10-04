/-
# Dai et al., Theorem 2(b), on the program's own chain

The machine chain of `dai_sarathi.sq` (`DaiStable.kernel`) never revisits a
state: it keeps the clock and every ended session. What recurs is the
engine being empty, `σ m = []`, and every machine with an empty engine
behaves alike: its job list follows `DaiChain`'s chain from `[]`
(`DaiSim.simulation`). Below capacity, on the machine chain itself:

* the engine empties in bounded expected time from every state
  (`hit_idle_le`);
* it empties again in expected time bounded by one constant from every
  machine whose engine is empty (`return_idle`): the set of empty machines
  is a positive recurrent atom.
-/
import Serq.Papers.DaiSim
import Serq.Papers.DaiRecurrent

namespace SerqLang
namespace Papers
namespace DaiProgram

open Foster DaiStable

/-- The engine of the machine holds no job. -/
def Idle {K : ℕ} (x : State K) : Prop := DaiSim.σ x.1 = []

instance {K : ℕ} : DecidablePred (Idle (K := K)) := fun x => by unfold Idle; infer_instance

/-- From every state the engine empties in bounded expected time. -/
theorem hit_idle_le {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) :
    ∃ W : State K → ℝ, ∀ n x, hit (kernel A) Idle n x ≤ W x := by
  sorry

/-- From every machine with an empty engine the expected time until it is
empty again is bounded by one constant. -/
theorem return_idle {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) :
    ∃ C : ℝ, ∀ n x, Idle x → 1 + (kernel A).apply (hit (kernel A) Idle n) x ≤ C := by
  sorry

end DaiProgram
end Papers
end SerqLang
