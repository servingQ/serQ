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
every state of `F` (`returnTime_le`).
-/
import Serq.Chain
import Serq.Papers.DaiBounded

namespace SerqLang
namespace Papers
namespace DaiStable

open Exec Foster

/-- The deployment of `dai_sarathi.sq`. -/
abbrev D : Deployment := Claims.DaiSarathi.deployment

/-- What a session runs once it has arrived: `dai_sarathi.sq`'s session
without its arrival delay (`run 1 (x.attr 10)`). -/
def arrived : Prog :=
  match Claims.DaiSarathi.prog with
  | .run _ _ _ _ k => k
  | p => p

/-- No session, the engine idle. -/
def empty : Machine := Exec.initial D 0 (fun _ _ => 0) arrived { init := [], computedSlot := some 8 }

/-- One slot with `k` arrivals. -/
def slot (k : ℕ) (m : Machine) : Machine := Exec.slot D arrived (fun _ => 0) k m

/-- The machines reached from `empty` by slots of at most `K` arrivals. -/
inductive Reach (K : ℕ) : Machine → Prop
  | empty : Reach K empty
  | slot {m : Machine} (k : ℕ) : k ≤ K → Reach K m → Reach K (slot k m)

/-- The chain's states. -/
def State (K : ℕ) : Type := {m : Machine // Reach K m}

/-- The tokens the engine still has to serve: a prefill's left work and the
decode it will then run, a decode's left work. At an iteration's start the
running batch is not yet subtracted. -/
def backlog (m : Machine) : ℕ :=
  (m.jobs.map fun j => j.left + if j.mode = .prefill then 990 else 0).sum

/-- The engine is idle, or its batch is not full. -/
def F {K : ℕ} (x : State K) : Prop := x.1.iterEnd = none ∨ x.1.last.stats.tokens < 128

instance {K : ℕ} : DecidablePred (F (K := K)) := fun x => by unfold F; infer_instance

/-- The arrival distribution: `p k` for `k ≤ K`. At most 10 000 arrive in a
slot, so that each becomes a job within its slot: an instant's commands run
with finite fuel (`Exec.settle`), as the claims' families stop at 500
sessions. -/
structure Arrivals (K : ℕ) where
  small : K ≤ 10000
  p : ℕ → ℝ
  nonneg : ∀ k, 0 ≤ p k
  sum_one : ∑ k ∈ Finset.range (K + 1), p k = 1

/-- The mean number of arrivals per slot. -/
def Arrivals.mean {K : ℕ} (A : Arrivals K) : ℝ := ∑ k ∈ Finset.range (K + 1), A.p k * k

/-- The chain: a slot with `k` arrivals, `k` drawn from `A`. -/
noncomputable def kernel {K : ℕ} (A : Arrivals K) : Kernel (State K) :=
  Kernel.ofOutcomes K A.p A.nonneg A.sum_one fun x k =>
    if hk : k ≤ K then ⟨slot k x.1, Reach.slot k hk x.2⟩ else x

/-- The drift: `ε = 128 - 1280 · mean`. -/
def ε {K : ℕ} (A : Arrivals K) : ℝ := 128 - 1280 * A.mean

/-! ### The invariant of the chain

The states are those of `Serq/Papers/DaiBounded.lean`'s ghost (`DaiSarathi.Ghost`):
each session's place in the program and the tokens its job has left. Every
session has arrived, so none is at its first command or in its arrival
delay, and the chain has no delays at all. -/

open DaiSarathi (Cat Ghost rem fresh isReady isJob Shape shareOf P1 P2 P3 P4 Keeps wantG)

theorem arrived_eq_P1 : arrived = P1 := rfl

/-- The tokens arrived and not yet served, over the first `n` sessions. -/
def WnC (n : ℕ) (g : Ghost) : ℕ := ∑ i ∈ Finset.range n, rem g i

/-- The invariant of an instant. -/
structure CI (g : Ghost) (m : Machine) : Prop where
  sess : ∀ i < m.sess.size, Shape (getS m i) (g.c i) ∧ (getS m i).stack = [] ∧ fresh (g.c i) = false
  jobs : ∀ j ∈ m.jobs, j.owner < m.sess.size ∧
    ((g.c j.owner = .p ∧ j.mode = .prefill) ∨ (g.c j.owner = .d ∧ j.mode = .decode)) ∧
    j.growing = none ∧ j.left = g.left j.owner
  jobsP : ∀ i < m.sess.size, isJob (g.c i) = true → ∃ j ∈ m.jobs, j.owner = i
  jobsNodup : (m.jobs.map (·.owner)).Nodup
  leftP : ∀ i < m.sess.size, g.c i = .p → 1 ≤ g.left i ∧ g.left i ≤ 290
  leftD : ∀ i < m.sess.size, g.c i = .d → 1 ≤ g.left i ∧ g.left i ≤ 990
  ready : m.ready.Nodup
  readyMem : ∀ i, i ∈ m.ready ↔ i < m.sess.size ∧ isReady (g.c i) = true
  delays : m.delays = []
  iterNone : m.iterEnd = none → m.iter = []
  iterOwn : ∀ e ∈ m.iter, e.1 < m.sess.size
  share : ∀ i < m.sess.size, shareOf m.iter i ≤ if isJob (g.c i) then g.left i else 0
  tok : tokSum m.iter ≤ 128

/-- At a slot's end: settled, an idle engine has no job, and a running
iteration is the greedy fill of the residents. -/
structure SB (g : Ghost) (m : Machine) : Prop extends CI g m where
  rdy : m.ready = []
  idle : m.iterEnd = none → m.jobs = []
  busy : m.iterEnd.isSome = true → m.last.stats.tokens = tokSum m.iter ∧
    tokSum m.iter = min 128 (∑ i ∈ Finset.range m.sess.size, wantG g i)

theorem CI.lt_of_ready {g : Ghost} {m : Machine} (hI : CI g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) : i < m.sess.size ∧ isReady (g.c i) = true :=
  (hI.readyMem i).mp (by rw [hr]; simp)

theorem CI.owner_ne {g : Ghost} {m : Machine} (hI : CI g m) {i : ℕ}
    (hc : isJob (g.c i) = false) : ∀ j ∈ m.jobs, j.owner ≠ i := by
  intro j hj he
  obtain ⟨-, h, -⟩ := hI.jobs j hj
  rw [he] at h
  rcases h with ⟨h, -⟩ | ⟨h, -⟩ <;> rw [h] at hc <;> simp [isJob] at hc

theorem CI.not_ready {g : Ghost} {m : Machine} (hI : CI g m) (hr : m.ready = []) :
    ∀ i < m.sess.size, isReady (g.c i) = false := by
  intro i hi
  by_contra h
  have := (hI.readyMem i).mpr ⟨hi, by simpa using h⟩
  rw [hr] at this; simp at this

/-- A ready session starts a job at the engine. -/
theorem ci_addJob {g : Ghost} {m : Machine} (hI : CI g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hcj : isJob (g.c i) = false)
    (k : Cat) (md : Mode) (L : ℕ)
    (hk : (k = .p ∧ md = .prefill ∧ 1 ≤ L ∧ L ≤ 290) ∨ (k = .d ∧ md = .decode ∧ 1 ≤ L ∧ L ≤ 990))
    (M : Machine) (s' : Sess) (a b : List Job) (hab : a ++ b = m.jobs)
    (hsz : M.sess.size = m.sess.size)
    (hget : ∀ j, getS M j = if j = i then s' else getS m j)
    (hjobs : M.jobs = a ++ ⟨i, md, L, none⟩ :: b) (hready : M.ready = rest)
    (hdl : M.delays = m.delays) (hit : M.iter = m.iter) (hie : M.iterEnd = m.iterEnd)
    (hs' : Shape s' k ∧ s'.stack = []) :
    CI (g.set i k L) M := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hkf : fresh k = false := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> rfl
  have hkr : isReady k = false := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> rfl
  have hown := hI.owner_ne hcj
  have hgc : ∀ j, (g.set i k L).c j = if j = i then k else g.c j := DaiSarathi.Ghost.set_c g i k L
  have hgl : ∀ j, (g.set i k L).left j = if j = i then L else g.left j := DaiSarathi.Ghost.set_left g i k L
  have hmem : ∀ x, x ∈ M.jobs ↔ x ∈ m.jobs ∨ x = ⟨i, md, L, none⟩ := by
    intro x
    rw [hjobs, ← hab]
    simp only [List.mem_append, List.mem_cons]
    tauto
  have hnd' : rest.Nodup := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
  have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  refine
    { sess := fun j hj => ?_
      jobs := fun x hx => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := ?_
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := hready ▸ hnd'
      readyMem := fun j => ?_
      delays := hdl.trans hI.delays
      iterNone := fun h => hit ▸ hI.iterNone (hie ▸ h)
      iterOwn := fun e he => by rw [hsz]; exact hI.iterOwn e (hit ▸ he)
      share := fun j hj => ?_
      tok := by rw [hit]; exact hI.tok }
  · rw [hsz] at hj
    rw [hget, hgc]
    split_ifs with h
    · exact ⟨hs'.1, hs'.2, hkf⟩
    · exact hI.sess j hj
  · rw [hsz]
    rcases (hmem x).mp hx with hx | rfl
    · obtain ⟨h1, h2, h3, h4⟩ := hI.jobs x hx
      have hne := hown x hx
      simp only [hgc, hgl, hne, if_false]
      exact ⟨h1, h2, h3, h4⟩
    · simp only [hgc, hgl, if_true]
      refine ⟨hin, ?_, by simp, by simp⟩
      rcases hk with ⟨rfl, rfl, -⟩ | ⟨rfl, rfl, -⟩
      · exact Or.inl ⟨rfl, rfl⟩
      · exact Or.inr ⟨rfl, rfl⟩
  · rw [hsz] at hj
    by_cases h : j = i
    · subst h; exact ⟨_, (hmem _).mpr (Or.inr rfl), rfl⟩
    · rw [hgc, if_neg h] at hjj
      obtain ⟨x, hx, hxo⟩ := hI.jobsP j hj hjj
      exact ⟨x, (hmem x).mpr (Or.inl hx), hxo⟩
  · rw [hjobs]
    have hp : List.Perm ((a ++ ⟨i, md, L, none⟩ :: b).map (·.owner)) (i :: (a ++ b).map (·.owner)) := by
      simp only [List.map_append, List.map_cons]
      exact List.perm_middle
    refine hp.nodup_iff.mpr (List.nodup_cons.mpr ⟨fun hm => ?_, hab ▸ hI.jobsNodup⟩)
    obtain ⟨x, hx, hxo⟩ := List.mem_map.mp hm
    exact hown x (hab ▸ hx) hxo
  · rw [hsz] at hj
    rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; rw [hgl, if_pos rfl]
      rcases hk with ⟨-, -, h1, h2⟩ | ⟨rfl, -⟩
      · exact ⟨h1, h2⟩
      · exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftP j hj hjc
  · rw [hsz] at hj
    rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; rw [hgl, if_pos rfl]
      rcases hk with ⟨rfl, -⟩ | ⟨-, -, h1, h2⟩
      · exact absurd hjc (by decide)
      · exact ⟨h1, h2⟩
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftD j hj hjc
  · rw [hready, hsz, hgc]
    by_cases h : j = i
    · subst h; simp only [if_true, hkr]; simp [hir]
    · simp only [h, if_false]
      rw [← hI.readyMem j, hr]
      simp [h]
  · rw [hsz] at hj
    rw [hit, hgc, hgl]
    by_cases h : j = i
    · subst h
      have := hI.share j hj
      rw [hcj, if_neg (by simp)] at this
      omega
    · simp only [h, if_false]; exact hI.share j hj

/-- An arrived session sets `t0` and starts its prefill. -/
theorem ci_r1 {g : Ghost} {m : Machine} (hI : CI g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    CI (g.set i .p 290) (exec D 10000 { m with ready := rest } i) ∧
      (exec D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec D 10000 { m with ready := rest } i) m ∧
      (exec D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, -⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    DaiSarathi.exec_set D _ m0 i 9 (fun x => x.now) P2 (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl)]
  set s1 : Sess := { getS m0 i with attr := (getS m0 i).attr.upd 9 (evalE m0 i fun x => x.now), prog := P2 }
    with hs1
  set m1 := setS m0 i s1 with hm1
  have hg1 : getS m1 i = s1 := KongSvf.getS_setS_self m0 s1 (by simpa using hisz)
  obtain ⟨a, b, hab, he⟩ := DaiSarathi.exec_runEngine' D 9998 m1 i .prefill (fun _ => 290) P3
    (by rw [hg1]; exact hst) (by rw [hg1]; rfl) (by simp [evalE])
  rw [he]
  have hev : evalE m1 i (fun _ => 290) = 290 := rfl
  rw [hev]
  set s2 : Sess := { getS m1 i with prog := P3, status := .engine } with hs2
  refine ⟨ci_addJob hI hr (by rw [hc]; rfl) .p .prefill 290 (Or.inl ⟨rfl, rfl, by norm_num, le_rfl⟩)
    _ s2 a b hab (by simp [hm1, hm0]) (fun j => ?_) rfl rfl rfl rfl rfl ?_, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩,
    by simp [hm1, hm0]⟩
  · show getS (setS m1 i s2) j = _
    rw [DaiSarathi.getS_setS m1 s2 (by simpa [hm1, hm0] using hisz)]
    split_ifs with h
    · rfl
    · rw [hm1, DaiSarathi.getS_setS m0 s1 (by simpa [hm0] using hisz), if_neg h]; rfl
  · rw [hs2, hg1, hs1]
    exact ⟨⟨rfl, rfl⟩, hstk⟩

/-- A prefilled session starts its decode. -/
theorem ci_r3 {g : Ghost} {m : Machine} (hI : CI g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r3) :
    CI (g.set i .d 990) (exec D 10000 { m with ready := rest } i) ∧
      (exec D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec D 10000 { m with ready := rest } i) m ∧
      (exec D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, -⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  obtain ⟨a, b, hab, he⟩ := DaiSarathi.exec_runEngine' D 9999 m0 i .decode (fun _ => 990) P4
    (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl) (by simp [evalE])
  rw [show (10000 : ℕ) = 9999 + 1 from rfl, he]
  have hev : evalE m0 i (fun _ => 990) = 990 := rfl
  rw [hev]
  set s2 : Sess := { getS m0 i with prog := P4, status := .engine } with hs2
  refine ⟨ci_addJob hI hr (by rw [hc]; rfl) .d .decode 990 (Or.inr ⟨rfl, rfl, by norm_num, le_rfl⟩)
    _ s2 a b hab (by simp [hm0]) (fun j => ?_) rfl rfl rfl rfl rfl ?_, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩,
    by simp [hm0]⟩
  · show getS (setS m0 i s2) j = _
    rw [DaiSarathi.getS_setS m0 s2 (by simpa [hm0] using hisz)]; rfl
  · rw [hs2, hg0]
    exact ⟨⟨rfl, rfl⟩, hstk⟩

/-- A decoded session observes its latency and ends. -/
theorem ci_r4 {g : Ghost} {m : Machine} (hI : CI g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r4) :
    CI (g.set i .e 0) (exec D 10000 { m with ready := rest } i) ∧
      (exec D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec D 10000 { m with ready := rest } i) m ∧
      (exec D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, -⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    KongSvf.exec_observe D _ m0 i 0 (fun x => x.now - x.attr 9) .stop (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl)]
  set m1 : Machine := { setS m0 i { getS m0 i with prog := .stop } with
    obs := (0, (getS m0 i).serial, m0.now, evalE m0 i fun x => x.now - x.attr 9) :: m0.obs } with hm1
  have hg1 : getS m1 i = { getS m0 i with prog := .stop } :=
    KongSvf.getS_setS_self m0 _ (by simpa using hisz)
  rw [KongSvf.exec_stop D _ m1 i (by rw [hg1]; exact hst) (by rw [hg1]) (by rw [hg1]; exact hstk)]
  set s' : Sess := { getS m1 i with status := .ended, stack := [] } with hs'
  have hget : ∀ j, getS (setS m1 i s') j = if j = i then s' else getS m j := by
    intro j
    rw [DaiSarathi.getS_setS m1 s' (by simpa [hm1, hm0] using hisz)]
    split_ifs with h
    · rfl
    · show getS (setS m0 i _) j = _
      rw [DaiSarathi.getS_setS m0 _ (by simpa [hm0] using hisz), if_neg h]; rfl
  have hgc : ∀ j, (g.set i .e 0).c j = if j = i then .e else g.c j := DaiSarathi.Ghost.set_c g i .e 0
  have hgl : ∀ j, (g.set i .e 0).left j = if j = i then 0 else g.left j := DaiSarathi.Ghost.set_left g i .e 0
  have hown := hI.owner_ne (i := i) (by rw [hc]; rfl)
  have hnd' : rest.Nodup := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
  have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  set M := setS m1 i s' with hM
  have hsz : M.sess.size = m.sess.size := by simp [hM, hm1, hm0]
  refine ⟨{
      sess := fun j hj => ?_
      jobs := fun x hx => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := hI.jobsNodup
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := hnd'
      readyMem := fun j => ?_
      delays := hI.delays
      iterNone := hI.iterNone
      iterOwn := fun e he => by rw [hsz]; exact hI.iterOwn e he
      share := fun j hj => ?_
      tok := hI.tok }, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩, hsz⟩
  · rw [hsz] at hj
    rw [hget, hgc]
    split_ifs with h
    · exact ⟨rfl, rfl, rfl⟩
    · exact hI.sess j hj
  · obtain ⟨h1, h2, h3, h4⟩ := hI.jobs x hx
    have hne := hown x hx
    simp only [hgc, hgl, hne, if_false]
    exact ⟨by rw [hsz]; exact h1, h2, h3, h4⟩
  · rw [hsz] at hj
    by_cases h : j = i
    · subst h; rw [hgc, if_pos rfl] at hjj; exact absurd hjj (by decide)
    · rw [hgc, if_neg h] at hjj; exact hI.jobsP j hj hjj
  · rw [hsz] at hj
    rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftP j hj hjc
  · rw [hsz] at hj
    rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftD j hj hjc
  · show j ∈ rest ↔ _
    rw [hsz, hgc]
    by_cases h : j = i
    · subst h; simp [hir, isReady]
    · simp only [h, if_false]
      rw [← hI.readyMem j, hr]
      simp [h]
  · rw [hsz] at hj
    show shareOf m.iter j ≤ _
    rw [hgc, hgl]
    by_cases h : j = i
    · subst h
      have := hI.share j hj
      rw [hc] at this
      simp [isJob] at this
      simp [this]
    · simp only [h, if_false]; exact hI.share j hj

/-- Draining keeps every session's tokens left. -/
def SameR (n : ℕ) (g g' : Ghost) : Prop := ∀ j < n, rem g' j = rem g j

theorem SameR.refl (n : ℕ) (g : Ghost) : SameR n g g := fun _ _ => rfl

theorem SameR.trans {n : ℕ} {a b c : Ghost} (h1 : SameR n a b) (h2 : SameR n b c) : SameR n a c :=
  fun j hj => (h2 j hj).trans (h1 j hj)

theorem SameR.set {n : ℕ} (g : Ghost) (i : ℕ) (k : Cat) (l : ℕ) (hr : rem (g.set i k l) i = rem g i) :
    SameR n g (g.set i k l) := fun j _ => by
  by_cases h : j = i
  · subst h; exact hr
  · exact DaiSarathi.rem_set g i k l j h

theorem WnC_congr {n : ℕ} {g g' : Ghost} (h : SameR n g g') : WnC n g' = WnC n g :=
  Finset.sum_congr rfl fun j hj => h j (Finset.mem_range.mp hj)

/-- **Draining.** -/
theorem drain_ci : ∀ (f : ℕ) (g : Ghost) (m : Machine), CI g m → m.ready.length ≤ f →
    ∃ g', CI g' (drain D f m) ∧ (drain D f m).ready = [] ∧ Keeps (drain D f m) m ∧
      (drain D f m).sess.size = m.sess.size ∧ SameR m.sess.size g g'
  | 0, g, m, hI, hf => by
    have hr : m.ready = [] := List.eq_nil_of_length_eq_zero (by omega)
    exact ⟨g, hI, by simp [drain, hr], DaiSarathi.Keeps.refl m, rfl, SameR.refl _ g⟩
  | f + 1, g, m, hI, hf => by
    unfold drain
    split
    · rename_i hr
      exact ⟨g, hI, hr, DaiSarathi.Keeps.refl m, rfl, SameR.refl _ g⟩
    · rename_i i rest hr
      have hlen : rest.length ≤ f := by rw [hr] at hf; simpa using hf
      obtain ⟨hin, hcr⟩ := hI.lt_of_ready hr
      have hfr := (hI.sess i hin).2.2
      obtain ⟨g1, hI1, hr1, hk1, hz1, hs1⟩ : ∃ g1, CI g1 (exec D 10000 { m with ready := rest } i) ∧
          (exec D 10000 { m with ready := rest } i).ready = rest ∧
          Keeps (exec D 10000 { m with ready := rest } i) m ∧
          (exec D 10000 { m with ready := rest } i).sess.size = m.sess.size ∧ SameR m.sess.size g g1 := by
        cases hc : g.c i with
        | r1 =>
          obtain ⟨h1, h2, h3, h4⟩ := ci_r1 hI hr hc
          exact ⟨_, h1, h2, h3, h4, SameR.set g i .p 290 (by simp [rem, DaiSarathi.Ghost.set_c,
            DaiSarathi.Ghost.set_left, hc])⟩
        | r3 =>
          obtain ⟨h1, h2, h3, h4⟩ := ci_r3 hI hr hc
          exact ⟨_, h1, h2, h3, h4, SameR.set g i .d 990 (by simp [rem, DaiSarathi.Ghost.set_c,
            DaiSarathi.Ghost.set_left, hc])⟩
        | r4 =>
          obtain ⟨h1, h2, h3, h4⟩ := ci_r4 hI hr hc
          exact ⟨_, h1, h2, h3, h4, SameR.set g i .e 0 (by simp [rem, DaiSarathi.Ghost.set_c, hc])⟩
        | s0 => rw [hc] at hfr; simp [fresh] at hfr
        | w => rw [hc] at hfr; simp [fresh] at hfr
        | _ => rw [hc] at hcr; simp [isReady] at hcr
      obtain ⟨g2, hI2, hr2, hk2, hz2, hs2⟩ := drain_ci f g1 _ hI1 (by rw [hr1]; exact hlen)
      rw [hz1] at hs2
      exact ⟨g2, hI2, hr2, hk1.trans hk2, hz2.trans hz1, hs1.trans hs2⟩

/-- **Settling an instant** of at most 10 000 ready sessions. -/
theorem settle_ci {g : Ghost} {m : Machine} (hI : CI g m) (hf : m.ready.length ≤ 10000) :
    ∃ g', CI g' (settle D m) ∧ (settle D m).ready = [] ∧ Keeps (settle D m) m ∧
      (settle D m).sess.size = m.sess.size ∧ SameR m.sess.size g g' := by
  obtain ⟨g', h1, h2, h3, h4, h5⟩ := drain_ci 10000 g m hI hf
  have hs : settle D m = drain D 10000 m := by
    unfold settle
    rw [show (1000 : ℕ) = 999 + 1 from rfl, settleLoop]
    simp only [DaiSarathi.admitAll_dai, h2, List.isEmpty_nil, ↓reduceIte]
  rw [hs]
  exact ⟨g', h1, h2, h3, h4, h5⟩

/-! ### Arrivals -/

theorem getS_inject (m : Machine) (j : ℕ) :
    getS (inject arrived (fun _ => 0) m) j =
      if j = m.sess.size then ⟨m.sess.size, ⟨fun _ => 0, []⟩, 0, arrived, [], .ready, 0, 0⟩ else getS m j := by
  unfold getS inject
  simp only [Array.getD_eq_getD_getElem?, Array.getElem?_push]
  split_ifs with h
  · subst h; simp
  · rfl

theorem shareOf_eq_zero {l : List (ℕ × ℕ)} {i : ℕ} (h : ∀ e ∈ l, e.1 ≠ i) : shareOf l i = 0 := by
  unfold shareOf
  rw [List.filter_eq_nil_iff.mpr fun e he => by simpa using h e he]
  rfl

/-- **One arrival.** -/
theorem ci_inject {g : Ghost} {m : Machine} (hI : CI g m) :
    CI (g.set m.sess.size .r1 0) (inject arrived (fun _ => 0) m) ∧
      (inject arrived (fun _ => 0) m).sess.size = m.sess.size + 1 ∧
      (inject arrived (fun _ => 0) m).ready = m.ready ++ [m.sess.size] ∧
      Keeps (inject arrived (fun _ => 0) m) m ∧
      WnC (m.sess.size + 1) (g.set m.sess.size .r1 0) = WnC m.sess.size g + 1280 := by
  set n := m.sess.size with hn
  set M := inject arrived (fun _ => 0) m with hM
  have hsz : M.sess.size = n + 1 := by simp [hM, inject, hn]
  have hget := getS_inject m
  have hgc : ∀ j, (g.set n .r1 0).c j = if j = n then .r1 else g.c j := DaiSarathi.Ghost.set_c g n .r1 0
  have hgl : ∀ j, (g.set n .r1 0).left j = if j = n then 0 else g.left j :=
    DaiSarathi.Ghost.set_left g n .r1 0
  have hjo : ∀ x ∈ m.jobs, x.owner ≠ n := fun x hx => Nat.ne_of_lt (hI.jobs x hx).1
  have hro : n ∉ m.ready := fun h => by have := ((hI.readyMem n).mp h).1; omega
  have hio : ∀ e ∈ m.iter, e.1 ≠ n := fun e he => Nat.ne_of_lt (hI.iterOwn e he)
  refine ⟨{
      sess := fun j hj => ?_
      jobs := fun x hx => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := hI.jobsNodup
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := ?_
      readyMem := fun j => ?_
      delays := hI.delays
      iterNone := hI.iterNone
      iterOwn := fun e he => by rw [hsz]; have := hI.iterOwn e he; omega
      share := fun j hj => ?_
      tok := hI.tok }, hsz, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩, ?_⟩
  · rw [hsz] at hj
    rw [hget, hgc]
    by_cases h : j = n
    · rw [if_pos h, if_pos h]; exact ⟨⟨rfl, rfl⟩, rfl, rfl⟩
    · rw [if_neg h, if_neg h]; exact hI.sess j (by omega)
  · obtain ⟨h1, h2, h3, h4⟩ := hI.jobs x hx
    have hne := hjo x hx
    simp only [hgc, hgl, if_neg hne]
    exact ⟨by rw [hsz]; omega, h2, h3, h4⟩
  · rw [hsz] at hj
    rw [hgc] at hjj
    by_cases h : j = n
    · rw [if_pos h] at hjj; simp [isJob] at hjj
    · rw [if_neg h] at hjj; exact hI.jobsP j (by omega) hjj
  · rw [hsz] at hj
    rw [hgc] at hjc
    by_cases h : j = n
    · rw [if_pos h] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftP j (by omega) hjc
  · rw [hsz] at hj
    rw [hgc] at hjc
    by_cases h : j = n
    · rw [if_pos h] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftD j (by omega) hjc
  · show (m.ready ++ [n]).Nodup
    rw [List.nodup_append]
    refine ⟨hI.ready, List.nodup_singleton n, fun a ha b hb => ?_⟩
    rw [List.mem_singleton] at hb; subst hb
    exact fun h => hro (h ▸ ha)
  · show j ∈ m.ready ++ [n] ↔ _
    rw [hsz, hgc, List.mem_append, List.mem_singleton]
    by_cases h : j = n
    · subst h; simp [hro, isReady]
    · simp only [h, if_false, or_false]
      rw [hI.readyMem j]
      constructor
      · rintro ⟨a, b⟩; exact ⟨by omega, b⟩
      · rintro ⟨a, b⟩; exact ⟨by omega, b⟩
  · rw [hsz] at hj
    show shareOf m.iter j ≤ _
    rw [hgc, hgl]
    by_cases h : j = n
    · subst h; rw [if_pos rfl, if_pos rfl, shareOf_eq_zero hio]; exact Nat.zero_le _
    · rw [if_neg h, if_neg h]; exact hI.share j (by omega)
  · unfold WnC
    rw [Finset.sum_range_succ]
    have h1 : ∑ i ∈ Finset.range n, rem (g.set n .r1 0) i = ∑ i ∈ Finset.range n, rem g i :=
      Finset.sum_congr rfl fun j hj => DaiSarathi.rem_set g n .r1 0 j (Nat.ne_of_lt (Finset.mem_range.mp hj))
    have h2 : rem (g.set n .r1 0) n = 1280 := by simp [rem, hgc]
    rw [h1, h2]

/-- **`k` arrivals.** -/
theorem ci_injects {g : Ghost} {m : Machine} (hI : CI g m) : ∀ k, ∃ g',
    CI g' ((inject arrived (fun _ => 0))^[k] m) ∧
      ((inject arrived (fun _ => 0))^[k] m).sess.size = m.sess.size + k ∧
      ((inject arrived (fun _ => 0))^[k] m).ready.length = m.ready.length + k ∧
      Keeps ((inject arrived (fun _ => 0))^[k] m) m ∧
      WnC (m.sess.size + k) g' = WnC m.sess.size g + 1280 * k
  | 0 => ⟨g, hI, rfl, rfl, DaiSarathi.Keeps.refl m, by simp⟩
  | k + 1 => by
    obtain ⟨g1, h1, h2, h3, h4, h5⟩ := ci_injects hI k
    rw [Function.iterate_succ_apply']
    obtain ⟨c1, c2, c3, c4, c5⟩ := ci_inject h1
    refine ⟨_, c1, by rw [c2, h2]; ring, by rw [c3]; simp [h3]; ring, h4.trans c4, ?_⟩
    rw [h2] at c5 ⊢
    rw [show m.sess.size + (k + 1) = m.sess.size + k + 1 by ring, c5, h5]
    ring

/-! ### An iteration ends -/

/-- **An iteration ends.** Its tokens leave the backlog; the sessions that
finish are ready, at most one per token. -/
theorem ci_end {g : Ghost} {m : Machine} (hI : CI g m) (hr : m.ready = []) {a qa : ℕ}
    (hie : m.iterEnd = some (a, qa)) :
    CI (g.tick m.iter) (handle m a qa) ∧
      WnC m.sess.size (g.tick m.iter) + tokSum m.iter = WnC m.sess.size g ∧
      (handle m a qa).ready.length ≤ 128 ∧ (handle m a qa).iterEnd = none ∧
      (handle m a qa).sess.size = m.sess.size := by
  set n := m.sess.size with hn
  set it := m.iter with hit
  set g' := g.tick it with hg'
  have hgc : ∀ j, g'.c j =
      if isJob (g.c j) = true ∧ g.left j ≤ shareOf it j then (if g.c j = .p then .r3 else .r4) else g.c j :=
    fun _ => rfl
  have hgl : ∀ j, g'.left j = g.left j - shareOf it j := fun _ => rfl
  have hshj : ∀ j < n, isJob (g.c j) = true → shareOf it j ≤ g.left j := by
    intro j hj hjj; have := hI.share j hj; rw [if_pos hjj] at this; exact this
  have hsh0 : ∀ j < n, isJob (g.c j) = false → shareOf it j = 0 := by
    intro j hj hjj; exact Nat.le_zero.mp (by simpa [hjj] using hI.share j hj)
  have hcj : ∀ j < n, ¬ (isJob (g.c j) = true ∧ g.left j ≤ shareOf it j) → g'.c j = g.c j :=
    fun j _ h => by rw [hgc, if_neg h]
  have hrem : ∀ j < n, rem g' j + shareOf it j = rem g j := by
    intro j hj
    unfold rem
    rw [hgc, hgl]
    cases hc : g.c j
    all_goals first
      | (have h0 := hsh0 j hj (by rw [hc]; rfl); simp [isJob, h0])
      | skip
    · have h1 := hshj j hj (by rw [hc]; rfl)
      by_cases hf : g.left j ≤ shareOf it j
      · simp [isJob, hf]; omega
      · simp [isJob, hf]; omega
    · have h1 := hshj j hj (by rw [hc]; rfl)
      by_cases hf : g.left j ≤ shareOf it j
      · simp [isJob, hf]; omega
      · simp [isJob, hf]; omega
  have hWn : WnC n g' + tokSum it = WnC n g := by
    unfold WnC
    rw [← DaiSarathi.sum_shareOf n it hI.iterOwn, ← Finset.sum_add_distrib]
    exact Finset.sum_congr rfl fun j hj => hrem j (Finset.mem_range.mp hj)
  -- the machine
  set M0 : Machine := { m with now := a, iterEnd := none } with hM0
  set jobs' := M0.jobs.map fun j => { j with left := j.left -
      ((M0.iter.filter (fun e : ℕ × ℕ => decide (e.1 = j.owner))).map (fun e : ℕ × ℕ => e.2)).sum } with hjobs'
  have hj' : ∀ x, x ∈ jobs' ↔ ∃ x0 ∈ m.jobs, x = { x0 with left := x0.left - shareOf it x0.owner } := by
    intro x; rw [hjobs']; simp only [List.mem_map]
    constructor
    · rintro ⟨x0, hx0, rfl⟩; exact ⟨x0, hx0, rfl⟩
    · rintro ⟨x0, hx0, rfl⟩; exact ⟨x0, hx0, rfl⟩
  set done := (jobs'.filter (·.left = 0)).map (·.owner) with hdone
  set M1 : Machine := { M0 with jobs := jobs'.filter (·.left ≠ 0), iter := [] } with hM1
  have hH : handle m a qa = KongSvf.readyAll done M1 := by
    unfold handle
    simp only [hie, if_true]
    rfl
  rw [hH]
  have hfin : ∀ x0 ∈ m.jobs, (x0.left - shareOf it x0.owner = 0 ↔ g.left x0.owner ≤ shareOf it x0.owner) := by
    intro x0 hx0; rw [(hI.jobs x0 hx0).2.2.2]; omega
  have hmemd : ∀ j, j ∈ done ↔ j < n ∧ isJob (g.c j) = true ∧ g.left j ≤ shareOf it j := by
    intro j
    rw [hdone]
    simp only [List.mem_map, List.mem_filter, decide_eq_true_eq]
    constructor
    · rintro ⟨x, ⟨hx, hl⟩, rfl⟩
      obtain ⟨x0, hx0, rfl⟩ := (hj' x).mp hx
      obtain ⟨h1, h2, -⟩ := hI.jobs x0 hx0
      refine ⟨h1, ?_, (hfin x0 hx0).mp hl⟩
      rcases h2 with ⟨hc, -⟩ | ⟨hc, -⟩ <;> rw [hc] <;> rfl
    · rintro ⟨hj, hjj, hl⟩
      obtain ⟨x0, hx0, rfl⟩ := hI.jobsP j hj hjj
      exact ⟨_, ⟨(hj' _).mpr ⟨x0, hx0, rfl⟩, (hfin x0 hx0).mpr hl⟩, rfl⟩
  have hdn : done.Nodup := by
    refine List.Nodup.sublist ((List.filter_sublist).map _) ?_
    simpa [hjobs', List.map_map, Function.comp_def] using hI.jobsNodup
  have hdb : ∀ i ∈ done, i < M1.sess.size := fun i hi => ((hmemd i).mp hi).1
  obtain ⟨f1, f2, -, -, -, f6, -, f8, f9, f10, f11⟩ := KongSvf.readyAll_fields done M1 hdn hdb
  set R := KongSvf.readyAll done M1 with hR
  have hgetR : ∀ j, getS R j = if j ∈ done then { getS m j with status := .ready } else getS m j := f11
  have hfinc : ∀ j < n, j ∈ done → (g.c j = .p ∧ g'.c j = .r3) ∨ (g.c j = .d ∧ g'.c j = .r4) := by
    intro j hj hd
    obtain ⟨-, hjj, hl⟩ := (hmemd j).mp hd
    rw [hgc, if_pos ⟨hjj, hl⟩]
    cases hc : g.c j <;> simp [hc, isJob] at hjj ⊢
  have hnfin : ∀ j < n, j ∉ done → g'.c j = g.c j := by
    intro j hj hd
    exact hcj j hj fun h => hd ((hmemd j).mpr ⟨hj, h⟩)
  have hR1 : R.jobs = jobs'.filter (·.left ≠ 0) := f2
  have hRit : R.iter = [] := f8
  have hRie : R.iterEnd = none := f9
  have hRsz : R.sess.size = n := f10
  have hlen : done.length ≤ tokSum it := by
    rw [← List.toFinset_card_of_nodup hdn, ← DaiSarathi.sum_shareOf n it hI.iterOwn]
    calc done.toFinset.card = ∑ j ∈ done.toFinset, 1 := by simp
      _ ≤ ∑ j ∈ done.toFinset, shareOf it j := Finset.sum_le_sum fun j hj => by
          obtain ⟨hj1, hjj, hl⟩ := (hmemd j).mp (List.mem_toFinset.mp hj)
          have : 1 ≤ g.left j := by
            cases hc : g.c j <;> simp [hc, isJob] at hjj
            · exact (hI.leftP j hj1 hc).1
            · exact (hI.leftD j hj1 hc).1
          omega
      _ ≤ ∑ j ∈ Finset.range n, shareOf it j :=
          Finset.sum_le_sum_of_subset_of_nonneg
            (fun j hj => Finset.mem_range.mpr ((hmemd j).mp (List.mem_toFinset.mp hj)).1)
            (fun _ _ _ => Nat.zero_le _)
  refine ⟨{
      sess := fun j hj => ?_
      jobs := fun x hx => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := ?_
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := by rw [f1]; simpa [hM1, hM0, hr] using hdn
      readyMem := fun j => ?_
      delays := by rw [f6]; exact hI.delays
      iterNone := fun _ => hRit
      iterOwn := fun e he => by rw [hRit] at he; simp at he
      share := fun j hj => by rw [hRit]; simp [shareOf]
      tok := by rw [hRit]; simp [tokSum] }, hWn, ?_, hRie, hRsz⟩
  · rw [hRsz] at hj
    rw [hgetR]
    by_cases hd : j ∈ done
    · rw [if_pos hd]
      obtain ⟨hs, hst, -⟩ := hI.sess j hj
      rcases hfinc j hj hd with ⟨hc, hc'⟩ | ⟨hc, hc'⟩
      · rw [hc] at hs; rw [hc']; exact ⟨⟨hs.1, rfl⟩, hst, rfl⟩
      · rw [hc] at hs; rw [hc']; exact ⟨⟨hs.1, rfl⟩, hst, rfl⟩
    · rw [if_neg hd, hnfin j hj hd]; exact hI.sess j hj
  · rw [hR1] at hx
    obtain ⟨hx, hl⟩ := List.mem_filter.mp hx
    obtain ⟨x0, hx0, rfl⟩ := (hj' x).mp hx
    obtain ⟨h1, h2, h3, h4⟩ := hI.jobs x0 hx0
    have hnd : x0.owner ∉ done := fun hd => by
      have := ((hmemd _).mp hd).2.2
      have := (hfin x0 hx0).mpr this
      simp at hl; omega
    refine ⟨by rw [hRsz]; exact h1, by rw [hnfin _ h1 hnd]; exact h2, h3, ?_⟩
    show x0.left - shareOf it x0.owner = g'.left x0.owner
    rw [hgl, h4]
  · rw [hRsz] at hj
    by_cases hd : j ∈ done
    · rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] at hjj <;> simp [isJob] at hjj
    · rw [hnfin j hj hd] at hjj
      obtain ⟨x0, hx0, rfl⟩ := hI.jobsP _ hj hjj
      refine ⟨{ x0 with left := x0.left - shareOf it x0.owner }, ?_, rfl⟩
      rw [hR1]
      refine List.mem_filter.mpr ⟨(hj' _).mpr ⟨x0, hx0, rfl⟩, ?_⟩
      have : ¬ (x0.left - shareOf it x0.owner = 0) := fun h0 =>
        hd ((hmemd _).mpr ⟨hj, hjj, (hfin x0 hx0).mp h0⟩)
      simpa using this
  · rw [hR1]
    refine List.Nodup.sublist ((List.filter_sublist).map _) ?_
    simpa [hjobs', List.map_map, Function.comp_def] using hI.jobsNodup
  · rw [hRsz] at hj
    by_cases hd : j ∈ done
    · rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] at hjc <;> simp at hjc
    · rw [hnfin j hj hd] at hjc
      have hjj : isJob (g.c j) = true := by rw [hjc]; rfl
      have h1 := hI.leftP j hj hjc
      have : ¬ g.left j ≤ shareOf it j := fun h => hd ((hmemd j).mpr ⟨hj, hjj, h⟩)
      rw [hgl]; omega
  · rw [hRsz] at hj
    by_cases hd : j ∈ done
    · rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] at hjc <;> simp at hjc
    · rw [hnfin j hj hd] at hjc
      have hjj : isJob (g.c j) = true := by rw [hjc]; rfl
      have h1 := hI.leftD j hj hjc
      have : ¬ g.left j ≤ shareOf it j := fun h => hd ((hmemd j).mpr ⟨hj, hjj, h⟩)
      rw [hgl]; omega
  · rw [f1, hRsz]
    show j ∈ m.ready ++ done ↔ _
    rw [hr, List.nil_append]
    constructor
    · intro hd
      have hj := ((hmemd j).mp hd).1
      refine ⟨hj, ?_⟩
      rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] <;> rfl
    · rintro ⟨hj, hrd⟩
      by_contra hd
      rw [hnfin j hj hd, hI.not_ready hr j hj] at hrd
      exact absurd hrd (by decide)
  · rw [f1]
    show (m.ready ++ done).length ≤ 128
    rw [hr, List.nil_append]
    exact hlen.trans hI.tok

/-! ### An iteration starts -/

theorem ci_sum_jobs {g : Ghost} {m : Machine} (hI : CI g m) (f : ℕ → ℕ)
    (hf : ∀ i < m.sess.size, isJob (g.c i) = false → f i = 0) :
    (m.jobs.map fun j => f j.owner).sum = ∑ i ∈ Finset.range m.sess.size, f i := by
  have h1 : (m.jobs.map fun j => f j.owner) = (m.jobs.map (·.owner)).map f := by
    simp [List.map_map, Function.comp_def]
  rw [h1, ← List.sum_toFinset _ hI.jobsNodup]
  apply Finset.sum_subset
  · intro x hx
    obtain ⟨j, hj, rfl⟩ := List.mem_map.mp (List.mem_toFinset.mp hx)
    exact Finset.mem_range.mpr (hI.jobs j hj).1
  · intro x hx hnx
    have hxn := Finset.mem_range.mp hx
    apply hf x hxn
    by_contra hj
    simp only [Bool.not_eq_false] at hj
    obtain ⟨j, hj2, rfl⟩ := hI.jobsP x hxn hj
    exact hnx (List.mem_toFinset.mpr (List.mem_map.mpr ⟨j, hj2, rfl⟩))

theorem ci_want_job {g : Ghost} {m : Machine} (hI : CI g m) {j : Job} (hj : j ∈ m.jobs) :
    wantOf D j = wantG g j.owner := by
  obtain ⟨h1, h2, -, h4⟩ := hI.jobs j hj
  rcases h2 with ⟨hc, hm⟩ | ⟨hc, hm⟩
  · simp [wantOf, wantG, hm, hc, h4, Claims.DaiSarathi.deployment]
  · have := (hI.leftD _ h1 hc).1
    simp only [wantOf, wantG, hm, hc, h4]; omega

theorem ci_demand {g : Ghost} {m : Machine} (hI : CI g m) :
    (m.jobs.map (wantOf D)).sum = ∑ i ∈ Finset.range m.sess.size, wantG g i := by
  rw [← ci_sum_jobs hI (wantG g) fun i _ h => ?_]
  · congr 1; exact List.map_congr_left fun j hj => ci_want_job hI hj
  · unfold wantG; split <;> simp_all [isJob]

/-- **An iteration starts** on an idle, settled engine: the greedy fill of
the residents, or nothing if there is none. -/
theorem ci_start {g : Ghost} {m : Machine} (hI : CI g m) (hr : m.ready = []) :
    SB g (startIteration D m) ∧ (startIteration D m).sess.size = m.sess.size := by
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
    have hne : it ≠ [] := by rw [hit, hjs']; exact DaiSarathi.fillIter_ne_nil D j0 js 128 hw0 (by norm_num)
    have hne' : it.isEmpty = false := by simpa using hne
    simp only [hne', Bool.not_false, Bool.true_or, ↓reduceIte]
    have hk : tokSum it = min 128 (∑ i ∈ Finset.range m.sess.size, wantG g i) := by
      rw [hit, tokSum_fillIter, ci_demand hI]
    have hmemit : ∀ e ∈ it, ∃ j ∈ m.jobs, e.1 = j.owner := fun e he => by
      obtain ⟨j, hj, h1, -⟩ := mem_fillIter D _ 128 e he
      exact ⟨j, hj, h1⟩
    have hsh : ∀ i < m.sess.size, shareOf it i ≤ if isJob (g.c i) then g.left i else 0 := by
      intro i hi
      have h1 := DaiSarathi.shareOf_fillIter D m.jobs 128 i
      by_cases hj : isJob (g.c i) = true
      · rw [if_pos hj]
        obtain ⟨j, hjm, rfl⟩ := hI.jobsP i hi hj
        rw [DaiSarathi.filter_owner_eq hI.jobsNodup hjm] at h1
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
        tok := by rw [hk]; exact min_le_left _ _ }, hr, fun h => by simp at h, fun _ => ⟨rfl, hk⟩⟩, by first | rfl | trivial⟩

/-! ### A slot -/

/-- After an event on an idle engine: settle, then start an iteration. -/
theorem ci_after_idle {g : Ghost} {m : Machine} (hI : CI g m) (hf : m.ready.length ≤ 10000)
    (hie : m.iterEnd = none) :
    ∃ g', SB g' (afterEvent D m) ∧ (afterEvent D m).sess.size = m.sess.size ∧ SameR m.sess.size g g' := by
  obtain ⟨g', h1, h2, h3, h4, h5⟩ := settle_ci hI hf
  have hie' : (settle D m).iterEnd = none := h3.iterEnd.trans hie
  have hpend : pendingBy (settle D m) (settle D m).now = false := by
    simp [pendingBy, nextEvent, hie', h1.delays]
  have hA : afterEvent D m = startIteration D (settle D m) := by
    unfold afterEvent; simp [hie', hpend]
  obtain ⟨s1, s2⟩ := ci_start h1 h2
  rw [hA]
  exact ⟨g', s1, s2.trans h4, h5⟩

theorem after_busy (m : Machine) {a q : ℕ} (h : (settle D m).iterEnd = some (a, q)) :
    afterEvent D m = settle D m := by
  unfold afterEvent; simp [h]

/-- **A slot**: `k` arrivals, then the running iteration (if any) ends and
the next one starts. -/
theorem slot_sb {g : Ghost} {m : Machine} (hB : SB g m) (k : ℕ) (hk : k ≤ 10000) :
    ∃ g', SB g' (slot k m) ∧ (slot k m).sess.size = m.sess.size + k ∧
      WnC (m.sess.size + k) g' + (if m.iterEnd.isSome then tokSum m.iter else 0) =
        WnC m.sess.size g + 1280 * k := by
  obtain ⟨g1, h1, h2, h3, h4, h5⟩ := ci_injects hB.toCI k
  set m1 := (inject arrived (fun _ => 0))^[k] m with hm1
  have hlen : m1.ready.length ≤ 10000 := by rw [h3, hB.rdy]; simpa using hk
  have hsl : slot k m = if m.iterEnd.isSome then step D (afterEvent D m1) else afterEvent D m1 := rfl
  rcases hie : m.iterEnd with _ | ⟨a, qa⟩
  · have hie1 : m1.iterEnd = none := h4.iterEnd.trans hie
    obtain ⟨g2, s1, s2, s3⟩ := ci_after_idle h1 hlen hie1
    rw [hsl, hie]
    simp only [Option.isSome_none, Bool.false_eq_true, ↓reduceIte, Nat.add_zero]
    refine ⟨g2, s1, by rw [s2, h2], ?_⟩
    rw [h2] at s3
    rw [WnC_congr s3, h5]
  · have hie1 : m1.iterEnd = some (a, qa) := h4.iterEnd.trans hie
    obtain ⟨g2, c1, c2, c3, c4, c5⟩ := settle_ci h1 hlen
    set m2 := settle D m1 with hm2
    have hie2 : m2.iterEnd = some (a, qa) := c3.iterEnd.trans hie1
    have hA : afterEvent D m1 = m2 := after_busy m1 hie2
    have hstep : step D m2 = afterEvent D (handle m2 a qa) := by
      unfold step
      rw [show nextEvent m2 = some (a, qa) by simp [nextEvent, hie2, c1.delays]]
    obtain ⟨e1, e2, e3, e4, e5⟩ := ci_end c1 c2 hie2
    obtain ⟨g4, s1, s2, s3⟩ := ci_after_idle e1 (by omega) e4
    rw [hsl, hie]
    simp only [Option.isSome_some, ↓reduceIte]
    rw [hA, hstep]
    have hz : m2.sess.size = m.sess.size + k := c4.trans h2
    refine ⟨g4, s1, by rw [s2, e5, hz], ?_⟩
    have hiter : m2.iter = m.iter := c3.iter.trans h4.iter
    rw [e5, hz] at s3
    rw [hz] at e2
    rw [h2] at c5
    rw [WnC_congr s3, ← hiter, e2, WnC_congr c5, h5]

/-- The chain starts from no session. -/
theorem empty_sb : SB ⟨fun _ => .e, fun _ => 0⟩ empty := by
  have hs : empty.sess.size = 0 := by simp [empty, Exec.initial]
  refine ⟨{
      sess := fun i hi => by rw [hs] at hi; omega
      jobs := fun j hj => by simp [empty, Exec.initial] at hj
      jobsP := fun i hi _ => by rw [hs] at hi; omega
      jobsNodup := by simp [empty, Exec.initial]
      leftP := fun i hi _ => by rw [hs] at hi; omega
      leftD := fun i hi _ => by rw [hs] at hi; omega
      ready := by simp [empty, Exec.initial]
      readyMem := fun i => by simp [empty, Exec.initial]
      delays := rfl
      iterNone := fun _ => rfl
      iterOwn := fun e he => by simp [empty, Exec.initial] at he
      share := fun i hi => by rw [hs] at hi; omega
      tok := by simp [empty, Exec.initial, tokSum] }, by simp [empty, Exec.initial], fun _ => rfl,
    fun h => by simp [empty, Exec.initial] at h⟩

/-- **Every state of the chain is at a slot's end.** -/
theorem reach_sb {K : ℕ} (hK : K ≤ 10000) {m : Machine} (h : Reach K m) : ∃ g, SB g m := by
  induction h with
  | empty => exact ⟨_, empty_sb⟩
  | slot k hk _ ih =>
    obtain ⟨g, hB⟩ := ih
    obtain ⟨g', h1, -⟩ := slot_sb hB k (hk.trans hK)
    exact ⟨g', h1⟩

/-- At a slot's end the backlog is the ghost's tokens left. -/
theorem ci_backlog {g : Ghost} {m : Machine} (hI : CI g m) (hr : m.ready = []) :
    backlog m = WnC m.sess.size g := by
  unfold backlog WnC
  rw [← ci_sum_jobs hI (rem g) fun i hi hj => ?_]
  · congr 1
    refine List.map_congr_left fun j hj => ?_
    obtain ⟨-, h2, -, h4⟩ := hI.jobs j hj
    rcases h2 with ⟨hc, hm⟩ | ⟨hc, hm⟩
    · simp [rem, hc, hm, h4]
    · simp [rem, hc, hm, h4]
  · have hnr := hI.not_ready hr i hi
    unfold rem
    cases hc : g.c i <;> simp_all [isJob, isReady]

/-- After settling, the backlog is at most the demand plus a decode for
each resident, and there are no more residents than the demand. -/
theorem ci_wn_le {g : Ghost} {m : Machine} (hI : CI g m) (hr : m.ready = []) :
    WnC m.sess.size g ≤ ∑ i ∈ Finset.range m.sess.size, wantG g i +
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
    cases hc : g.c i <;> simp [hc, isReady, isJob] at h1 ⊢
    have := (hI.leftD i hi' hc).2; omega
  · apply Finset.sum_le_sum
    intro i hi
    have hi' := Finset.mem_range.mp hi
    unfold wantG
    cases hc : g.c i <;> simp [isJob]
    exact (hI.leftP i hi' hc).1

/-- A full batch serves 128 tokens and `k` arrivals bring `1280 k`: the
backlog after the slot. -/
theorem backlog_slot {K : ℕ} (hK : K ≤ 10000) (x : State K) (hx : ¬ F x) (k : ℕ) (hk : k ≤ K) :
    backlog (slot k x.1) + 128 = backlog x.1 + 1280 * k := by
  obtain ⟨g, hB⟩ := reach_sb hK x.2
  simp only [F, not_or, not_lt] at hx
  have hbusy : x.1.iterEnd.isSome = true := Option.isSome_iff_ne_none.mpr hx.1
  obtain ⟨hb1, -⟩ := hB.busy hbusy
  have h128 : tokSum x.1.iter = 128 := by have := hB.tok; omega
  obtain ⟨g', s1, s2, s3⟩ := slot_sb hB k (hk.trans hK)
  rw [ci_backlog s1.toCI s1.rdy, ci_backlog hB.toCI hB.rdy, s2]
  rw [if_pos hbusy, h128] at s3
  omega

/-- `F` is small: a batch that is not full served every resident's demand
(`Exec.work_conserving`), so fewer than 128 residents are left, each with
less than one request's work. -/
theorem backlog_lt_of_F {K : ℕ} (hK : K ≤ 10000) (x : State K) (hx : F x) :
    backlog x.1 < 128 * 1280 := by
  obtain ⟨g, hB⟩ := reach_sb hK x.2
  rcases hie : x.1.iterEnd with _ | ⟨a, q⟩
  · simp [backlog, hB.idle hie]
  · have hbusy : x.1.iterEnd.isSome = true := by rw [hie]; rfl
    have htok : x.1.last.stats.tokens < 128 := by
      rcases hx with hx | hx
      · rw [hie] at hx; simp at hx
      · exact hx
    obtain ⟨hb1, hb2⟩ := hB.busy hbusy
    obtain ⟨hW1, hW2⟩ := ci_wn_le hB.toCI hB.rdy
    have hS : ∑ i ∈ Finset.range x.1.sess.size, wantG g i < 128 := by
      rw [hb1] at htok; rw [hb2] at htok; omega
    rw [ci_backlog hB.toCI hB.rdy]
    omega

/-- Foster's drift condition, below capacity. -/
theorem drift {K : ℕ} (A : Arrivals K) (hA : 1280 * A.mean < 128) :
    Drift (kernel A) F (fun x => (backlog x.1 : ℝ)) (ε A) where
  nonneg _ := Nat.cast_nonneg _
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
          have h1 := backlog_slot A.small x hx k hk'
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
  ⟨⟨le_of_lt (by norm_num), fun k => if k = 0 then 19 / 20 else if k = 1 then 1 / 20 else 0,
      fun k => by split_ifs <;> norm_num,
      by simp [Finset.sum_range_succ]; norm_num⟩,
    by simp [Arrivals.mean, Finset.sum_range_succ]; norm_num⟩

end DaiStable
end Papers
end SerqLang
