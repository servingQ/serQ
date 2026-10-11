/-
# Dai et al., Theorem 2(b): the chain is positive recurrent

On Sarathi's chain on job lists (`DaiChain`), below capacity: the empty
list is reached in bounded expected time from every state (`hit_nil_le`),
every state reaches every state (`irreducible`), and every state is
positive recurrent (`positive_recurrent`). The machine chain of the program
projects onto this one (`DaiSim.simulation`).

Below capacity, `1280 · E[k] < 128`, no arrival has probability at least
`1 − E[k] > 0.9`, and a slot with no arrival serves at least one token while
there is a job, so from a state of bounded backlog `b` the empty list is
`b` arrival-free slots away (`Recurrence.hit_le_of_reach`).
-/
import Serq.Recurrence
import papers.DaiChain

namespace SerqLang
namespace Papers
namespace DaiRecurrent

open Foster DaiChain

/-! ### The job lists: what every reached list satisfies -/

/-- One request's tokens counted by `backlog`. -/
def bk (j : AJob) : ℕ := j.2 + if j.1 = .prefill then 990 else 0

theorem backlog_cons (j : AJob) (js : List AJob) : backlog (j :: js) = bk j + backlog js := by
  simp [backlog, bk]

theorem backlog_append (l l' : List AJob) : backlog (l ++ l') = backlog l + backlog l' := by
  simp [backlog]

theorem backlog_nil : backlog [] = 0 := rfl

theorem backlog_replicate (k : ℕ) : backlog (List.replicate k (.prefill, 290)) = 1280 * k := by
  induction k with
  | zero => rfl
  | succ k ih => rw [List.replicate_succ, backlog_cons, ih]; simp [bk]; ring

/-- A job of a reached list: some work left, at most a request's. -/
def GoodJob (j : AJob) : Prop :=
  1 ≤ j.2 ∧ ((j.1 = .prefill ∧ j.2 ≤ 290) ∨ (j.1 = .decode ∧ j.2 ≤ 990))

def Good (js : List AJob) : Prop := ∀ j ∈ js, GoodJob j

theorem want_le (j : AJob) : want j ≤ j.2 := by
  obtain ⟨md, l⟩ := j
  cases md <;> simp [want]

theorem one_le_want {j : AJob} (h : GoodJob j) : 1 ≤ want j := by
  obtain ⟨md, l⟩ := j
  obtain ⟨h1, h2⟩ := h
  rcases h2 with ⟨hm, _⟩ | ⟨hm, _⟩ <;> simp only at hm <;> subst hm <;> simp [want] <;> omega

theorem bk_le {j : AJob} (h : GoodJob j) : bk j ≤ 1280 := by
  obtain ⟨md, l⟩ := j
  obtain ⟨h1, h2⟩ := h
  rcases h2 with ⟨hm, hl⟩ | ⟨hm, hl⟩ <;> simp only at hm hl <;> subst hm <;> simp [bk] <;> omega

theorem one_le_bk {j : AJob} (h : GoodJob j) : 1 ≤ bk j := by
  unfold bk; have := h.1; omega

/-- The jobs after the batch: each loses its share. -/
def after (js : List AJob) (b : ℕ) : List AJob :=
  List.zipWith (fun j s => (j.1, j.2 - s)) js (shares js b)

theorem after_cons (j : AJob) (js : List AJob) (b : ℕ) :
    after (j :: js) b = (j.1, j.2 - min (want j) b) :: after js (b - min (want j) b) := rfl

theorem shares_sum_le : ∀ (js : List AJob) (b : ℕ), (shares js b).sum ≤ b
  | [], _ => by simp [shares]
  | j :: js, b => by
    simp only [shares, List.sum_cons]
    have := shares_sum_le js (b - min (want j) b)
    omega

theorem backlog_after : ∀ (js : List AJob) (b : ℕ),
    backlog (after js b) + (shares js b).sum = backlog js
  | [], _ => rfl
  | j :: js, b => by
    rw [after_cons, backlog_cons, backlog_cons]
    simp only [shares, List.sum_cons]
    have ih := backlog_after js (b - min (want j) b)
    have hw := want_le j
    have : min (want j) b ≤ j.2 := le_trans (min_le_left _ _) hw
    unfold bk
    simp only
    split <;> omega

theorem mem_after : ∀ (js : List AJob) (b : ℕ), ∀ x ∈ after js b,
    ∃ j ∈ js, x.1 = j.1 ∧ x.2 ≤ j.2
  | [], _, x, hx => by simp [after] at hx
  | j :: js, b, x, hx => by
    rw [after_cons] at hx
    rcases List.mem_cons.1 hx with rfl | hx
    · exact ⟨j, List.mem_cons_self .., rfl, Nat.sub_le _ _⟩
    · obtain ⟨j', hj', h1, h2⟩ := mem_after js _ x hx
      exact ⟨j', List.mem_cons_of_mem _ hj', h1, h2⟩

/-- A finished request: a done prefill comes back as a decode. -/
theorem backlog_split : ∀ l : List AJob,
    backlog (l.filter (·.2 ≠ 0)) +
      backlog ((l.filter fun j => j.2 = 0 ∧ j.1 = .prefill).map fun _ => ((.decode, 990) : AJob)) =
    backlog l
  | [] => rfl
  | j :: l => by
    have ih := backlog_split l
    simp only [ne_eq] at ih ⊢
    by_cases h0 : j.2 = 0 <;> by_cases hp : j.1 = .prefill <;>
      simp [h0, hp, backlog_cons, bk] at ih ⊢ <;> omega

theorem absSlot_ne {k : ℕ} {js : List AJob} (h : js ≠ []) :
    absSlot k js = (after js 128).filter (·.2 ≠ 0) ++ List.replicate k (.prefill, 290) ++
      ((after js 128).filter fun j => j.2 = 0 ∧ j.1 = .prefill).map fun _ => ((.decode, 990) : AJob) := by
  unfold absSlot; rw [if_neg h]; rfl

/-- The backlog after a slot: the batch's tokens leave, the arrivals' come. -/
theorem backlog_absSlot (k : ℕ) {js : List AJob} (h : js ≠ []) :
    backlog (absSlot k js) + (shares js 128).sum = backlog js + 1280 * k := by
  rw [absSlot_ne h, backlog_append, backlog_append, backlog_replicate]
  have h1 := backlog_split (after js 128)
  have h2 := backlog_after js 128
  omega

theorem backlog_absSlot_nil (k : ℕ) : backlog (absSlot k []) = 1280 * k := by
  simp [absSlot, backlog_replicate]

theorem good_absSlot (k : ℕ) {js : List AJob} (hg : Good js) : Good (absSlot k js) := by
  by_cases h : js = []
  · subst h
    intro j hj
    simp only [absSlot, if_true, List.mem_replicate] at hj
    rw [hj.2]; exact ⟨by norm_num, Or.inl ⟨rfl, le_rfl⟩⟩
  · rw [absSlot_ne h]
    intro j hj
    simp only [List.mem_append, List.mem_filter, List.mem_replicate, List.mem_map] at hj
    rcases hj with (⟨hj, hne⟩ | ⟨_, rfl⟩) | ⟨_, _, rfl⟩
    · obtain ⟨j', hj', h1, h2⟩ := mem_after js 128 j hj
      have hg' := hg j' hj'
      simp only [ne_eq, decide_eq_true_eq] at hne
      refine ⟨by omega, ?_⟩
      rcases hg'.2 with ⟨hm, hl⟩ | ⟨hm, hl⟩
      · exact Or.inl ⟨h1.trans hm, by omega⟩
      · exact Or.inr ⟨h1.trans hm, by omega⟩
    · exact ⟨by norm_num, Or.inl ⟨rfl, le_rfl⟩⟩
    · exact ⟨by norm_num, Or.inr ⟨rfl, le_rfl⟩⟩

theorem good_of_areach {K : ℕ} {A : DaiStable.Arrivals K} {js : List AJob} (h : AReach A js) :
    Good js := by
  induction h with
  | nil => intro j hj; simp at hj
  | step k _ _ _ ih => exact good_absSlot k ih

/-- A batch that is not full served every job its want. -/
theorem shares_sum_eq_of_lt : ∀ (js : List AJob) (b : ℕ), (shares js b).sum < b →
    (shares js b).sum = (js.map want).sum
  | [], _, _ => rfl
  | j :: js, b, h => by
    simp only [shares, List.sum_cons, List.map_cons] at h ⊢
    have hw : want j < b := by
      by_contra hc
      push Not at hc
      rw [min_eq_right hc] at h
      omega
    rw [min_eq_left hw.le] at h ⊢
    rw [shares_sum_eq_of_lt js (b - want j) (by omega)]

theorem length_le_want {js : List AJob} (hg : Good js) : js.length ≤ (js.map want).sum := by
  induction js with
  | nil => simp
  | cons j js ih =>
    simp only [List.length_cons, List.map_cons, List.sum_cons]
    have := one_le_want (hg j (List.mem_cons_self ..))
    have := ih fun x hx => hg x (List.mem_cons_of_mem _ hx)
    omega

theorem backlog_le_length {js : List AJob} (hg : Good js) : backlog js ≤ 1280 * js.length := by
  induction js with
  | nil => simp [backlog]
  | cons j js ih =>
    rw [backlog_cons]
    have := bk_le (hg j (List.mem_cons_self ..))
    have := ih fun x hx => hg x (List.mem_cons_of_mem _ hx)
    simp only [List.length_cons]; omega

theorem backlog_pos {js : List AJob} (hg : Good js) (h : js ≠ []) : 1 ≤ backlog js := by
  obtain ⟨j, js', rfl⟩ := List.exists_cons_of_ne_nil h
  rw [backlog_cons]
  have := one_le_bk (hg j (List.mem_cons_self ..))
  omega

theorem one_le_shares {js : List AJob} (hg : Good js) (h : js ≠ []) : 1 ≤ (shares js 128).sum := by
  obtain ⟨j, js', rfl⟩ := List.exists_cons_of_ne_nil h
  simp only [shares, List.sum_cons]
  have := one_le_want (hg j (List.mem_cons_self ..))
  have : 1 ≤ min (want j) 128 := le_min this (by norm_num)
  omega


/-! ### The chain -/

section Chain

variable {K : ℕ} (A : DaiStable.Arrivals K)

/-- The state after a slot with `k` arrivals, as `akernel` draws it. -/
noncomputable def next (x : AState A) (k : ℕ) : AState A :=
  if h : k ≤ K ∧ 0 < A.p k then ⟨absSlot k x.1, .step k h.1 h.2 x.2⟩ else x

theorem apply_akernel (V : AState A → ℝ) (x : AState A) :
    (akernel A).apply V x = ∑ k ∈ Finset.range (K + 1), A.p k * V (next A x k) :=
  Kernel.apply_ofOutcomes _ _ _ _ _ _ _

theorem next_val {x : AState A} {k : ℕ} (h : k ≤ K ∧ 0 < A.p k) :
    (next A x k).1 = absSlot k x.1 := by
  have : next A x k = ⟨absSlot k x.1, .step k h.1 h.2 x.2⟩ := dif_pos h
  rw [this]

theorem next_of_not {x : AState A} {k : ℕ} (h : ¬ (k ≤ K ∧ 0 < A.p k)) : next A x k = x :=
  dif_neg h

theorem sum_p_mul (c : ℝ) : ∑ k ∈ Finset.range (K + 1), A.p k * c = c := by
  rw [← Finset.sum_mul, A.sum_one, one_mul]

/-- The engine is idle, or its batch is not full. -/
def F (x : AState A) : Prop := x.1 = [] ∨ (shares x.1 128).sum < 128

instance : DecidablePred (F A) := fun x => by unfold F; infer_instance

/-- The Lyapunov function: the backlog. -/
def V (x : AState A) : ℝ := backlog x.1

/-- The drift: `ε = 128 − 1280 · E[k]`. -/
def ε : ℝ := 128 - 1280 * A.mean

theorem good (x : AState A) : Good x.1 := good_of_areach x.2

/-- Off `F` a slot with `k` arrivals moves the backlog by `1280 k − 128`. -/
theorem term_off_F {x : AState A} (hx : ¬ F A x) {k : ℕ} (hk : k ∈ Finset.range (K + 1)) :
    A.p k * V A (next A x k) = A.p k * (V A x - 128 + 1280 * k) := by
  by_cases hp : 0 < A.p k
  · have hkK : k ≤ K := Nat.lt_succ_iff.mp (Finset.mem_range.mp hk)
    unfold V
    rw [next_val A ⟨hkK, hp⟩]
    simp only [F, not_or] at hx
    have hs : (shares x.1 128).sum = 128 := le_antisymm (shares_sum_le _ _) (by omega)
    have := backlog_absSlot k hx.1
    rw [hs] at this
    have : (backlog (absSlot k x.1) : ℝ) = backlog x.1 - 128 + 1280 * k := by
      have h' : ((backlog (absSlot k x.1) + 128 : ℕ) : ℝ) = ((backlog x.1 + 1280 * k : ℕ) : ℝ) := by
        rw [this]
      push_cast at h'
      linarith
    rw [this]
  · have : A.p k = 0 := le_antisymm (not_lt.mp hp) (A.nonneg k)
    simp [this]

/-- `E[k] ≥ 1 − p₀`: so below capacity, a slot without arrival is likely. -/
theorem one_sub_le_p0 : 1 - A.p 0 ≤ A.mean := by
  unfold DaiStable.Arrivals.mean
  have h1 : ∑ k ∈ Finset.range (K + 1), (if k = 0 then 0 else A.p k) ≤
      ∑ k ∈ Finset.range (K + 1), A.p k * k := by
    refine Finset.sum_le_sum fun k _ => ?_
    split
    · subst_vars; simp
    · have : (1 : ℝ) ≤ k := by exact_mod_cast Nat.one_le_iff_ne_zero.mpr ‹_›
      nlinarith [A.nonneg k]
  have h2 : ∑ k ∈ Finset.range (K + 1), (if k = 0 then 0 else A.p k) = 1 - A.p 0 := by
    have hs := A.sum_one
    rw [Finset.sum_range_succ'] at hs ⊢
    simp only [Nat.add_one_ne_zero, ↓reduceIte, add_zero]
    linarith
  linarith

theorem p0_pos (hA : 1280 * A.mean < 128) : 0 < A.p 0 := by
  have := one_sub_le_p0 A
  linarith

theorem p0_le_one : A.p 0 ≤ 1 := by
  rw [← A.sum_one]
  exact Finset.single_le_sum (fun k _ => A.nonneg k) (Finset.mem_range.mpr (Nat.succ_pos K))

theorem drift (hA : 1280 * A.mean < 128) : Drift (akernel A) (F A) (V A) (ε A) where
  nonneg x := Nat.cast_nonneg _
  integrable := Kernel.integrable_ofOutcomes _ _ _ _ _ _
  pos := by unfold ε; linarith
  drift x hx := by
    rw [apply_akernel, Finset.sum_congr rfl fun k hk => term_off_F A hx hk]
    have : ∑ k ∈ Finset.range (K + 1), A.p k * (V A x - 128 + 1280 * k) =
        (V A x - 128) + 1280 * A.mean := by
      unfold DaiStable.Arrivals.mean
      simp only [mul_add, Finset.sum_add_distrib, sum_p_mul, Finset.mul_sum]
      congr 1
      exact Finset.sum_congr rfl fun k _ => by ring
    rw [this]; unfold ε; linarith

/-- `F` is small: its backlog is below `128 · 1280`. -/
theorem V_le_of_F {x : AState A} (hx : F A x) : V A x ≤ 128 * 1280 := by
  unfold V
  have : backlog x.1 ≤ 128 * 1280 := by
    rcases hx with h | h
    · rw [h]; simp [backlog]
    · have h1 := shares_sum_eq_of_lt x.1 128 h
      have h2 := length_le_want (good A x)
      have h3 := backlog_le_length (good A x)
      omega
  exact_mod_cast this

/-- A slot adds at most `1280 K` to the backlog. -/
theorem V_next_le (x : AState A) {k : ℕ} (hk : k ∈ Finset.range (K + 1)) :
    V A (next A x k) ≤ V A x + 1280 * K := by
  have hkK : k ≤ K := Nat.lt_succ_iff.mp (Finset.mem_range.mp hk)
  by_cases h : k ≤ K ∧ 0 < A.p k
  · unfold V
    rw [next_val A h]
    have : backlog (absSlot k x.1) ≤ backlog x.1 + 1280 * K := by
      by_cases h0 : x.1 = []
      · rw [h0, backlog_absSlot_nil]; omega
      · have := backlog_absSlot k h0; omega
    exact_mod_cast this
  · rw [next_of_not A h]
    have : (0 : ℝ) ≤ 1280 * K := by positivity
    linarith

theorem applyN_le : ∀ (n : ℕ) (x : AState A),
    (akernel A).applyN n (V A) x ≤ V A x + 1280 * K * n
  | 0, x => by simp [Kernel.applyN]
  | n + 1, x => by
    simp only [Kernel.applyN]
    have h1 := (akernel A).apply_mono (Kernel.integrable_ofOutcomes _ _ _ _ _ _) (Kernel.integrable_ofOutcomes _ _ _ _ _ _) (applyN_le n) x
    rw [(akernel A).apply_add (Kernel.integrable_ofOutcomes _ _ _ _ _ _) (Kernel.integrable_ofOutcomes _ _ _ _ _ _), Kernel.apply_const] at h1
    have h2 : (akernel A).apply (V A) x ≤ V A x + 1280 * K := by
      rw [apply_akernel]
      calc ∑ k ∈ Finset.range (K + 1), A.p k * V A (next A x k)
          ≤ ∑ k ∈ Finset.range (K + 1), A.p k * (V A x + 1280 * K) :=
            Finset.sum_le_sum fun k hk => mul_le_mul_of_nonneg_left (V_next_le A x hk) (A.nonneg k)
        _ = V A x + 1280 * K := sum_p_mul A _
    push_cast; nlinarith

/-- One slot's term bounds the expectation from below. -/
theorem le_apply (f : AState A → ℝ) (hf : ∀ y, 0 ≤ f y) (x : AState A) {k : ℕ} (hk : k ≤ K) :
    A.p k * f (next A x k) ≤ (akernel A).apply f x :=
  Kernel.le_apply_ofOutcomes _ _ _ _ _ f hf x hk

/-- Without arrivals the engine drains: from backlog `b ≤ n`, the empty
list within `n` slots with probability at least `p₀ⁿ`. -/
theorem drain (hA : 1280 * A.mean < 128) : ∀ (n : ℕ) (x : AState A), backlog x.1 ≤ n →
    A.p 0 ^ n ≤ reach (akernel A) (· = nil A) n x
  | 0, x, hb => by
    have : x.1 = [] := by
      by_contra h
      have := backlog_pos (good A x) h
      omega
    have hx : x = nil A := Subtype.ext this
    rw [reach_of_mem (akernel A) (· = nil A) hx]; simp
  | n + 1, x, hb => by
    by_cases hx : x = nil A
    · rw [reach_of_mem (akernel A) (· = nil A) hx]
      exact pow_le_one₀ (A.nonneg 0) (p0_le_one A)
    · have hne : x.1 ≠ [] := fun h => hx (Subtype.ext h)
      have hp := p0_pos A hA
      have hval := next_val A (x := x) ⟨Nat.zero_le K, hp⟩
      have hb' : backlog (next A x 0).1 ≤ n := by
        rw [hval]
        have h1 := backlog_absSlot 0 hne
        have h2 := one_le_shares (good A x) hne
        omega
      have ih := drain hA n (next A x 0) hb'
      have h3 := le_apply A (reach (akernel A) (· = nil A) n) (reach_nonneg (akernel A) _ n) x (Nat.zero_le K)
      show A.p 0 ^ (n + 1) ≤ (if x = nil A then 1 else (akernel A).apply (reach (akernel A) (· = nil A) n) x)
      rw [if_neg hx, pow_succ]
      calc A.p 0 ^ n * A.p 0 ≤ reach (akernel A) (· = nil A) n (next A x 0) * A.p 0 :=
            mul_le_mul_of_nonneg_right ih (A.nonneg 0)
        _ = A.p 0 * reach (akernel A) (· = nil A) n (next A x 0) := mul_comm _ _
        _ ≤ _ := h3

theorem hit_bound (hA : 1280 * A.mean < 128) : ∀ n x,
    hit (akernel A) (· = nil A) n x ≤
      V A x / ε A + ((128 * 1280 : ℕ) + (128 * 1280 + 1280 * K * (128 * 1280 : ℕ)) / ε A) /
        A.p 0 ^ (128 * 1280) :=
  hit_le_of_reach (akernel A) (· = nil A) (fun _ => Kernel.integrable_ofOutcomes _ _ _ _ _ _) (drift A hA)
    (fun x hx => Or.inl (by rw [hx]; rfl)) (pow_pos (p0_pos A hA) _)
    (fun x hx => drain A hA _ x (by
      have := V_le_of_F A hx
      unfold V at this
      exact_mod_cast this))
    (fun x hx => by
      have h1 := applyN_le A (128 * 1280) x
      have h2 := V_le_of_F A hx
      linarith)

/-- A slot of positive probability, then on. -/
theorem reaches_step {x y : AState A} {k : ℕ} (hk : k ≤ K) (hp : 0 < A.p k)
    (h : Reaches (akernel A) (next A x k) y) : Reaches (akernel A) x y :=
  Reaches.step _ hp (fun f hf _ => le_apply A f hf x hk) h

/-- The empty list reaches every state: along the arrivals that built it. -/
theorem nil_reaches : ∀ (js : List AJob) (h : AReach A js), Reaches (akernel A) (nil A) ⟨js, h⟩
  | _, .nil => Reaches.refl _ _
  | _, .step k hk hp h => by
    refine (nil_reaches _ h).trans _ (reaches_step A hk hp ?_)
    have : next A ⟨_, h⟩ k = ⟨absSlot k _, .step k hk hp h⟩ := Subtype.ext (next_val A ⟨hk, hp⟩)
    rw [this]; exact Reaches.refl _ _

/-- Every state reaches the empty list: by draining. -/
theorem reaches_nil (hA : 1280 * A.mean < 128) (x : AState A) : Reaches (akernel A) x (nil A) :=
  ⟨backlog x.1, lt_of_lt_of_le (pow_pos (p0_pos A hA) _) (drain A hA _ x le_rfl)⟩

end Chain

/-- From every state the expected number of slots until the engine is
empty is bounded (uniformly in the truncation). -/
theorem hit_nil_le {K : ℕ} (A : DaiStable.Arrivals K) (hA : 1280 * A.mean < 128) :
    ∃ W : AState A → ℝ, ∀ n x, hit (akernel A) (· = nil A) n x ≤ W x :=
  ⟨_, hit_bound A hA⟩

/-- Every state reaches every state with positive probability. -/
theorem irreducible {K : ℕ} (A : DaiStable.Arrivals K) (hA : 1280 * A.mean < 128) :
    Irreducible (akernel A) :=
  fun x y => (reaches_nil A hA x).trans _ (nil_reaches A y.1 y.2)

/-- Theorem 2(b): below capacity, every state of Sarathi's chain is positive
recurrent. -/
theorem positive_recurrent {K : ℕ} (A : DaiStable.Arrivals K) (hA : 1280 * A.mean < 128)
    (y : AState A) : PositiveRecurrent (akernel A) y := by
  obtain ⟨W, hW⟩ := hit_nil_le A hA
  exact positiveRecurrent_of_hit (akernel A) (nil A) (fun _ => Kernel.integrable_ofOutcomes _ _ _ _ _ _) hW y (nil_reaches A y.1 y.2)

end DaiRecurrent
end Papers
end SerqLang
