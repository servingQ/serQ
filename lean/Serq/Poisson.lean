/-
# Poisson arrivals in a slot

Requests arriving as a Poisson stream of rate `λ` bring, during an
iteration of `t` clock units, a Poisson number of requests of mean `λ t`
(`pois`), and, when each request's type is drawn independently with
probabilities `q`, a compound Poisson list. These are the arrival laws of
the slot chains with continuous-time arrivals (`Serq/Papers/DaiPoisson.lean`,
`Serq/Papers/BariPoisson.lean`); a slot's outcomes are then infinitely
many, which `Foster.Kernel` allows.

* `hasSum_pois`, `hasSum_mul_pois`: the law sums to one, its mean is `λ t`;
* `hasSum_compound`, `hasSum_compound_work`: a compound Poisson list (a
  count `k`, then `k` types) sums to one, and the work it brings has mean
  `λ t · Σ_j q j · w j` (Wald's identity for a Poisson count).
-/
import Mathlib.Probability.Distributions.Poisson.Basic

namespace SerqLang.Poisson

open Finset

/-- The Poisson law of mean `r`. -/
noncomputable def pois (r : ℝ) (k : ℕ) : ℝ := Real.exp (-r) * r ^ k / k.factorial

variable {r : ℝ}

theorem pois_nonneg (hr : 0 ≤ r) (k : ℕ) : 0 ≤ pois r k := by unfold pois; positivity

theorem hasSum_pois (hr : 0 ≤ r) : HasSum (pois r) 1 :=
  ProbabilityTheory.hasSum_one_poissonMeasure ⟨r, hr⟩

/-- The mean is `r`. -/
theorem hasSum_mul_pois (hr : 0 ≤ r) : HasSum (fun k : ℕ => (k : ℝ) * pois r k) r := by
  have hs : (fun k : ℕ => ((k + 1 : ℕ) : ℝ) * pois r (k + 1)) = fun k => r * pois r k := by
    funext k
    unfold pois
    rw [Nat.factorial_succ, pow_succ]
    push_cast
    field_simp
  have h1 : HasSum (fun k : ℕ => ((k + 1 : ℕ) : ℝ) * pois r (k + 1)) r := by
    rw [hs]; simpa using (hasSum_pois hr).mul_left r
  have h2 : HasSum (fun k : ℕ => (fun n : ℕ => (n : ℝ) * pois r n) (k + 1))
      (r + ∑ i ∈ range 1, (i : ℝ) * pois r i - ∑ i ∈ range 1, (i : ℝ) * pois r i) := by
    simpa using h1
  have := (hasSum_nat_add_iff' 1).mp h2
  simpa using this

variable {T : Type*} [Fintype T]

/-- `Σ_{v : Fin k → T} Π_i f i (v i) = Π_i Σ_t f i t`. -/
theorem sum_prod_fn (k : ℕ) (f : Fin k → T → ℝ) :
    ∑ v : Fin k → T, ∏ i, f i (v i) = ∏ i, ∑ t, f i t := by
  rw [Finset.prod_univ_sum, Fintype.piFinset_univ]

/-- A compound list: `k` requests, of types `v`. -/
abbrev Outcome (T : Type*) := Σ k : ℕ, Fin k → T

/-- The compound law: count `c k`, each type `q` independently. -/
noncomputable def compound (c : ℕ → ℝ) (q : T → ℝ) (s : Outcome T) : ℝ := c s.1 * ∏ i, q (s.2 i)

variable {c : ℕ → ℝ} {q : T → ℝ}

omit [Fintype T] in
theorem compound_nonneg (hc : ∀ k, 0 ≤ c k) (hq : ∀ t, 0 ≤ q t) (s : Outcome T) : 0 ≤ compound c q s :=
  mul_nonneg (hc _) (prod_nonneg fun i _ => hq _)

/-- A nonnegative family on `Outcome T` whose fibre over `k` sums to `g k`,
`g` summing to `a`, sums to `a`. -/
theorem hasSum_outcome {f : Outcome T → ℝ} (hf : ∀ s, 0 ≤ f s) {g : ℕ → ℝ} {a : ℝ}
    (hfib : ∀ k, ∑ v : Fin k → T, f ⟨k, v⟩ = g k) (hg : HasSum g a) : HasSum f a := by
  have hs : Summable f := by
    refine (summable_sigma_of_nonneg hf).mpr ⟨fun k => (hasSum_fintype _).summable, ?_⟩
    simpa [tsum_fintype, hfib] using hg.summable
  convert hs.hasSum using 1
  rw [hs.tsum_sigma]
  simp only [tsum_fintype, hfib]
  exact hg.tsum_eq.symm

theorem hasSum_compound (hc : ∀ k, 0 ≤ c k) (hc1 : HasSum c 1) (hq : ∀ t, 0 ≤ q t) (hq1 : ∑ t, q t = 1) :
    HasSum (compound c q) 1 :=
  hasSum_outcome (compound_nonneg hc hq) (fun k => by
    simp only [compound, ← mul_sum, sum_prod_fn k fun _ => q, hq1, prod_const_one, mul_one]) hc1

/-- The types of a list sum to `k` times the mean, under the product law. -/
theorem sum_prod_mul_sum (hq1 : ∑ t, q t = 1) (w : T → ℝ) (k : ℕ) :
    ∑ v : Fin k → T, (∏ i, q (v i)) * ∑ i, w (v i) = k * ∑ t, q t * w t := by
  simp only [mul_sum]
  rw [sum_comm]
  have hi : ∀ i : Fin k, ∑ v : Fin k → T, (∏ j, q (v j)) * w (v i) = ∑ t, q t * w t := by
    intro i
    have := sum_prod_fn k fun j t => if j = i then q t * w t else q t
    rw [prod_eq_single i (fun j _ hj => by simp [hj, hq1]) (by simp)] at this
    simp only [if_pos] at this
    rw [← this]
    refine sum_congr rfl fun v _ => ?_
    rw [← prod_mul_prod_compl ({i} : Finset (Fin k)), ← prod_mul_prod_compl ({i} : Finset (Fin k))
      (fun j => if j = i then q (v j) * w (v j) else q (v j))]
    simp only [prod_singleton, if_pos]
    rw [prod_congr rfl fun j hj => if_neg (by simpa using hj)]
    ring
  simp [hi, mul_sum]

/-- Wald's identity: the work a compound list brings has mean `μ · E[w]`,
`μ` the mean count. -/
theorem hasSum_compound_work (hc : ∀ k, 0 ≤ c k) {μ : ℝ} (hcm : HasSum (fun k : ℕ => (k : ℝ) * c k) μ)
    (hq : ∀ t, 0 ≤ q t) (hq1 : ∑ t, q t = 1) (w : T → ℝ) (hw : ∀ t, 0 ≤ w t) :
    HasSum (fun s : Outcome T => compound c q s * ∑ i, w (s.2 i)) (μ * ∑ t, q t * w t) := by
  refine hasSum_outcome (fun s => mul_nonneg (compound_nonneg hc hq s) (sum_nonneg fun i _ => hw _))
    (g := fun k => (k : ℝ) * c k * ∑ t, q t * w t) (fun k => ?_) (hcm.mul_right _)
  simp only [compound, mul_assoc, ← mul_sum, sum_prod_mul_sum hq1 w k]
  ring

end SerqLang.Poisson
