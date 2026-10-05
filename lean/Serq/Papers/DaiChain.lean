/-
# Sarathi's chain on job lists

What the engine of `dai_sarathi.sq` holds at a slot boundary, with
everything else forgotten: the list of its jobs, in admission order, each a
mode and the tokens it has left. `absSlot` is one slot on that list. The
machine chain of `Serq/Papers/DaiStable.lean` projects onto it
(`DaiSim.simulation`), and on it the empty list is a single state, to which
the chain returns (`DaiRecurrent`).
-/
import Serq.Papers.DaiStable

namespace SerqLang
namespace Papers
namespace DaiChain

open Exec Foster

/-- A job: its mode and the tokens it has left. -/
abbrev AJob := Mode × ℕ

/-- What a job takes with an unlimited budget: one token for a decode, the
rest of its prompt for a prefill (no chunk cap). -/
def want : AJob → ℕ
  | (.decode, l) => min 1 l
  | (_, l) => l

/-- The batch: each job's share, greedy in admission order, from a budget
of `b`. -/
def shares : List AJob → ℕ → List ℕ
  | [], _ => []
  | j :: js, b => min (want j) b :: shares js (b - min (want j) b)

/-- One slot with `k` arrivals. An idle engine (no job) starts on the `k`
new prefills. A busy one has the `k` new prefills join behind its jobs, then
its batch ends: every job loses its share, a finished prefill comes back as
a decode of 990 tokens behind the rest, a finished decode leaves. -/
def absSlot (k : ℕ) (js : List AJob) : List AJob :=
  if js = [] then List.replicate k (.prefill, 290)
  else
    let after := List.zipWith (fun j s => (j.1, j.2 - s)) js (shares js 128)
    after.filter (·.2 ≠ 0) ++ List.replicate k (.prefill, 290) ++
      ((after.filter fun j => j.2 = 0 ∧ j.1 = .prefill).map fun _ => (.decode, 990))

/-- The states the chain reaches from the empty list with positive
probability under the arrival distribution `A`. -/
inductive AReach {K : ℕ} (A : DaiStable.Arrivals K) : List AJob → Prop
  | nil : AReach A []
  | step {js : List AJob} (k : ℕ) : k ≤ K → 0 < A.p k → AReach A js → AReach A (absSlot k js)

/-- The chain's states. -/
def AState {K : ℕ} (A : DaiStable.Arrivals K) : Type := {js : List AJob // AReach A js}

instance {K : ℕ} (A : DaiStable.Arrivals K) : DecidableEq (AState A) :=
  fun x y => decidable_of_iff (x.1 = y.1) Subtype.ext_iff.symm

/-- The empty list: no job, the engine idle. -/
def nil {K : ℕ} (A : DaiStable.Arrivals K) : AState A := ⟨[], .nil⟩

/-- The chain: a slot with `k` arrivals, `k` drawn from `A` (an outcome of
probability 0 leaves the state, never taken). -/
noncomputable def akernel {K : ℕ} (A : DaiStable.Arrivals K) : Kernel (AState A) ℕ :=
  Kernel.ofOutcomes K A.p A.nonneg A.sum_one fun x k =>
    if h : k ≤ K ∧ 0 < A.p k then ⟨absSlot k x.1, .step k h.1 h.2 x.2⟩ else x

/-- The tokens the engine still has to serve. -/
def backlog (js : List AJob) : ℕ :=
  (js.map fun j => j.2 + if j.1 = .prefill then 990 else 0).sum

end DaiChain
end Papers
end SerqLang
