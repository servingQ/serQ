/-
# An iteration of the step engine fills its budget in serving order

`Exec.assign` builds the iteration: residents in serving order, each takes
what it wants (one token for a decode, the rest or a chunk for a prefill)
from the budget left. With nothing growing past its allocation and no
request waiting for the engine (so no admission during the iteration), it
is the greedy fill of the budget over the job list, `fillIter`; and the
greedy fill serves in order: a job gets tokens only if every job before it
got all it wanted (`fillAmounts_fifo`). This is what a FIFO prefill queue
needs from the engine (serving-queue-theory's `StepEngine`).

Key definitions: `Exec.wantOf`, `Exec.fillAmounts`, `Exec.fillIter`.
Key theorems: `Exec.assign_eq_fillIter`, `Exec.fillIter_eq_amounts`,
`Exec.fillAmounts_fifo`, `Exec.fillAmounts_le_want`.
-/
import Serq.Exec

namespace SerqLang
namespace Exec

variable (D : Deployment)

/-- The tokens the greedy fill gives the jobs `js`, in order, from `left`:
each takes `min want left`. -/
def fillAmounts : List Job → ℕ → List ℕ
  | [], _ => []
  | j :: js, left => min (wantOf D j) left :: fillAmounts js (left - min (wantOf D j) left)

/-- The iteration of the greedy fill: (owner, tokens) for each job that takes
a token, in order, until the budget is spent. -/
def fillIter : List Job → ℕ → List (ℕ × ℕ)
  | [], _ => []
  | j :: js, left =>
    if min (wantOf D j) left = 0 then fillIter js left
    else if left - min (wantOf D j) left = 0 then [(j.owner, min (wantOf D j) left)]
    else (j.owner, min (wantOf D j) left) :: fillIter js (left - min (wantOf D j) left)

theorem fillAmounts_zero : ∀ js : List Job, fillAmounts D js 0 = js.map fun _ => 0
  | [] => rfl
  | j :: js => by simp [fillAmounts, fillAmounts_zero js]

theorem fillAmounts_length : ∀ (js : List Job) (left : ℕ), (fillAmounts D js left).length = js.length
  | [], _ => rfl
  | _ :: js, _ => by simp [fillAmounts, fillAmounts_length js]

/-- No job gets more than it wants. -/
theorem fillAmounts_le_want : ∀ (js : List Job) (left k : ℕ),
    (fillAmounts D js left).getD k 0 ≤ (js.map (wantOf D)).getD k 0
  | [], _, _ => by simp [fillAmounts]
  | j :: js, left, k => by
    cases k with
    | zero => simp [fillAmounts]
    | succ k => simpa [fillAmounts] using fillAmounts_le_want js _ k

/-- The iteration is the amounts, without the jobs that take nothing. -/
theorem fillIter_eq_amounts : ∀ (js : List Job) (left : ℕ),
    fillIter D js left =
      ((js.zip (fillAmounts D js left)).filter (fun p => 0 < p.2)).map fun p => (p.1.owner, p.2)
  | [], _ => rfl
  | j :: js, left => by
    unfold fillIter fillAmounts
    by_cases h0 : min (wantOf D j) left = 0
    · rw [if_pos h0, h0, fillIter_eq_amounts js left]
      simp
    · rw [if_neg h0]
      by_cases h1 : left - min (wantOf D j) left = 0
      · rw [if_pos h1, h1, fillAmounts_zero]
        have : 0 < min (wantOf D j) left := Nat.pos_of_ne_zero h0
        simp only [List.zip_cons_cons, List.filter_cons, this, decide_true, ite_true, List.map_cons,
          List.cons.injEq, true_and]
        rw [List.filter_eq_nil_iff.mpr]
        · rfl
        · intro p hp
          have := (List.of_mem_zip hp).2
          simp only [List.mem_map] at this
          obtain ⟨_, _, h⟩ := this
          simp [← h]
      · rw [if_neg h1, fillIter_eq_amounts js]
        have : 0 < min (wantOf D j) left := Nat.pos_of_ne_zero h0
        simp [this]

/-- **The fill serves in order.** If job `k'` gets a token, every job
`k < k'` got all it wanted. -/
theorem fillAmounts_fifo : ∀ (js : List Job) (left k k' : ℕ), k < k' →
    0 < (fillAmounts D js left).getD k' 0 →
    (fillAmounts D js left).getD k 0 = (js.map (wantOf D)).getD k 0
  | [], _, _, _, _, h => by simp [fillAmounts] at h
  | j :: js, left, k, k', hk, h => by
    cases k with
    | zero =>
      cases k' with
      | zero => omega
      | succ k' =>
        simp only [fillAmounts, List.getD_cons_succ] at h
        simp only [fillAmounts, List.getD_cons_zero, List.map_cons]
        by_contra hne
        have hlt : min (wantOf D j) left = left := by omega
        rw [hlt, Nat.sub_self, fillAmounts_zero] at h
        rcases Nat.lt_or_ge k' js.length with h2 | h2
        · simp [List.getD_eq_getElem?_getD, h2] at h
        · simp [List.getD_eq_getElem?_getD, h2] at h
    | succ k =>
      cases k' with
      | zero => omega
      | succ k' =>
        simp only [fillAmounts, List.getD_cons_succ, List.map_cons] at h ⊢
        exact fillAmounts_fifo js _ k k' (by omega) h

/-- The pools the engine serves have nobody waiting, so an iteration admits
no one. -/
def engineQueuesEmpty (m : Machine) : Prop :=
  ∀ p, (pdef D p).viaEngine = true → (pst m p).queue = []

theorem admitVia_empty (m : Machine) (left : ℕ) (h : engineQueuesEmpty D m) :
    admitVia D m left = (m, false) := by
  unfold admitVia
  have : (List.range D.pools.length).find?
      (fun p => (pdef D p).viaEngine && !(pst m p).queue.isEmpty) = none := by
    rw [List.find?_eq_none]
    intro p _
    by_cases hv : (pdef D p).viaEngine = true
    · simp [hv, h p hv]
    · simp [hv]
  rw [this]

/-- **`assign` is the greedy fill.** With no job growing and nobody waiting
for the engine, the iteration `assign` builds from job `idx` on, with `left`
tokens, is `fillIter` of the rest of the job list (with enough fuel). -/
theorem assign_eq_fillIter (m : Machine) (hq : engineQueuesEmpty D m)
    (hg : ∀ j ∈ m.jobs, j.growing = none) (pre0 : ℕ) :
    ∀ (f idx left : ℕ) (acc : List (ℕ × ℕ)), m.jobs.length - idx < f →
      assign D f { m with iter := acc } idx left pre0
        = { m with iter := acc ++ fillIter D (m.jobs.drop idx) left } := by
  intro f
  induction f with
  | zero => intro idx left acc h; omega
  | succ f ih =>
    intro idx left acc hf
    rw [assign]
    simp only
    cases hj : m.jobs[idx]? with
    | none =>
      have hlen : m.jobs.length ≤ idx := by
        simpa [List.getElem?_eq_none_iff] using hj
      have hdrop : m.jobs.drop idx = [] := List.drop_eq_nil_of_le hlen
      simp only [hdrop, fillIter, List.append_nil]
      split_ifs
      · have := admitVia_empty D { m with iter := acc } left hq
        rw [this]
      · rfl
    | some j =>
      have hmem : j ∈ m.jobs := List.mem_of_getElem? hj
      have hdrop : m.jobs.drop idx = j :: m.jobs.drop (idx + 1) := by
        rw [List.drop_eq_getElem_cons (by
          rcases List.getElem?_eq_some_iff.mp hj with ⟨hl, _⟩; exact hl)]
        congr 1
        rcases List.getElem?_eq_some_iff.mp hj with ⟨_, he⟩; exact he
      have hlen : idx < m.jobs.length := by
        rcases List.getElem?_eq_some_iff.mp hj with ⟨hl, _⟩; exact hl
      rw [hdrop, fillIter]
      simp only [hg j hmem]
      by_cases h0 : min (wantOf D j) left = 0
      · rw [if_pos h0, if_pos h0]
        exact ih (idx + 1) left acc (by omega)
      · rw [if_neg h0, if_neg h0]
        by_cases h1 : left - min (wantOf D j) left = 0
        · rw [if_pos h1, if_pos h1]
        · rw [if_neg h1, if_neg h1]
          have := ih (idx + 1) (left - min (wantOf D j) left) (acc ++ [(j.owner, min (wantOf D j) left)])
            (by omega)
          rw [this]
          simp

end Exec
end SerqLang
