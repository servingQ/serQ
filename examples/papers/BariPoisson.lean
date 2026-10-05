/-
# Bari et al.: Foster's drift for Poisson arrivals in continuous time

`BariStable` draws a slot's arrivals from a distribution fixed in advance.
Here requests arrive as a Poisson stream of rate `λ` per clock unit, each
of a type `t` drawn independently with probability `q t`, of (prompt,
output) lengths `len t` (`Mix`). On a busy engine a slot brings the requests
that arrive while the running iteration lasts: a Poisson number of mean
`λ · dur`, a compound Poisson list (`Serq/Poisson.lean`); on an idle engine
it brings the next request.

Outside `F` RAD's batch is full, and a full batch lasts
`t_{Lin} + 128 t_{nl} = 4000 + 5 · 128 = 4640` (`dur_full`). The drift is

  `E[V'] = V − 128 + λ · 4640 · E[v_p + v_d]`  (Wald's identity),

so `λ · 4640 · E[v_p + v_d] < 128` is Theorem 1's bound, `λ E[v_p + v_d] <
b_col / t_{batch}`, for one node, with nothing assumed about the shorter
slots inside `F`. This is the step of Theorem 2 that `BariStable` proves for
a fixed distribution. Then, as `BariProgram` for a fixed distribution, the
engine empties in bounded expected time from every state (`hit_idle_le`)
and the empty machines are a positive recurrent atom (`return_idle`).
-/
import Serq.Poisson
import papers.BariProgram

namespace SerqLang
namespace Papers
namespace BariPoisson

open Exec Foster BariStable Poisson

/-- The requests' types: type `t` has lengths `len t` and probability `q t`. -/
structure Mix (n : ℕ) where
  len : Fin n → ℕ × ℕ
  fits : ∀ t, Fits (len t)
  q : Fin n → ℝ
  nonneg : ∀ t, 0 ≤ q t
  sum_one : ∑ t, q t = 1

variable {n : ℕ} (X : Mix n)

/-- A request's mean tokens, `E[v_p + v_d]`. -/
def Mix.work : ℝ := ∑ t, X.q t * (((X.len t).1 + (X.len t).2 : ℕ) : ℝ)

/-- The requests of an outcome: `k` of them, of types `s.2`. -/
def arrivals (s : Outcome (Fin n)) : List (ℕ × ℕ) := List.ofFn fun i => X.len (s.2 i)

theorem arrivals_fit (s : Outcome (Fin n)) : ∀ r ∈ arrivals X s, Fits r := by
  intro r hr
  obtain ⟨i, rfl⟩ := List.mem_ofFn.mp hr
  exact X.fits _

theorem work_arrivals (s : Outcome (Fin n)) :
    ((((arrivals X s).map fun r => r.1 + r.2).sum : ℕ) : ℝ) =
      ∑ i, (((X.len (s.2 i)).1 + (X.len (s.2 i)).2 : ℕ) : ℝ) := by
  simp [arrivals, List.map_ofFn, List.sum_ofFn, Function.comp_def]

/-- A reached machine's running iteration lasts its cost. -/
theorem reach_dur {m : Machine} (h : Reach m) : Slot.Dur D m := by
  induction h with
  | empty => intro a q h; simp [BariStable.empty, Slot.empty, Exec.initial] at h
  | @slot m rs hf hm _ =>
    obtain ⟨L, g, hB⟩ := reach_sb hm
    exact Slot.slot_dur ci_start hB _ (fits_attrs hf)

/-- A full batch lasts `4640`. -/
theorem dur_full (x : State) (hx : ¬ F x) : Slot.dur x.1 = 4640 := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  simp only [F, not_or, not_lt] at hx
  obtain ⟨⟨a, q⟩, hie⟩ := Option.ne_none_iff_exists'.mp hx.1
  have hbusy : x.1.iterEnd.isSome = true := by rw [hie]; rfl
  obtain ⟨hb1, -⟩ := hB.busy hbusy
  have htok : x.1.last.stats.tokens = 128 := by have : tokSum x.1.iter ≤ 128 := hB.tok; omega
  have ha := reach_dur x.2 a q hie
  simp only [Slot.dur, hie]
  rw [ha]
  simp [D, Claims.BariRad.deployment, htok]

/-- What a slot does to the backlog. -/
theorem backlog_slot' {m : Machine} (h : Reach m) (rs : List (ℕ × ℕ)) (hf : ∀ r ∈ rs, Fits r) :
    backlog (slot rs m) + (if m.iterEnd.isSome then tokSum m.iter else 0) =
      backlog m + (rs.map fun r => r.1 + r.2).sum := by
  obtain ⟨L, g, hB⟩ := reach_sb h
  obtain ⟨L', g', s1, s2, s3⟩ := slot_sb hB rs hf
  unfold BariStable.backlog
  rw [Slot.ci_backlog s1.toCI s1.rdy, Slot.ci_backlog hB.toCI hB.rdy, s2]
  exact s3

/-- How many requests a slot brings: Poisson of mean `λ · dur` on a busy
engine, one on an idle one. -/
noncomputable def count (lam : ℝ) (x : State) : ℕ → ℝ :=
  if x.1.iterEnd.isSome then pois (lam * Slot.dur x.1) else fun k => if k = 1 then 1 else 0

variable {lam : ℝ} (hlam : 0 ≤ lam)
include hlam

theorem count_nonneg (x : State) (k : ℕ) : 0 ≤ count lam x k := by
  unfold count
  by_cases hb : x.1.iterEnd.isSome = true
  · simp only [hb, if_true]; exact pois_nonneg (by positivity) k
  · simp only [hb, Bool.false_eq_true, if_false]; split_ifs <;> norm_num

theorem hasSum_count (x : State) : HasSum (count lam x) 1 := by
  unfold count; split_ifs
  · exact hasSum_pois (by positivity)
  · exact hasSum_ite_eq 1 1

theorem hasSum_count_mean (x : State) :
    HasSum (fun k : ℕ => (k : ℝ) * count lam x k)
      (if x.1.iterEnd.isSome then lam * Slot.dur x.1 else 1) := by
  unfold count; split_ifs
  · exact hasSum_mul_pois (by positivity)
  · convert hasSum_ite_eq (1 : ℕ) (1 : ℝ) using 1
    funext k; split_ifs with h <;> simp [h]

omit hlam in
/-- The chain: a slot brings a compound Poisson list of requests. -/
noncomputable def kernel (hlam : 0 ≤ lam) : Kernel State (Outcome (Fin n)) where
  p x s := compound (count lam x) X.q s
  next x s := ⟨slot (arrivals X s) x.1, Reach.slot _ (arrivals_fit X s) x.2⟩
  nonneg x s := compound_nonneg (count_nonneg hlam x) X.nonneg s
  sum_one x := hasSum_compound (count_nonneg hlam x) (hasSum_count hlam x) X.nonneg X.sum_one

/-- The work at the engine. -/
def V (x : State) : ℝ := backlog x.1

omit hlam in
theorem V_next (x : State) (s : Outcome (Fin n)) :
    V ⟨slot (arrivals X s) x.1, Reach.slot _ (arrivals_fit X s) x.2⟩ +
      (if x.1.iterEnd.isSome then (tokSum x.1.iter : ℝ) else 0) =
      V x + ∑ i, (((X.len (s.2 i)).1 + (X.len (s.2 i)).2 : ℕ) : ℝ) := by
  have := backlog_slot' x.2 (arrivals X s) (arrivals_fit X s)
  unfold V
  rw [← work_arrivals]
  split_ifs at this ⊢ <;> exact_mod_cast this

/-- The expectation of `V` after a slot, term by term. -/
theorem hasSum_V (x : State) :
    HasSum (fun s => compound (count lam x) X.q s * (V x + ∑ i, (((X.len (s.2 i)).1 + (X.len (s.2 i)).2 : ℕ) : ℝ)))
      (V x + (if x.1.iterEnd.isSome then lam * Slot.dur x.1 else 1) * X.work) := by
  have h1 := (hasSum_compound (count_nonneg hlam x) (hasSum_count hlam x) X.nonneg X.sum_one).mul_right (V x)
  have h2 := hasSum_compound_work (count_nonneg hlam x) (hasSum_count_mean hlam x) X.nonneg X.sum_one
    (fun t => (((X.len t).1 + (X.len t).2 : ℕ) : ℝ)) (fun t => Nat.cast_nonneg _)
  convert h1.add h2 using 1
  · funext s; ring
  · unfold Mix.work; ring

theorem integrable : (kernel X hlam).Integrable V := by
  intro x
  refine Summable.of_nonneg_of_le (fun s => mul_nonneg ((kernel X hlam).nonneg x s) (Nat.cast_nonneg _))
    (fun s => mul_le_mul_of_nonneg_left ?_ ((kernel X hlam).nonneg x s)) (hasSum_V X hlam x).summable
  have := V_next X x s
  have h0 : (0 : ℝ) ≤ if x.1.iterEnd.isSome then (tokSum x.1.iter : ℝ) else 0 := by
    split_ifs <;> positivity
  show V ⟨slot (arrivals X s) x.1, _⟩ ≤ _
  linarith

/-- The drift: `ε = 128 − λ · 4640 · E[v_p + v_d]`. -/
def ε (lam : ℝ) : ℝ := 128 - lam * 4640 * X.work

/-- Foster's drift condition, below Theorem 1's bound. -/
theorem drift (hA : lam * 4640 * X.work < 128) : Drift (kernel X hlam) F V (ε X lam) where
  nonneg _ := Nat.cast_nonneg _
  integrable := integrable X hlam
  pos := by unfold ε; linarith
  drift x hx := by
    obtain ⟨L, g, hB⟩ := reach_sb x.2
    have hF := hx
    simp only [F, not_or, not_lt] at hF
    have hbusy : x.1.iterEnd.isSome = true := Option.isSome_iff_ne_none.mpr hF.1
    have h128 : tokSum x.1.iter = 128 := by
      obtain ⟨hb1, -⟩ := hB.busy hbusy
      have : tokSum x.1.iter ≤ 128 := hB.tok; omega
    have hH := hasSum_V X hlam x
    rw [if_pos hbusy, dur_full x hx] at hH
    have hE : (kernel X hlam).apply V x = V x + lam * (4640 : ℕ) * X.work - 128 := by
      unfold Kernel.apply
      have h128' := (hasSum_compound (count_nonneg hlam x) (hasSum_count hlam x) X.nonneg X.sum_one).mul_right
        (128 : ℝ)
      rw [show V x + lam * ((4640 : ℕ) : ℝ) * X.work - 128 =
        V x + lam * ((4640 : ℕ) : ℝ) * X.work - 1 * 128 by ring, ← (hH.sub h128').tsum_eq]
      refine tsum_congr fun s => ?_
      have := V_next X x s
      rw [if_pos hbusy, h128] at this
      show compound (count lam x) X.q s * V _ = _
      push_cast at this ⊢
      rw [← mul_sub]
      congr 1
      rw [show (kernel X hlam).next x s =
        ⟨slot (arrivals X s) x.1, Reach.slot _ (arrivals_fit X s) x.2⟩ from rfl]
      linarith
    rw [hE]
    unfold ε; push_cast; linarith

/-- Foster's drift for Poisson arrivals: from every state, the expected number of
slots until the batch is not full (or the engine idle) is at most
`backlog / ε`. -/
theorem hitTime_le (hA : lam * 4640 * X.work < 128) (x : State) :
    ε X lam * hitTime (kernel X hlam) F x ≤ backlog x.1 :=
  Foster.hitTime_le (drift X hlam hA) x

/-- … and it is finite: the truncations converge to it. -/
theorem hit_tendsto (hA : lam * 4640 * X.work < 128) (x : State) :
    Filter.Tendsto (fun n => hit (kernel X hlam) F n x) Filter.atTop
      (nhds (hitTime (kernel X hlam) F x)) :=
  Foster.hit_tendsto (drift X hlam hA) x

/-- From every state of `F`, the expected return time to `F` is finite. -/
theorem returnTime_le (hA : lam * 4640 * X.work < 128) (x : State) (hx : F x) :
    returnTime (kernel X hlam) F x ≤ 1 + (kernel X hlam).apply V x / ε X lam :=
  Foster.returnTime_le_of_drift (drift X hlam hA) x hx

/-- Not vacuous: one request of one tile makes a full batch, a state
outside `F` … -/
example : ¬ F ⟨slot [(128, 1)] empty,
    Reach.slot _ (by intro r hr; simp at hr; subst hr; exact ⟨by decide, le_rfl, by decide, le_rfl, by decide⟩)
      .empty⟩ := by
  have h : (slot [(128, 1)] empty).iterEnd.isSome = true ∧
      (slot [(128, 1)] empty).last.stats.tokens = 128 := by
    decide +kernel
  intro hF
  change (slot [(128, 1)] empty).iterEnd = none ∨ (slot [(128, 1)] empty).last.stats.tokens < 128 at hF
  rcases hF with hF | hF
  · rw [hF] at h; exact absurd h.1 (by decide)
  · omega

/-- … and one type, a tile of prompt and one output token, at one
request per 10 000 clock units is below the bound (`4640 · 129 / 10⁴ ≈ 60`). -/
example : ∃ X : Mix 1, (1 / 10000 : ℝ) * 4640 * X.work < 128 :=
  ⟨⟨fun _ => (128, 1), fun _ => ⟨by decide, le_rfl, by decide, le_rfl, by decide⟩, fun _ => 1,
      fun _ => zero_le_one, by simp⟩, by simp [Mix.work]; norm_num⟩

/-! ### The empty engine recurs

As in `DaiPoisson`: what recurs is the engine being empty
(`BariProgram.Idle`). On a busy engine a slot brings no request with
probability `e^{-λ · dur} ≥ e^{-4640 λ}` (`p0_ge`) and then serves at least
one token, so from a state of backlog `b` the empty engine is `b`
arrival-free slots away (`drain`); a slot adds `(λ · 4640 + 1) E[v_p + v_d]`
tokens at most in expectation (`apply_V_le`). -/

open BariProgram (Idle)

omit hlam in
/-- An engine with a job is busy. -/
theorem busy_of_not_idle {x : State} (hx : ¬ Idle x) : x.1.iterEnd.isSome = true := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  rcases hie : x.1.iterEnd with _ | ⟨a, q⟩
  · exact absurd (by unfold Idle BariSim.σ; rw [hB.idle hie]; rfl) hx
  · rfl

omit hlam in
/-- No iteration lasts longer than a full batch, `4640`. -/
theorem dur_le (x : State) : Slot.dur x.1 ≤ 4640 := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  rcases hie : x.1.iterEnd with _ | ⟨a, q⟩
  · simp [Slot.dur, hie]
  · have hbusy : x.1.iterEnd.isSome = true := by rw [hie]; rfl
    obtain ⟨hb1, -⟩ := hB.busy hbusy
    have ht : x.1.last.stats.tokens ≤ 128 := by have : tokSum x.1.iter ≤ 128 := hB.tok; omega
    have ha := reach_dur x.2 a q hie
    simp only [Slot.dur, hie]
    rw [ha]
    simp only [D, Claims.BariRad.deployment]
    omega

/-- No request. -/
def none0 : Outcome (Fin n) := ⟨0, Fin.elim0⟩

omit hlam in
theorem arrivals_none0 : arrivals X (none0 (n := n)) = [] := by simp [arrivals, none0]

/-- On a busy engine a slot brings no request with probability at least
`e^{-4640 λ}`. -/
theorem p0_ge {x : State} (hb : x.1.iterEnd.isSome = true) :
    Real.exp (-(lam * 4640)) ≤ (kernel X hlam).p x none0 := by
  have hd : (Slot.dur x.1 : ℝ) ≤ 4640 := by exact_mod_cast dur_le x
  simp only [kernel, compound, count, none0, hb, if_true, pois, Finset.univ_eq_empty,
    Finset.prod_empty, pow_zero, Nat.factorial_zero, Nat.cast_one, div_one, mul_one]
  exact Real.exp_le_exp.mpr (by nlinarith)

/-- Without arrivals the engine drains: from a backlog `≤ j`, it is empty
within `j` slots with probability at least `e^{-4640 λ j}`. -/
theorem drain : ∀ (j : ℕ) (x : State), backlog x.1 ≤ j →
    Real.exp (-(lam * 4640)) ^ j ≤ reach (kernel X hlam) Idle j x
  | 0, x, hb => by
    have hx : Idle x := by
      by_contra h
      have := BariRecurrent.backlog_pos _ (BariProgram.good_σ x.2) h
      rw [BariProgram.backlog_eq] at hb; omega
    rw [reach_of_mem _ Idle hx]; simp
  | j + 1, x, hb => by
    have hq1 : Real.exp (-(lam * 4640)) ≤ 1 := Real.exp_le_one_iff.mpr (by nlinarith)
    by_cases hx : Idle x
    · rw [reach_of_mem _ Idle hx]; exact pow_le_one₀ (Real.exp_nonneg _) hq1
    · have hb' : backlog ((kernel X hlam).next x none0).1 ≤ j := by
        show backlog (slot (arrivals X none0) x.1) ≤ j
        have h1 := BariRecurrent.absSlot_backlog [] (BariSim.σ x.1)
        rw [← BariSim.simulation x [] (by simp), ← BariProgram.backlog_eq,
          ← BariProgram.backlog_eq] at h1
        have h2 := (BariRecurrent.sS_cases _ (BariProgram.good_σ x.2) hx).1
        rw [arrivals_none0]
        simp only [List.map_nil, List.sum_nil] at h1
        omega
      have ih := drain j _ hb'
      have h3 := (kernel X hlam).le_apply (reach_integrable _ Idle j) (reach_nonneg _ Idle j) x none0
      have h4 := p0_ge X hlam (busy_of_not_idle hx)
      show _ ≤ (if Idle x then 1 else (kernel X hlam).apply (reach (kernel X hlam) Idle j) x)
      rw [if_neg hx, pow_succ]
      calc Real.exp (-(lam * 4640)) ^ j * Real.exp (-(lam * 4640))
          ≤ reach (kernel X hlam) Idle j ((kernel X hlam).next x none0) * (kernel X hlam).p x none0 :=
            mul_le_mul ih h4 (Real.exp_nonneg _) (reach_nonneg _ _ _ _)
        _ = (kernel X hlam).p x none0 * reach (kernel X hlam) Idle j ((kernel X hlam).next x none0) :=
            mul_comm _ _
        _ ≤ _ := h3

omit hlam in
theorem work_nonneg : 0 ≤ X.work :=
  Finset.sum_nonneg fun t _ => mul_nonneg (X.nonneg t) (Nat.cast_nonneg _)

/-- A slot adds at most `(λ · 4640 + 1) E[v_p + v_d]` tokens in expectation. -/
theorem apply_V_le (x : State) : (kernel X hlam).apply V x ≤ V x + (lam * 4640 + 1) * X.work := by
  have hH := hasSum_V X hlam x
  have h1 : (kernel X hlam).apply V x ≤
      V x + (if x.1.iterEnd.isSome then lam * Slot.dur x.1 else 1) * X.work := by
    unfold Kernel.apply
    rw [← hH.tsum_eq]
    refine (integrable X hlam x).tsum_le_tsum (fun s => mul_le_mul_of_nonneg_left ?_
      ((kernel X hlam).nonneg x s)) hH.summable
    have := V_next X x s
    have h0 : (0 : ℝ) ≤ if x.1.iterEnd.isSome then (tokSum x.1.iter : ℝ) else 0 := by
      split_ifs <;> positivity
    show V ⟨slot (arrivals X s) x.1, _⟩ ≤ _
    linarith
  have h2 : (if x.1.iterEnd.isSome then lam * Slot.dur x.1 else 1) ≤ lam * 4640 + 1 := by
    have hd : (Slot.dur x.1 : ℝ) ≤ 4640 := by exact_mod_cast dur_le x
    split_ifs <;> nlinarith
  nlinarith [work_nonneg X]

/-- The truncated hitting times of the empty engine, bounded. -/
theorem hit_le (hA : lam * 4640 * X.work < 128) :
    ∃ c : ℝ, ∀ j x, hit (kernel X hlam) Idle j x ≤ V x / ε X lam + c := by
  have hV := (kernel X hlam).applyN_le_of_apply_le (integrable X hlam) (fun _ => Nat.cast_nonneg _)
    (apply_V_le X hlam)
  exact ⟨_, hit_le_of_reach (kernel X hlam) Idle (fun j => (hV j).1) (drift X hlam hA)
    BariProgram.F_of_idle (L := 128 * 512) (B := 128 * 512 + (lam * 4640 + 1) * X.work * (128 * 512))
    (pow_pos (Real.exp_pos _) _)
    (fun x hx => drain X hlam _ x (backlog_lt_of_F x hx).le)
    (fun x hx => by
      have h1 := (hV (128 * 512)).2 x
      have h2 : V x ≤ 128 * 512 := by unfold V; exact_mod_cast (backlog_lt_of_F x hx).le
      push_cast at h1 ⊢
      linarith)⟩

/-- Below Theorem 1's bound, from every state the engine empties in bounded
expected time. -/
theorem hit_idle_le (hA : lam * 4640 * X.work < 128) :
    ∃ W : State → ℝ, ∀ j x, hit (kernel X hlam) Idle j x ≤ W x := by
  obtain ⟨c, hc⟩ := hit_le X hlam hA
  exact ⟨_, hc⟩

/-- Theorem 2 for Poisson arrivals: below Theorem 1's bound, from every
machine with an empty engine the expected time until it is empty again is
bounded by one constant, so the empty engine is a positive recurrent atom. -/
theorem return_idle (hA : lam * 4640 * X.work < 128) :
    ∃ C : ℝ, ∀ j x, Idle x → 1 + (kernel X hlam).apply (hit (kernel X hlam) Idle j) x ≤ C := by
  obtain ⟨c, hc⟩ := hit_le X hlam hA
  have hε : 0 < ε X lam := (drift X hlam hA).pos
  have hVI := integrable X hlam
  refine ⟨1 + (1 / ε X lam) * ((lam * 4640 + 1) * X.work) + c, fun j x hx => ?_⟩
  have h1 : ∀ y, hit (kernel X hlam) Idle j y ≤ (1 / ε X lam) * V y + c := fun y => by
    have := hc j y
    rw [one_div, ← div_eq_inv_mul]; exact this
  have h2 := (kernel X hlam).apply_mono (hit_integrable _ _ j)
    ((hVI.const_mul _).add ((kernel X hlam).integrable_const c)) h1 x
  rw [(kernel X hlam).apply_add (hVI.const_mul _) ((kernel X hlam).integrable_const c),
    Kernel.apply_const, Kernel.apply_const_mul] at h2
  have h3 := apply_V_le X hlam x
  have h0 : V x = 0 := by
    unfold V
    rw [BariProgram.backlog_eq, show BariSim.σ x.1 = [] from hx]
    simp [BariChain.backlog]
  rw [h0] at h3
  have h4 : (1 / ε X lam) * (kernel X hlam).apply V x ≤ (1 / ε X lam) * ((lam * 4640 + 1) * X.work) :=
    mul_le_mul_of_nonneg_left (by linarith) (by positivity)
  linarith

end BariPoisson
end Papers
end SerqLang
