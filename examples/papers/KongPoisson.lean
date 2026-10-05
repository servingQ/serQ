import Mathlib.Tactic
import Mathlib.Analysis.SpecificLimits.Normed
import Mathlib.Data.Nat.Choose.Cast

/-!
# Kong et al., Theorems 3.4 and 3.5: Poisson arrivals

Kong, Qi, Ye, Zhou, "Geometry-Aware Online Scheduling for LLM Serving"
(arXiv 2606.22327), §3 and Appendix "Stochastic Bounds under Poisson
Arrivals", stated and proved independently of the serving system, as
`KongMath` does for Theorem 3.2.

* `harris`: Harris's (Chebyshev's) inequality, weighted and finite;
* `aux_bound`, `aux_mono`, `aux_sum`: the priority recursion. Per-class
  waits obeying the balance (A) are bounded by `aux`, which is monotone in
  the class and whose load-weighted sum telescopes to `W₀ Λ / (D − Λ)`;
* `wait_bound`: `E[W_q] ≤ E[v] / (E[v] − ε) · W₀ / (D − Λ)` for classes served
  in ascending proxy volume, with `ε = E[Δ] − Δ_min`;
* geometric lengths: the moments (`hasSum_geo`), `memoryless`, the tail and
  low parts (`cmean_tail`, `cvar_tail`, `cmean_low`, `cvar_low`), the penalty
  `ε = ½ (θ q^θ / (1 − q^θ))²` (`penalty`), `mean_vol`, `w0_closed`, `w_exact`;
* `svf_poisson` (Theorem 3.4) and `svf_1bit_poisson` (Theorem 3.5), as
  compositions of these with the paper's steady-state premises.

What is proved, and what stays a premise. The steady-state statements of the
proof are four hypotheses, named for the paper's step: the balance (A) per
class (`hA`), which rests on Lemma 1's rate `(1−α)M/2` and on ergodicity;
Lemma 2's value of `W₀ = E[U_run]` (`hW0`); Lemma `opt_poisson_lb`, the lower
bound on `E[OPT]` (`hopt`); and SVF's latency as decode plus wait,
`E[SVF] ≤ E[o] + E[W_q]` (`hsvf`). From these the bound follows here without
a gap. The load `Λ` in `ρ` and the volume in `W₀` are not required to be the
same `E[vol]`; the bound holds either way.

Where this differs from the paper:

* The paper applies Harris's inequality to the volumes and the *actual* waits,
  `E[v · W_q] ≥ E[v] E[W_q]`, which needs the waits monotone in the class. It
  does not show that, and (A) does not give it. Here Harris is applied to the
  proxy volumes and the auxiliary bound `aux`, which is monotone
  (`aux_mono`), and `E[W_q] ≤ E[aux]` closes the step.
* The bound is a ratio of expectations, `E[SVF] / E[OPT]`, what the proof
  bounds; the paper writes it `E[CR]`.
* Theorem 3.5 needs `E[v] > ε`, which the paper leaves implicit.
* Lemma 1 needs every peak `p_j ≤ αM`, which a geometric output length
  violates with positive probability; `hA` assumes the rate regardless, so
  in the paper's model this premise may fail.
* Classes are indexed from `0`: Lean's class `k` is the paper's class `k+1`,
  and `load k`, the load of the classes before `k`, is the paper's `R_k`;
  `D` is `(1−α)M/2`.

The paper's algebra checks otherwise: the closed form of the auxiliary
sequence, the telescoping sum, `E[vol] = E[s]/μ + 1/μ²`, `W_exact = (1−μ) W₀`,
`O_0`, and `ε`.
-/

namespace SerqLang
namespace Papers
namespace KongPoisson

open Finset

/-! ### Harris's inequality -/

/-- Harris's inequality, weighted and finite: `f` and `g` that vary together
have `(Σ p f)(Σ p g) ≤ (Σ p)(Σ p f g)`. -/
theorem harris {ι : Type*} (s : Finset ι) (p f g : ι → ℝ) (hp : ∀ i ∈ s, 0 ≤ p i)
    (h : ∀ i ∈ s, ∀ j ∈ s, 0 ≤ (f i - f j) * (g i - g j)) :
    (∑ i ∈ s, p i * f i) * (∑ i ∈ s, p i * g i) ≤ (∑ i ∈ s, p i) * ∑ i ∈ s, p i * (f i * g i) := by
  have key : 2 * ((∑ i ∈ s, p i) * ∑ i ∈ s, p i * (f i * g i)
      - (∑ i ∈ s, p i * f i) * (∑ i ∈ s, p i * g i))
      = ∑ i ∈ s, ∑ j ∈ s, p i * p j * ((f i - f j) * (g i - g j)) := by
    have e : ∀ a b c d : ℝ, 2 * (a * b - c * d) = a * b + b * a - c * d - d * c := by
      intros; ring
    rw [e]
    simp only [Finset.sum_mul_sum, ← Finset.sum_add_distrib, ← Finset.sum_sub_distrib]
    refine Finset.sum_congr rfl fun i _ => Finset.sum_congr rfl fun j _ => ?_
    ring
  have : 0 ≤ ∑ i ∈ s, ∑ j ∈ s, p i * p j * ((f i - f j) * (g i - g j)) :=
    Finset.sum_nonneg fun i hi => Finset.sum_nonneg fun j hj =>
      mul_nonneg (mul_nonneg (hp i hi) (hp j hj)) (h i hi j hj)
  linarith

/-! ### The priority recursion -/

/-- The load of the classes before `k`, `Σ_{j<k} λ_j v_j`. -/
def load (lam v : ℕ → ℝ) (k : ℕ) : ℝ := ∑ j ∈ range k, lam j * v j

/-- The auxiliary sequence `aux k = W₀ D / ((D − load k)(D − load (k+1)))`. -/
noncomputable def aux (lam v : ℕ → ℝ) (D W0 : ℝ) (k : ℕ) : ℝ :=
  W0 * D / ((D - load lam v k) * (D - load lam v (k + 1)))

/-- The mean over classes under their arrival rates, `Σ λ_k f_k / Σ λ_k`. -/
noncomputable def mean (K : ℕ) (lam f : ℕ → ℝ) : ℝ :=
  (∑ k ∈ range K, lam k * f k) / ∑ k ∈ range K, lam k

section recursion
variable {K : ℕ} {lam v : ℕ → ℝ} {D W0 : ℝ}
  (hlam : ∀ k, 0 ≤ lam k) (hv : ∀ k, 0 ≤ v k) (hD : load lam v K < D)
include hlam hv hD

omit hD in
theorem load_mono {k m : ℕ} (h : k ≤ m) : load lam v k ≤ load lam v m :=
  Finset.sum_le_sum_of_subset_of_nonneg (range_mono h)
    fun j _ _ => mul_nonneg (hlam j) (hv j)

theorem gap_pos {k : ℕ} (h : k ≤ K) : 0 < D - load lam v k := by
  have := load_mono hlam hv h; linarith

/-- The auxiliary sequence satisfies (A) with equality, in closed form:
`W₀ + Σ_{j<k} λ_j v_j aux j = W₀ D / (D − load k)`. -/
theorem tele : ∀ k ≤ K,
    W0 + ∑ j ∈ range k, lam j * v j * aux lam v D W0 j = W0 * D / (D - load lam v k)
  | 0, h => by
    have := gap_pos hlam hv hD h
    simp only [load, range_zero, sum_empty, sub_zero, add_zero] at this ⊢
    field_simp
  | k + 1, h => by
    rw [sum_range_succ, ← add_assoc, tele k (by omega)]
    have h1 := gap_pos hlam hv hD (show k ≤ K by omega)
    have h2 := gap_pos hlam hv hD h
    have hs : load lam v (k + 1) = load lam v k + lam k * v k := sum_range_succ _ _
    unfold aux
    rw [hs] at h2 ⊢
    field_simp
    ring

/-- The induction of the paper: waits obeying (A) are bounded by `aux`. -/
theorem aux_bound (w : ℕ → ℝ)
    (hA : ∀ k < K, w k * (D - load lam v (k + 1)) ≤ W0 + ∑ j ∈ range k, lam j * v j * w j) :
    ∀ k < K, w k ≤ aux lam v D W0 k := by
  intro k
  induction k using Nat.strong_induction_on with
  | _ k ih =>
    intro hk
    have hg := gap_pos hlam hv hD (show k + 1 ≤ K by omega)
    have hg0 := gap_pos hlam hv hD (show k ≤ K by omega)
    have hsum : ∑ j ∈ range k, lam j * v j * w j ≤ ∑ j ∈ range k, lam j * v j * aux lam v D W0 j :=
      sum_le_sum fun j hj => mul_le_mul_of_nonneg_left
        (ih j (mem_range.mp hj) (by have := mem_range.mp hj; omega)) (mul_nonneg (hlam j) (hv j))
    have heq : aux lam v D W0 k * (D - load lam v (k + 1)) = W0 * D / (D - load lam v k) := by
      unfold aux; field_simp
    refine le_of_mul_le_mul_right ?_ hg
    rw [heq, ← tele hlam hv hD k (by omega)]
    linarith [hA k hk]

/-- `aux` is monotone in the class. -/
theorem aux_mono (hW0 : 0 ≤ W0) {i j : ℕ} (hij : i ≤ j) (hj : j < K) :
    aux lam v D W0 i ≤ aux lam v D W0 j := by
  have hD0 := gap_pos hlam hv hD (Nat.zero_le K)
  simp only [load, range_zero, sum_empty, sub_zero] at hD0
  have a1 := gap_pos hlam hv hD (show j ≤ K by omega)
  have a2 := gap_pos hlam hv hD (show j + 1 ≤ K by omega)
  have b1 := load_mono hlam hv hij
  have b2 := load_mono hlam hv (show i + 1 ≤ j + 1 by omega)
  exact div_le_div_of_nonneg_left (mul_nonneg hW0 hD0.le) (mul_pos a1 a2)
    (mul_le_mul (by linarith) (by linarith) a2.le (by linarith))

theorem aux_nonneg (hW0 : 0 ≤ W0) {k : ℕ} (hk : k < K) : 0 ≤ aux lam v D W0 k := by
  have hD0 := gap_pos hlam hv hD (Nat.zero_le K)
  simp only [load, range_zero, sum_empty, sub_zero] at hD0
  exact div_nonneg (mul_nonneg hW0 hD0.le)
    (mul_nonneg (gap_pos hlam hv hD hk.le).le (gap_pos hlam hv hD hk).le)

/-- The telescoping sum: `Σ_k λ_k v_k W'_k = W₀ Λ / (D − Λ)`. -/
theorem aux_sum :
    ∑ k ∈ range K, lam k * v k * aux lam v D W0 k = W0 * load lam v K / (D - load lam v K) := by
  have h := tele (W0 := W0) hlam hv hD K le_rfl
  have hg := gap_pos hlam hv hD (le_refl K)
  rw [eq_div_iff hg.ne'] at h ⊢
  linarith

end recursion

/-- The expected wait, from the balance (A) and Harris's inequality, for
classes served in ascending proxy volume `vh` with true mean volume `v` and
`v − vh ≥ Δmin`: `E[W_q] ≤ E[v] / (E[v] − ε) · W₀ / (D − Λ)`, with
`ε = E[v − vh] − Δmin`. -/
theorem wait_bound (K : ℕ) (lam v vh w : ℕ → ℝ) (D W0 Δmin : ℝ)
    (hlam : ∀ k, 0 ≤ lam k) (hv : ∀ k, 0 ≤ v k) (hL : 0 < ∑ k ∈ range K, lam k)
    (hW0 : 0 ≤ W0) (hD : load lam v K < D)
    (hsort : MonotoneOn vh (range K : Set ℕ))
    (hΔ : ∀ k < K, Δmin ≤ v k - vh k)
    (hA : ∀ k < K, w k * (D - load lam v (k + 1)) ≤ W0 + ∑ j ∈ range k, lam j * v j * w j)
    (hε : mean K lam (fun k => v k - vh k) - Δmin < mean K lam v) :
    mean K lam w ≤ mean K lam v / (mean K lam v - (mean K lam (fun k => v k - vh k) - Δmin))
      * (W0 / (D - load lam v K)) := by
  have hg : 0 < D - load lam v K := gap_pos hlam hv hD le_rfl
  have hsum := aux_sum (D := D) (W0 := W0) hlam hv hD
  set a := aux lam v D W0
  set L := ∑ k ∈ range K, lam k
  -- the waits are below `aux`
  have hwa : ∑ k ∈ range K, lam k * w k ≤ ∑ k ∈ range K, lam k * a k :=
    sum_le_sum fun k hk => mul_le_mul_of_nonneg_left
      (aux_bound hlam hv hD w hA k (mem_range.mp hk)) (hlam k)
  -- Harris on the proxy volumes and `aux`, both monotone in the class
  have hh := harris (range K) lam vh a (fun k _ => hlam k) fun i hi j hj => by
    have hi' := mem_range.mp hi; have hj' := mem_range.mp hj
    rcases le_total i j with h | h
    · have := hsort (by simpa using hi) (by simpa using hj) h
      have := aux_mono hlam hv hD hW0 h hj'
      nlinarith
    · have := hsort (by simpa using hj) (by simpa using hi) h
      have := aux_mono hlam hv hD hW0 h hi'
      nlinarith
  -- the gap `v − vh` is at least `Δmin`
  have hd : Δmin * ∑ k ∈ range K, lam k * a k ≤ ∑ k ∈ range K, lam k * ((v k - vh k) * a k) := by
    rw [mul_sum]
    refine sum_le_sum fun k hk => ?_
    have := mul_le_mul_of_nonneg_left (hΔ k (mem_range.mp hk))
      (mul_nonneg (hlam k) (aux_nonneg hlam hv hD hW0 (mem_range.mp hk)))
    linarith
  have hsplit : ∑ k ∈ range K, lam k * v k * a k
      = ∑ k ∈ range K, lam k * (vh k * a k) + ∑ k ∈ range K, lam k * ((v k - vh k) * a k) := by
    rw [← sum_add_distrib]; exact sum_congr rfl fun k _ => by ring
  -- the means, as sums over `L`
  have hmd : mean K lam (fun k => v k - vh k)
      = (∑ k ∈ range K, lam k * v k - ∑ k ∈ range K, lam k * vh k) / L := by
    unfold mean; rw [← sum_sub_distrib]; congr 1; exact sum_congr rfl fun k _ => by ring
  have hmv : mean K lam v = (∑ k ∈ range K, lam k * v k) / L := rfl
  have hmw : mean K lam w = (∑ k ∈ range K, lam k * w k) / L := rfl
  have hload : load lam v K = ∑ k ∈ range K, lam k * v k := rfl
  rw [hmd, hmv] at hε
  rw [hmd, hmv, hmw]
  rw [hload] at hg hsum ⊢
  set Sv := ∑ k ∈ range K, lam k * v k
  set Svh := ∑ k ∈ range K, lam k * vh k
  set Sa := ∑ k ∈ range K, lam k * a k
  have e1 : Sv / L - ((Sv - Svh) / L - Δmin) = (Svh + L * Δmin) / L := by
    field_simp; ring
  have hX : 0 < Svh + L * Δmin := by
    have : 0 < (Svh + L * Δmin) / L := by rw [← e1]; linarith
    exact (div_pos_iff_of_pos_right hL).mp this
  rw [e1]
  rw [show Sv / L / ((Svh + L * Δmin) / L) * (W0 / (D - Sv))
      = Sv * W0 / ((Svh + L * Δmin) * (D - Sv)) by field_simp,
    div_le_div_iff₀ hL (mul_pos hX hg)]
  -- `X · Σ λ a ≤ L · Σ λ v a = L · W₀ Λ / (D − Λ)`
  have e : (Svh + L * Δmin) * Sa ≤ L * ∑ k ∈ range K, lam k * v k * a k := by
    rw [hsplit]; nlinarith [mul_le_mul_of_nonneg_left hd hL.le]
  have hsum' : (∑ k ∈ range K, lam k * v k * a k) * (D - Sv) = W0 * Sv := by
    rw [hsum]; field_simp
  have := mul_le_mul_of_nonneg_left hwa (mul_nonneg hX.le hg.le)
  nlinarith [mul_le_mul_of_nonneg_right e hg.le]

/-! ### Geometric lengths -/

/-- The term `P(o = n+1) · f(n+1)` of `o ~ Geo(μ)` on `{1, 2, …}`. -/
noncomputable def term (μ : ℝ) (f : ℝ → ℝ) (n : ℕ) : ℝ := μ * (1 - μ) ^ n * f (n + 1)

/-- `E[f(o)]`. -/
noncomputable def geo (μ : ℝ) (f : ℝ → ℝ) : ℝ := ∑' n, term μ f n

/-- `E[f(o); o ≤ θ]`, a finite sum. -/
noncomputable def low (μ : ℝ) (θ : ℕ) (f : ℝ → ℝ) : ℝ := ∑ n ∈ range θ, term μ f n

/-- `E[f(o); o > θ]`. -/
noncomputable def tail (μ : ℝ) (θ : ℕ) (f : ℝ → ℝ) : ℝ := ∑' n, term μ f (n + θ)

/-- The conditional mean of a restricted expectation `E`, `E[o; A] / P(A)`. -/
noncomputable def cmean (E : (ℝ → ℝ) → ℝ) : ℝ := E (fun x => x) / E (fun _ => 1)

/-- The conditional variance. -/
noncomputable def cvar (E : (ℝ → ℝ) → ℝ) : ℝ := E (fun x => x ^ 2) / E (fun _ => 1) - cmean E ^ 2

/-- The penalty of the 1-bit classes, `½ (θ q^θ / (1 − q^θ))²`. -/
noncomputable def eps (μ : ℝ) (θ : ℕ) : ℝ := (θ * (1 - μ) ^ θ / (1 - (1 - μ) ^ θ)) ^ 2 / 2

section geo
variable {μ : ℝ} (h0 : 0 < μ) (h1 : μ < 1)
include h0 h1

/-- The moments up to the third, at once: `E[o] = 1/μ`, `E[o²] = (2−μ)/μ²`,
`E[o³] = (6 − 6μ + μ²)/μ³`. -/
theorem hasSum_geo (a b c d : ℝ) :
    HasSum (term μ fun x => a + b * x + c * x ^ 2 + d * x ^ 3)
      (a + b / μ + c * (2 - μ) / μ ^ 2 + d * (6 - 6 * μ + μ ^ 2) / μ ^ 3) := by
  have hq : ‖1 - μ‖ < 1 := by rw [Real.norm_eq_abs, abs_lt]; constructor <;> linarith
  have H := fun k => hasSum_choose_mul_geometric_of_norm_lt_one (𝕜 := ℝ) k hq
  have c2 : ∀ n : ℕ, (((n + 2).choose 2 : ℕ) : ℝ) = (n + 1) * (n + 2) / 2 := fun n => by
    rw [Nat.cast_choose_two]; push_cast; ring
  have c3 : ∀ n : ℕ, (((n + 3).choose 3 : ℕ) : ℝ) = (n + 1) * (n + 2) * (n + 3) / 6 :=
    fun n => by
      rw [← Nat.choose_symm_add, Nat.cast_add_choose]
      simp [Nat.factorial_succ]
      field_simp
      ring
  have := (((H 0).mul_left a).add (((H 1).mul_left (b - c + d)).add
    (((H 2).mul_left (2 * c - 6 * d)).add ((H 3).mul_left (6 * d))))).mul_left μ
  convert this using 1
  · funext n
    simp only [term, c2, c3, Nat.choose_zero_right, Nat.choose_one_right]
    push_cast
    ring
  · rw [show (1 : ℝ) - (1 - μ) = μ by ring]
    field_simp
    ring

/-- A polynomial of degree at most three, in expectation. -/
theorem geo_poly (f : ℝ → ℝ) (a b c d : ℝ) (hf : ∀ x, f x = a + b * x + c * x ^ 2 + d * x ^ 3) :
    geo μ f = a + b / μ + c * (2 - μ) / μ ^ 2 + d * (6 - 6 * μ + μ ^ 2) / μ ^ 3 := by
  rw [show f = _ from funext hf]; exact (hasSum_geo h0 h1 a b c d).tsum_eq

theorem summable_poly (f : ℝ → ℝ) (a b c d : ℝ)
    (hf : ∀ x, f x = a + b * x + c * x ^ 2 + d * x ^ 3) : Summable (term μ f) := by
  rw [show f = _ from funext hf]; exact (hasSum_geo h0 h1 a b c d).summable

omit h0 h1 in
/-- Memorylessness: past `θ`, the length is `θ` plus a fresh geometric length,
`E[f(o); o > θ] = q^θ E[f(θ + o)]`. -/
theorem memoryless (θ : ℕ) (f : ℝ → ℝ) :
    tail μ θ f = (1 - μ) ^ θ * geo μ (fun x => f (θ + x)) := by
  rw [geo, ← tsum_mul_left]
  refine tsum_congr fun n => ?_
  simp only [term]
  rw [show ((n + θ : ℕ) : ℝ) + 1 = θ + (n + 1) by push_cast; ring, pow_add]
  ring

omit h0 h1 in
/-- The low part and the tail make the whole. -/
theorem split (θ : ℕ) (f : ℝ → ℝ) (hs : Summable (term μ f)) :
    geo μ f = low μ θ f + tail μ θ f :=
  (hs.sum_add_tsum_nat_add θ).symm

theorem geo_one : geo μ (fun _ => 1) = 1 := by
  rw [geo_poly h0 h1 _ 1 0 0 0 (fun x => by ring)]; ring

theorem geo_id : geo μ (fun x => x) = 1 / μ := by
  rw [geo_poly h0 h1 _ 0 1 0 0 (fun x => by ring)]; ring

theorem geo_sq : geo μ (fun x => x ^ 2) = (2 - μ) / μ ^ 2 := by
  rw [geo_poly h0 h1 _ 0 0 1 0 (fun x => by ring)]; ring

theorem tail_one (θ : ℕ) : tail μ θ (fun _ => 1) = (1 - μ) ^ θ := by
  rw [memoryless, geo_one h0 h1, mul_one]

theorem tail_id (θ : ℕ) : tail μ θ (fun x => x) = (1 - μ) ^ θ * (θ + 1 / μ) := by
  rw [memoryless, geo_poly h0 h1 _ θ 1 0 0 (fun x => by ring)]; ring

theorem tail_sq (θ : ℕ) :
    tail μ θ (fun x => x ^ 2) = (1 - μ) ^ θ * (θ ^ 2 + 2 * θ / μ + (2 - μ) / μ ^ 2) := by
  rw [memoryless, geo_poly h0 h1 _ (θ ^ 2) (2 * θ) 1 0 (fun x => by ring)]; ring

theorem low_eq (θ : ℕ) (f : ℝ → ℝ) (a b c d : ℝ)
    (hf : ∀ x, f x = a + b * x + c * x ^ 2 + d * x ^ 3) : low μ θ f = geo μ f - tail μ θ f := by
  rw [split θ f (summable_poly h0 h1 f a b c d hf)]; ring

omit h0 in
theorem q_pos (θ : ℕ) : 0 < (1 - μ) ^ θ := pow_pos (by linarith) θ

theorem q_lt_one {θ : ℕ} (hθ : 1 ≤ θ) : (1 - μ) ^ θ < 1 :=
  pow_lt_one₀ (by linarith) (by linarith) (by omega)

/-- `E[o | o > θ] = θ + 1/μ`, the paper's long proxy `O_1`. -/
theorem cmean_tail (θ : ℕ) : cmean (tail μ θ) = θ + 1 / μ := by
  have := q_pos h1 θ
  unfold cmean; rw [tail_id h0 h1, tail_one h0 h1]; field_simp

/-- `Var(o | o > θ) = Var(o) = (1−μ)/μ²`. -/
theorem cvar_tail (θ : ℕ) : cvar (tail μ θ) = (1 - μ) / μ ^ 2 := by
  have := q_pos h1 θ
  unfold cvar; rw [cmean_tail h0 h1, tail_sq h0 h1, tail_one h0 h1]; field_simp; ring

theorem low_one (θ : ℕ) : low μ θ (fun _ => 1) = 1 - (1 - μ) ^ θ := by
  rw [low_eq h0 h1 θ _ 1 0 0 0 (fun x => by ring), geo_one h0 h1, tail_one h0 h1]

/-- `E[o | o ≤ θ] = 1/μ − θ q^θ / (1 − q^θ)`, the paper's short proxy `O_0`. -/
theorem cmean_low {θ : ℕ} (hθ : 1 ≤ θ) :
    cmean (low μ θ) = 1 / μ - θ * (1 - μ) ^ θ / (1 - (1 - μ) ^ θ) := by
  have := q_lt_one h0 h1 hθ
  have : 1 - (1 - μ) ^ θ ≠ 0 := by linarith
  unfold cmean
  rw [low_eq h0 h1 θ _ 0 1 0 0 (fun x => by ring), geo_id h0 h1, tail_id h0 h1, low_one h0 h1]
  field_simp; ring

/-- `Var(o | o ≤ θ) = Var(o) − q^θ θ² / (1 − q^θ)²`, the law of total variance. -/
theorem cvar_low {θ : ℕ} (hθ : 1 ≤ θ) :
    cvar (low μ θ) = (1 - μ) / μ ^ 2 - (1 - μ) ^ θ * θ ^ 2 / (1 - (1 - μ) ^ θ) ^ 2 := by
  have := q_lt_one h0 h1 hθ
  have : 1 - (1 - μ) ^ θ ≠ 0 := by linarith
  unfold cvar
  rw [cmean_low h0 h1 hθ, low_eq h0 h1 θ _ 0 0 1 0 (fun x => by ring), geo_sq h0 h1,
    tail_sq h0 h1, low_one h0 h1]
  field_simp; ring

/-- The penalty: with `Δ_m = Var(o | m)/2`, `p₁ = 1 − q^θ`, `p₂ = q^θ`,
`p₁ Δ₁ + p₂ Δ₂ − min Δ₁ Δ₂ = ½ (θ q^θ / (1 − q^θ))²`. -/
theorem penalty {θ : ℕ} (hθ : 1 ≤ θ) :
    (1 - (1 - μ) ^ θ) * (cvar (low μ θ) / 2) + (1 - μ) ^ θ * (cvar (tail μ θ) / 2)
      - min (cvar (low μ θ) / 2) (cvar (tail μ θ) / 2) = eps μ θ := by
  have := q_lt_one h0 h1 hθ
  have := q_pos h1 θ
  have : 1 - (1 - μ) ^ θ ≠ 0 := by linarith
  rw [cvar_low h0 h1 hθ, cvar_tail h0 h1, min_eq_left]
  · unfold eps; field_simp; ring
  · have : 0 ≤ (1 - μ) ^ θ * θ ^ 2 / (1 - (1 - μ) ^ θ) ^ 2 := by positivity
    linarith

/-- `E[vol] = E[s]/μ + 1/μ²` for `vol = s·o + (o² + o)/2`, `s` independent
of `o` (its mean `Es` taken out of the expectation over `o`). -/
theorem mean_vol (Es : ℝ) : geo μ (fun x => Es * x + (x ^ 2 + x) / 2) = Es / μ + 1 / μ ^ 2 := by
  rw [geo_poly h0 h1 _ 0 (Es + 1 / 2) (1 / 2) 0 (fun x => by ring)]; field_simp; ring

/-- Lemma 2's value, in closed form: `λ E[vol] / μ + λ / μ³ = λ (E[s]/μ² + 2/μ³)`
(the lemma itself, `W₀ = E[U_run]`, is the premise `hW0`). -/
theorem w0_closed (lt Es : ℝ) :
    lt * geo μ (fun x => Es * x + (x ^ 2 + x) / 2) / μ + lt / μ ^ 3
      = lt * (Es / μ ^ 2 + 2 / μ ^ 3) := by
  rw [mean_vol h0 h1]; field_simp; ring

/-- `E[s (o² − o)/2 + (o³ − o)/3] = (1 − μ)(E[s]/μ² + 2/μ³)`, so
`W_exact = (1 − μ) W₀`. -/
theorem w_exact (Es : ℝ) :
    geo μ (fun x => Es * (x ^ 2 - x) / 2 + (x ^ 3 - x) / 3)
      = (1 - μ) * (Es / μ ^ 2 + 2 / μ ^ 3) := by
  rw [geo_poly h0 h1 _ 0 (-Es / 2 - 1 / 3) (Es / 2) (1 / 3) (fun x => by ring)]
  field_simp; ring

end geo

/-! ### Theorems 3.4 and 3.5 -/

/-- The last step of both theorems: an SVF wait at most `B · W₀/(D − Λ)` and
the two lower bounds on OPT give `E[SVF]/E[OPT] ≤ 1 + B · 2/((1−α)(1−μ)(1−ρ))`. -/
theorem ratio (α M μ Λ W0 B Ew Eo Esvf Eopt ρ : ℝ) (hα : α < 1) (hM : 0 < M) (hμ : μ < 1)
    (hΛ : 0 ≤ Λ) (hD : Λ < (1 - α) * M / 2) (hρ : ρ = Λ / ((1 - α) * M / 2)) (hB : 0 ≤ B)
    (hw : Ew ≤ B * (W0 / ((1 - α) * M / 2 - Λ))) (hsvf : Esvf ≤ Eo + Ew) (hEo : 0 < Eo)
    (ho : Eo ≤ Eopt) (hx : (1 - μ) * W0 / M ≤ Eopt) :
    Esvf / Eopt ≤ 1 + B * (2 / ((1 - α) * (1 - μ) * (1 - ρ))) := by
  have hg : 0 < (1 - α) * M / 2 - Λ := by linarith
  have hD0 : 0 < (1 - α) * M / 2 := by linarith
  have hμ' : 0 < 1 - μ := by linarith
  have hopt : 0 < Eopt := by linarith
  have hc : 2 / ((1 - α) * (1 - μ) * (1 - ρ)) = M / ((1 - μ) * ((1 - α) * M / 2 - Λ)) := by
    have : 1 - α ≠ 0 := by linarith
    rw [hρ]; field_simp
  have hW : W0 / ((1 - α) * M / 2 - Λ) ≤ M / ((1 - μ) * ((1 - α) * M / 2 - Λ)) * Eopt := by
    rw [div_le_iff₀ hM] at hx
    rw [show M / ((1 - μ) * ((1 - α) * M / 2 - Λ)) * Eopt
        = M * Eopt / (1 - μ) / ((1 - α) * M / 2 - Λ) by field_simp]
    refine (div_le_div_iff_of_pos_right hg).mpr ?_
    rw [le_div_iff₀ hμ']
    linarith
  rw [hc, div_le_iff₀ hopt]
  have := mul_le_mul_of_nonneg_left hW hB
  nlinarith

/-- **Theorem 3.4** (SVF under Poisson arrivals, geometric lengths):
`E[SVF] / E[OPT] ≤ 1 + 2 / ((1−α)(1−μ)(1−ρ))`.

Classes `k < K` are served in ascending volume `v` (`hsort`), arrive at rates
`lam`, and wait `w k` on average. The premises are the paper's steady-state
steps: the balance (A) at the rate `(1−α)M/2` of Lemma 1 (`hA`), Lemma 2's
value of the residual work `W₀` (`hW0`), the latency of SVF as decode plus
wait (`hsvf`), and Lemma `opt_poisson_lb` (`hopt`). `Es` is the mean prompt,
independent of the output length. -/
theorem svf_poisson (K : ℕ) (lam v w : ℕ → ℝ) (α M μ Es W0 Esvf Eopt ρ : ℝ)
    (hα : α < 1) (hM : 0 < M) (hμ0 : 0 < μ) (hμ1 : μ < 1) (hEs : 0 ≤ Es)
    (hlam : ∀ k, 0 ≤ lam k) (hv : ∀ k, 0 ≤ v k) (hL : 0 < ∑ k ∈ range K, lam k)
    (hEv : 0 < mean K lam v) (hsort : MonotoneOn v (range K : Set ℕ))
    (hstable : load lam v K < (1 - α) * M / 2)
    (hρ : ρ = load lam v K / ((1 - α) * M / 2))
    (hW0 : W0 = (∑ k ∈ range K, lam k) * geo μ (fun x => Es * x + (x ^ 2 + x) / 2) / μ
      + (∑ k ∈ range K, lam k) / μ ^ 3)
    (hA : ∀ k < K, w k * ((1 - α) * M / 2 - load lam v (k + 1))
      ≤ W0 + ∑ j ∈ range k, lam j * v j * w j)
    (hsvf : Esvf ≤ geo μ (fun x => x) + mean K lam w)
    (hopt : max (geo μ (fun x => x))
      ((∑ k ∈ range K, lam k) * geo μ (fun x => Es * (x ^ 2 - x) / 2 + (x ^ 3 - x) / 3) / M)
      ≤ Eopt) :
    Esvf / Eopt ≤ 1 + 2 / ((1 - α) * (1 - μ) * (1 - ρ)) := by
  rw [w0_closed hμ0 hμ1] at hW0
  have hW0n : 0 ≤ W0 := by rw [hW0]; positivity
  have hwait := wait_bound K lam v v w _ W0 0 hlam hv hL hW0n hstable hsort
    (fun k _ => by simp) hA (by simpa [mean] using hEv)
  have hz : mean K lam (fun k => v k - v k) = 0 := by simp [mean]
  rw [hz, sub_zero, sub_zero, div_self hEv.ne'] at hwait
  rw [w_exact hμ0 hμ1, geo_id hμ0 hμ1] at hopt
  rw [geo_id hμ0 hμ1] at hsvf
  have h := ratio α M μ (load lam v K) W0 1 _ _ _ Eopt ρ hα hM hμ1
    (sum_nonneg fun k _ => mul_nonneg (hlam k) (hv k)) hstable hρ zero_le_one hwait hsvf
    (by positivity) (le_of_max_le_left hopt)
    (by rw [hW0]; convert le_of_max_le_right hopt using 2; ring)
  simpa using h

/-- **Theorem 3.5** (1-bit SVF under Poisson arrivals):
`E[SVF₁] / E[OPT] ≤ 1 + E[v] / (E[v] − ε) · 2 / ((1−α)(1−μ)(1−ρ))`, with
`ε = ½ (θ q^θ / (1 − q^θ))²` (`eps`).

Class `k` has prompt `s k` and is long (`o > θ`) or short; its proxy volume
`vh k` uses the proxy length `O = E[o | class]` (`hvh`), its true mean volume
`v k` is the conditional mean of `s·o + (o²+o)/2` (`hv`), and the long classes
carry the share `P(o > θ) = q^θ` of the arrivals (`hmix`). The classes are
served in ascending `vh` (`hsort`). The other premises are those of
`svf_poisson`, and `E[v] > ε` (`hε`). -/
theorem svf_1bit_poisson (K θ : ℕ) (hθ : 1 ≤ θ) (lam s v vh w : ℕ → ℝ) (long : ℕ → Bool)
    (α M μ Es W0 Esvf Eopt ρ : ℝ)
    (hα : α < 1) (hM : 0 < M) (hμ0 : 0 < μ) (hμ1 : μ < 1) (hEs : 0 ≤ Es)
    (hlam : ∀ k, 0 ≤ lam k) (hv0 : ∀ k, 0 ≤ v k) (hL : 0 < ∑ k ∈ range K, lam k)
    (hv : ∀ k < K, v k = s k * cmean (if long k then tail μ θ else low μ θ)
      + ((if long k then tail μ θ else low μ θ) (fun x => x ^ 2)
          / (if long k then tail μ θ else low μ θ) (fun _ => 1)
        + cmean (if long k then tail μ θ else low μ θ)) / 2)
    (hvh : ∀ k < K, vh k = s k * cmean (if long k then tail μ θ else low μ θ)
      + (cmean (if long k then tail μ θ else low μ θ) ^ 2
        + cmean (if long k then tail μ θ else low μ θ)) / 2)
    (hmix : ∑ k ∈ range K, (if long k then lam k else 0) = (1 - μ) ^ θ * ∑ k ∈ range K, lam k)
    (hsort : MonotoneOn vh (range K : Set ℕ))
    (hε : eps μ θ < mean K lam v)
    (hstable : load lam v K < (1 - α) * M / 2)
    (hρ : ρ = load lam v K / ((1 - α) * M / 2))
    (hW0 : W0 = (∑ k ∈ range K, lam k) * geo μ (fun x => Es * x + (x ^ 2 + x) / 2) / μ
      + (∑ k ∈ range K, lam k) / μ ^ 3)
    (hA : ∀ k < K, w k * ((1 - α) * M / 2 - load lam v (k + 1))
      ≤ W0 + ∑ j ∈ range k, lam j * v j * w j)
    (hsvf : Esvf ≤ geo μ (fun x => x) + mean K lam w)
    (hopt : max (geo μ (fun x => x))
      ((∑ k ∈ range K, lam k) * geo μ (fun x => Es * (x ^ 2 - x) / 2 + (x ^ 3 - x) / 3) / M)
      ≤ Eopt) :
    Esvf / Eopt ≤ 1 + mean K lam v / (mean K lam v - eps μ θ)
      * (2 / ((1 - α) * (1 - μ) * (1 - ρ))) := by
  set V1 := cvar (low μ θ) / 2
  set V2 := cvar (tail μ θ) / 2
  -- the gap of a class is half its conditional variance
  have hgap : ∀ k < K, v k - vh k = if long k then V2 else V1 := by
    intro k hk
    rw [hv k hk, hvh k hk]
    cases long k <;> simp [V1, V2, cvar] <;> ring
  have hΔ : ∀ k < K, min V1 V2 ≤ v k - vh k := by
    intro k hk; rw [hgap k hk]; split
    · exact min_le_right _ _
    · exact min_le_left _ _
  -- the mixture of the two classes
  have hmean : mean K lam (fun k => v k - vh k) = (1 - (1 - μ) ^ θ) * V1 + (1 - μ) ^ θ * V2 := by
    have hs : ∑ k ∈ range K, lam k * (v k - vh k)
        = ∑ k ∈ range K, lam k * (if long k then V2 else V1) :=
      sum_congr rfl fun k hk => by rw [hgap k (mem_range.mp hk)]
    show (∑ k ∈ range K, lam k * (v k - vh k)) / _ = _
    rw [hs]
    have e : ∀ k, lam k * (if long k then V2 else V1)
        = (if long k then lam k else 0) * V2 + (lam k - if long k then lam k else 0) * V1 := by
      intro k; split <;> ring
    simp only [e, sum_add_distrib, ← sum_mul, sum_sub_distrib, hmix]
    field_simp
    ring
  have hpen : mean K lam (fun k => v k - vh k) - min V1 V2 = eps μ θ := by
    rw [hmean]; exact penalty hμ0 hμ1 hθ
  rw [w0_closed hμ0 hμ1] at hW0
  have hW0n : 0 ≤ W0 := by rw [hW0]; positivity
  have hwait := wait_bound K lam v vh w _ W0 (min V1 V2) hlam hv0 hL hW0n hstable hsort hΔ hA
    (by rw [hpen]; exact hε)
  rw [hpen] at hwait
  rw [w_exact hμ0 hμ1, geo_id hμ0 hμ1] at hopt
  rw [geo_id hμ0 hμ1] at hsvf
  have heps : 0 ≤ eps μ θ := by unfold eps; positivity
  exact ratio α M μ (load lam v K) W0 _ _ _ _ Eopt ρ hα hM hμ1
    (sum_nonneg fun k _ => mul_nonneg (hlam k) (hv0 k)) hstable hρ
    (div_nonneg (by linarith) (by linarith)) hwait hsvf
    (by positivity) (le_of_max_le_left hopt)
    (by rw [hW0]; convert le_of_max_le_right hopt using 2; ring)

end KongPoisson
end Papers
end SerqLang
