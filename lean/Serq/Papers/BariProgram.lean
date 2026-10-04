/-
# Bari et al., Theorem 2, on the program's own chain

The machine chain of `bari_rad.sq` (`BariStable.kernel`) never revisits a
state: it keeps the clock and every ended session. What recurs is the
engine being empty, `σ m = []`, and every machine with an empty engine
behaves alike: its job list follows `BariChain`'s chain from `[]`
(`BariSim.simulation`). Below capacity, on the machine chain itself:

* the engine empties in bounded expected time from every state
  (`hit_idle_le`);
* it empties again in expected time bounded by one constant from every
  machine whose engine is empty (`return_idle`): the set of empty machines
  is a positive recurrent atom.
-/
import Serq.Papers.BariSim
import Serq.Papers.BariRecurrent

namespace SerqLang
namespace Papers
namespace BariProgram

open Foster BariStable

/-- The engine of the machine holds no job. -/
def Idle (x : State) : Prop := BariSim.σ x.1 = []

instance : DecidablePred Idle := fun x => by unfold Idle; infer_instance

/-- From every state the engine empties in bounded expected time. -/
theorem hit_idle_le {N : ℕ} (A : Arrivals N) (hA : A.load < 128) :
    ∃ W : State → ℝ, ∀ n x, hit (kernel A) Idle n x ≤ W x := by
  sorry

/-- From every machine with an empty engine the expected time until it is
empty again is bounded by one constant. -/
theorem return_idle {N : ℕ} (A : Arrivals N) (hA : A.load < 128) :
    ∃ C : ℝ, ∀ n x, Idle x → 1 + (kernel A).apply (hit (kernel A) Idle n) x ≤ C := by
  sorry

end BariProgram
end Papers
end SerqLang
