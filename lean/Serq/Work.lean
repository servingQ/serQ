/-
# Claims over iterations from what an iteration's start sees

`every_iteration_of` reduces a claim over every iteration of every path to
a statement about one iteration's start: if an invariant `I` holds of the
initial machine and is kept by an event's handling, by `settle` and by
starting an iteration, and every iteration started from a machine satisfying
`I` satisfies `Q`, then every iteration of every path satisfies `Q`.

Applied to the greedy fill (`Serq/Fill.lean`), it lifts work conservation
from one iteration to every iteration of every path: for a program that
grows no hold, on an engine that serves every resident (no `serve only`) and
admits for no pool, each iteration takes `min budget demand` tokens
(`work_conserving`): the condition (8)-(9) of Dai et al. (arXiv
2504.07347), a batch is full whenever the residents could fill it.

Key theorems: `Exec.every_iteration_of`, `Exec.work_conserving`.
-/
import Serq.Inv
import Serq.Fill

namespace SerqLang
namespace Exec

variable (D : Deployment)

/-- The record of the running iteration satisfies `Q`. -/
def Good (Q : IterRec → Prop) (m : Machine) : Prop := m.iterEnd.isSome → Q m.last

theorem good_handle {Q : IterRec → Prop} (m : Machine) (t q : ℕ) (h : Good Q m) :
    Good Q (handle m t q) := by
  unfold handle; simp only
  split
  · have := key0_endIteration { m with now := t, iterEnd := none }
    simp only [key0, Prod.mk.injEq] at this
    intro hs; rw [this.2.2.1] at hs; simp at hs
  · split
    · split
      · exact h
      · exact h
    · exact h

theorem every_iteration_of (I : Machine → Prop) (Q : IterRec → Prop) (w : Workload) (P : Prog)
    (h0 : I (Exec.initial D w.init.length w.attr P w))
    (hh : ∀ m t q, I m → I (handle m t q))
    (hs : ∀ m, I m → I (settle D m))
    (hst : ∀ m, I m → I (startIteration D m))
    (hq : ∀ m, I m → (startIteration D m).iterEnd.isSome → Q (startIteration D m).last) :
    ∀ m, Reach D w P m → I m ∧ Good Q m := by
  have hae : ∀ m, I m → Good Q m → I (afterEvent D m) ∧ Good Q (afterEvent D m) := by
    intro m hi hg
    have hsm := same_settle D m
    unfold afterEvent; simp only
    split
    · exact ⟨hst _ (hs _ hi), hq _ (hs _ hi)⟩
    · refine ⟨hs _ hi, fun he => ?_⟩
      rw [hsm.last]; exact hg (hsm.iterEnd ▸ he)
  intro m hr
  induction hr with
  | start => exact hae _ h0 (fun he => by simp [Exec.initial] at he)
  | step _ ih =>
    unfold step
    split
    · exact ih
    · exact hae _ (hh _ _ _ ih.1) (good_handle _ _ _ ih.2)

/-- The tokens of the greedy fill. -/
theorem tokSum_fillIter (js : List Job) (left : ℕ) :
    tokSum (fillIter D js left) = min left (js.map (wantOf D)).sum := by
  rw [fillIter_eq_amounts, ← fillAmounts_sum]
  have key : ∀ (js : List Job) (xs : List ℕ), xs.length = js.length →
      ((((js.zip xs).filter fun p => 0 < p.2).map fun p => (p.1.owner, p.2)).map (·.2)).sum = xs.sum := by
    intro js
    induction js with
    | nil => intro xs h; cases xs <;> simp_all
    | cons j js ih =>
      intro xs h
      cases xs with
      | nil => simp at h
      | cons x xs =>
        simp only [List.length_cons, add_left_inj] at h
        have := ih xs h
        by_cases hx : 0 < x
        · simp [List.filter_cons, hx] at this ⊢; omega
        · have : x = 0 := by omega
          subst this
          simpa [List.filter_cons] using ih xs h
  exact key js _ (fillAmounts_length D js left)

/-- An iteration started on `m`, when the engine serves every resident,
admits for no pool and no job grows, takes `min budget demand`. -/
theorem startIteration_wc (hD : D.only = none) (hv : ∀ p, (pdef D p).viaEngine = false)
    (m : Machine) (hg : ∀ j ∈ m.jobs, j.growing = none) :
    (startIteration D m).iterEnd.isSome →
      (startIteration D m).last.stats.tokens = min D.budget (startIteration D m).last.demand := by
  have hq : engineQueuesEmpty D m := fun p hp => by simp [hv p] at hp
  have ha := assign_eq_fillIter D hD m hq hg m.preempts (m.jobs.length + 100000) 0 D.budget []
    (by omega)
  unfold startIteration
  simp only
  split
  · simp only [List.drop_zero, List.nil_append] at ha
    rw [ha]
    split
    · intro _
      simp only [iterRec, iterStats_tokens]
      exact tokSum_fillIter D m.jobs D.budget
    · intro h; simp at h
  · intro h; simp at h

/-- **Work conservation.** On an engine that serves every resident and admits
for no pool, every iteration of a program that grows no hold takes
`min budget demand` tokens. -/
theorem work_conserving (hD : D.only = none) (hv : ∀ p, (pdef D p).viaEngine = false)
    (W : Workload → Prop) (P : Prog) (hg : Route.grows P = false) :
    EveryIteration D W P fun r => r.stats.tokens = min D.budget r.demand := by
  intro w _ m hr
  have := every_iteration_of D (Inv P) (fun r => r.stats.tokens = min D.budget r.demand) w P
    (inv_initial D w P) (fun m t q h => inv_handle m t q h) (fun m h => inv_settle D m h)
    (fun m h => inv_startIteration D m h)
    (fun m h => startIteration_wc D hD hv m fun j hj => by
      obtain ⟨w', k, hs⟩ := h.2 j hj
      have := grows_sub hs hg
      simp [Route.grows] at this
      exact Option.not_isSome_iff_eq_none.mp (by simp [this.1]))
    m hr
  exact this.2

/-- **`assign` under `serve only` is the greedy fill of the residents it
serves.** With no job growing and nobody waiting for the engine, the
iteration from job `idx` on is `fillIter` of the jobs `serve only` lets
take tokens (read on the residents as they stand, which the iteration does
not change). -/
theorem assign_eq_fillIter_only (m : Machine) (hq : engineQueuesEmpty D m)
    (hg : ∀ j ∈ m.jobs, j.growing = none) (pre0 : ℕ) :
    ∀ (f idx left : ℕ) (acc : List (ℕ × ℕ)), m.jobs.length - idx < f →
      assign D f { m with iter := acc } idx left pre0
        = { m with iter := acc ++ fillIter D ((m.jobs.drop idx).filter (serves D m)) left } := by
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
      simp only [hdrop, List.filter_nil, fillIter, List.append_nil]
      split_ifs
      · have := admitVia_empty D { m with iter := acc } left hq
        rw [this]
      · rfl
    | some j =>
      have hmem : j ∈ m.jobs := List.mem_of_getElem? hj
      have hlen : idx < m.jobs.length := by
        rcases List.getElem?_eq_some_iff.mp hj with ⟨hl, _⟩; exact hl
      have hdrop : m.jobs.drop idx = j :: m.jobs.drop (idx + 1) := by
        rw [List.drop_eq_getElem_cons hlen]
        congr 1
        rcases List.getElem?_eq_some_iff.mp hj with ⟨_, he⟩; exact he
      rw [hdrop]
      have hsv : serves D { m with iter := acc } j = serves D m j := rfl
      by_cases hs : serves D m j = true
      · rw [List.filter_cons_of_pos hs, fillIter]
        simp only [hsv, hs, Bool.not_true, Bool.false_eq_true, ↓reduceIte, hg j hmem]
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
      · rw [List.filter_cons_of_neg hs]
        simp only [hsv, hs, Bool.not_false, ↓reduceIte]
        exact ih (idx + 1) left acc (by omega)

end Exec
end SerqLang
