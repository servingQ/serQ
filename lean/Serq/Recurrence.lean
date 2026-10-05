/-
# Positive recurrence, from Foster's criterion

`Serq/Foster.lean` bounds the expected time to reach a set `F` under a drift
condition. Positive recurrence is about returning to a *state*. This module
closes the gap, still without a path measure: every statement is about the
kernel's first-step recursions, truncated at `n` steps, and bounded uniformly
in `n` (so no supremum in ℝ is read where it might be junk).

* `Kernel.applyN n V`: the expectation of `V` after `n` steps, `P^n V`.
* `reach K T n x`: the probability of being in `T` within `n` steps.
* `hit_le_of_reach`: if the chain drifts to `F`, and from every state of
  `F` reaches `T ⊆ F` within `L` steps with probability at least `δ`, the
  truncated expected hitting times of `T` are bounded by
  `V x / ε + (L + B / ε) / δ`, `B` a bound on `P^L V` over `F`. (Each visit
  to `F` is a trial that succeeds with probability `δ`, and costs at most
  `L` steps and the expected return.)
* `PositiveRecurrent K x`: the truncated expected return times to `x` are
  bounded. `Irreducible K`: every state reaches every state with positive
  probability.
* `positiveRecurrent_of_hit`: if the expected hitting time of one state `o`
  is bounded from everywhere, every state that `o` reaches is positive
  recurrent.

The two theorems take the expectation of one unbounded function, the
Lyapunov function `V` (the bound `W`), after several steps, so they ask that
`P^n V` converge for every `n` (`Kernel.IntegrableN`). It does for finitely
many outcomes (`Kernel.integrable_ofOutcomes`), and whenever a step adds at
most a constant to `V` in expectation (`Kernel.integrableN_of_apply_le`), as a
Poisson number of arrivals does. Every other function they take an
expectation of is bounded.
-/
import Serq.Foster

namespace SerqLang.Foster

variable {α ι : Type*}

/-- `P^n V`: the expectation of `V` after `n` steps. -/
noncomputable def Kernel.applyN (K : Kernel α ι) : ℕ → (α → ℝ) → α → ℝ
  | 0, V => V
  | n + 1, V => K.apply (Kernel.applyN K n V)

variable (K : Kernel α ι)

/-- `P^n V` converges for every `n`. -/
def Kernel.IntegrableN (V : α → ℝ) : Prop := ∀ n, K.Integrable (K.applyN n V)

variable (T : α → Prop) [DecidablePred T]

/-- The probability of being in `T` at some step `≤ n`. -/
noncomputable def reach : ℕ → α → ℝ
  | 0, x => if T x then 1 else 0
  | n + 1, x => if T x then 1 else K.apply (reach n) x


namespace Kernel

theorem applyN_nonneg {V : α → ℝ} (h : ∀ y, 0 ≤ V y) : ∀ n x, 0 ≤ K.applyN n V x
  | 0, x => h x
  | n + 1, x => K.apply_nonneg (applyN_nonneg h n) x

/-- A step that adds at most `c` to `V` in expectation: `P^n V ≤ V + c n`,
and every `P^n V` converges. -/
theorem applyN_le_of_apply_le {V : α → ℝ} {c : ℝ} (hV : K.Integrable V) (h0 : ∀ y, 0 ≤ V y)
    (h : ∀ x, K.apply V x ≤ V x + c) :
    ∀ n, K.Integrable (K.applyN n V) ∧ ∀ x, K.applyN n V x ≤ V x + c * n
  | 0 => ⟨hV, fun x => by simp [applyN]⟩
  | n + 1 => by
    obtain ⟨hI, hle⟩ := applyN_le_of_apply_le hV h0 h n
    have hle' : ∀ x, K.applyN (n + 1) V x ≤ V x + c * (n + 1 : ℕ) := fun x => by
      have h1 := K.apply_mono hI (hV.add (K.integrable_const (c * n))) hle x
      rw [K.apply_add hV (K.integrable_const _), K.apply_const] at h1
      have := h x
      show K.apply (K.applyN n V) x ≤ _
      push_cast; linarith
    exact ⟨Integrable.of_le (hV.add (K.integrable_const _)) (applyN_nonneg K h0 (n + 1)) hle', hle'⟩

theorem integrableN_of_apply_le {V : α → ℝ} {c : ℝ} (hV : K.Integrable V) (h0 : ∀ y, 0 ≤ V y)
    (h : ∀ x, K.apply V x ≤ V x + c) : K.IntegrableN V :=
  fun n => (applyN_le_of_apply_le K hV h0 h n).1

/-- Below a function whose `P^n` converge, `P^n` converge too. -/
theorem IntegrableN.of_le {K : Kernel α ι} {V W : α → ℝ} (hW : K.IntegrableN W) (h0 : ∀ y, 0 ≤ V y)
    (h : ∀ y, V y ≤ W y) : K.IntegrableN V := by
  have key : ∀ n, K.Integrable (K.applyN n V) ∧ ∀ x, K.applyN n V x ≤ K.applyN n W x := by
    intro n
    induction n with
    | zero => exact ⟨Integrable.of_le (hW 0) h0 h, h⟩
    | succ n ih =>
      have hle : ∀ x, K.applyN (n + 1) V x ≤ K.applyN (n + 1) W x :=
        K.apply_mono ih.1 (hW n) ih.2
      exact ⟨Integrable.of_le (hW (n + 1)) (applyN_nonneg K h0 (n + 1)) hle, hle⟩
  exact fun n => (key n).1

end Kernel

/-! ### Truncated hitting times and reaching probabilities -/

theorem reach_of_mem {x : α} (hx : T x) : ∀ n, reach K T n x = 1
  | 0 => by simp [reach, hx]
  | _ + 1 => by simp [reach, hx]

theorem reach_nonneg : ∀ n x, 0 ≤ reach K T n x
  | 0, x => by unfold reach; split <;> norm_num
  | n + 1, x => by
    unfold reach; split
    · norm_num
    · exact K.apply_nonneg (reach_nonneg n) x

theorem reach_integrable' (n : ℕ) (h : ∀ x, reach K T n x ≤ 1) : K.Integrable (reach K T n) :=
  K.integrable_of_bdd 1 fun y => by rw [abs_of_nonneg (reach_nonneg K T n y)]; exact h y

theorem reach_le_one : ∀ n x, reach K T n x ≤ 1
  | 0, x => by unfold reach; split <;> norm_num
  | n + 1, x => by
    unfold reach
    split
    · exact le_rfl
    · calc K.apply (reach K T n) x ≤ K.apply (fun _ => 1) x :=
            K.apply_mono (reach_integrable' K T n (reach_le_one n)) (K.integrable_const 1) (reach_le_one n) x
        _ = 1 := K.apply_const 1 x

theorem reach_integrable (n : ℕ) : K.Integrable (reach K T n) :=
  reach_integrable' K T n (reach_le_one K T n)

theorem hit_of_mem {x : α} (hx : T x) : ∀ n, hit K T n x = 0
  | 0 => rfl
  | _ + 1 => by simp [hit, hx]

/-- The kernel killed in `T`, iterated: `E_x[f(X_j); X_0, …, X_{j-1} ∉ T]`. -/
noncomputable def killN : ℕ → (α → ℝ) → α → ℝ
  | 0, f => f
  | j + 1, f => fun x => if T x then 0 else K.apply (killN j f) x

theorem killN_nonneg {f : α → ℝ} (hf : ∀ y, 0 ≤ f y) : ∀ j x, 0 ≤ killN K T j f x
  | 0, x => hf x
  | j + 1, x => by
    simp only [killN]
    split
    · exact le_rfl
    · exact K.apply_nonneg (killN_nonneg hf j) x

/-- Killing keeps a function between `0` and `c`. -/
theorem killN_le {f : α → ℝ} {c : ℝ} (hf0 : ∀ y, 0 ≤ f y) (hf : ∀ y, f y ≤ c) :
    ∀ j x, killN K T j f x ≤ c
  | 0, x => hf x
  | j + 1, x => by
    simp only [killN]
    split
    · exact le_trans (hf0 x) (hf x)
    · have hI := K.integrable_of_bdd c fun y => by
        rw [abs_of_nonneg (killN_nonneg K T hf0 j y)]; exact killN_le hf0 hf j y
      have := K.apply_mono hI (K.integrable_const c) (killN_le hf0 hf j) x
      rwa [K.apply_const] at this

theorem killN_integrable_of_le {f : α → ℝ} {c : ℝ} (hf0 : ∀ y, 0 ≤ f y) (hf : ∀ y, f y ≤ c)
    (j : ℕ) : K.Integrable (killN K T j f) :=
  K.integrable_of_bdd c fun y => by
    rw [abs_of_nonneg (killN_nonneg K T hf0 j y)]; exact killN_le K T hf0 hf j y

variable {K} in
/-- Killing keeps a function below `a · P^j V + b` if it starts below `a V + b`. -/
theorem killN_le_affine {V : α → ℝ} (hV : K.IntegrableN V) (hV0 : ∀ y, 0 ≤ V y) {a b : ℝ}
    (ha : 0 ≤ a) (hb : 0 ≤ b) {f : α → ℝ} (hf0 : ∀ y, 0 ≤ f y) (hf : ∀ y, f y ≤ a * V y + b) :
    ∀ j, K.Integrable (killN K T j f) ∧ ∀ x, killN K T j f x ≤ a * K.applyN j V x + b
  | 0 => ⟨Kernel.Integrable.of_le (((hV 0).const_mul a).add (K.integrable_const b)) hf0 hf, hf⟩
  | j + 1 => by
    obtain ⟨hI, hle⟩ := killN_le_affine hV hV0 ha hb hf0 hf j
    have hle' : ∀ x, killN K T (j + 1) f x ≤ a * K.applyN (j + 1) V x + b := by
      intro x
      simp only [killN]
      split
      · have := K.applyN_nonneg hV0 (j + 1) x
        positivity
      · have h1 := K.apply_mono hI (((hV j).const_mul a).add (K.integrable_const b)) hle x
        rw [K.apply_add ((hV j).const_mul a) (K.integrable_const b), K.apply_const_mul,
          K.apply_const] at h1
        exact h1
    exact ⟨Kernel.Integrable.of_le (((hV (j + 1)).const_mul a).add (K.integrable_const b))
      (killN_nonneg K T hf0 (j + 1)) hle', hle'⟩

variable {K} in
theorem killN_mono {f g : α → ℝ} (hf : ∀ j, K.Integrable (killN K T j f))
    (hg : ∀ j, K.Integrable (killN K T j g)) (h : ∀ y, f y ≤ g y) :
    ∀ j x, killN K T j f x ≤ killN K T j g x
  | 0, x => h x
  | j + 1, x => by
    simp only [killN]
    split
    · exact le_rfl
    · exact K.apply_mono (hf j) (hg j) (killN_mono hf hg h j) x

variable {K} in
theorem killN_add {f g : α → ℝ} (hf : ∀ j, K.Integrable (killN K T j f))
    (hg : ∀ j, K.Integrable (killN K T j g)) : ∀ j x,
    killN K T j (fun y => f y + g y) x = killN K T j f x + killN K T j g x
  | 0, x => rfl
  | j + 1, x => by
    simp only [killN]
    split
    · simp
    · rw [show (killN K T j fun y => f y + g y) = fun y => killN K T j f y + killN K T j g y from
          funext (killN_add hf hg j)]
      exact K.apply_add (hf j) (hg j) x

theorem killN_const_mul (c : ℝ) (f : α → ℝ) : ∀ j x,
    killN K T j (fun y => c * f y) x = c * killN K T j f x
  | 0, x => rfl
  | j + 1, x => by
    simp only [killN]
    split
    · simp
    · rw [show (killN K T j fun y => c * f y) = fun y => c * killN K T j f y from
          funext (killN_const_mul c f j)]
      exact K.apply_const_mul c _ x

/-- Surviving `j` steps outside `T`, and still outside at step `j`. -/
theorem killN_out : ∀ j x, killN K T j (fun y => if T y then 0 else 1) x = 1 - reach K T j x
  | 0, x => by simp only [killN, reach]; split <;> norm_num
  | j + 1, x => by
    simp only [killN, reach]
    split
    · norm_num
    · rw [show killN K T j (fun y => if T y then 0 else 1) = fun y => 1 - reach K T j y from
          funext (killN_out j)]
      exact K.apply_const_sub 1 (reach_integrable K T j) x

/-- `E_x[min(τ_T, m + j)] ≤ j + E_x[E_{X_j}[min(τ_T, m)]; τ_T ≥ j]`. -/
theorem hit_add_le : ∀ (j m : ℕ) (x : α), hit K T (m + j) x ≤ j + killN K T j (hit K T m) x
  | 0, m, x => by simp [killN]
  | j + 1, m, x => by
    rw [← Nat.add_assoc]
    simp only [hit, killN]
    split
    · positivity
    · have hI := killN_integrable_of_le K T (hit_nonneg K T m) (hit_le_n K T m) j
      have h1 := K.apply_mono (hit_integrable K T _) ((K.integrable_const _).add hI) (hit_add_le j m) x
      rw [K.apply_add (K.integrable_const _) hI, K.apply_const] at h1
      push_cast; linarith

variable {T}

/-- The decomposition at `F ⊇ T`: from `y`, first reach `F`, then `T`. -/
theorem hit_le_hit_add {F : α → Prop} [DecidablePred F] (hTF : ∀ x, T x → F x) {M : ℝ}
    (hM0 : 0 ≤ M) : ∀ n, (∀ z, F z → hit K T n z ≤ M) → ∀ y, hit K T n y ≤ hit K F n y + M
  | 0, _, y => by simp [hit, hM0]
  | n + 1, hM, y => by
    by_cases hy : F y
    · have := hM y hy
      have := hit_nonneg K F (n + 1) y
      linarith
    · have hTy : ¬ T y := fun h => hy (hTF y h)
      have ih := hit_le_hit_add hTF hM0 n (fun z hz => le_trans (hit_le_succ K T n z) (hM z hz))
      simp only [hit, hy, hTy, ite_false]
      have h1 := K.apply_mono (hit_integrable K T n) ((hit_integrable K F n).add (K.integrable_const M)) ih y
      rw [K.apply_add (hit_integrable K F n) (K.integrable_const M), K.apply_const] at h1
      linarith

/-- A drift to `F` makes `F` nonempty. -/
theorem exists_mem_of_drift {F : α → Prop} [DecidablePred F] {V : α → ℝ} {ε : ℝ}
    (hD : Drift K F V ε) (x : α) : ∃ z, F z := by
  by_contra hne'
  have hne : ∀ z, ¬ F z := fun z hz => hne' ⟨z, hz⟩
  have hn : ∀ n y, hit K F n y = n := by
    intro n
    induction n with
    | zero => intro y; simp [hit]
    | succ n ih =>
      intro y
      simp only [hit, hne y, ite_false]
      rw [show hit K F n = fun _ => (n : ℝ) from funext ih, K.apply_const]
      push_cast; ring
  obtain ⟨n, hn'⟩ := exists_nat_gt (V x / ε)
  have := drift_bound hD n x
  rw [hn] at this
  rw [div_lt_iff₀ hD.pos] at hn'
  linarith

variable (T)

/-- Reaching a target from a set the chain keeps returning to: each visit to
`F` reaches `T` within `L` steps with probability at least `δ`. -/
theorem hit_le_of_reach {F : α → Prop} [DecidablePred F] {V : α → ℝ} {ε : ℝ}
    (hV : K.IntegrableN V) (hD : Drift K F V ε) (hTF : ∀ x, T x → F x) {L : ℕ} {δ B : ℝ} (hδ : 0 < δ)
    (hreach : ∀ x, F x → δ ≤ reach K T L x) (hB : ∀ x, F x → K.applyN L V x ≤ B) :
    ∀ n x, hit K T n x ≤ V x / ε + (L + B / ε) / δ := by
  intro n x
  have hε := hD.pos
  obtain ⟨z0, hz0⟩ := exists_mem_of_drift K hD x
  have hδ1 : δ ≤ 1 := le_trans (hreach z0 hz0) (reach_le_one K T L z0)
  have hB0 : 0 ≤ B := le_trans (K.applyN_nonneg hD.nonneg L z0) (hB z0 hz0)
  set M := (L + B / ε) / δ with hMdef
  have hC : 0 ≤ L + B / ε := by positivity
  have hLM : (L : ℝ) + B / ε ≤ M := by
    rw [hMdef, le_div_iff₀ hδ]
    nlinarith
  have hM0 : 0 ≤ M := le_trans hC hLM
  have hMδ : M * δ = L + B / ε := by rw [hMdef]; field_simp
  -- the bound on `F`, by strong induction on the truncation
  have hF : ∀ n z, F z → hit K T n z ≤ M := by
    intro n
    induction n using Nat.strong_induction_on with
    | _ n ih =>
      intro z hz
      by_cases hTz : T z
      · rw [hit_of_mem K T hTz]; exact hM0
      by_cases hn : n < L
      · calc hit K T n z ≤ n := hit_le_n K T n z
          _ ≤ L := by exact_mod_cast hn.le
          _ ≤ M := by linarith [div_nonneg hB0 hε.le]
      · have hL : 0 < L := by
          rcases Nat.eq_zero_or_pos L with h0 | h0
          · have := hreach z hz
            rw [h0] at this
            simp [reach, hTz] at this
            linarith
          · exact h0
        obtain ⟨m, rfl⟩ : ∃ m, n = m + L := ⟨n - L, by omega⟩
        have hm := ih m (by omega)
        have hdec := hit_le_hit_add K hTF hM0 m hm
        -- `hit T m ≤ V / ε + M · 1_{T^c}`
        have hpt : ∀ y, hit K T m y ≤ (1 / ε) * V y + M * (if T y then 0 else 1) := by
          intro y
          by_cases hTy : T y
          · rw [hit_of_mem K T hTy]; simp only [hTy, ite_true, mul_zero, add_zero]
            exact mul_nonneg (by positivity) (hD.nonneg y)
          · simp only [hTy, ite_false, mul_one]
            have h1 := hdec y
            have h2 := drift_bound hD m y
            have h3 : hit K F m y ≤ (1 / ε) * V y := by
              rw [one_div, ← div_eq_inv_mul, le_div_iff₀ hε]; linarith
            linarith
        have hind0 : ∀ y, (0 : ℝ) ≤ if T y then 0 else 1 := fun y => by split <;> norm_num
        have hind1 : ∀ y, (if T y then (0 : ℝ) else 1) ≤ 1 := fun y => by split <;> norm_num
        have hε' : 0 ≤ 1 / ε := by positivity
        have hIV := killN_le_affine T hV hD.nonneg hε' le_rfl
          (fun y => mul_nonneg hε' (hD.nonneg y)) (fun y => by simp)
        have hII : ∀ j, K.Integrable (killN K T j fun y => M * if T y then 0 else 1) :=
          killN_integrable_of_le K T (c := M) (fun y => mul_nonneg hM0 (hind0 y))
            (fun y => by nlinarith [hind0 y, hind1 y])
        have hIg := fun j => (killN_le_affine T hV hD.nonneg hε' hM0
          (fun y => add_nonneg (mul_nonneg hε' (hD.nonneg y)) (mul_nonneg hM0 (hind0 y)))
          (fun y => by nlinarith [hind0 y, hind1 y]) j).1
        have h1 := hit_add_le K T L m z
        have h2 := killN_mono T (killN_integrable_of_le K T (hit_nonneg K T m) (hit_le_n K T m)) hIg hpt L z
        rw [killN_add T (fun j => (hIV j).1) hII, killN_const_mul, killN_const_mul, killN_out] at h2
        have h3 : killN K T L V z ≤ K.applyN L V z := by
          simpa using (killN_le_affine T hV hD.nonneg zero_le_one le_rfl hD.nonneg
            (fun y => by simp) L).2 z
        have h4 := hB z hz
        have h5 := hreach z hz
        have h6 : (1 / ε) * killN K T L V z ≤ B / ε := by
          rw [one_div, ← div_eq_inv_mul]
          exact div_le_div_of_nonneg_right (by linarith) hε.le
        have h7 : M * (1 - reach K T L z) ≤ M * (1 - δ) := mul_le_mul_of_nonneg_left (by linarith) hM0
        nlinarith
  have hdec := hit_le_hit_add K hTF hM0 n (hF n) x
  have h2 := drift_bound hD n x
  have h3 : hit K F n x ≤ V x / ε := by rw [le_div_iff₀ hε]; linarith
  linarith

/-- A bound on the return time to `T` from one on the hitting times: if
`hit ≤ a V + c` everywhere and `P V ≤ d` at `x`, the truncated return times
from `x` are at most `1 + a d + c`. -/
theorem return_le_of_hit_le {V : α → ℝ} (hV : K.Integrable V) {a c d : ℝ} (ha : 0 ≤ a)
    (hh : ∀ n y, hit K T n y ≤ a * V y + c) {x : α} (hd : K.apply V x ≤ d) (n : ℕ) :
    1 + K.apply (hit K T n) x ≤ 1 + a * d + c := by
  have h1 := K.apply_mono (hit_integrable K T n) ((hV.const_mul a).add (K.integrable_const c)) (hh n) x
  rw [K.apply_add (hV.const_mul a) (K.integrable_const c), K.apply_const, K.apply_const_mul] at h1
  have := mul_le_mul_of_nonneg_left hd ha
  linarith

/-- `x` is positive recurrent: the truncated expected return times to `x`
are bounded. The truncation is given by its first-step recursion,
`1 + Σ_y P x y · E_y[min(τ_x, n)]`, which the path measure's
`E_x[min(τ_x⁺, n + 1)]` satisfies (`Serq/Foster.lean`'s header); bounded in
`n`, it says `E_x[τ_x⁺] < ∞`. -/
def PositiveRecurrent [DecidableEq α] (x : α) : Prop :=
  ∃ C : ℝ, ∀ n, 1 + K.apply (hit K (· = x) n) x ≤ C

/-- Every state reaches every state with positive probability. -/
def Irreducible [DecidableEq α] : Prop :=
  ∀ x y : α, ∃ n, 0 < reach K (· = y) n x

/-- `x` reaches `y` with positive probability. -/
def Reaches [DecidableEq α] (x y : α) : Prop := ∃ n, 0 < reach K (· = y) n x

theorem Reaches.refl [DecidableEq α] (x : α) : Reaches K x x :=
  ⟨0, by rw [reach_of_mem K (· = x) rfl]; norm_num⟩

/-- A step of weight `c > 0` to `z` (the expectation of every nonnegative
function at `x` is at least `c` times its value at `z`), then on to `y`. -/
theorem Reaches.step [DecidableEq α] {x z y : α} {c : ℝ} (hc : 0 < c)
    (hle : ∀ f : α → ℝ, (∀ w, 0 ≤ f w) → (∀ w, f w ≤ 1) → c * f z ≤ K.apply f x) (h : Reaches K z y) :
    Reaches K x y := by
  obtain ⟨n, hn⟩ := h
  refine ⟨n + 1, ?_⟩
  by_cases hxy : x = y
  · rw [reach_of_mem K (· = y) hxy]; norm_num
  · show 0 < (if x = y then 1 else K.apply (reach K (· = y) n) x)
    rw [if_neg hxy]
    exact lt_of_lt_of_le (mul_pos hc hn) (hle _ (reach_nonneg K _ n) (reach_le_one K _ n))

theorem Reaches.trans [DecidableEq α] {x z y : α} (h1 : Reaches K x z) (h2 : Reaches K z y) :
    Reaches K x y := by
  obtain ⟨n, hn⟩ := h1
  induction n generalizing x with
  | zero =>
    have : x = z := by
      by_contra h
      simp [reach, h] at hn
    subst this; exact h2
  | succ n ih =>
    by_cases hxz : x = z
    · subst hxz; exact h2
    · have hn' : 0 < K.apply (reach K (· = z) n) x := by
        have : reach K (· = z) (n + 1) x = K.apply (reach K (· = z) n) x := by simp [reach, hxz]
        rw [← this]; exact hn
      unfold Kernel.apply at hn'
      obtain ⟨w, hpos⟩ : ∃ w, 0 < K.p x w * reach K (· = z) n (K.next x w) := by
        by_contra hne
        push Not at hne
        exact absurd (tsum_nonpos hne) (not_le.mpr hn')
      have hP : 0 < K.p x w := by
        by_contra h
        have : K.p x w = 0 := le_antisymm (not_lt.mp h) (K.nonneg x w)
        rw [this, zero_mul] at hpos; exact lt_irrefl 0 hpos
      have hr : 0 < reach K (· = z) n (K.next x w) := by
        by_contra h
        have : reach K (· = z) n (K.next x w) = 0 := le_antisymm (not_lt.mp h) (reach_nonneg K _ n _)
        rw [this, mul_zero] at hpos; exact lt_irrefl 0 hpos
      refine Reaches.step K hP (fun f hf hf1 => ?_) (ih hr)
      unfold Kernel.apply
      exact (K.integrable_of_bdd 1 (fun v => by rw [abs_of_nonneg (hf v)]; exact hf1 v) x).le_tsum w
        fun v _ => mul_nonneg (K.nonneg x v) (hf _)

/-- If the expected hitting time of `o` is bounded from every state, every
state `o` reaches with positive probability is positive recurrent. -/
theorem positiveRecurrent_of_hit [DecidableEq α] (o : α) {W : α → ℝ} (hWI : K.IntegrableN W)
    (hW : ∀ n x, hit K (· = o) n x ≤ W x) (y : α) (hy : ∃ n, 0 < reach K (· = y) n o) :
    PositiveRecurrent K y := by
  classical
  obtain ⟨L, hL⟩ := hy
  set V : α → ℝ := hitTime K (· = o) with hVdef
  have hbdd : ∀ x, BddAbove (Set.range fun n => hit K (· = o) n x) :=
    fun x => ⟨W x, by rintro _ ⟨n, rfl⟩; exact hW n x⟩
  have hV0 : ∀ x, 0 ≤ V x := fun x => le_trans (hit_nonneg K _ 0 x) (le_ciSup (hbdd x) 0)
  have hVW : ∀ x, V x ≤ W x := fun x => ciSup_le fun n => hW n x
  have hVI : K.Integrable V := Kernel.Integrable.of_le (hWI 0) hV0 hVW
  have htend : ∀ x, Filter.Tendsto (fun n => hit K (· = o) n x) Filter.atTop (nhds (V x)) :=
    fun x => tendsto_atTop_ciSup (hit_mono K _ x) (hbdd x)
  -- off `o`, `1 + P V ≤ V`
  have hdrift : ∀ x, x ≠ o → K.apply V x ≤ V x - 1 := by
    intro x hx
    have h1 : Filter.Tendsto (fun n => 1 + K.apply (hit K (· = o) n) x) Filter.atTop
        (nhds (1 + K.apply V x)) := by
      refine Filter.Tendsto.const_add _ ?_
      unfold Kernel.apply
      refine tendsto_tsum_of_dominated_convergence (hWI 0 x) (fun i => (htend _).const_mul _)
        (Filter.Eventually.of_forall fun n i => ?_)
      rw [Real.norm_eq_abs, abs_mul, abs_of_nonneg (K.nonneg x i), abs_of_nonneg (hit_nonneg K _ n _)]
      exact mul_le_mul_of_nonneg_left (hW n _) (K.nonneg x i)
    have h2 : ∀ n, 1 + K.apply (hit K (· = o) n) x ≤ V x := by
      intro n
      have : hit K (· = o) (n + 1) x = 1 + K.apply (hit K (· = o) n) x := by
        simp [hit, hx]
      rw [← this]
      exact le_ciSup (hbdd x) (n + 1)
    have := le_of_tendsto' h1 h2
    linarith
  let F : α → Prop := fun x => x = o ∨ x = y
  have hD : Drift K F V 1 :=
    ⟨hV0, hVI, one_pos, fun x hx => hdrift x (fun h => hx (Or.inl h))⟩
  set δ := min (reach K (· = y) L o) 1 with hδdef
  have hδ : 0 < δ := lt_min hL one_pos
  set B := max (K.applyN L V o) (K.applyN L V y)
  have hmain := hit_le_of_reach K (· = y) (hWI.of_le hV0 hVW) hD (fun x hx => Or.inr hx) (L := L) (δ := δ) (B := B) hδ
    (by
      rintro x (hx | hx) <;> rw [hx]
      · exact min_le_left _ _
      · rw [reach_of_mem K (· = y) rfl]; exact min_le_right _ _)
    (by
      rintro x (hx | hx) <;> rw [hx]
      · exact le_max_left _ _
      · exact le_max_right _ _)
  refine ⟨1 + K.apply V y + (L + B / 1) / δ, fun n => ?_⟩
  have h1 : ∀ z, hit K (· = y) n z ≤ V z + (L + B / 1) / δ := fun z => by
    simpa using hmain n z
  have h2 := K.apply_mono (hit_integrable K _ n) (hVI.add (K.integrable_const _)) h1 y
  rw [K.apply_add hVI (K.integrable_const _), K.apply_const] at h2
  linarith

end SerqLang.Foster
