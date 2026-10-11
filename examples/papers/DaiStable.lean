/-
# Dai et al., Theorem 2(b), for random arrivals

The Sarathi program of `examples/papers/dai_sarathi.sq`, with its sessions
arriving at random instead of one gap apart: in each slot (one iteration of
the engine) `k ≤ K` requests arrive with probability `p k`, independently
of the past (`Serq/Chain.lean`). The states are the machines this reaches
from the empty one.

Theorem 2(b) of the paper: a work-conserving scheduler is stable whenever
the load is below the engine's capacity. Here the load is
`λ (v_p + v_d) = 1280 · Σ_k k p_k` tokens per slot, and the capacity is
`b_max = 128` tokens per slot (one iteration). Below it, the work at the
engine `V` (`backlog`) has a negative drift whenever the batch is full:

  `E[V'] = V - 128 + 1280 · Σ_k k p_k ≤ V - ε`,  `ε = 128 - 1280 · Σ_k k p_k`,

and a batch that is not full is one the residents could not fill
(`Exec.work_conserving`), the set `F`. Foster's criterion
(`Serq/Foster.lean`) then bounds the expected time to reach `F` by `V / ε`
from every state (`hitTime_le`), and the expected return time to `F` from
every state of `F` (`returnTime_le`). The invariant of a slot's end is
`Serq/Slot.lean`'s, with every request of lengths `(290, 990)`; what is
Sarathi's own is its batch, the greedy fill (`ci_start`).
-/
import Serq.Slot
import Serq.Work
import Serq.Claims

namespace SerqLang
namespace Papers
namespace DaiStable

open Exec Foster Slot

/-- The deployment of `dai_sarathi.sq`. -/
abbrev D : Deployment := Claims.DaiSarathi.deployment

/-- `dai_sarathi.sq` once a request has arrived: a prompt of 290 tokens and
an output of 990, for every request. -/
abbrev M : Slot.Model where
  D := D
  admit _ := by simp [admitAll, Claims.DaiSarathi.deployment]
  len _ := (290, 990)
  len_set _ _ := rfl
  Fits r := r = (290, 990)
  u := 1
  fits r h := by subst h; exact ⟨by norm_num, one_dvd _, by norm_num⟩
  B := 128

/-- What a session runs once it has arrived: `dai_sarathi.sq`'s session
without its arrival delay (`run 1 (x.attr 10)`). -/
def arrived : Prog :=
  match Claims.DaiSarathi.prog with
  | .run _ _ _ _ k => k
  | p => p

theorem arrived_eq : arrived = Q1 M := rfl

/-- No session, the engine idle. -/
def empty : Machine := Slot.empty M

/-- One slot with `k` arrivals. -/
def slot (k : ℕ) (m : Machine) : Machine := Exec.slot D (Q1 M) (fun _ => 0) k m

theorem slot_eq (k : ℕ) (m : Machine) :
    slot k m = slotL M.D (Q1 M) (List.replicate k (fun _ => 0)) m :=
  Exec.slot_eq_slotL D _ _ k m

/-- The machines reached from `empty` by slots of at most `K` arrivals. -/
inductive Reach (K : ℕ) : Machine → Prop
  | empty : Reach K empty
  | slot {m : Machine} (k : ℕ) : k ≤ K → Reach K m → Reach K (slot k m)

/-- The chain's states. -/
def State (K : ℕ) : Type := {m : Machine // Reach K m}

/-- The tokens the engine still has to serve: a prefill's left work and the
decode it will then run, a decode's left work. At an iteration's start the
running batch is not yet subtracted. -/
abbrev backlog (m : Machine) : ℕ := Slot.backlog M m

/-- The engine is idle, or its batch is not full. -/
def F {K : ℕ} (x : State K) : Prop := x.1.iterEnd = none ∨ x.1.last.stats.tokens < 128

instance {K : ℕ} : DecidablePred (F (K := K)) := fun x => by unfold F; infer_instance

/-- The arrival distribution: `p k` for `k ≤ K`. -/
structure Arrivals (K : ℕ) where
  p : ℕ → ℝ
  nonneg : ∀ k, 0 ≤ p k
  sum_one : ∑ k ∈ Finset.range (K + 1), p k = 1

/-- The mean number of arrivals per slot. -/
def Arrivals.mean {K : ℕ} (A : Arrivals K) : ℝ := ∑ k ∈ Finset.range (K + 1), A.p k * k

/-- The chain: a slot with `k` arrivals, `k` drawn from `A`. -/
noncomputable def kernel {K : ℕ} (A : Arrivals K) : Kernel (State K) ℕ :=
  Kernel.ofOutcomes K A.p A.nonneg A.sum_one fun x k =>
    if hk : k ≤ K then ⟨slot k x.1, Reach.slot k hk x.2⟩ else x

/-- The drift: `ε = 128 - 1280 · mean`. -/
def ε {K : ℕ} (A : Arrivals K) : ℝ := 128 - 1280 * A.mean

/-! ### Sarathi's batch -/

/-- What a request wants of an iteration: the rest of its prompt, or one
decode token. -/
def wantG (g : Ghost) (i : ℕ) : ℕ :=
  match g.c i with
  | .p => g.left i
  | .d => 1
  | _ => 0

/-- A running iteration is the greedy fill of the residents. -/
abbrev Busy (_ : ℕ → ℕ × ℕ) (g : Ghost) (m : Machine) : Prop :=
  m.last.stats.tokens = tokSum m.iter ∧ tokSum m.iter = min 128 (∑ i ∈ Finset.range m.sess.size, wantG g i)

/-- The invariant of a slot's end. -/
abbrev SB := Slot.SB M Busy

/-- Every request has Sarathi's lengths. -/
theorem len_of {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {i : ℕ} (hi : i < m.sess.size) :
    L i = (290, 990) :=
  (hI.sess i hi).2.2.2

theorem ci_want_job {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {j : Job} (hj : j ∈ m.jobs) :
    wantOf D j = wantG g j.owner := by
  obtain ⟨h1, h2, -, h4⟩ := hI.jobs j hj
  rcases h2 with ⟨hc, hm⟩ | ⟨hc, hm⟩
  · simp [wantOf, wantG, hm, hc, h4, Claims.DaiSarathi.deployment]
  · have := (hI.leftD _ h1 hc).1
    simp only [wantOf, wantG, hm, hc, h4]; omega

theorem ci_demand {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) :
    (m.jobs.map (wantOf D)).sum = ∑ i ∈ Finset.range m.sess.size, wantG g i := by
  rw [← ci_sum_jobs hI (wantG g) fun i _ h => ?_]
  · congr 1; exact List.map_congr_left fun j hj => ci_want_job hI hj
  · unfold wantG; split <;> simp_all [isJob]

/-- **An iteration starts** on an idle, settled engine: the greedy fill of
the residents, or nothing if there is none. -/
theorem ci_start : Starts M Busy := by
  intro L g m hI hr _
  show SB L g (startIteration D m) ∧ (startIteration D m).sess.size = m.sess.size
  have hq : engineQueuesEmpty D m := fun p hp => by simp [pdef, Claims.DaiSarathi.deployment] at hp
  have hg : ∀ j ∈ m.jobs, j.growing = none := fun j hj => (hI.jobs j hj).2.2.1
  have ha := assign_eq_fillIter D rfl m hq hg m.preempts (m.jobs.length + 100000) 0 128 [] (by omega)
  simp only [List.drop_zero, List.nil_append] at ha
  have hvia : ((List.range D.pools.length).any fun p =>
      (pdef D p).viaEngine && !(pst m p).queue.isEmpty) = false := by
    simp [Claims.DaiSarathi.deployment]
  have hb : D.budget = 128 := rfl
  unfold startIteration
  -- the deployment has no `chunkAt`: every iteration runs it as it is
  simp only [iterDeployment_of_none _ (rfl : Claims.DaiSarathi.deployment.chunkAt = none)]
  rw [hvia, Bool.or_false]
  by_cases hjs : m.jobs = []
  · have he0 : m.jobs.isEmpty = true := by simp [hjs]
    simp only [he0, Bool.not_true, Bool.false_eq_true, ↓reduceIte]
    exact ⟨⟨{ hI with
        iterNone := fun _ => rfl
        iterOwn := fun e he => by simp at he
        share := fun i hi => by simp [shareOf]
        shareP := fun i hi _ => by simp [shareOf]
        tok := by simp [tokSum] }, hr, fun _ => hjs, fun h => by simp at h⟩, by first | rfl | trivial⟩
  · obtain ⟨j0, js, hjs'⟩ := List.exists_cons_of_ne_nil hjs
    have hne0 : m.jobs.isEmpty = false := by simpa using hjs
    simp only [hne0, Bool.not_false, ↓reduceIte]
    rw [hb, ha]
    set it := fillIter D m.jobs 128 with hit
    have hj0 : j0 ∈ m.jobs := by rw [hjs']; exact List.mem_cons_self
    have hw0 : 0 < wantOf D j0 := by
      rw [ci_want_job hI hj0]
      obtain ⟨h1, h2, -⟩ := hI.jobs j0 hj0
      rcases h2 with ⟨hc, -⟩ | ⟨hc, -⟩
      · simp [wantG, hc]; exact (hI.leftP _ h1 hc).1
      · simp [wantG, hc]
    have hne : it ≠ [] := by rw [hit, hjs']; exact Exec.fillIter_ne_nil D j0 js 128 hw0 (by norm_num)
    have hne' : it.isEmpty = false := by simpa using hne
    simp only [hne', Bool.not_false, Bool.true_or, ↓reduceIte]
    have hk : tokSum it = min 128 (∑ i ∈ Finset.range m.sess.size, wantG g i) := by
      rw [hit, tokSum_fillIter, ci_demand hI]
    have hmemit : ∀ e ∈ it, ∃ j ∈ m.jobs, e.1 = j.owner := fun e he => by
      obtain ⟨j, hj, h1, -⟩ := mem_fillIter D _ 128 e he
      exact ⟨j, hj, h1⟩
    have hsh : ∀ i < m.sess.size, shareOf it i ≤ if isJob (g.c i) then g.left i else 0 := by
      intro i hi
      have h1 := Exec.shareOf_fillIter D m.jobs 128 i
      by_cases hj : isJob (g.c i) = true
      · rw [if_pos hj]
        obtain ⟨j, hjm, rfl⟩ := hI.jobsP i hi hj
        rw [Exec.filter_owner_eq hI.jobsNodup hjm] at h1
        simp only [List.map_cons, List.map_nil, List.sum_cons, List.sum_nil, Nat.add_zero] at h1
        rw [ci_want_job hI hjm] at h1
        refine h1.trans ?_
        obtain ⟨-, h2, -⟩ := hI.jobs j hjm
        rcases h2 with ⟨hc, -⟩ | ⟨hc, -⟩
        · simp [wantG, hc]
        · simp [wantG, hc]; exact (hI.leftD _ (hI.jobs j hjm).1 hc).1
      · rw [if_neg hj]
        have : m.jobs.filter (fun x => decide (x.owner = i)) = [] := by
          rw [List.filter_eq_nil_iff]
          intro x hx
          simp only [decide_eq_true_eq]
          intro he
          obtain ⟨-, h2, -⟩ := hI.jobs x hx
          rw [he] at h2
          rcases h2 with ⟨hc, -⟩ | ⟨hc, -⟩ <;> rw [hc] at hj <;> simp [isJob] at hj
        rw [this] at h1; simpa using h1
    exact ⟨⟨{ hI with
        iterNone := fun h => by simp at h
        iterOwn := fun e he => by
          obtain ⟨j, hj, he1⟩ := hmemit e he
          rw [he1]; exact (hI.jobs j hj).1
        share := hsh
        shareP := fun _ _ _ => one_dvd _
        tok := by rw [hk]; exact min_le_left _ _ }, hr, fun h => by simp at h, fun _ => ⟨rfl, hk⟩⟩, by first | rfl | trivial⟩

/-! ### A slot -/

/-- **A slot** keeps the invariant: `k` arrivals bring `1280 k` tokens. -/
theorem slot_sb {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hB : SB L g m) (k : ℕ) :
    ∃ L' g', SB L' g' (slot k m) ∧ (slot k m).sess.size = m.sess.size + k ∧
      WnC (m.sess.size + k) L' g' + (if m.iterEnd.isSome then tokSum m.iter else 0) =
        WnC m.sess.size L g + 1280 * k := by
  obtain ⟨L', g', h1, h2, h3⟩ := Slot.slot_sb ci_start hB (List.replicate k (fun _ => 0)) (fun _ _ => rfl)
  rw [← slot_eq, List.length_replicate] at *
  refine ⟨L', g', h1, h2, ?_⟩
  rw [h3]; simp [Model.work, M]; ring

/-- The chain starts from no session. -/
theorem empty_sb : SB (fun _ => (290, 990)) ⟨fun _ => .e, fun _ => 0⟩ empty := Slot.empty_sb _

/-- **Every state of the chain is at a slot's end.** -/
theorem reach_sb {K : ℕ} {m : Machine} (h : Reach K m) : ∃ L g, SB L g m := by
  induction h with
  | empty => exact ⟨_, _, empty_sb⟩
  | slot k hk _ ih =>
    obtain ⟨L, g, hB⟩ := ih
    obtain ⟨L', g', h1, -⟩ := slot_sb hB k
    exact ⟨L', g', h1⟩

/-- After settling, the backlog is at most the demand plus a decode for
each resident, and there are no more residents than the demand. -/
theorem ci_wn_le {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hr : m.ready = []) :
    WnC m.sess.size L g ≤ ∑ i ∈ Finset.range m.sess.size, wantG g i +
      990 * ∑ i ∈ Finset.range m.sess.size, (if isJob (g.c i) then 1 else 0) ∧
    ∑ i ∈ Finset.range m.sess.size, (if isJob (g.c i) then 1 else 0) ≤
      ∑ i ∈ Finset.range m.sess.size, wantG g i := by
  have hnr := hI.not_ready hr
  constructor
  · rw [WnC, Finset.mul_sum, ← Finset.sum_add_distrib]
    apply Finset.sum_le_sum
    intro i hi
    have hi' := Finset.mem_range.mp hi
    have h1 := hnr i hi'
    unfold rem wantG
    have hL := len_of hI hi'
    cases hc : g.c i <;> simp [hc, isReady, isJob, hL] at h1 ⊢
    have := (hI.leftD i hi' hc).2; rw [hL] at this; omega
  · apply Finset.sum_le_sum
    intro i hi
    have hi' := Finset.mem_range.mp hi
    unfold wantG
    cases hc : g.c i <;> simp [isJob]
    exact (hI.leftP i hi' hc).1

/-- A full batch serves 128 tokens and `k` arrivals bring `1280 k`: the
backlog after the slot. -/
theorem backlog_slot {K : ℕ} (x : State K) (hx : ¬ F x) (k : ℕ) (hk : k ≤ K) :
    backlog (slot k x.1) + 128 = backlog x.1 + 1280 * k := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  simp only [F, not_or, not_lt] at hx
  have hbusy : x.1.iterEnd.isSome = true := Option.isSome_iff_ne_none.mpr hx.1
  obtain ⟨hb1, -⟩ := hB.busy hbusy
  have h128 : tokSum x.1.iter = 128 := by have : tokSum x.1.iter ≤ 128 := hB.tok; omega
  obtain ⟨L', g', s1, s2, s3⟩ := slot_sb hB k
  unfold backlog
  rw [ci_backlog s1.toCI s1.rdy, ci_backlog hB.toCI hB.rdy, s2]
  rw [if_pos hbusy, h128] at s3
  omega

/-- `F` is small: a batch that is not full served every resident's demand
(`Exec.work_conserving`), so fewer than 128 residents are left, each with
less than one request's work. -/
theorem backlog_lt_of_sb {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hB : SB L g m)
    (hx : m.iterEnd = none ∨ m.last.stats.tokens < 128) : backlog m < 128 * 1280 := by
  rcases hie : m.iterEnd with _ | ⟨a, q⟩
  · simp [Slot.backlog, hB.idle hie]
  · have hbusy : m.iterEnd.isSome = true := by rw [hie]; rfl
    have htok : m.last.stats.tokens < 128 := by
      rcases hx with hx | hx
      · rw [hie] at hx; simp at hx
      · exact hx
    obtain ⟨hb1, hb2⟩ := hB.busy hbusy
    obtain ⟨hW1, hW2⟩ := ci_wn_le hB.toCI hB.rdy
    have hS : ∑ i ∈ Finset.range m.sess.size, wantG g i < 128 := by
      rw [hb1] at htok; rw [hb2] at htok; omega
    unfold backlog
    rw [ci_backlog hB.toCI hB.rdy]
    omega

theorem backlog_lt_of_F {K : ℕ} (x : State K) (hx : F x) : backlog x.1 < 128 * 1280 := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  exact backlog_lt_of_sb hB hx

/-- Foster's drift condition, below capacity. -/
theorem drift {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) :
    Drift (kernel A) F (fun x => (backlog x.1 : ℝ)) (ε A) where
  nonneg _ := Nat.cast_nonneg _
  integrable := Kernel.integrable_ofOutcomes _ _ _ _ _ _
  pos := by unfold ε; linarith
  drift x hx := by
    rw [kernel, Kernel.apply_ofOutcomes]
    calc _ = ∑ k ∈ Finset.range (K + 1),
            (A.p k * backlog x.1 + 1280 * (A.p k * k) - 128 * A.p k) := by
          refine Finset.sum_congr rfl fun k hk => ?_
          have hk' : k ≤ K := Nat.lt_succ_iff.mp (Finset.mem_range.mp hk)
          -- the slot's state, by `dif_pos` under the expectation's function (a `rw` cannot
          -- abstract a `State K` behind the coercion to `Machine`)
          have key : (if hk : k ≤ K then (⟨slot k x.1, Reach.slot k hk x.2⟩ : State K) else x) =
              ⟨slot k x.1, Reach.slot k hk' x.2⟩ := dif_pos hk'
          refine (congrArg (fun y : State K => A.p k * (backlog y.1 : ℝ)) key).trans ?_
          show A.p k * (backlog (slot k x.1) : ℝ) = _
          have h1 := backlog_slot x hx k hk'
          have h2 : (backlog (slot k x.1) : ℝ) + 128 = backlog x.1 + 1280 * k := by exact_mod_cast h1
          have h3 : (backlog (slot k x.1) : ℝ) = backlog x.1 + 1280 * k - 128 := by linarith
          rw [h3]; ring
      _ = (backlog x.1 : ℝ) * ∑ k ∈ Finset.range (K + 1), A.p k + 1280 * A.mean -
            128 * ∑ k ∈ Finset.range (K + 1), A.p k := by
          rw [Finset.sum_sub_distrib, Finset.sum_add_distrib, ← Finset.mul_sum, ← Finset.mul_sum,
            Arrivals.mean]
          rw [← Finset.sum_mul]; ring
      _ ≤ (backlog x.1 : ℝ) - ε A := by
          rw [A.sum_one]; unfold ε; linarith

/-- Theorem 2(b): from every state, the expected number of slots until the
batch is not full (or the engine idle) is at most `backlog / ε`. -/
theorem hitTime_le {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) (x : State K) :
    ε A * hitTime (kernel A) F x ≤ backlog x.1 :=
  Foster.hitTime_le (drift A hA) x

/-- Theorem 2(b): from every state of `F`, the expected return time to `F`
is finite. -/
theorem returnTime_le {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) (x : State K) (hx : F x) :
    returnTime (kernel A) F x ≤ 1 + (kernel A).apply (fun y => (backlog y.1 : ℝ)) x / ε A :=
  Foster.returnTime_le_of_drift (drift A hA) x hx

/-- The expected hitting time is finite: the truncated expectations are
bounded and converge to `hitTime`. (`hitTime` is a supremum in ℝ, which
would read 0 were they unbounded, so `hitTime_le` alone does not say this.) -/
theorem hit_tendsto {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) (x : State K) :
    Filter.Tendsto (fun n => hit (kernel A) F n x) Filter.atTop (nhds (hitTime (kernel A) F x)) :=
  Foster.hit_tendsto (drift A hA) x

/-- The theorem is not vacuous: one request makes a full batch (290 prompt
tokens, a budget of 128), a state outside `F`. -/
example : ¬ F (K := 1) ⟨slot 1 empty, Reach.slot 1 le_rfl .empty⟩ := by
  have h : (slot 1 empty).iterEnd.isSome = true ∧ (slot 1 empty).last.stats.tokens = 128 := by
    decide +kernel
  intro hF
  change (slot 1 empty).iterEnd = none ∨ (slot 1 empty).last.stats.tokens < 128 at hF
  rcases hF with hF | hF
  · rw [hF] at h; exact absurd h.1 (by decide)
  · omega

/-- … and the load condition can hold: one arrival in a slot with
probability 1/20 is 64 tokens per slot. -/
example : ∃ A : Arrivals 1, 1280 * A.mean < 128 :=
  ⟨⟨fun k => if k = 0 then 19 / 20 else if k = 1 then 1 / 20 else 0,
      fun k => by split_ifs <;> norm_num,
      by simp [Finset.sum_range_succ]; norm_num⟩,
    by simp [Arrivals.mean, Finset.sum_range_succ]; norm_num⟩

end DaiStable
end Papers
end SerqLang
