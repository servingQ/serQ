/-
Foster's criterion, without measure theory.

A discrete-time Markov chain on `α` is given here by its kernel only: from `x`
outcome `i : ι` happens with probability `p x i` and the chain moves to
`next x i`. The outcomes may be infinitely many (a Poisson number of
arrivals), so an expectation `Σ_i p x i · V (next x i)` is a series, and the
statements that compare two of them ask that they converge (`Integrable`);
a bounded function always does, and so does every function when the
outcomes are finitely many. No path space and no probability measure on it
are built. What is proved is a set of statements about the kernel's
equations.

`hit K F n x` is the expected value of `min(τ_F, n)` started at `x`, where `τ_F`
is the first time `≥ 0` the chain is in `F`. It is defined by the first-step
recursion that this expectation satisfies, so it needs no measure:
`hit (n+1) x = 0` on `F` and `1 + Σ_i p x i · hit n (next x i)` off `F`.
`hitTime K F x` is its supremum over `n`, the expected hitting time of `F`.

Foster's drift condition is a function `V ≥ 0`, integrable, and an `ε > 0`
with `Σ_i p x i · V (next x i) ≤ V x - ε` at every `x ∉ F`. Under it:

* `drift_bound`: `ε · hit K F n x ≤ V x` for every `n`, so the truncated hitting
  times are bounded and `hitTime` is their limit (`hit_tendsto`), with
  `ε · hitTime K F x ≤ V x` (`hitTime_le`);
* `hitTime_eq`: `hitTime` satisfies the first-step equation
  `hitTime x = if F x then 0 else 1 + Σ_i p x i · hitTime (next x i)`;
* `returnTime_le`, `returnTime_le_of_drift`: the expected return time to
  `F`, one step and then the hitting time of `F`, is at most
  `1 + (Σ_i p x i · V (next x i)) / ε`, a finite number, from every state.

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
import Mathlib.Analysis.Normed.Group.Tannery

namespace SerqLang.Foster

open Filter Topology

/-- A Markov kernel on `α` with outcomes `ι`: from `x` outcome `i` has
probability `p x i` and leads to `next x i`. -/
structure Kernel (α : Type*) (ι : Type*) where
  p : α → ι → ℝ
  next : α → ι → α
  nonneg : ∀ x i, 0 ≤ p x i
  sum_one : ∀ x, HasSum (p x) 1

variable {α ι : Type*}

/-- `PV x = Σ_i p x i · V (next x i)`. -/
noncomputable def Kernel.apply (K : Kernel α ι) (V : α → ℝ) (x : α) : ℝ :=
  ∑' i, K.p x i * V (K.next x i)

/-- `V`'s expectation converges from every state. -/
def Kernel.Integrable (K : Kernel α ι) (V : α → ℝ) : Prop :=
  ∀ x, Summable fun i => K.p x i * V (K.next x i)

namespace Kernel

variable (K : Kernel α ι)

theorem integrable_of_bdd {V : α → ℝ} (c : ℝ) (h : ∀ y, |V y| ≤ c) : K.Integrable V := fun x =>
  Summable.of_norm_bounded ((K.sum_one x).summable.mul_right c) fun i => by
    rw [Real.norm_eq_abs, abs_mul, abs_of_nonneg (K.nonneg x i)]
    exact mul_le_mul_of_nonneg_left (h _) (K.nonneg x i)

theorem Integrable.of_le {K : Kernel α ι} {V W : α → ℝ} (hW : K.Integrable W) (h0 : ∀ y, 0 ≤ V y) (h : ∀ y, V y ≤ W y) :
    K.Integrable V := fun x =>
  Summable.of_nonneg_of_le (fun i => mul_nonneg (K.nonneg x i) (h0 _))
    (fun i => mul_le_mul_of_nonneg_left (h _) (K.nonneg x i)) (hW x)

theorem Integrable.const_mul {K : Kernel α ι} {V : α → ℝ} (hV : K.Integrable V) (c : ℝ) : K.Integrable fun y => c * V y :=
  fun x => ((hV x).mul_left c).congr fun i => by ring

theorem Integrable.add {K : Kernel α ι} {V W : α → ℝ} (hV : K.Integrable V) (hW : K.Integrable W) :
    K.Integrable fun y => V y + W y :=
  fun x => ((hV x).add (hW x)).congr fun i => by ring

theorem integrable_const (c : ℝ) : K.Integrable fun _ => c :=
  K.integrable_of_bdd |c| fun _ => le_rfl

theorem apply_mono {V W : α → ℝ} (hV : K.Integrable V) (hW : K.Integrable W) (h : ∀ y, V y ≤ W y)
    (x : α) : K.apply V x ≤ K.apply W x :=
  (hV x).tsum_le_tsum (fun i => mul_le_mul_of_nonneg_left (h _) (K.nonneg x i)) (hW x)

theorem apply_nonneg {V : α → ℝ} (h : ∀ y, 0 ≤ V y) (x : α) : 0 ≤ K.apply V x :=
  tsum_nonneg fun i => mul_nonneg (K.nonneg x i) (h _)

/-- One outcome's term bounds the expectation of a nonnegative function. -/
theorem le_apply {V : α → ℝ} (hV : K.Integrable V) (h : ∀ y, 0 ≤ V y) (x : α) (i : ι) :
    K.p x i * V (K.next x i) ≤ K.apply V x :=
  (hV x).le_tsum i fun j _ => mul_nonneg (K.nonneg x j) (h _)

theorem apply_const_mul (c : ℝ) (V : α → ℝ) (x : α) :
    K.apply (fun y => c * V y) x = c * K.apply V x := by
  unfold apply
  rw [← tsum_mul_left]
  exact tsum_congr fun i => by ring

theorem apply_const (c : ℝ) (x : α) : K.apply (fun _ => c) x = c := by
  unfold apply
  rw [tsum_mul_right, (K.sum_one x).tsum_eq, one_mul]

theorem apply_add {f g : α → ℝ} (hf : K.Integrable f) (hg : K.Integrable g) (x : α) :
    K.apply (fun y => f y + g y) x = K.apply f x + K.apply g x := by
  unfold apply
  rw [← (hf x).tsum_add (hg x)]
  exact tsum_congr fun i => by ring

theorem apply_const_sub (c : ℝ) {f : α → ℝ} (hf : K.Integrable f) (x : α) :
    K.apply (fun y => c - f y) x = c - K.apply f x := by
  have h := K.apply_add (K.integrable_const c) (hf.const_mul (-1)) x
  rw [K.apply_const, K.apply_const_mul] at h
  rw [show (fun y => c - f y) = fun y => c + -1 * f y from funext fun y => by ring, h]
  ring

/-- `|PV| ≤ c` when `|V| ≤ c`. -/
theorem abs_apply_le {V : α → ℝ} {c : ℝ} (h : ∀ y, |V y| ≤ c) (x : α) : |K.apply V x| ≤ c := by
  have hs := K.integrable_of_bdd c h x
  have hc : K.Integrable fun _ => c := K.integrable_const c
  have h1 : K.apply V x ≤ c := by
    have := K.apply_mono (K.integrable_of_bdd c h) hc (fun y => (abs_le.mp (h y)).2) x
    rwa [K.apply_const] at this
  have h2 : -c ≤ K.apply V x := by
    have := K.apply_mono (K.integrable_const (-c)) (K.integrable_of_bdd c h)
      (fun y => (abs_le.mp (h y)).1) x
    rwa [K.apply_const] at this
  exact abs_le.mpr ⟨h2, h1⟩

end Kernel

variable (K : Kernel α ι) (F : α → Prop) [DecidablePred F]

/-- Expected time to reach `F`, truncated at `n` steps: `E_x[min(τ_F, n)]`,
`τ_F` the first time `≥ 0` in `F`. -/
noncomputable def hit : ℕ → α → ℝ
  | 0, _ => 0
  | n + 1, x => if F x then 0 else 1 + K.apply (hit n) x

/-- The expected hitting time of `F`: the supremum of the truncations. -/
noncomputable def hitTime (x : α) : ℝ := ⨆ n, hit K F n x

/-- The expected return time to `F`: one step, then the time to come back. -/
noncomputable def returnTime (x : α) : ℝ := 1 + K.apply (hitTime K F) x

/-- Foster's drift condition for `F` with Lyapunov function `V` and drift `ε`. -/
structure Drift (V : α → ℝ) (ε : ℝ) : Prop where
  nonneg : ∀ x, 0 ≤ V x
  integrable : K.Integrable V
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

theorem hit_le_n : ∀ n x, hit K F n x ≤ n
  | 0, x => by simp [hit]
  | n + 1, x => by
    unfold hit
    split
    · positivity
    · have := K.apply_mono (K.integrable_of_bdd n fun y => by
        rw [abs_of_nonneg (hit_nonneg K F n y)]; exact hit_le_n n y) (K.integrable_const n) (hit_le_n n) x
      rw [K.apply_const] at this
      push_cast; linarith

/-- A truncated hitting time is bounded, so its expectations converge. -/
theorem hit_integrable (n : ℕ) : K.Integrable (hit K F n) :=
  K.integrable_of_bdd n fun y => by rw [abs_of_nonneg (hit_nonneg K F n y)]; exact hit_le_n K F n y

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
      linarith [K.apply_mono (hit_integrable K F n) (hit_integrable K F (n + 1)) ih x]

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
    · have h1 := K.apply_mono ((hit_integrable K F n).const_mul ε) h.integrable ih x
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

/-- The hitting time's expectation converges: it is at most `V / ε`. -/
theorem hitTime_integrable (h : Drift K F V ε) : K.Integrable (hitTime K F) :=
  Kernel.Integrable.of_le (h.integrable.const_mul (1 / ε)) (hitTime_nonneg h) fun y => by
    have := hitTime_le h y
    rw [one_div, ← div_eq_inv_mul, le_div_iff₀ h.pos]; linarith

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
      -- dominated by `p · V / ε`
      refine tendsto_tsum_of_dominated_convergence ((h.integrable.const_mul (1 / ε)) x)
        (fun i => (hit_tendsto h _).const_mul _) (Eventually.of_forall fun n i => ?_)
      rw [Real.norm_eq_abs, abs_mul, abs_of_nonneg (K.nonneg x i), abs_of_nonneg (hit_nonneg K F n _)]
      have := drift_bound h n (K.next x i)
      have : hit K F n (K.next x i) ≤ 1 / ε * V (K.next x i) := by
        rw [one_div, ← div_eq_inv_mul, le_div_iff₀ h.pos]; linarith
      calc K.p x i * hit K F n (K.next x i) ≤ K.p x i * (1 / ε * V (K.next x i)) :=
            mul_le_mul_of_nonneg_left this (K.nonneg x i)
        _ = 1 / ε * (K.p x i * V (K.next x i)) := by ring
        _ = _ := by ring
    exact tendsto_nhds_unique h1 h2

theorem returnTime_le (h : Drift K F V ε) (x : α) :
    ε * returnTime K F x ≤ ε + K.apply V x := by
  have h1 := K.apply_mono ((hitTime_integrable h).const_mul ε) h.integrable (hitTime_le h) x
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

/-- The reflected walk: down (`false`) or up (`true`). -/
noncomputable def walk : Kernel ℕ Bool where
  p _ b := if b then 1 / 3 else 2 / 3
  next x b := if b then x + 1 else x - 1
  nonneg _ b := by cases b <;> norm_num
  sum_one _ := by
    have h := hasSum_fintype (fun b : Bool => if b then (1 / 3 : ℝ) else 2 / 3)
    rwa [show (∑ b : Bool, if b then (1 / 3 : ℝ) else 2 / 3) = 1 by
      rw [Fintype.sum_bool]; norm_num] at h

theorem walk_drift :
    Drift walk (· = 0) (fun n => (n : ℝ)) (1 / 3) where
  nonneg n := Nat.cast_nonneg n
  integrable x := (hasSum_fintype _).summable
  pos := by norm_num
  drift x hx := by
    obtain ⟨m, rfl⟩ := Nat.exists_eq_succ_of_ne_zero hx
    simp only [Kernel.apply, walk, tsum_fintype, Fintype.sum_bool]
    norm_num
    linarith

theorem walk_hitTime_le (n : ℕ) : hitTime walk (· = 0) n ≤ 3 * n := by
  linarith [hitTime_le walk_drift n]

end SerqLang.Foster
