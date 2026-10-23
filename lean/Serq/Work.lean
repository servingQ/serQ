/-
# Claims over iterations from what an iteration's start sees

`every_iteration_of` reduces a claim over every iteration of every path to
a statement about one iteration's start: if an invariant `I` holds of the
initial machine and is kept by an event's handling, by `settle` and by
starting an iteration, and every iteration started from a machine satisfying
`I` satisfies `Q`, then every iteration of every path satisfies `Q`.

It lifts work conservation from one iteration (`assign_tokSum`, in any
serving order) to every iteration of every path: for a program that grows
no hold, on an engine that serves every resident (no `serve only`) and
admits for no pool, each iteration takes `min budget demand` tokens
(`work_conserving`): the condition (8)-(9) of Dai et al. (arXiv
2504.07347), a batch is full whenever the residents could fill it.

Key theorems: `Exec.every_iteration_of`, `Exec.assign_tokSum`,
`Exec.work_conserving`, `Exec.plan_eq_fillIter_only`.
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

/-- **`assign` hands out `min left demand`, in any serving order.** With no
job of `todo` growing, `serve only` serving every resident and nobody
waiting for the engine, the batch `assign` adds takes `min left` of what
`todo` wants, whichever order the serve keys pick the jobs in. -/
theorem assign_tokSum (hD : D.only = none) (m : Machine) (hq : engineQueuesEmpty D m) (pre0 : ℕ) :
    ∀ (f : ℕ) (todo : List Job) (left : ℕ) (acc : List (ℕ × ℕ)), (∀ j ∈ todo, j.growing = none) →
      todo.length < f →
      ∃ X, assign D f { m with iter := acc } todo left pre0 = { m with iter := acc ++ X } ∧
        tokSum X = min left (todo.map (wantOf D)).sum := by
  intro f
  induction f with
  | zero => intro todo left acc _ h; omega
  | succ f ih =>
    intro todo left acc hg hf
    rw [assign]
    cases hp : pick D { m with iter := acc } todo with
    | none =>
      have h0 : todo = [] := (pick_eq_none D _ todo).mp hp
      subst h0
      refine ⟨[], ?_, by simp [tokSum]⟩
      simp only [List.append_nil]
      split_ifs
      · rw [admitVia_empty D { m with iter := acc } left hq]
      · rfl
    | some j =>
      have hj := pick_mem D _ hp
      have hsum := List.sum_map_erase (wantOf D) hj
      have hg' : ∀ j' ∈ todo.erase j, j'.growing = none := fun j' hj' => hg j' (List.mem_of_mem_erase hj')
      have hlen : (todo.erase j).length < f := by
        rw [List.length_erase_of_mem hj]; have := List.length_pos_of_mem hj; omega
      simp only [serves_of_none D hD, Bool.not_true, Bool.false_eq_true, ↓reduceIte, hg j hj]
      by_cases h0 : min (wantOf D j) left = 0
      · rw [if_pos h0]
        obtain ⟨X, hX, hs⟩ := ih (todo.erase j) left acc hg' hlen
        refine ⟨X, hX, ?_⟩
        rw [hs, ← hsum]; omega
      · rw [if_neg h0]
        by_cases h1 : left - min (wantOf D j) left = 0
        · rw [if_pos h1]
          refine ⟨[(j.owner, min (wantOf D j) left)], rfl, ?_⟩
          simp only [tokSum, List.map_cons, List.map_nil, List.sum_cons, List.sum_nil, Nat.add_zero]
          rw [← hsum]; omega
        · rw [if_neg h1]
          obtain ⟨X, hX, hs⟩ := ih (todo.erase j) (left - min (wantOf D j) left)
            (acc ++ [(j.owner, min (wantOf D j) left)]) hg' hlen
          refine ⟨(j.owner, min (wantOf D j) left) :: X, by rw [hX]; simp, ?_⟩
          simp only [tokSum, List.map_cons, List.sum_cons] at hs ⊢
          rw [hs, ← hsum]; omega

/-- An iteration started on `m`, when the engine serves every resident,
admits for no pool and no job grows, takes `min budget demand`, whatever
its serving order. -/
theorem startIteration_wc (hD : D.only = none) (hv : ∀ p, (pdef D p).viaEngine = false)
    (m : Machine) (hg : ∀ j ∈ m.jobs, j.growing = none) :
    (startIteration D m).iterEnd.isSome →
      (startIteration D m).last.stats.tokens = min D.budget (startIteration D m).last.demand := by
  -- the iteration runs `iterDeployment D m`, which is `D` up to its chunk cap
  have hq : engineQueuesEmpty (iterDeployment D m) m := fun p hp => by
    simp [iterDeployment_pdef, hv p] at hp
  have hD' : (iterDeployment D m).only = none := (iterDeployment_only D m).trans hD
  obtain ⟨X, hX, hs⟩ := assign_tokSum (iterDeployment D m) hD' m hq m.preempts
    (m.jobs.length + 100000) m.jobs (iterDeployment D m).budget [] hg (by omega)
  have hp : plan (iterDeployment D m) m = { m with iter := X } := by
    unfold plan
    rw [hX, List.nil_append]
    exact settleIter_eq_self _ hg
  unfold startIteration
  simp only
  split
  · rw [hp]
    split
    · intro _
      simp only [iterRec, iterStats_tokens]
      rw [hs, iterDeployment_budget]
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
serves.** Without serve keys, with no job of `todo` growing and nobody
waiting for the engine, the iteration from `todo` is `fillIter` of the jobs
`serve only` lets take tokens (read on the residents as they stand, which
the iteration does not change). -/
theorem assign_eq_fillIter_only (hK : D.serveBy = []) (m : Machine) (hq : engineQueuesEmpty D m)
    (pre0 : ℕ) :
    ∀ (f : ℕ) (todo : List Job) (left : ℕ) (acc : List (ℕ × ℕ)), (∀ j ∈ todo, j.growing = none) →
      todo.length < f →
      assign D f { m with iter := acc } todo left pre0
        = { m with iter := acc ++ fillIter D (todo.filter (serves D m)) left } := by
  intro f
  induction f with
  | zero => intro todo left acc _ h; omega
  | succ f ih =>
    intro todo left acc hg hf
    rw [assign]
    cases todo with
    | nil =>
      simp only [pick_of_nil D hK, List.head?_nil, List.filter_nil, fillIter, List.append_nil]
      split_ifs
      · rw [admitVia_empty D { m with iter := acc } left hq]
      · rfl
    | cons j js =>
      have hg' : ∀ j' ∈ js, j'.growing = none := fun j' hj' => hg j' (List.mem_cons_of_mem _ hj')
      simp only [List.length_cons] at hf
      simp only [pick_of_nil D hK, List.head?_cons, List.erase_cons_head]
      have hsv : serves D { m with iter := acc } j = serves D m j := rfl
      by_cases hs : serves D m j = true
      · rw [List.filter_cons_of_pos hs, fillIter]
        simp only [hsv, hs, Bool.not_true, Bool.false_eq_true, ↓reduceIte, hg j List.mem_cons_self]
        by_cases h0 : min (wantOf D j) left = 0
        · rw [if_pos h0, if_pos h0]
          exact ih js left acc hg' (by omega)
        · rw [if_neg h0, if_neg h0]
          by_cases h1 : left - min (wantOf D j) left = 0
          · rw [if_pos h1, if_pos h1]
          · rw [if_neg h1, if_neg h1]
            rw [ih js (left - min (wantOf D j) left) (acc ++ [(j.owner, min (wantOf D j) left)]) hg'
              (by omega)]
            simp
      · rw [List.filter_cons_of_neg hs]
        simp only [hsv, hs, Bool.not_false, ↓reduceIte]
        exact ih js left acc hg' (by omega)

/-- **The iteration an engine plans under `serve only`** is the greedy fill
of the residents it serves, without serve keys, when no resident grows and
nobody waits for the engine. -/
theorem plan_eq_fillIter_only (hK : D.serveBy = []) (m : Machine) (hq : engineQueuesEmpty D m)
    (hg : ∀ j ∈ m.jobs, j.growing = none) :
    plan D m = { m with iter := fillIter D (m.jobs.filter (serves D m)) D.budget } := by
  unfold plan
  rw [assign_eq_fillIter_only D hK m hq m.preempts _ m.jobs D.budget [] hg (by omega), List.nil_append]
  exact settleIter_eq_self _ hg
/-- An entry of the greedy fill is a job's, at most what it wants. -/
theorem mem_fillIter (D : Deployment) : ∀ (js : List Job) (left : ℕ) (e : ℕ × ℕ), e ∈ fillIter D js left →
    ∃ j ∈ js, e.1 = j.owner ∧ 0 < e.2 ∧ e.2 ≤ wantOf D j
  | [], _, _, h => by simp [fillIter] at h
  | j :: js, left, e, h => by
    unfold fillIter at h
    split at h
    · obtain ⟨j', hj', h'⟩ := mem_fillIter D js left e h
      exact ⟨j', List.mem_cons_of_mem _ hj', h'⟩
    · rename_i h0
      split at h
      · simp only [List.mem_singleton] at h
        subst h
        exact ⟨j, List.mem_cons_self, rfl, Nat.pos_of_ne_zero h0, min_le_left _ _⟩
      · rcases List.mem_cons.mp h with rfl | h
        · exact ⟨j, List.mem_cons_self, rfl, Nat.pos_of_ne_zero h0, min_le_left _ _⟩
        · obtain ⟨j', hj', h'⟩ := mem_fillIter D js _ e h
          exact ⟨j', List.mem_cons_of_mem _ hj', h'⟩

/-- The fill of jobs that want one token each, fewer than the budget. -/
theorem fillIter_ones (D : Deployment) : ∀ (js : List Job) (B : ℕ), (∀ j ∈ js, wantOf D j = 1) →
    js.length < B → fillIter D js B = js.map fun j => (j.owner, 1)
  | [], _, _, _ => rfl
  | j :: js, B, h1, hl => by
    have hj := h1 j List.mem_cons_self
    simp only [List.length_cons] at hl
    unfold fillIter
    rw [hj, if_neg (by omega), if_neg (by omega)]
    simp only [List.map_cons, List.cons.injEq]
    rw [show min 1 B = 1 by omega]
    exact ⟨rfl, fillIter_ones D js (B - 1) (fun j hj' => h1 j (List.mem_cons_of_mem _ hj')) (by omega)⟩


end Exec
end SerqLang
