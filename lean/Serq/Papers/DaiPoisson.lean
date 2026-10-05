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
distribution: the expected time to reach `F` is finite. Positive
recurrence (`DaiRecurrent`) is not yet proved for this chain.
-/
import Serq.Poisson
import Serq.Papers.DaiStable

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

end DaiPoisson
end Papers
end SerqLang
