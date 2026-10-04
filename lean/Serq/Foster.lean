/-
Foster's criterion, without measure theory.

A discrete-time Markov chain on `α` is given here by its kernel only: from `x` it
moves to `y ∈ supp x` with probability `P x y`, and `supp x` is finite. No path
space and no probability measure on it are built. What is proved is a set of
statements about the kernel's equations.

`hit K F n x` is the expected value of `min(τ_F, n)` started at `x`, where `τ_F`
is the first time `≥ 0` the chain is in `F`. It is defined by the first-step
recursion that this expectation satisfies, so it needs no measure:
`hit (n+1) x = 0` on `F` and `1 + Σ_y P x y · hit n y` off `F`.
`hitTime K F x` is its supremum over `n`, the expected hitting time of `F`.

Foster's drift condition is a function `V ≥ 0` and an `ε > 0` with
`Σ_y P x y · V y ≤ V x - ε` at every `x ∉ F`. Under it:

* `drift_bound`: `ε · hit K F n x ≤ V x` for every `n`, so the truncated hitting
  times are bounded and `hitTime` is their limit (`hit_tendsto`), with
  `ε · hitTime K F x ≤ V x` (`hitTime_le`);
* `hitTime_eq`: `hitTime` satisfies the first-step equation
  `hitTime x = if F x then 0 else 1 + Σ_y P x y · hitTime y`;
* `returnTime_le`, `returnTime_le_of_drift`: the expected return time to
  `F`, one step and then the hitting time of `F`, is at most
  `1 + (Σ_y P x y · V y) / ε`, a finite number, from every state.

Why this is what positive recurrence needs. For an irreducible chain, if a
finite set `F` has a finite expected return time from each of its states, the
chain is positive recurrent. That implication is a fact about Markov chains
that does not depend on the drift; the content Foster's criterion supplies is
the finite expected return times, and that is what is proved here.

What ties `hitTime` to the path measure is the minimal-solution
characterisation: the expected hitting time of `F` is the least nonnegative
solution of the first-step equation, and it is the increasing limit of the
truncations `hit n`, which is exactly how `hitTime` is defined. We use the
truncations and never the path measure, so nothing here depends on it.
-/
import Mathlib.Tactic
import Mathlib.Topology.Algebra.Order.LiminfLimsup
import Mathlib.Topology.Order.MonotoneConvergence
import Mathlib.Topology.Algebra.Monoid

namespace SerqLang.Foster

open Filter Topology

/-- A Markov kernel on `α` with finite support: from `x` the chain moves to
`y ∈ supp x` with probability `P x y`. -/
structure Kernel (α : Type*) where
  supp : α → Finset α
  P : α → α → ℝ
  nonneg : ∀ x y, 0 ≤ P x y
  sum_one : ∀ x, ∑ y ∈ supp x, P x y = 1

variable {α : Type*}

/-- `PV x = Σ_y P x y V y`. -/
def Kernel.apply (K : Kernel α) (V : α → ℝ) (x : α) : ℝ :=
  ∑ y ∈ K.supp x, K.P x y * V y

namespace Kernel

variable (K : Kernel α)

theorem apply_mono {V W : α → ℝ} (h : ∀ y, V y ≤ W y) (x : α) :
    K.apply V x ≤ K.apply W x :=
  Finset.sum_le_sum fun y _ => mul_le_mul_of_nonneg_left (h y) (K.nonneg x y)

theorem apply_nonneg {V : α → ℝ} (h : ∀ y, 0 ≤ V y) (x : α) : 0 ≤ K.apply V x :=
  Finset.sum_nonneg fun y _ => mul_nonneg (K.nonneg x y) (h y)

theorem apply_const_mul (c : ℝ) (V : α → ℝ) (x : α) :
    K.apply (fun y => c * V y) x = c * K.apply V x := by
  unfold apply
  rw [Finset.mul_sum]
  exact Finset.sum_congr rfl fun y _ => by ring

end Kernel

variable (K : Kernel α) (F : α → Prop) [DecidablePred F]

/-- Expected time to reach `F`, truncated at `n` steps: `E_x[min(τ_F, n)]`,
`τ_F` the first time `≥ 0` in `F`. -/
def hit : ℕ → α → ℝ
  | 0, _ => 0
  | n + 1, x => if F x then 0 else 1 + K.apply (hit n) x

/-- The expected hitting time of `F`: the supremum of the truncations. -/
noncomputable def hitTime (x : α) : ℝ := ⨆ n, hit K F n x

/-- The expected return time to `F`: one step, then the time to come back. -/
noncomputable def returnTime (x : α) : ℝ := 1 + K.apply (hitTime K F) x

/-- Foster's drift condition for `F` with Lyapunov function `V` and drift `ε`. -/
structure Drift (V : α → ℝ) (ε : ℝ) : Prop where
  nonneg : ∀ x, 0 ≤ V x
  pos : 0 < ε
  drift : ∀ x, ¬ F x → K.apply V x ≤ V x - ε

theorem hit_nonneg : ∀ n x, 0 ≤ hit K F n x := by
  intro n
  induction n with
  | zero => intro x; simp [hit]
  | succ n ih =>
    intro x
    by_cases hx : F x
    · simp [hit, hx]
    · simp only [hit, hx, if_false]
      linarith [K.apply_nonneg ih x]

theorem hit_le_succ : ∀ n x, hit K F n x ≤ hit K F (n + 1) x := by
  intro n
  induction n with
  | zero => intro x; simpa [hit] using hit_nonneg K F 1 x
  | succ n ih =>
    intro x
    by_cases hx : F x
    · simp [hit, hx]
    · show (if F x then 0 else 1 + K.apply (hit K F n) x) ≤
          (if F x then 0 else 1 + K.apply (hit K F (n + 1)) x)
      rw [if_neg hx, if_neg hx]
      linarith [K.apply_mono ih x]

theorem hit_mono (x : α) : Monotone fun n => hit K F n x :=
  monotone_nat_of_le_succ fun n => hit_le_succ K F n x

variable {K F}
variable {V : α → ℝ} {ε : ℝ}

/-- Foster's drift bound: `ε · E_x[min(τ_F, n)] ≤ V x`. -/
theorem drift_bound (h : Drift K F V ε) : ∀ n x, ε * hit K F n x ≤ V x := by
  intro n
  induction n with
  | zero => intro x; simp [hit, h.nonneg x]
  | succ n ih =>
    intro x
    by_cases hx : F x
    · simp [hit, hx, h.nonneg x]
    · have h1 := K.apply_mono ih x
      rw [K.apply_const_mul] at h1
      have h2 := h.drift x hx
      simp only [hit, hx, if_false]
      rw [mul_add]
      linarith

theorem hit_bdd (h : Drift K F V ε) (x : α) :
    BddAbove (Set.range fun n => hit K F n x) := by
  refine ⟨V x / ε, ?_⟩
  rintro _ ⟨n, rfl⟩
  rw [le_div_iff₀ h.pos]
  linarith [drift_bound h n x]

theorem hitTime_le (h : Drift K F V ε) (x : α) : ε * hitTime K F x ≤ V x := by
  have : hitTime K F x ≤ V x / ε :=
    ciSup_le fun n => (le_div_iff₀ h.pos).2 (by linarith [drift_bound h n x])
  rw [le_div_iff₀ h.pos] at this
  linarith

theorem hitTime_nonneg (h : Drift K F V ε) (x : α) : 0 ≤ hitTime K F x :=
  le_trans (hit_nonneg K F 0 x) (le_ciSup (hit_bdd h x) 0)

theorem hit_tendsto (h : Drift K F V ε) (x : α) :
    Tendsto (fun n => hit K F n x) atTop (𝓝 (hitTime K F x)) :=
  tendsto_atTop_ciSup (hit_mono K F x) (hit_bdd h x)

/-- The first-step equation holds in the limit. -/
theorem hitTime_eq (h : Drift K F V ε) (x : α) :
    hitTime K F x = if F x then 0 else 1 + K.apply (hitTime K F) x := by
  by_cases hx : F x
  · have h0 : ∀ n, hit K F n x = 0 := fun n => by cases n <;> simp [hit, hx]
    simp only [hitTime, h0, ciSup_const, hx, if_true]
  · rw [if_neg hx]
    have h1 := (hit_tendsto h x).comp (tendsto_add_atTop_nat 1)
    have h2 : Tendsto (fun n => hit K F (n + 1) x) atTop
        (𝓝 (1 + K.apply (hitTime K F) x)) := by
      simp only [hit, hx, if_false]
      refine Tendsto.const_add _ ?_
      unfold Kernel.apply
      exact tendsto_finset_sum _ fun y _ => (hit_tendsto h y).const_mul _
    exact tendsto_nhds_unique h1 h2

theorem returnTime_le (h : Drift K F V ε) (x : α) :
    ε * returnTime K F x ≤ ε + K.apply V x := by
  have h1 := K.apply_mono (hitTime_le h) x
  rw [K.apply_const_mul] at h1
  unfold returnTime
  rw [mul_add, mul_one]
  linarith

/-- Every state of `F` has a finite expected return time to `F`. -/
theorem returnTime_le_of_drift (h : Drift K F V ε) :
    ∀ x, F x → returnTime K F x ≤ 1 + K.apply V x / ε := by
  intro x _
  have h1 := returnTime_le h x
  have e : 1 + K.apply V x / ε = (ε + K.apply V x) / ε := by
    field_simp [h.pos.ne']
  rw [e, le_div_iff₀ h.pos]
  linarith

/-! ### Sanity check: a reflected walk on `ℕ`

From `n + 1` the walk goes down with probability `2/3` and up with `1/3`; from
`0` it stays with `2/3` and goes up with `1/3`. With `V n = n`, `F = {0}` and
`ε = 1/3`, the expected time to reach `0` from `n` is at most `3n`. -/

/-- The reflected walk's transition probabilities. -/
noncomputable def walkP (x y : ℕ) : ℝ :=
  if y + 1 = x then 2 / 3 else if y = x + 1 then 1 / 3
  else if x = 0 ∧ y = 0 then 2 / 3 else 0

/-- The reflected walk. -/
noncomputable def walk : Kernel ℕ where
  supp x := if x = 0 then {0, 1} else {x - 1, x + 1}
  P := walkP
  nonneg x y := by unfold walkP; split_ifs <;> norm_num
  sum_one x := by
    rcases x with _ | m
    · simp [walkP]; norm_num
    · have h1 : m ≠ m + 1 + 1 := by omega
      have h2 : m + 1 + 1 ≠ m := by omega
      simp [walkP, Finset.sum_pair h1, h2]
      norm_num

theorem walk_drift :
    Drift walk (· = 0) (fun n => (n : ℝ)) (1 / 3) where
  nonneg n := Nat.cast_nonneg n
  pos := by norm_num
  drift x hx := by
    obtain ⟨m, rfl⟩ := Nat.exists_eq_succ_of_ne_zero hx
    have h1 : m ≠ m + 1 + 1 := by omega
    have h2 : m + 1 + 1 ≠ m := by omega
    simp [Kernel.apply, walk, walkP, Finset.sum_pair h1, h2]
    linarith

theorem walk_hitTime_le (n : ℕ) : hitTime walk (· = 0) n ≤ 3 * n := by
  linarith [hitTime_le walk_drift n]

end SerqLang.Foster
