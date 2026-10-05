/-
# Bari et al., Theorem 2: RAD's chain is positive recurrent

On RAD's chain on job lists (`BariChain`), below capacity: the empty list
is reached in bounded expected time from every state (`hit_nil_le`), every
state reaches every state (`irreducible`), and every state is positive
recurrent (`positive_recurrent`). The machine chain of the program projects
onto this one (`BariSim.simulation`).

Every request brings at least 129 tokens (a tile of prompt and one of
output), so below capacity, load < 128, a slot brings no request with
positive probability; and a slot with no arrival serves at least one token
while there is a job, so from a state of bounded backlog the empty list is
a bounded number of arrival-free slots away (`Recurrence.hit_le_of_reach`).
-/
import Serq.Recurrence
import papers.BariChain

namespace SerqLang
namespace Papers
namespace BariRecurrent

open Foster BariChain

/-! ### The jobs the chain holds -/

/-- A job of the chain: a prefill of 1 to 8 whole tiles with an output of 1
to 512 tokens, or a decode with 1 to 512 tokens left. -/
def GoodJob (j : AJob) : Prop :=
  (j.1 = .prefill ∧ 1 ≤ j.2.1 ∧ 128 ∣ j.2.1 ∧ j.2.1 ≤ 1024 ∧ 1 ≤ j.2.2 ∧ j.2.2 ≤ 512) ∨
  (j.1 = .decode ∧ 1 ≤ j.2.1 ∧ j.2.1 ≤ 512)

/-- Every job of the list is one the chain can hold. -/
def Good (js : List AJob) : Prop := ∀ j ∈ js, GoodJob j

/-- A job after the batch: as `GoodJob`, but its left work may be 0. -/
def WeakJob (j : AJob) : Prop :=
  (j.1 = .prefill ∧ 128 ∣ j.2.1 ∧ j.2.1 ≤ 1024 ∧ 1 ≤ j.2.2 ∧ j.2.2 ≤ 512) ∨
  (j.1 = .decode ∧ j.2.1 ≤ 512)

/-- After the batch: every job less its share. -/
def aft (dm : Bool) (js : List AJob) (b : ℕ) : List AJob :=
  List.zipWith (fun j s => (j.1, j.2.1 - s, j.2.2)) js (shares dm js b)

theorem aft_cons (dm : Bool) (j : AJob) (js : List AJob) (b : ℕ) : aft dm (j :: js) b =
    (j.1, j.2.1 - min (want dm j) b, j.2.2) :: aft dm js (b - min (want dm j) b) := rfl

/-- The tokens a job still brings to the backlog. -/
def bk (j : AJob) : ℕ := j.2.1 + if j.1 = .prefill then j.2.2 else 0

theorem backlog_nil : backlog [] = 0 := rfl

theorem backlog_cons (j : AJob) (js : List AJob) : backlog (j :: js) = bk j + backlog js := by
  simp [backlog, bk]

theorem backlog_append (a b : List AJob) : backlog (a ++ b) = backlog a + backlog b := by
  simp [backlog, List.map_append, List.sum_append]

theorem want_le (dm : Bool) (j : AJob) : want dm j ≤ j.2.1 := by
  obtain ⟨m, l, o⟩ := j
  cases m <;> cases dm <;> simp [want]

/-- The batch takes `min b (Σ want)`. -/
theorem shares_sum (dm : Bool) : ∀ (js : List AJob) (b : ℕ),
    (shares dm js b).sum = min b (js.map (want dm)).sum
  | [], b => by simp [shares]
  | j :: js, b => by
    simp only [shares, List.sum_cons, List.map_cons, shares_sum dm js]
    simp only [Nat.min_def]
    split_ifs <;> omega

/-- Every job loses its share. -/
theorem aft_backlog (dm : Bool) : ∀ (js : List AJob) (b : ℕ),
    backlog (aft dm js b) + (shares dm js b).sum = backlog js
  | [], b => rfl
  | j :: js, b => by
    rw [aft_cons, backlog_cons, backlog_cons]
    simp only [shares, List.sum_cons]
    have h1 := aft_backlog dm js (b - min (want dm j) b)
    have h2 : min (want dm j) b ≤ j.2.1 := le_trans (min_le_left _ _) (want_le dm j)
    simp only [bk] at *
    omega

/-- A finished prefill comes back as a decode of its output, a finished
decode leaves: the backlog is the same. -/
theorem split_backlog : ∀ l : List AJob, backlog (l.filter (·.2.1 ≠ 0)) +
    backlog ((l.filter fun j => j.2.1 = 0 ∧ j.1 = .prefill).map
      fun j => ((.decode, j.2.2, j.2.2) : AJob)) = backlog l
  | [] => rfl
  | x :: l => by
    have ih := split_backlog l
    by_cases h0 : x.2.1 = 0
    · by_cases hp : x.1 = .prefill
      · simp only [List.filter_cons, h0, hp, ne_eq, not_true_eq_false, decide_false,
          and_self, decide_true, List.map_cons, backlog_cons, Bool.false_eq_true, ↓reduceIte]
        simp only [bk, hp, h0, ↓reduceIte, reduceCtorEq] at *
        simp only [ne_eq] at ih
        omega
      · simp only [List.filter_cons, h0, hp, ne_eq, not_true_eq_false, decide_false,
          and_false, backlog_cons, Bool.false_eq_true, ↓reduceIte]
        simp only [bk, hp, h0, ↓reduceIte] at *
        simp only [ne_eq] at ih
        omega
    · simp only [List.filter_cons, h0, ne_eq, not_false_eq_true, decide_true, false_and,
        decide_false, backlog_cons, Bool.false_eq_true, ↓reduceIte]
      simp only [ne_eq] at ih
      omega

/-- Arrivals bring their prompts and outputs. -/
theorem new_backlog : ∀ rs : List (ℕ × ℕ),
    backlog (rs.map fun r => ((.prefill, r.1, r.2) : AJob)) = (rs.map fun r => r.1 + r.2).sum
  | [] => rfl
  | r :: rs => by
    rw [List.map_cons, backlog_cons, new_backlog rs]
    simp [bk]

/-- The tokens of the batch a list schedules. -/
def sS (js : List AJob) : ℕ := (shares (decodeMode js) js 128).sum

theorem absSlot_eq (rs : List (ℕ × ℕ)) (js : List AJob) (h : js ≠ []) : absSlot rs js =
    (aft (decodeMode js) js 128).filter (·.2.1 ≠ 0) ++ rs.map (fun r => ((.prefill, r.1, r.2) : AJob)) ++
      (((aft (decodeMode js) js 128).filter fun j => j.2.1 = 0 ∧ j.1 = .prefill).map
        fun j => ((.decode, j.2.2, j.2.2) : AJob)) := by
  simp only [absSlot, if_neg h]
  rfl

theorem absSlot_nil (rs : List (ℕ × ℕ)) :
    absSlot rs [] = rs.map fun r => ((.prefill, r.1, r.2) : AJob) := by
  simp [absSlot]

/-- A slot: the batch leaves, the arrivals come. -/
theorem absSlot_backlog (rs : List (ℕ × ℕ)) (js : List AJob) :
    backlog (absSlot rs js) + sS js = backlog js + (rs.map fun r => r.1 + r.2).sum := by
  by_cases h : js = []
  · subst h
    rw [absSlot_nil, new_backlog]
    simp [sS, shares, backlog_nil]
  · rw [absSlot_eq rs js h, backlog_append, backlog_append, new_backlog]
    have h1 := split_backlog (aft (decodeMode js) js 128)
    have h2 := aft_backlog (decodeMode js) js 128
    simp only [sS]
    omega

/-! ### What the chain can hold -/

theorem aft_weak (dm : Bool) : ∀ (js : List AJob) (b : ℕ), Good js → (dm = false → 128 ∣ b) →
    ∀ x ∈ aft dm js b, WeakJob x
  | [], _, _, _ => by simp [aft]
  | j :: js, b, hg, hb => by
    rw [aft_cons]
    obtain ⟨m, l, o⟩ := j
    have hj : GoodJob (m, l, o) := hg _ (List.mem_cons_self ..)
    have hg' : Good js := fun x hx => hg x (List.mem_cons_of_mem _ hx)
    rcases hj with ⟨hm, h1, hd, h2, h3, h4⟩ | ⟨hm, h1, h2⟩
    · simp only at hm h1 hd h2 h3 h4
      subst hm
      cases dm
      · have hw : want false (.prefill, l, o) = 128 := by
          simp only [want, Bool.false_eq_true, ↓reduceIte]; omega
        have hb' := hb rfl
        have hs : min (want false (.prefill, l, o)) b = 0 ∨ min (want false (.prefill, l, o)) b = 128 := by
          rw [hw]; omega
        intro x hx
        rcases List.mem_cons.mp hx with rfl | hx
        · left; refine ⟨rfl, ?_, ?_, h3, h4⟩ <;> (simp only; omega)
        · exact aft_weak false js _ hg' (fun _ => by omega) x hx
      · have hw : want true (.prefill, l, o) = 0 := by simp [want]
        intro x hx
        rcases List.mem_cons.mp hx with rfl | hx
        · left; refine ⟨rfl, ?_, ?_, h3, h4⟩ <;> (simp only [hw]; omega)
        · exact aft_weak true js _ hg' (fun h => by cases h) x hx
    · simp only at hm h1 h2
      subst hm
      intro x hx
      rcases List.mem_cons.mp hx with rfl | hx
      · right; exact ⟨rfl, by simp only; omega⟩
      · refine aft_weak dm js _ hg' (fun h => ?_) x hx
        subst h
        have hw : want false (.decode, l, o) = 0 := by simp [want]
        rw [hw]; simpa using hb rfl

theorem good_absSlot (rs : List (ℕ × ℕ)) (js : List AJob) (hg : Good js)
    (hf : ∀ r ∈ rs, BariStable.Fits r) : Good (absSlot rs js) := by
  have hnew : ∀ x ∈ rs.map (fun r => ((.prefill, r.1, r.2) : AJob)), GoodJob x := by
    intro x hx
    obtain ⟨r, hr, rfl⟩ := List.mem_map.mp hx
    obtain ⟨hd, h1, h2, h3, h4⟩ := hf r hr
    left; exact ⟨rfl, by simp only; omega, hd, h2, h3, h4⟩
  by_cases h : js = []
  · subst h; rw [absSlot_nil]; exact hnew
  · rw [absSlot_eq rs js h]
    have hw := aft_weak (decodeMode js) js 128 hg (fun _ => dvd_refl _)
    intro x hx
    simp only [List.mem_append] at hx
    rcases hx with (hx | hx) | hx
    · obtain ⟨hx, hx0⟩ := List.mem_filter.mp hx
      simp only [ne_eq, decide_eq_true_eq] at hx0
      rcases hw x hx with ⟨a, b, c, d, e⟩ | ⟨a, b⟩
      · left; exact ⟨a, by omega, b, c, d, e⟩
      · right; exact ⟨a, by omega, b⟩
    · exact hnew x hx
    · obtain ⟨y, hy, rfl⟩ := List.mem_map.mp hx
      obtain ⟨hy, hy0⟩ := List.mem_filter.mp hy
      simp only [decide_eq_true_eq] at hy0
      rcases hw y hy with ⟨_, _, _, d, e⟩ | ⟨a, _⟩
      · right; exact ⟨rfl, d, e⟩
      · rw [hy0.2] at a; cases a

theorem good_of_reach {N : ℕ} {A : BariStable.Arrivals N} {js : List AJob} (h : AReach A js) :
    Good js := by
  induction h with
  | nil => intro j hj; cases hj
  | step o _ _ _ ih => exact good_absSlot _ _ ih (A.fits o)

/-! ### The batch -/

theorem want_true_sum : ∀ js : List AJob, Good js →
    (js.map (want true)).sum = (js.filter (·.1 = .decode)).length
  | [], _ => rfl
  | j :: js, hg => by
    have ih := want_true_sum js fun x hx => hg x (List.mem_cons_of_mem _ hx)
    obtain ⟨m, l, o⟩ := j
    rcases hg _ (List.mem_cons_self ..) with ⟨hm, h1, -⟩ | ⟨hm, h1, -⟩
    · simp only at hm h1; subst hm
      simp [want, ih]
    · simp only at hm h1; subst hm
      simp only [List.map_cons, List.sum_cons, List.filter_cons, decide_true, ↓reduceIte,
        List.length_cons, ih]
      simp only [want, ↓reduceIte]
      omega

theorem want_false_ge : ∀ js : List AJob, Good js → (∃ j ∈ js, j.1 = .prefill) →
    128 ≤ (js.map (want false)).sum
  | [], _, ⟨_, h, _⟩ => by cases h
  | j :: js, hg, ⟨x, hx, hxp⟩ => by
    simp only [List.map_cons, List.sum_cons]
    obtain ⟨m, l, o⟩ := j
    by_cases hp : m = .prefill
    · subst hp
      rcases hg _ (List.mem_cons_self ..) with ⟨-, h1, hd, -⟩ | ⟨hm, -⟩
      · simp only at h1 hd
        have : want false (.prefill, l, o) = 128 := by
          simp only [want, Bool.false_eq_true, ↓reduceIte]; omega
        omega
      · cases hm
    · rcases List.mem_cons.mp hx with rfl | hx
      · exact absurd hxp hp
      · have := want_false_ge js (fun y hy => hg y (List.mem_cons_of_mem _ hy)) ⟨x, hx, hxp⟩
        omega

theorem decode_backlog : ∀ js : List AJob, (∀ j ∈ js, j.1 = .decode ∧ j.2.1 ≤ 512) →
    backlog js ≤ 512 * js.length
  | [], _ => by simp [backlog_nil]
  | j :: js, h => by
    rw [backlog_cons, List.length_cons]
    have ih := decode_backlog js fun x hx => h x (List.mem_cons_of_mem _ hx)
    obtain ⟨h1, h2⟩ := h j (List.mem_cons_self ..)
    have : bk j = j.2.1 := by simp [bk, h1]
    omega

theorem sS_le (js : List AJob) : sS js ≤ 128 := by
  rw [sS, shares_sum]; exact min_le_left _ _

/-- A non-empty list schedules a token, and a batch that is not full holds
fewer than 128 decodes and nothing else. -/
theorem sS_cases (js : List AJob) (hg : Good js) (hne : js ≠ []) :
    1 ≤ sS js ∧ (sS js < 128 → backlog js ≤ 512 * 127) := by
  rw [sS, shares_sum]
  set d := (js.filter (·.1 = .decode)).length with hd
  by_cases hm : 128 ≤ d ∨ d = js.length
  · have hdm : decodeMode js = true := by simp only [decodeMode, decide_eq_true_eq]; exact hm
    rw [hdm, want_true_sum js hg, ← hd]
    have hlen : 1 ≤ js.length := List.length_pos_iff.mpr hne
    refine ⟨by rcases hm with hm | hm <;> simp only [Nat.min_def] <;> split_ifs <;> omega, fun hlt => ?_⟩
    have hd128 : d < 128 := by simp only [Nat.min_def] at hlt; split_ifs at hlt <;> omega
    have hdl : d = js.length := by omega
    have hall : ∀ j ∈ js, j.1 = .decode := by
      have := List.length_filter_eq_length_iff.mp (hd ▸ hdl)
      intro j hj; simpa using this j hj
    have hb := decode_backlog js fun j hj => by
      refine ⟨hall j hj, ?_⟩
      rcases hg j hj with ⟨hp, -⟩ | ⟨-, -, h2⟩
      · rw [hall j hj] at hp; cases hp
      · exact h2
    have : js.length ≤ 127 := by omega
    calc backlog js ≤ 512 * js.length := hb
      _ ≤ 512 * 127 := Nat.mul_le_mul_left _ this
  · have hdm : decodeMode js = false := by simp only [decodeMode, decide_eq_false_iff_not]; exact hm
    rw [hdm]
    have hex : ∃ j ∈ js, j.1 = .prefill := by
      by_contra hc
      push Not at hc
      apply hm; right
      rw [hd, List.length_filter_eq_length_iff]
      intro j hj
      rcases hg j hj with ⟨hp, -⟩ | ⟨hdc, -⟩
      · exact absurd hp (hc j hj)
      · simpa using hdc
    have := want_false_ge js hg hex
    refine ⟨by simp only [Nat.min_def]; split_ifs; omega, fun hlt => ?_⟩
    simp only [Nat.min_def] at hlt; split_ifs at hlt; omega

theorem backlog_pos (js : List AJob) (hg : Good js) (hne : js ≠ []) : 1 ≤ backlog js := by
  obtain ⟨j, js', rfl⟩ := List.exists_cons_of_ne_nil hne
  rw [backlog_cons]
  rcases hg j (List.mem_cons_self ..) with ⟨-, h1, -⟩ | ⟨-, h1, -⟩ <;> simp only [bk] <;> omega

theorem work_le : ∀ rs : List (ℕ × ℕ), (∀ r ∈ rs, BariStable.Fits r) →
    (rs.map fun r => r.1 + r.2).sum ≤ 1536 * rs.length
  | [], _ => by simp
  | r :: rs, h => by
    simp only [List.map_cons, List.sum_cons, List.length_cons]
    have ih := work_le rs fun x hx => h x (List.mem_cons_of_mem _ hx)
    obtain ⟨-, -, h2, -, h4⟩ := h r (List.mem_cons_self ..)
    omega


/-! ### The chain -/

section Chain

variable {N : ℕ} (A : BariStable.Arrivals N)

/-- The state after a slot with the arrivals of outcome `o`. -/
noncomputable def nxt (x : AState A) (o : ℕ) : AState A :=
  if h : o ≤ N ∧ 0 < A.p o then ⟨absSlot (A.arr o) x.1, .step o h.1 h.2 x.2⟩ else x

theorem nxt_val {x : AState A} {o : ℕ} (h : o ≤ N ∧ 0 < A.p o) :
    (nxt A x o).1 = absSlot (A.arr o) x.1 := by
  have e : nxt A x o = (⟨absSlot (A.arr o) x.1, .step o h.1 h.2 x.2⟩ : AState A) := dif_pos h
  exact congrArg Subtype.val e

theorem nxt_of_not {x : AState A} {o : ℕ} (h : ¬ (o ≤ N ∧ 0 < A.p o)) : nxt A x o = x :=
  dif_neg h

theorem apply_ak (V : AState A → ℝ) (x : AState A) :
    (akernel A).apply V x = ∑ o ∈ Finset.range (N + 1), A.p o * V (nxt A x o) :=
  Kernel.apply_ofOutcomes N A.p A.nonneg A.sum_one (nxt A) V x

/-- Outcomes of probability 0 do not count. -/
theorem sum_congr_pos {f : AState A → ℝ} {g : ℕ → ℝ} (x : AState A)
    (h : ∀ o ≤ N, 0 < A.p o → f (nxt A x o) = g o) :
    ∑ o ∈ Finset.range (N + 1), A.p o * f (nxt A x o) = ∑ o ∈ Finset.range (N + 1), A.p o * g o := by
  refine Finset.sum_congr rfl fun o ho => ?_
  have hoN : o ≤ N := Nat.lt_succ_iff.mp (Finset.mem_range.mp ho)
  rcases (A.nonneg o).lt_or_eq with hp | hp
  · rw [h o hoN hp]
  · rw [← hp]; simp

theorem eq_nil_of {x : AState A} (h : x.1 = []) : x = nil A := Subtype.ext h

theorem ne_nil_of {x : AState A} (h : x ≠ nil A) : x.1 ≠ [] := fun h' => h (eq_nil_of A h')

/-- The work at the engine. -/
def Vb (x : AState A) : ℝ := (backlog x.1 : ℝ)

/-- The drift below capacity. -/
def εb : ℝ := 128 - A.load

/-- The engine is idle, or its batch is not full. -/
def FS (x : AState A) : Prop := x.1 = [] ∨ sS x.1 < 128

instance : DecidablePred (FS A) := fun x => by unfold FS; infer_instance

theorem FS_backlog (x : AState A) (hx : FS A x) : backlog x.1 ≤ 512 * 127 := by
  by_cases hne : x.1 = []
  · rw [hne]; simp [backlog_nil]
  · rcases hx with h | h
    · exact absurd h hne
    · exact (sS_cases x.1 (good_of_reach x.2) hne).2 h

/-- A slot with the arrivals of outcome `o`, as reals. -/
theorem backlog_nxt (x : AState A) (o : ℕ) (h : o ≤ N ∧ 0 < A.p o) :
    (backlog (nxt A x o).1 : ℝ) + sS x.1 = backlog x.1 + A.work o := by
  rw [nxt_val A h]
  have := absSlot_backlog (A.arr o) x.1
  unfold BariStable.Arrivals.work
  exact_mod_cast this

theorem drift_b (hA : A.load < 128) : Drift (akernel A) (FS A) (Vb A) (εb A) where
  nonneg x := Nat.cast_nonneg _
  integrable := Kernel.integrable_ofOutcomes _ _ _ _ _ _
  pos := by unfold εb; linarith
  drift x hx := by
    have hS : sS x.1 = 128 :=
      le_antisymm (sS_le _) (not_lt.mp fun h => hx (Or.inr h))
    rw [apply_ak, sum_congr_pos A (g := fun o => Vb A x - 128 + A.work o) x ?_]
    · have : ∑ o ∈ Finset.range (N + 1), A.p o * (Vb A x - 128 + A.work o) =
          (Vb A x - 128) * ∑ o ∈ Finset.range (N + 1), A.p o + A.load := by
        unfold BariStable.Arrivals.load
        rw [Finset.mul_sum, ← Finset.sum_add_distrib]
        exact Finset.sum_congr rfl fun o _ => by ring
      rw [this, A.sum_one]; unfold εb; linarith
    · intro o hoN hp
      have := backlog_nxt A x o ⟨hoN, hp⟩
      rw [hS] at this
      unfold Vb; push_cast at this ⊢; linarith

/-- A slot changes the work by at most 10 000 requests' worth. -/
theorem Vb_nxt_le (x : AState A) (o : ℕ) : Vb A (nxt A x o) ≤ Vb A x + 15360000 := by
  by_cases h : o ≤ N ∧ 0 < A.p o
  · have h1 := backlog_nxt A x o h
    have h2 := work_le (A.arr o) (A.fits o)
    have h3 := A.small o
    have h4 : (A.work o : ℝ) ≤ 15360000 := by
      unfold BariStable.Arrivals.work
      have : ((A.arr o).map fun r => r.1 + r.2).sum ≤ 15360000 := by omega
      exact_mod_cast this
    have h5 : (0 : ℝ) ≤ sS x.1 := Nat.cast_nonneg _
    unfold Vb; linarith
  · rw [nxt_of_not A h]; linarith

theorem applyN_le : ∀ n (x : AState A),
    (akernel A).applyN n (Vb A) x ≤ Vb A x + 15360000 * n
  | 0, x => by simp [Kernel.applyN]
  | n + 1, x => by
    show (akernel A).apply ((akernel A).applyN n (Vb A)) x ≤ _
    rw [apply_ak]
    calc ∑ o ∈ Finset.range (N + 1), A.p o * (akernel A).applyN n (Vb A) (nxt A x o)
        ≤ ∑ o ∈ Finset.range (N + 1), A.p o * (Vb A x + 15360000 * ((n : ℝ) + 1)) := by
          refine Finset.sum_le_sum fun o _ => mul_le_mul_of_nonneg_left ?_ (A.nonneg o)
          have := applyN_le n (nxt A x o)
          have := Vb_nxt_le A x o
          linarith
      _ = Vb A x + 15360000 * ((n + 1 : ℕ) : ℝ) := by
          rw [← Finset.sum_mul, A.sum_one]; push_cast; ring

/-- Every request brings at least 129 tokens, so below capacity some outcome
of positive probability brings none. -/
theorem exists_empty (hA : A.load < 128) : ∃ o0, (o0 ≤ N ∧ 0 < A.p o0) ∧ A.arr o0 = [] := by
  by_contra hc
  have hw : ∀ o ∈ Finset.range (N + 1), A.p o * 129 ≤ A.p o * A.work o := by
    intro o ho
    have hoN : o ≤ N := Nat.lt_succ_iff.mp (Finset.mem_range.mp ho)
    rcases (A.nonneg o).lt_or_eq with hp | hp
    · refine mul_le_mul_of_nonneg_left ?_ (A.nonneg o)
      have hne : A.arr o ≠ [] := fun h => hc ⟨o, ⟨hoN, hp⟩, h⟩
      obtain ⟨r, rs, hr⟩ := List.exists_cons_of_ne_nil hne
      obtain ⟨-, h1, -, h3, -⟩ := A.fits o r (by rw [hr]; exact List.mem_cons_self ..)
      have : 129 ≤ A.work o := by
        unfold BariStable.Arrivals.work; rw [hr]; simp only [List.map_cons, List.sum_cons]; omega
      exact_mod_cast this
    · rw [← hp]; simp
  have := Finset.sum_le_sum hw
  rw [← Finset.sum_mul, A.sum_one] at this
  unfold BariStable.Arrivals.load at hA
  linarith

end Chain

section Walks

variable {N : ℕ} (A : BariStable.Arrivals N)

/-- The term of one outcome is at most the expectation. -/
theorem term_le_apply (f : AState A → ℝ) (hf : ∀ y, 0 ≤ f y) (x : AState A) {o : ℕ} (hoN : o ≤ N) :
    A.p o * f (nxt A x o) ≤ (akernel A).apply f x :=
  Kernel.le_apply_ofOutcomes _ _ _ _ _ f hf x hoN

/-- From a state of backlog at most `n`, `n` slots without arrivals empty the
engine. -/
theorem reach_drain {o0 : ℕ} (ho0 : o0 ≤ N ∧ 0 < A.p o0) (harr : A.arr o0 = []) :
    ∀ n (x : AState A), backlog x.1 ≤ n → A.p o0 ^ n ≤ reach (akernel A) (· = nil A) n x
  | 0, x, hx => by
    have hx0 : x = nil A := by
      by_contra hne
      have := backlog_pos x.1 (good_of_reach x.2) (ne_nil_of A hne)
      omega
    simp [reach, hx0]
  | n + 1, x, hx => by
    have hp1 : A.p o0 ≤ 1 := by
      rw [← A.sum_one]
      exact Finset.single_le_sum (f := A.p) (fun o _ => A.nonneg o)
        (Finset.mem_range.mpr (Nat.lt_succ_of_le ho0.1))
    by_cases hxn : x = nil A
    · simp only [reach, hxn, ↓reduceIte]
      exact pow_le_one₀ (A.nonneg o0) hp1
    · simp only [reach, hxn, ↓reduceIte]
      have hne := ne_nil_of A hxn
      have hb : backlog (nxt A x o0).1 ≤ n := by
        have h1 := absSlot_backlog (A.arr o0) x.1
        rw [harr] at h1
        have h2 := (sS_cases x.1 (good_of_reach x.2) hne).1
        rw [nxt_val A ho0, harr]
        simp only [List.map_nil, List.sum_nil] at h1
        omega
      have ih := reach_drain ho0 harr n (nxt A x o0) hb
      calc A.p o0 ^ (n + 1) = A.p o0 * A.p o0 ^ n := by ring
        _ ≤ A.p o0 * reach (akernel A) (· = nil A) n (nxt A x o0) :=
          mul_le_mul_of_nonneg_left ih (A.nonneg o0)
        _ ≤ _ := term_le_apply A _ (reach_nonneg _ _ n) x ho0.1

/-- A slot of positive probability, then on. -/
theorem reaches_step {x y : AState A} {o : ℕ} (hoN : o ≤ N) (hp : 0 < A.p o)
    (h : Reaches (akernel A) (nxt A x o) y) : Reaches (akernel A) x y :=
  Reaches.step _ hp (fun f hf _ => term_le_apply A f hf x hoN) h

/-- The empty engine reaches every state of the chain. -/
theorem walk_from_nil : ∀ (js : List AJob) (h : AReach A js), Reaches (akernel A) (nil A) ⟨js, h⟩ := by
  intro js h
  induction h with
  | nil => exact Reaches.refl _ _
  | @step js o hoN hp hjs ih =>
    refine ih.trans _ (reaches_step A hoN hp ?_)
    have : nxt A ⟨js, hjs⟩ o = ⟨absSlot (A.arr o) js, .step o hoN hp hjs⟩ :=
      Subtype.ext (nxt_val A ⟨hoN, hp⟩)
    rw [this]; exact Reaches.refl _ _

/-- Every state reaches the empty engine. -/
theorem walk_to_nil {o0 : ℕ} (ho0 : o0 ≤ N ∧ 0 < A.p o0) (harr : A.arr o0 = []) :
    ∀ n (x : AState A), backlog x.1 ≤ n → Reaches (akernel A) x (nil A)
  | 0, x, hx => by
    have hx0 : x = nil A := by
      by_contra hne
      have := backlog_pos x.1 (good_of_reach x.2) (ne_nil_of A hne)
      omega
    rw [hx0]; exact Reaches.refl _ _
  | n + 1, x, hx => by
    by_cases hxn : x = nil A
    · rw [hxn]; exact Reaches.refl _ _
    · have hne := ne_nil_of A hxn
      have hb : backlog (nxt A x o0).1 ≤ n := by
        have h1 := absSlot_backlog (A.arr o0) x.1
        rw [harr] at h1
        have h2 := (sS_cases x.1 (good_of_reach x.2) hne).1
        rw [nxt_val A ho0, harr]
        simp only [List.map_nil, List.sum_nil] at h1
        omega
      exact reaches_step A ho0.1 ho0.2 (walk_to_nil ho0 harr n _ hb)

end Walks


/-! ### The theorems -/

/-- From every state the expected number of slots until the engine is
empty is bounded (uniformly in the truncation). -/
theorem hit_nil_le {N : ℕ} (A : BariStable.Arrivals N) (hA : A.load < 128) :
    ∃ W : AState A → ℝ, ∀ n x, hit (akernel A) (· = nil A) n x ≤ W x := by
  obtain ⟨o0, ho0, harr⟩ := exists_empty A hA
  have hδ : 0 < A.p o0 ^ 65536 := pow_pos ho0.2 _
  refine ⟨_, hit_le_of_reach (akernel A) (· = nil A) (fun _ => Kernel.integrable_ofOutcomes _ _ _ _ _ _) (drift_b A hA)
    (fun x hx => Or.inl (by subst hx; rfl)) (L := 65536) (B := 512 * 127 + 15360000 * 65536) hδ
    (fun x hx => reach_drain A ho0 harr 65536 x (le_trans (FS_backlog A x hx) (by norm_num)))
    (fun x hx => ?_)⟩
  have h1 := applyN_le A 65536 x
  have h2 : Vb A x ≤ 512 * 127 := by unfold Vb; exact_mod_cast FS_backlog A x hx
  push_cast at h1
  linarith

/-- Every state reaches every state with positive probability. -/
theorem irreducible {N : ℕ} (A : BariStable.Arrivals N) (hA : A.load < 128) :
    Irreducible (akernel A) := by
  obtain ⟨o0, ho0, harr⟩ := exists_empty A hA
  intro x y
  exact (walk_to_nil A ho0 harr _ x le_rfl).trans _ (walk_from_nil A y.1 y.2)

/-- Theorem 2: below capacity, every state of RAD's chain is positive
recurrent. -/
theorem positive_recurrent {N : ℕ} (A : BariStable.Arrivals N) (hA : A.load < 128)
    (y : AState A) : PositiveRecurrent (akernel A) y := by
  obtain ⟨W, hW⟩ := hit_nil_le A hA
  exact positiveRecurrent_of_hit (akernel A) (fun _ => Kernel.integrable_ofOutcomes _ _ _ _ _ _) (nil A) W hW y (walk_from_nil A y.1 y.2)

end BariRecurrent
end Papers
end SerqLang
