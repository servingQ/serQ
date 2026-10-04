/-
# An iteration of the step engine fills its budget in serving order

`Exec.assign` builds the iteration: residents in serving order, each takes
what it wants (one token for a decode, the rest or a chunk for a prefill)
from the budget left. When no growth in it needs more than the hold already
has (every growing job's allocation covers the position it will reach, so
`assign` calls no `grow` and preempts no one) and nobody waits for the
engine (so it admits no one), it is the greedy fill of the budget over the
job list, `fillIter`; and the greedy fill serves in order (a job gets tokens
only if every job before it got all it wanted, `fillAmounts_fifo`), never
gives more than wanted (`fillAmounts_le_want`) and is work-conserving
(`fillAmounts_sum`). This is what a FIFO prefill queue needs from the engine
(serving-queue-theory's `StepEngine`).

Not covered yet: an iteration whose growth crosses a block boundary (the
vLLM programs' decode every `bs` tokens and their prefill chunks after the
first) and one that admits waiting requests; for those programs the
theorems apply to the iterations that do neither.

Key definitions: `Exec.wantOf`, `Exec.fillAmounts`, `Exec.fillIter`.
Key theorems: `Exec.assign_iter_eq_fillIter`, `Exec.assign_eq_fillIter`,
`Exec.fillIter_eq_amounts`, `Exec.fillAmounts_fifo`, `Exec.fillAmounts_le_want`,
`Exec.fillAmounts_sum`.
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

/-- The fill is work-conserving: it hands out the budget, or all that is
wanted if that is less. -/
theorem fillAmounts_sum : ∀ (js : List Job) (left : ℕ),
    (fillAmounts D js left).sum = min left (js.map (wantOf D)).sum
  | [], _ => by simp [fillAmounts]
  | j :: js, left => by
    simp only [fillAmounts, List.sum_cons, List.map_cons, fillAmounts_sum js]
    omega

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

theorem serves_of_none (hD : D.only = none) (m : Machine) (j : Job) : serves D m j = true := by
  simp [serves, hD]

theorem getS_setS_ne (m : Machine) {i k : ℕ} (s : Sess) (h : i ≠ k) :
    getS (setS m i s) k = getS m k := by
  simp only [getS, setS, Array.getD_eq_getD_getElem?, Array.getElem?_setIfInBounds_ne h]

/-- **`assign` is the greedy fill, growing jobs included.** If nobody waits
for the engine, the owners of the jobs are distinct, and every growing job
from `idx` on holds enough of its pool for the position it will reach, then
the iteration `assign` builds is `fillIter` of the rest of the job list (with
enough fuel): it advances the holds' positions but calls no `grow`. -/
theorem assign_iter_eq_fillIter (hD : D.only = none) (pre0 : ℕ) :
    ∀ (f : ℕ) (M : Machine) (idx left : ℕ), engineQueuesEmpty D M →
      (M.jobs.map (·.owner)).Nodup →
      (∀ j ∈ M.jobs.drop idx, ∀ p, j.growing = some p →
        ∃ a x, holdOn (getS M j.owner) p = some (a, x) ∧ x + wantOf D j ≤ a) →
      M.jobs.length - idx < f →
      (assign D f M idx left pre0).iter = M.iter ++ fillIter D (M.jobs.drop idx) left := by
  intro f
  induction f with
  | zero => intro M idx left _ _ _ h; omega
  | succ f ih =>
    intro M idx left hq hnd hcov hf
    rw [assign]
    cases hj : M.jobs[idx]? with
    | none =>
      have hlen : M.jobs.length ≤ idx := by simpa [List.getElem?_eq_none_iff] using hj
      simp only [List.drop_eq_nil_of_le hlen, fillIter, List.append_nil]
      split_ifs
      · rw [admitVia_empty D M left hq]
      · rfl
    | some j =>
      obtain ⟨hl, he⟩ := List.getElem?_eq_some_iff.mp hj
      have hdrop : M.jobs.drop idx = j :: M.jobs.drop (idx + 1) := by
        rw [List.drop_eq_getElem_cons hl, he]
      have hcov' : ∀ j' ∈ M.jobs.drop (idx + 1), ∀ p, j'.growing = some p →
          ∃ a x, holdOn (getS M j'.owner) p = some (a, x) ∧ x + wantOf D j' ≤ a :=
        fun j' hj' => hcov j' (by rw [hdrop]; exact List.mem_cons_of_mem _ hj')
      have hfresh : ∀ j' ∈ M.jobs.drop (idx + 1), j.owner ≠ j'.owner := by
        have hsub : ((M.jobs.drop idx).map (·.owner)).Nodup :=
          hnd.sublist ((List.drop_sublist _ _).map _)
        rw [hdrop, List.map_cons, List.nodup_cons] at hsub
        intro j' hj' heq
        exact hsub.1 (heq ▸ List.mem_map.mpr ⟨j', hj', rfl⟩)
      rw [hdrop, fillIter]
      simp only [serves_of_none D hD, Bool.not_true, Bool.false_eq_true, ↓reduceIte]
      by_cases h0 : min (wantOf D j) left = 0
      · rw [if_pos h0, if_pos h0]
        exact ih M (idx + 1) left hq hnd hcov' (by omega)
      · rw [if_neg h0, if_neg h0]
        cases hg : j.growing with
        | none =>
          simp only
          by_cases h1 : left - min (wantOf D j) left = 0
          · rw [if_pos h1, if_pos h1]
          · rw [if_neg h1, if_neg h1]
            refine (ih _ (idx + 1) _ ?_ ?_ ?_ ?_).trans ?_
            · exact fun q hv => hq q hv
            · exact hnd
            · intro j' hj' p' hg'
              exact hcov' j' hj' p' hg'
            · exact (by omega : M.jobs.length - (idx + 1) < f)
            · simp
        | some p =>
          obtain ⟨a, x, hh, hax⟩ := hcov j (by rw [hdrop]; exact List.mem_cons_self) p hg
          simp only [hh]
          have hnot : ¬ (x + min (wantOf D j) left > a) := by
            have := min_le_left (wantOf D j) left; omega
          simp only [hnot, if_false, ite_true]
          by_cases h1 : left - min (wantOf D j) left = 0
          · rw [if_pos h1, if_pos h1]; simp [setS]
          · rw [if_neg h1, if_neg h1]
            refine (ih _ (idx + 1) _ ?_ ?_ ?_ ?_).trans ?_
            · exact fun q hv => hq q hv
            · exact hnd
            · intro j' hj' p' hg'
              have := hcov' j' hj' p' hg'
              simpa [getS, setS, Array.getD_eq_getD_getElem?,
                Array.getElem?_setIfInBounds_ne (hfresh j' hj')] using this
            · exact (by omega : M.jobs.length - (idx + 1) < f)
            · simp [setS]

/-- **`assign` is the greedy fill.** With no job growing and nobody waiting
for the engine, the iteration `assign` builds from job `idx` on, with `left`
tokens, is `fillIter` of the rest of the job list (with enough fuel). -/
theorem assign_eq_fillIter (hD : D.only = none) (m : Machine) (hq : engineQueuesEmpty D m)
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
      simp only [hg j hmem, serves_of_none D hD, Bool.not_true, Bool.false_eq_true, ↓reduceIte]
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
