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

The fill is `Serve.serve` (`Serq/Serve.lean`) on the engine's jobs. `Serve`
sees a job as the prefill work it has left, `0` for a decode (`prefillLeft`).
Without a chunk cap the fill leaves the jobs what `Serve.serve` leaves them
(`serve_fillAmounts`), with a cap `c` what `Serve.serveCap c` does
(`serveCap_fillAmounts`), for jobs that hold work (`prefillLeft` says why
every job on a machine `exec` reaches does; this is read off `exec`, not
proved). So `Serve`'s theorems hold of `Exec`'s jobs: while every
decoding job is ahead of every prefilling one (`Serve.Shape`), the fill in
the jobs' order is the fill in decode-first order
(`fillAmounts_eq_decode_first`), each of the first decoders takes one token
and the prefills share what they leave (`fillAmounts_decoders_first`), and
that is the iteration `assign` builds (`assign_decode_first`). An iteration
without a cap keeps the shape (`fill_preserves_shape`); one with a cap may
break it (`chunk_cap_breaks_fill_shape`).

Not covered yet: an iteration whose growth crosses a block boundary (the
vLLM programs' decode every `bs` tokens and their prefill chunks after the
first) and one that admits waiting requests; for those programs the
theorems apply to the iterations that do neither. Nor that a path keeps the
job list in the shape: a run joins the jobs by its session's latest hold
admission (`exec`, `.run`), so a request in a hold decodes in the place its
admission gave it, but on an engine whose requests hold nothing every run
joins behind the jobs started before it, and a decode waits behind the
prompts that arrived while it prefilled (`DaiChain.absSlot_leaves_shape`).

Key definitions: `Exec.wantOf`, `Exec.fillAmounts`, `Exec.fillIter`,
`Exec.prefillLeft`, `Exec.decodeFirstJobs`.
Key theorems: `Exec.assign_iter_eq_fillIter`, `Exec.assign_eq_fillIter`,
`Exec.fillIter_eq_amounts`, `Exec.fillAmounts_fifo`, `Exec.fillAmounts_le_want`,
`Exec.fillAmounts_sum`, `Exec.serve_fillAmounts`, `Exec.serveCap_fillAmounts`,
`Exec.fillAmounts_eq_decode_first`, `Exec.fillAmounts_decoders_first`,
`Exec.fill_preserves_shape`, `Exec.chunk_cap_breaks_fill_shape`,
`Exec.assign_decode_first`.
-/
import Serq.Exec
import Serq.Serve

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

/-! ### The fill is `Serve.serve` -/

/-- A job as `Serve` sees a resident: the prefill work it has left, `0` for
a decode. `Serve` cannot tell a decode from a prefill with nothing left, nor
a decode with a token left from one with none, so the theorems below take
every job to hold work (`0 < j.left`). Every job on a machine `exec` reaches
does: a run of zero work never becomes a job (`exec`, `.run`),
`endIteration` drops a job with nothing left, and preemption only removes
jobs. This is read off `exec`, not proved: `Inv` does not carry it. -/
def prefillLeft (j : Job) : ℕ := if j.mode = .decode then 0 else j.left

/-- The jobs in decode-first order: decodes first, each group in its order. -/
def decodeFirstJobs (js : List Job) : List Job :=
  js.filter (·.mode = .decode) ++ js.filter (·.mode ≠ .decode)

/-- **The greedy fill is `Serve.serve`** (no chunk cap): what the fill leaves
each job of its prefill is what `Serve.serve` leaves the resident. The cap
of an iteration is `iterDeployment`'s, so `D` is the iteration's
deployment. -/
theorem serve_fillAmounts (hc : D.chunk = 0) :
    ∀ (js : List Job) (left : ℕ), (∀ j ∈ js, 0 < j.left) →
      Serve.serve left (js.map prefillLeft) =
        (js.zip (fillAmounts D js left)).map fun p => prefillLeft p.1 - p.2
  | [], _, _ => rfl
  | j :: js, left, hpos => by
    have hj : 0 < j.left := hpos j List.mem_cons_self
    have hrest : ∀ j' ∈ js, 0 < j'.left := fun j' h => hpos j' (List.mem_cons_of_mem _ h)
    simp only [List.map_cons, fillAmounts, List.zip_cons_cons]
    by_cases hm : j.mode = .decode
    · have hr : prefillLeft j = 0 := by simp [prefillLeft, hm]
      have hw : wantOf D j = 1 := by simp [wantOf, hm]; omega
      rw [hr, hw, Serve.serve]
      have : left - min 1 left = left - 1 := by omega
      rw [this, serve_fillAmounts hc js (left - 1) hrest]
      simp
    · obtain ⟨r, hr⟩ : ∃ r, j.left = r + 1 := ⟨j.left - 1, by omega⟩
      have hres : prefillLeft j = r + 1 := by simp [prefillLeft, hm, hr]
      have hw : wantOf D j = r + 1 := by
        unfold wantOf; cases h : j.mode <;> simp_all
      rw [hres, hw, Serve.serve, serve_fillAmounts hc js _ hrest]

/-- **With a chunk cap, the greedy fill is `Serve.serveCap`.** The two
theorems are apart because a chunk of `0` means no cap to `wantOf` and no
prefill at all to `Serve.serveCap`. -/
theorem serveCap_fillAmounts (hc : 0 < D.chunk) :
    ∀ (js : List Job) (left : ℕ), (∀ j ∈ js, 0 < j.left) →
      Serve.serveCap D.chunk left (js.map prefillLeft) =
        (js.zip (fillAmounts D js left)).map fun p => prefillLeft p.1 - p.2
  | [], _, _ => rfl
  | j :: js, left, hpos => by
    have hj : 0 < j.left := hpos j List.mem_cons_self
    have hrest : ∀ j' ∈ js, 0 < j'.left := fun j' h => hpos j' (List.mem_cons_of_mem _ h)
    simp only [List.map_cons, fillAmounts, List.zip_cons_cons]
    by_cases hm : j.mode = .decode
    · have hr : prefillLeft j = 0 := by simp [prefillLeft, hm]
      have hw : wantOf D j = 1 := by simp [wantOf, hm]; omega
      rw [hr, hw, Serve.serveCap]
      have : left - min 1 left = left - 1 := by omega
      rw [this, serveCap_fillAmounts hc js (left - 1) hrest]
      simp
    · obtain ⟨r, hr⟩ : ∃ r, j.left = r + 1 := ⟨j.left - 1, by omega⟩
      have hres : prefillLeft j = r + 1 := by simp [prefillLeft, hm, hr]
      have hw : wantOf D j = min (r + 1) D.chunk := by
        unfold wantOf; cases h : j.mode <;> simp_all
      rw [hres, hw, Serve.serveCap, serveCap_fillAmounts hc js _ hrest]

/-- The two decode-first orders agree: `Serve.decodeFirst` of what `Serve`
reads of the jobs is what it reads of `decodeFirstJobs`. -/
theorem decodeFirst_prefillLeft (js : List Job) (hpos : ∀ j ∈ js, 0 < j.left) :
    Serve.decodeFirst (js.map prefillLeft) = (decodeFirstJobs js).map prefillLeft := by
  have key : ∀ j ∈ js, (prefillLeft j = 0 ↔ j.mode = .decode) := by
    intro j hj
    have := hpos j hj
    by_cases hm : j.mode = .decode <;> simp [prefillLeft, hm]; omega
  unfold Serve.decodeFirst decodeFirstJobs
  rw [List.map_append, List.filter_map, List.filter_map]
  congr 2
  · apply List.filter_congr; intro j hj; simp [key j hj]
  · apply List.filter_congr; intro j hj; simp [key j hj]

/-- In the shape, the jobs' order is decode-first order. -/
theorem decodeFirstJobs_eq_self :
    ∀ (js : List Job), (∀ j ∈ js, 0 < j.left) → Serve.Shape (js.map prefillLeft) →
      decodeFirstJobs js = js
  | [], _, _ => rfl
  | j :: js, hpos, h => by
    have hrest : ∀ j' ∈ js, 0 < j'.left := fun j' h => hpos j' (List.mem_cons_of_mem _ h)
    by_cases hm : j.mode = .decode
    · have hr : prefillLeft j = 0 := by simp [prefillLeft, hm]
      rw [List.map_cons, hr] at h
      have ih := decodeFirstJobs_eq_self js hrest h
      unfold decodeFirstJobs at ih ⊢
      simp only [List.filter_cons, hm, decide_true, ↓reduceIte, ne_eq, not_true_eq_false,
        decide_false, Bool.false_eq_true, List.cons_append]
      rw [ih]
    · have hj := hpos j List.mem_cons_self
      obtain ⟨r, hr⟩ : ∃ r, j.left = r + 1 := ⟨j.left - 1, by omega⟩
      have hres : prefillLeft j = r + 1 := by simp [prefillLeft, hm, hr]
      rw [List.map_cons, hres] at h
      simp only [Serve.Shape] at h
      have hnd : ∀ j' ∈ js, j'.mode ≠ .decode := by
        intro j' hj' hm'
        have := h (prefillLeft j') (List.mem_map.mpr ⟨j', hj', rfl⟩)
        simp [prefillLeft, hm'] at this
      have h0 : js.filter (·.mode = .decode) = [] := by
        rw [List.filter_eq_nil_iff]; intro j' hj'; simpa using hnd j' hj'
      have h1 : js.filter (·.mode ≠ .decode) = js := by
        rw [List.filter_eq_self]; intro j' hj'; simpa using hnd j' hj'
      unfold decodeFirstJobs
      simp only [List.filter_cons, hm, decide_false, Bool.false_eq_true, ↓reduceIte, ne_eq,
        not_false_eq_true, decide_true]
      simp only [ne_eq] at h1
      rw [h0, h1]; rfl

/-- **The jobs' order is decode-first, in `Exec`** (`Serve.serve_eq_decode_first`
for the fill): in the shape, the greedy fill of the jobs in their order is the
greedy fill in decode-first order. -/
theorem fillAmounts_eq_decode_first (js : List Job) (left : ℕ) (hpos : ∀ j ∈ js, 0 < j.left)
    (h : Serve.Shape (js.map prefillLeft)) :
    fillAmounts D js left = fillAmounts D (decodeFirstJobs js) left := by
  rw [decodeFirstJobs_eq_self js hpos h]

/-- The fill of two lists is the fill of the first, then the fill of the
second from what the first leaves. -/
theorem fillAmounts_append : ∀ (a b : List Job) (left : ℕ),
    fillAmounts D (a ++ b) left =
      fillAmounts D a left ++ fillAmounts D b (left - (fillAmounts D a left).sum)
  | [], b, left => by simp [fillAmounts]
  | j :: a, b, left => by
    simp only [List.cons_append, fillAmounts, List.sum_cons, fillAmounts_append a b, Nat.sub_sub]

/-- **The decoders are served first.** In the shape, each of the first `left`
decoding jobs takes one token, and the prefills have the fill of what the
decoders leave. -/
theorem fillAmounts_decoders_first (js : List Job) (left : ℕ) (hpos : ∀ j ∈ js, 0 < j.left)
    (h : Serve.Shape (js.map prefillLeft)) :
    fillAmounts D js left =
      fillAmounts D (js.filter (·.mode = .decode)) left ++
        fillAmounts D (js.filter (·.mode ≠ .decode)) (left - (js.filter (·.mode = .decode)).length) := by
  conv_lhs => rw [← decodeFirstJobs_eq_self js hpos h]
  unfold decodeFirstJobs
  rw [fillAmounts_append, fillAmounts_sum]
  have hw : ((js.filter (·.mode = .decode)).map (wantOf D)).sum = (js.filter (·.mode = .decode)).length := by
    have : ∀ j ∈ js.filter (·.mode = .decode), wantOf D j = 1 := by
      intro j hj
      obtain ⟨hjs, hm⟩ := List.mem_filter.mp hj
      have := hpos j hjs
      simp only [decide_eq_true_eq] at hm
      simp [wantOf, hm]; omega
    rw [List.map_congr_left this]; simp
  rw [hw]
  congr 2
  omega

/-- The fill keeps the shape (`Serve.serve_preserves_shape`), without a cap. -/
theorem fill_preserves_shape (hc : D.chunk = 0) (js : List Job) (left : ℕ)
    (hpos : ∀ j ∈ js, 0 < j.left) (h : Serve.Shape (js.map prefillLeft)) :
    Serve.Shape ((js.zip (fillAmounts D js left)).map fun p => prefillLeft p.1 - p.2) := by
  rw [← serve_fillAmounts D hc js left hpos]
  exact Serve.serve_preserves_shape left _ h

/-- A chunk cap breaks it (`Serve.chunk_cap_breaks_shape`): two prefills of 5
and 3 tokens, budget 8, cap 4. -/
theorem chunk_cap_breaks_fill_shape (hc : D.chunk = 4) :
    Serve.Shape (([⟨0, .prefill, 5, none⟩, ⟨1, .prefill, 3, none⟩] : List Job).map prefillLeft) ∧
    ¬ Serve.Shape (((([⟨0, .prefill, 5, none⟩, ⟨1, .prefill, 3, none⟩] : List Job)).zip
        (fillAmounts D [⟨0, .prefill, 5, none⟩, ⟨1, .prefill, 3, none⟩] 8)).map
        fun p => prefillLeft p.1 - p.2) := by
  rw [← serveCap_fillAmounts D (by omega) _ 8 (by simp), hc]
  exact Serve.chunk_cap_breaks_shape

/-- **`assign` serves decode-first.** Under the hypotheses of
`assign_iter_eq_fillIter`, with every job holding work and the jobs in the
shape, the iteration `assign` builds is the greedy fill in decode-first order. -/
theorem assign_decode_first (hD : D.only = none) (pre0 f left : ℕ) (M : Machine)
    (hq : engineQueuesEmpty D M) (hnd : (M.jobs.map (·.owner)).Nodup)
    (hcov : ∀ j ∈ M.jobs, ∀ p, j.growing = some p →
        ∃ a x, holdOn (getS M j.owner) p = some (a, x) ∧ x + wantOf D j ≤ a)
    (hf : M.jobs.length < f) (hpos : ∀ j ∈ M.jobs, 0 < j.left)
    (h : Serve.Shape (M.jobs.map prefillLeft)) :
    (assign D f M 0 left pre0).iter = M.iter ++ fillIter D (decodeFirstJobs M.jobs) left := by
  rw [assign_iter_eq_fillIter D hD pre0 f M 0 left hq hnd (by simpa using hcov) (by omega),
    List.drop_zero, decodeFirstJobs_eq_self _ hpos h]

end Exec
end SerqLang
