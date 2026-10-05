/-
# RAD's chain on job lists

What the engine of `bari_rad.sq` holds at a slot boundary, with everything
else forgotten: its jobs in admission order, each a mode, the tokens it has
left and the output it decodes once its prompt is done. `absSlot` is one
slot on that list. The machine chain of `Serq/Papers/BariStable.lean`
projects onto it (`BariSim.simulation`), and on it the empty list is a
single state, to which the chain returns (`BariRecurrent`).
-/
import Serq.Papers.BariStable

namespace SerqLang
namespace Papers
namespace BariChain

open Exec Foster

/-- A job: its mode, the tokens it has left, and its output length. -/
abbrev AJob := Mode × ℕ × ℕ

/-- RAD's mode: Decode Mode when 128 requests decode or every one does. -/
def decodeMode (js : List AJob) : Bool :=
  decide (128 ≤ (js.filter (·.1 = .decode)).length ∨ (js.filter (·.1 = .decode)).length = js.length)

/-- What a job takes in this iteration: in Decode Mode one token for a
decode, in Prefill Mode a chunk of at most 128 of a prefill, 0 otherwise. -/
def want (dm : Bool) : AJob → ℕ
  | (.decode, l, _) => if dm then min 1 l else 0
  | (_, l, _) => if dm then 0 else min l 128

/-- The batch: each job's share, greedy in admission order, from a budget
of `b`. -/
def shares (dm : Bool) : List AJob → ℕ → List ℕ
  | [], _ => []
  | j :: js, b => min (want dm j) b :: shares dm js (b - min (want dm j) b)

/-- One slot whose arrivals have (prompt, output) lengths `rs`. An idle
engine starts on the new prefills. A busy one has them join behind its jobs,
then its batch ends: every job loses its share, a finished prefill comes
back as a decode of its output behind the rest, a finished decode leaves. -/
def absSlot (rs : List (ℕ × ℕ)) (js : List AJob) : List AJob :=
  let new := rs.map fun r => ((.prefill, r.1, r.2) : AJob)
  if js = [] then new
  else
    let after := List.zipWith (fun j s => (j.1, j.2.1 - s, j.2.2)) js (shares (decodeMode js) js 128)
    after.filter (·.2.1 ≠ 0) ++ new ++
      ((after.filter fun j => j.2.1 = 0 ∧ j.1 = .prefill).map fun j => ((.decode, j.2.2, j.2.2) : AJob))

/-- The states the chain reaches from the empty list with positive
probability under `A`. -/
inductive AReach {N : ℕ} (A : BariStable.Arrivals N) : List AJob → Prop
  | nil : AReach A []
  | step {js : List AJob} (o : ℕ) : o ≤ N → 0 < A.p o → AReach A js → AReach A (absSlot (A.arr o) js)

/-- The chain's states. -/
def AState {N : ℕ} (A : BariStable.Arrivals N) : Type := {js : List AJob // AReach A js}

instance {N : ℕ} (A : BariStable.Arrivals N) : DecidableEq (AState A) :=
  fun x y => decidable_of_iff (x.1 = y.1) Subtype.ext_iff.symm

/-- The empty list: no job, the engine idle. -/
def nil {N : ℕ} (A : BariStable.Arrivals N) : AState A := ⟨[], .nil⟩

/-- The chain: a slot with the arrivals of outcome `o`, drawn from `A`. -/
noncomputable def akernel {N : ℕ} (A : BariStable.Arrivals N) : Kernel (AState A) ℕ :=
  Kernel.ofOutcomes N A.p A.nonneg A.sum_one fun x o =>
    if h : o ≤ N ∧ 0 < A.p o then ⟨absSlot (A.arr o) x.1, .step o h.1 h.2 x.2⟩ else x

/-- The tokens the engine still has to serve. -/
def backlog (js : List AJob) : ℕ :=
  (js.map fun j => j.2.1 + if j.1 = .prefill then j.2.2 else 0).sum

end BariChain
end Papers
end SerqLang
