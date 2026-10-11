/-
# Dai et al.: Foster's drift for Poisson arrivals in continuous time

`DaiStable` draws a slot's arrivals from a distribution fixed in advance.
Here they are those of a Poisson stream of rate `λ` per clock unit: on a busy
engine, the requests that arrive while the running iteration lasts, a
Poisson number of mean `λ · dur` (`dur` the iteration's length, its `cost`),
joining at its end; on an idle engine, the next request, which starts an
iteration at once. So a slot's arrivals depend on the state, are
unbounded, and `Exec.drain` runs them all (its fuel grows with the ready
list).

Outside `F` the batch is full, and a full batch lasts
`t_{b_max} = max 1 (1128 + 3547) = 4675` (`dur_full`). The drift is then

  `E[V'] = V - 128 + 1280 · λ · t_{b_max}`,

so `1280 λ t_{b_max} < 128` is Theorem 2(b)'s `λ (v_p + v_d) < b_max / t_{b_max}`
exactly, with nothing assumed about slots inside `F`, which are shorter.
This is the step of Theorem 2(b) that `DaiStable` proves for a fixed
distribution: the expected time to reach `F` is finite.

Then, as `DaiProgram` for a fixed distribution, the engine empties in
bounded expected time from every state (`hit_idle_le`) and the empty
machines are a positive recurrent atom (`return_idle`).
-/
import Serq.Poisson
import papers.DaiProgram

namespace SerqLang
namespace Papers
namespace DaiPoisson

open Exec Foster DaiStable Poisson

/-- The machines reached from `empty` by slots of any number of arrivals. -/
inductive Reach : Machine → Prop
  | empty : Reach empty
  | slot {m : Machine} (k : ℕ) : Reach m → Reach (slot k m)

/-- The chain's states. -/
def State : Type := {m : Machine // Reach m}

theorem reach_sb {m : Machine} (h : Reach m) : ∃ L g, SB L g m := by
  induction h with
  | empty => exact ⟨_, _, empty_sb⟩
  | slot k _ ih =>
    obtain ⟨L, g, hB⟩ := ih
    obtain ⟨L', g', h1, -⟩ := DaiStable.slot_sb hB k
    exact ⟨L', g', h1⟩

/-- A reached machine's running iteration lasts its cost. -/
theorem reach_dur {m : Machine} (h : Reach m) : Slot.Dur D m := by
  induction h with
  | empty => intro a q h; simp [DaiStable.empty, Slot.empty, Exec.initial] at h
  | @slot m k hm _ =>
    obtain ⟨L, g, hB⟩ := reach_sb hm
    rw [slot_eq]
    exact Slot.slot_dur ci_start hB _ fun _ _ => rfl

/-- The engine is idle, or its batch is not full. -/
def F (x : State) : Prop := x.1.iterEnd = none ∨ x.1.last.stats.tokens < 128

instance : DecidablePred F := fun x => by unfold F; infer_instance

/-- A full batch lasts `t_{b_max} = 4675`. -/
theorem dur_full (x : State) (hx : ¬ F x) : Slot.dur x.1 = 4675 := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  simp only [F, not_or, not_lt] at hx
  obtain ⟨⟨a, q⟩, hie⟩ := Option.ne_none_iff_exists'.mp hx.1
  have hbusy : x.1.iterEnd.isSome = true := by rw [hie]; rfl
  obtain ⟨hb1, -⟩ := hB.busy hbusy
  have htok : x.1.last.stats.tokens = 128 := by have : tokSum x.1.iter ≤ 128 := hB.tok; omega
  have ha := reach_dur x.2 a q hie
  simp only [Slot.dur, hie]
  rw [ha]
  simp [D, Claims.DaiSarathi.deployment, htok]

/-- What a slot does to the backlog: its arrivals' tokens join, the running
batch's leave. -/
theorem backlog_slot {m : Machine} (h : Reach m) (k : ℕ) :
    backlog (slot k m) + (if m.iterEnd.isSome then tokSum m.iter else 0) = backlog m + 1280 * k := by
  obtain ⟨L, g, hB⟩ := reach_sb h
  obtain ⟨L', g', s1, s2, s3⟩ := DaiStable.slot_sb hB k
  unfold DaiStable.backlog
  rw [Slot.ci_backlog s1.toCI s1.rdy, Slot.ci_backlog hB.toCI hB.rdy, s2]
  exact s3

/-- The chain: on a busy engine, the arrivals of a Poisson stream of rate
`λ` while the iteration lasts; on an idle one, the next arrival. -/
noncomputable def kernel (lam : ℝ) (hlam : 0 ≤ lam) : Kernel State ℕ where
  p x k := if x.1.iterEnd.isSome then pois (lam * Slot.dur x.1) k else if k = 1 then 1 else 0
  next x k := ⟨slot k x.1, .slot k x.2⟩
  nonneg x k := by
    split_ifs
    · exact pois_nonneg (by positivity) k
    · norm_num
    · norm_num
  sum_one x := by
    split_ifs
    · exact hasSum_pois (by positivity)
    · exact hasSum_ite_eq 1 1

/-- The work at the engine. -/
def V (x : State) : ℝ := backlog x.1

theorem V_slot_le (x : State) (k : ℕ) : V ⟨slot k x.1, .slot k x.2⟩ ≤ V x + 1280 * k := by
  have := backlog_slot x.2 k
  have : backlog (slot k x.1) ≤ backlog x.1 + 1280 * k := by omega
  unfold V; exact_mod_cast this

theorem integrable (lam : ℝ) (hlam : 0 ≤ lam) : (kernel lam hlam).Integrable V := by
  intro x
  refine Summable.of_nonneg_of_le (fun k => mul_nonneg ((kernel lam hlam).nonneg x k) (Nat.cast_nonneg _))
    (fun k => mul_le_mul_of_nonneg_left (V_slot_le x k) ((kernel lam hlam).nonneg x k)) ?_
  show Summable fun k => (kernel lam hlam).p x k * (V x + 1280 * k)
  by_cases hb : x.1.iterEnd.isSome = true
  · have h0 : 0 ≤ lam * Slot.dur x.1 := by positivity
    refine (((hasSum_pois h0).mul_right (V x)).add ((hasSum_mul_pois h0).mul_left 1280)).summable.congr
      fun k => ?_
    simp only [kernel, hb, if_true]; ring
  · refine summable_of_ne_finset_zero (s := {1}) fun k hk => ?_
    simp only [kernel, hb, Bool.false_eq_true, if_false]
    rw [if_neg (by simpa using hk), zero_mul]

/-- The drift: `ε = 128 − 1280 · λ · t_{b_max}`. -/
def ε (lam : ℝ) : ℝ := 128 - 1280 * (lam * 4675)

/-- Foster's drift condition, below the paper's capacity. -/
theorem drift (lam : ℝ) (hlam : 0 ≤ lam) (hA : 1280 * (lam * 4675) < 128) :
    Drift (kernel lam hlam) F V (ε lam) where
  nonneg _ := Nat.cast_nonneg _
  integrable := integrable lam hlam
  pos := by unfold ε; linarith
  drift x hx := by
    obtain ⟨L, g, hB⟩ := reach_sb x.2
    have hF := hx
    simp only [F, not_or, not_lt] at hF
    have hbusy : x.1.iterEnd.isSome = true := Option.isSome_iff_ne_none.mpr hF.1
    have h128 : tokSum x.1.iter = 128 := by
      obtain ⟨hb1, -⟩ := hB.busy hbusy
      have : tokSum x.1.iter ≤ 128 := hB.tok; omega
    have hk : ∀ k, V ⟨slot k x.1, .slot k x.2⟩ = V x + 1280 * k - 128 := by
      intro k
      have := backlog_slot x.2 k
      rw [if_pos hbusy, h128] at this
      unfold V
      have : (backlog (slot k x.1) : ℝ) + 128 = backlog x.1 + 1280 * k := by exact_mod_cast this
      linarith
    have h0 : (0 : ℝ) ≤ lam * 4675 := by positivity
    have hH : HasSum (fun k => pois (lam * 4675) k * V ⟨slot k x.1, .slot k x.2⟩)
        (V x + 1280 * (lam * 4675) - 128) := by
      convert (((hasSum_pois h0).mul_right (V x - 128)).add ((hasSum_mul_pois h0).mul_left 1280)) using 1
      · funext k; rw [hk]; ring
      · ring
    have happ : (kernel lam hlam).apply V x = ∑' k, pois (lam * 4675) k * V ⟨slot k x.1, .slot k x.2⟩ := by
      unfold Kernel.apply
      refine tsum_congr fun k => ?_
      simp only [kernel, hbusy, if_true, dur_full x hx]
      push_cast; rfl
    rw [happ, hH.tsum_eq]
    unfold ε; linarith

/-- Foster's drift for Poisson arrivals: from every state, the expected number
of slots until the batch is not full (or the engine idle) is at most
`backlog / ε`. -/
theorem hitTime_le (lam : ℝ) (hlam : 0 ≤ lam) (hA : 1280 * (lam * 4675) < 128) (x : State) :
    ε lam * hitTime (kernel lam hlam) F x ≤ backlog x.1 :=
  Foster.hitTime_le (drift lam hlam hA) x

/-- … and it is finite: the truncations converge to it. -/
theorem hit_tendsto (lam : ℝ) (hlam : 0 ≤ lam) (hA : 1280 * (lam * 4675) < 128) (x : State) :
    Filter.Tendsto (fun n => hit (kernel lam hlam) F n x) Filter.atTop
      (nhds (hitTime (kernel lam hlam) F x)) :=
  Foster.hit_tendsto (drift lam hlam hA) x

/-- From every state of `F`, the expected return time to `F` is finite. -/
theorem returnTime_le (lam : ℝ) (hlam : 0 ≤ lam) (hA : 1280 * (lam * 4675) < 128) (x : State) (hx : F x) :
    returnTime (kernel lam hlam) F x ≤ 1 + (kernel lam hlam).apply V x / ε lam :=
  Foster.returnTime_le_of_drift (drift lam hlam hA) x hx

/-- Not vacuous: one request makes a full batch, and a rate of one request
per 100 000 clock units is below capacity (`1280 · 4675 / 10⁵ ≈ 60 < 128`). -/
example : ¬ F ⟨slot 1 empty, .slot 1 .empty⟩ ∧ 1280 * ((1 / 100000 : ℝ) * 4675) < 128 := by
  refine ⟨?_, by norm_num⟩
  have h : (slot 1 empty).iterEnd.isSome = true ∧ (slot 1 empty).last.stats.tokens = 128 := by
    decide +kernel
  intro hF
  change (slot 1 empty).iterEnd = none ∨ (slot 1 empty).last.stats.tokens < 128 at hF
  rcases hF with hF | hF
  · rw [hF] at h; exact absurd h.1 (by decide)
  · omega

/-! ### The empty engine recurs

The machine chain never revisits a state (`DaiProgram`); what recurs is the
engine being empty. On a busy engine a slot brings no request with
probability `e^{-λ · dur} ≥ e^{-λ t_{b_max}}` (`p0_ge`), and then serves at
least one token, so from a state of backlog `b` the empty engine is `b`
arrival-free slots away (`drain`). A slot adds `1280 · E[k] ≤ 1280 (λ
t_{b_max} + 1)` tokens in expectation (`apply_V_le`), so the backlog's
expectations after any number of slots converge. -/

/-- The engine holds no job. -/
def Idle (x : State) : Prop := DaiSim.σ x.1 = []

instance : DecidablePred Idle := fun x => by unfold Idle; infer_instance

theorem reach_inv {m : Machine} (h : Reach m) : DaiSim.Inv m := by
  induction h with
  | empty => exact DaiSim.empty_inv
  | slot k _ ih => exact (DaiSim.slot_inv ih k).1

/-- One slot on the job list (`DaiSim.simulation`). -/
theorem σ_slot (x : State) (k : ℕ) :
    DaiSim.σ (slot k x.1) = DaiChain.absSlot k (DaiSim.σ x.1) :=
  (DaiSim.slot_inv (reach_inv x.2) k).2

theorem good_σ {m : Machine} (h : Reach m) : DaiRecurrent.Good (DaiSim.σ m) := by
  induction h with
  | empty => rw [DaiSim.σ_empty]; intro j hj; simp at hj
  | @slot m k hm ih =>
    rw [σ_slot ⟨m, hm⟩ k]
    exact DaiRecurrent.good_absSlot k ih

theorem V_idle {x : State} (hx : Idle x) : V x = 0 := by
  unfold V
  rw [DaiProgram.backlog_σ, show DaiSim.σ x.1 = [] from hx]
  simp [DaiChain.backlog]

/-- An engine with a job is busy. -/
theorem busy_of_not_idle {x : State} (hx : ¬ Idle x) : x.1.iterEnd.isSome = true := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  rcases hie : x.1.iterEnd with _ | ⟨a, q⟩
  · exact absurd (by unfold Idle DaiSim.σ; rw [hB.idle hie]; rfl) hx
  · rfl

/-- No iteration lasts longer than a full batch, `t_{b_max} = 4675`. -/
theorem dur_le (x : State) : Slot.dur x.1 ≤ 4675 := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  rcases hie : x.1.iterEnd with _ | ⟨a, q⟩
  · simp [Slot.dur, hie]
  · have hbusy : x.1.iterEnd.isSome = true := by rw [hie]; rfl
    obtain ⟨hb1, -⟩ := hB.busy hbusy
    have ht : x.1.last.stats.tokens ≤ 128 := by have : tokSum x.1.iter ≤ 128 := hB.tok; omega
    have ha := reach_dur x.2 a q hie
    simp only [Slot.dur, hie]
    rw [ha]
    simp only [D, Claims.DaiSarathi.deployment]
    omega

/-- On a busy engine a slot brings no request with probability at least
`e^{-λ t_{b_max}}`. -/
theorem p0_ge (lam : ℝ) (hlam : 0 ≤ lam) {x : State} (hb : x.1.iterEnd.isSome = true) :
    Real.exp (-(lam * 4675)) ≤ (kernel lam hlam).p x 0 := by
  have hd : (Slot.dur x.1 : ℝ) ≤ 4675 := by exact_mod_cast dur_le x
  simp only [kernel, hb, if_true, pois, pow_zero, Nat.factorial_zero, Nat.cast_one, div_one, mul_one]
  exact Real.exp_le_exp.mpr (by nlinarith)

/-- Without arrivals the engine drains: from a backlog `≤ n`, it is empty
within `n` slots with probability at least `e^{-λ t_{b_max} n}`. -/
theorem drain (lam : ℝ) (hlam : 0 ≤ lam) : ∀ (n : ℕ) (x : State), backlog x.1 ≤ n →
    Real.exp (-(lam * 4675)) ^ n ≤ reach (kernel lam hlam) Idle n x
  | 0, x, hb => by
    have hx : Idle x := by
      by_contra h
      have := DaiRecurrent.backlog_pos (good_σ x.2) h
      rw [DaiProgram.backlog_σ] at hb; omega
    rw [reach_of_mem _ Idle hx]; simp
  | n + 1, x, hb => by
    have hq1 : Real.exp (-(lam * 4675)) ≤ 1 := Real.exp_le_one_iff.mpr (by nlinarith)
    by_cases hx : Idle x
    · rw [reach_of_mem _ Idle hx]; exact pow_le_one₀ (Real.exp_nonneg _) hq1
    · have hb' : backlog (slot 0 x.1) ≤ n := by
        rw [DaiProgram.backlog_σ, σ_slot x 0]
        have h1 := DaiRecurrent.backlog_absSlot 0 hx
        have h2 := DaiRecurrent.one_le_shares (good_σ x.2) hx
        rw [DaiProgram.backlog_σ] at hb
        omega
      have ih := drain lam hlam n ⟨slot 0 x.1, .slot 0 x.2⟩ hb'
      have h3 := (kernel lam hlam).le_apply (reach_integrable _ Idle n) (reach_nonneg _ Idle n) x 0
      have h4 := p0_ge lam hlam (busy_of_not_idle hx)
      show _ ≤ (if Idle x then 1 else (kernel lam hlam).apply (reach (kernel lam hlam) Idle n) x)
      rw [if_neg hx, pow_succ]
      calc Real.exp (-(lam * 4675)) ^ n * Real.exp (-(lam * 4675))
          ≤ reach (kernel lam hlam) Idle n ((kernel lam hlam).next x 0) * (kernel lam hlam).p x 0 :=
            mul_le_mul ih h4 (Real.exp_nonneg _) (reach_nonneg _ _ _ _)
        _ = (kernel lam hlam).p x 0 * reach (kernel lam hlam) Idle n ((kernel lam hlam).next x 0) :=
            mul_comm _ _
        _ ≤ _ := h3

/-- A slot's mean number of arrivals: `λ · dur` on a busy engine, one on an
idle one. -/
noncomputable def mean (lam : ℝ) (x : State) : ℝ :=
  if x.1.iterEnd.isSome then lam * Slot.dur x.1 else 1

theorem hasSum_bound (lam : ℝ) (hlam : 0 ≤ lam) (x : State) :
    HasSum (fun k : ℕ => (kernel lam hlam).p x k * (V x + 1280 * k)) (V x + 1280 * mean lam x) := by
  by_cases hb : x.1.iterEnd.isSome = true
  · have h0 : 0 ≤ lam * Slot.dur x.1 := by positivity
    convert ((hasSum_pois h0).mul_right (V x)).add ((hasSum_mul_pois h0).mul_left 1280) using 1
    · funext k; simp only [kernel, hb, if_true]; ring
    · simp [mean, hb]
  · convert hasSum_ite_eq (1 : ℕ) (V x + 1280) using 1
    · funext k
      simp only [kernel, hb, Bool.false_eq_true, if_false]
      split_ifs with h <;> simp [h]
    · simp [mean, hb]

/-- A slot adds at most `1280 (λ t_{b_max} + 1)` tokens in expectation. -/
theorem apply_V_le (lam : ℝ) (hlam : 0 ≤ lam) (x : State) :
    (kernel lam hlam).apply V x ≤ V x + 1280 * (lam * 4675 + 1) := by
  have h1 : (kernel lam hlam).apply V x ≤ V x + 1280 * mean lam x := by
    unfold Kernel.apply
    rw [← (hasSum_bound lam hlam x).tsum_eq]
    exact (integrable lam hlam x).tsum_le_tsum
      (fun k => mul_le_mul_of_nonneg_left (V_slot_le x k) ((kernel lam hlam).nonneg x k))
      (hasSum_bound lam hlam x).summable
  have h2 : mean lam x ≤ lam * 4675 + 1 := by
    have hd : (Slot.dur x.1 : ℝ) ≤ 4675 := by exact_mod_cast dur_le x
    unfold mean
    split_ifs <;> nlinarith
  linarith

/-- `F`, or the engine empty. -/
def G (x : State) : Prop := F x ∨ Idle x

instance : DecidablePred G := fun x => by unfold G; infer_instance

theorem driftG (lam : ℝ) (hlam : 0 ≤ lam) (hA : 1280 * (lam * 4675) < 128) :
    Drift (kernel lam hlam) G V (ε lam) :=
  let hD := drift lam hlam hA
  ⟨hD.nonneg, hD.integrable, hD.pos, fun x hx => hD.drift x (fun h => hx (Or.inl h))⟩

theorem backlog_le_of_G {x : State} (hx : G x) : backlog x.1 ≤ 128 * 1280 := by
  rcases hx with h | h
  · obtain ⟨L, g, hB⟩ := reach_sb x.2
    exact (DaiStable.backlog_lt_of_sb hB h).le
  · have := V_idle h
    unfold V at this
    have : backlog x.1 = 0 := by exact_mod_cast this
    omega

/-- The truncated hitting times of the empty engine, bounded. -/
theorem hit_le (lam : ℝ) (hlam : 0 ≤ lam) (hA : 1280 * (lam * 4675) < 128) :
    ∃ c : ℝ, ∀ n x, hit (kernel lam hlam) Idle n x ≤ V x / ε lam + c := by
  have hV := (kernel lam hlam).applyN_le_of_apply_le (integrable lam hlam) (fun _ => Nat.cast_nonneg _)
    (apply_V_le lam hlam)
  exact ⟨_, hit_le_of_reach (kernel lam hlam) Idle (fun n => (hV n).1) (driftG lam hlam hA)
    (fun _ h => Or.inr h) (L := 128 * 1280) (B := 128 * 1280 + 1280 * (lam * 4675 + 1) * (128 * 1280))
    (pow_pos (Real.exp_pos _) _)
    (fun x hx => drain lam hlam _ x (backlog_le_of_G hx))
    (fun x hx => by
      have h1 := (hV (128 * 1280)).2 x
      have h2 : V x ≤ 128 * 1280 := by unfold V; exact_mod_cast backlog_le_of_G hx
      push_cast at h1 ⊢
      linarith)⟩

/-- Below capacity, from every state the engine empties in bounded expected
time. -/
theorem hit_idle_le (lam : ℝ) (hlam : 0 ≤ lam) (hA : 1280 * (lam * 4675) < 128) :
    ∃ W : State → ℝ, ∀ n x, hit (kernel lam hlam) Idle n x ≤ W x := by
  obtain ⟨c, hc⟩ := hit_le lam hlam hA
  exact ⟨_, hc⟩

/-- Theorem 2(b) for Poisson arrivals: below capacity, from every machine
with an empty engine the expected time until it is empty again is bounded by
one constant, so the empty engine is a positive recurrent atom. -/
theorem return_idle (lam : ℝ) (hlam : 0 ≤ lam) (hA : 1280 * (lam * 4675) < 128) :
    ∃ C : ℝ, ∀ n x, Idle x → 1 + (kernel lam hlam).apply (hit (kernel lam hlam) Idle n) x ≤ C := by
  obtain ⟨c, hc⟩ := hit_le lam hlam hA
  have hε : 0 < ε lam := (drift lam hlam hA).pos
  refine ⟨_, fun n x hx => return_le_of_hit_le (kernel lam hlam) Idle (integrable lam hlam)
    (a := 1 / ε lam) (by positivity) (fun n y => by rw [one_div, ← div_eq_inv_mul]; exact hc n y)
    (d := 1280 * (lam * 4675 + 1)) (by have := apply_V_le lam hlam x; rw [V_idle hx] at this; linarith) n⟩

end DaiPoisson
end Papers
end SerqLang
