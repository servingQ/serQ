/-
# Bari et al., Theorem 2, for random arrivals

The RAD program of `examples/papers/bari_rad.sq`, with requests arriving at
random: in each slot (one iteration of the engine) the arrivals are the list
`A.arr o` of (prompt, output) lengths with probability `A.p o`, independently
of the past (`Serq/Chain.lean`). Prompts are whole tiles (Assumption 3) and
outputs at most 512, as in the program. The states are the machines this
reaches from the empty one.

Theorem 2 of the paper: RAD is stable below the bound of Theorem 1. Here the
load is the mean tokens a slot brings, `Σ_o p_o Σ_{(v_p, v_d) ∈ arr o} (v_p + v_d)`,
and the capacity is `b_col = 128` tokens per slot. RAD's batch is full unless
every resident decodes and fewer than 128 do (`optimal_tiling`, the set `F`),
so outside `F` the work at the engine drifts down by `ε = 128 − load`, and
Foster's criterion (`Serq/Foster.lean`) bounds the expected time to reach
`F` by `backlog / ε`.
-/
import Serq.Chain
import Serq.Papers.Bari
import Serq.Papers.DaiStable

namespace SerqLang

namespace Papers
namespace BariStable

open Exec Foster

/-- The deployment of `bari_rad.sq`. -/
abbrev D : Deployment := Claims.BariRad.deployment

/-- What a request runs once it has arrived: `bari_rad.sq`'s session without
its arrival delay. -/
def arrived : Prog :=
  match Claims.BariRad.prog with
  | .run _ _ _ _ k => k
  | p => p

/-- A request's attributes: its prompt in slot 10 and its output in slot 11. -/
def attrs (r : ℕ × ℕ) : ℕ → ℕ := fun s => if s = 10 then r.1 else if s = 11 then r.2 else 0

/-- No request, the engine idle. -/
def empty : Machine := Exec.initial D 0 (fun _ _ => 0) arrived { init := [], computedSlot := some 8 }

/-- One slot whose arrivals have the (prompt, output) lengths `rs`. -/
def slot (rs : List (ℕ × ℕ)) (m : Machine) : Machine := Exec.slotL D arrived (rs.map attrs) m

/-- A request RAD's hypotheses allow: a prompt of 1 to 8 whole tiles
(Assumption 3) and an output of 1 to 512 tokens. -/
def Fits (r : ℕ × ℕ) : Prop := 128 ∣ r.1 ∧ 128 ≤ r.1 ∧ r.1 ≤ 1024 ∧ 1 ≤ r.2 ∧ r.2 ≤ 512

/-- The machines reached from `empty` by slots of at most 10 000 fitting
arrivals. -/
inductive Reach : Machine → Prop
  | empty : Reach empty
  | slot {m : Machine} (rs : List (ℕ × ℕ)) : rs.length ≤ 10000 → (∀ r ∈ rs, Fits r) →
      Reach m → Reach (slot rs m)

/-- The chain's states. -/
def State : Type := {m : Machine // Reach m}

/-- The tokens the engine still has to serve: a prefill's left work and the
output it will then decode (slot 11 of its session), a decode's left work. -/
def backlog (m : Machine) : ℕ :=
  (m.jobs.map fun j => j.left + if j.mode = .prefill then (getS m j.owner).attr.get 11 else 0).sum

/-- The engine is idle, or its batch is not full. -/
def F (x : State) : Prop := x.1.iterEnd = none ∨ x.1.last.stats.tokens < 128

instance : DecidablePred F := fun x => by unfold F; infer_instance

/-- The arrival distribution: outcome `o ≤ N` has probability `p o` and
brings the requests `arr o`. At most 10 000 arrive in a slot, so that each
becomes a job within its slot: that is the proof's bound (it follows one
round of `Exec.drain`), not the paper's. -/
structure Arrivals (N : ℕ) where
  p : ℕ → ℝ
  nonneg : ∀ o, 0 ≤ p o
  sum_one : ∑ o ∈ Finset.range (N + 1), p o = 1
  arr : ℕ → List (ℕ × ℕ)
  small : ∀ o, (arr o).length ≤ 10000
  fits : ∀ o, ∀ r ∈ arr o, Fits r

/-- The tokens outcome `o` brings. -/
def Arrivals.work {N : ℕ} (A : Arrivals N) (o : ℕ) : ℕ := ((A.arr o).map fun r => r.1 + r.2).sum

/-- The mean tokens a slot brings. -/
def Arrivals.load {N : ℕ} (A : Arrivals N) : ℝ := ∑ o ∈ Finset.range (N + 1), A.p o * A.work o

/-- The chain: a slot with the arrivals of outcome `o`, drawn from `A`. -/
noncomputable def kernel {N : ℕ} (A : Arrivals N) : Kernel State :=
  Kernel.ofOutcomes N A.p A.nonneg A.sum_one fun x o =>
    ⟨slot (A.arr o) x.1, Reach.slot _ (A.small o) (A.fits o) x.2⟩

/-- The drift: `ε = 128 − load`. -/
def ε {N : ℕ} (A : Arrivals N) : ℝ := 128 - A.load

/-! ### The invariant of the chain

As for Dai et al. (`Serq/Papers/DaiStable.lean`), each request has a place in
its program (`DaiSarathi.Cat`) and its job's tokens left (`DaiSarathi.Ghost`);
here each request also has its own lengths, `L i = (v_p, v_d)`, read from
slots 10 and 11 of its session. -/

open DaiSarathi (Cat Ghost isReady isJob shareOf Keeps)

def Q4 : Prog := .observe 0 (fun x => x.now - x.attr 9) .stop
def Q3 : Prog := .run 0 .decode (fun x => x.attr 11) none Q4
def Q2 : Prog := .run 0 .prefill (fun x => x.attr 10) none Q3
def Q1 : Prog := .set 9 (fun x => x.now) Q2

theorem arrived_eq_Q1 : arrived = Q1 := rfl

/-- What a session is, in each place an arrived request can be. -/
def Shape (s : Sess) : Cat → Prop
  | .r1 => s.prog = Q1 ∧ s.status = .ready
  | .p => s.prog = Q3 ∧ s.status = .engine
  | .r3 => s.prog = Q3 ∧ s.status = .ready
  | .d => s.prog = Q4 ∧ s.status = .engine
  | .r4 => s.prog = Q4 ∧ s.status = .ready
  | .e => s.status = .ended
  | _ => False

/-- The tokens request `i` has left to be served. -/
def rem (L : ℕ → ℕ × ℕ) (g : Ghost) (i : ℕ) : ℕ :=
  match g.c i with
  | .r1 => (L i).1 + (L i).2
  | .p => g.left i + (L i).2
  | .r3 => (L i).2
  | .d => g.left i
  | _ => 0

/-- The tokens arrived and not yet served, over the first `n` requests. -/
def WnC (n : ℕ) (L : ℕ → ℕ × ℕ) (g : Ghost) : ℕ := ∑ i ∈ Finset.range n, rem L g i

/-- The invariant of an instant. -/
structure CI (L : ℕ → ℕ × ℕ) (g : Ghost) (m : Machine) : Prop where
  sess : ∀ i < m.sess.size, Shape (getS m i) (g.c i) ∧ (getS m i).stack = [] ∧
    (getS m i).attr.get 10 = (L i).1 ∧ (getS m i).attr.get 11 = (L i).2 ∧ Fits (L i)
  jobs : ∀ j ∈ m.jobs, j.owner < m.sess.size ∧
    ((g.c j.owner = .p ∧ j.mode = .prefill) ∨ (g.c j.owner = .d ∧ j.mode = .decode)) ∧
    j.growing = none ∧ j.left = g.left j.owner
  jobsP : ∀ i < m.sess.size, isJob (g.c i) = true → ∃ j ∈ m.jobs, j.owner = i
  jobsNodup : (m.jobs.map (·.owner)).Nodup
  leftP : ∀ i < m.sess.size, g.c i = .p → 1 ≤ g.left i ∧ 128 ∣ g.left i
  leftD : ∀ i < m.sess.size, g.c i = .d → 1 ≤ g.left i ∧ g.left i ≤ 512
  ready : m.ready.Nodup
  readyMem : ∀ i, i ∈ m.ready ↔ i < m.sess.size ∧ isReady (g.c i) = true
  delays : m.delays = []
  iterNone : m.iterEnd = none → m.iter = []
  iterOwn : ∀ e ∈ m.iter, e.1 < m.sess.size
  share : ∀ i < m.sess.size, shareOf m.iter i ≤ if isJob (g.c i) then g.left i else 0
  shareP : ∀ i < m.sess.size, g.c i = .p → 128 ∣ shareOf m.iter i
  tok : tokSum m.iter ≤ 128

/-- At a slot's end: settled, an idle engine has no job, and a running
batch is full or every resident decodes in it. -/
structure SB (L : ℕ → ℕ × ℕ) (g : Ghost) (m : Machine) : Prop extends CI L g m where
  rdy : m.ready = []
  idle : m.iterEnd = none → m.jobs = []
  busy : m.iterEnd.isSome = true → m.last.stats.tokens = tokSum m.iter ∧
    (tokSum m.iter = 128 ∨ ((∀ j ∈ m.jobs, j.mode = .decode) ∧ tokSum m.iter = m.jobs.length))

theorem CI.lt_of_ready {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) : i < m.sess.size ∧ isReady (g.c i) = true :=
  (hI.readyMem i).mp (by rw [hr]; simp)

theorem CI.owner_ne {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) {i : ℕ}
    (hc : isJob (g.c i) = false) : ∀ j ∈ m.jobs, j.owner ≠ i := by
  intro j hj he
  obtain ⟨-, h, -⟩ := hI.jobs j hj
  rw [he] at h
  rcases h with ⟨h, -⟩ | ⟨h, -⟩ <;> rw [h] at hc <;> simp [isJob] at hc

theorem CI.not_ready {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) (hr : m.ready = []) :
    ∀ i < m.sess.size, isReady (g.c i) = false := by
  intro i hi
  by_contra h
  have := (hI.readyMem i).mpr ⟨hi, by simpa using h⟩
  rw [hr] at this; simp at this

theorem attr_upd11 (a : Attrs) (k v : ℕ) (hk : k ≠ 11) : (a.upd k v).get 11 = a.get 11 := by
  rw [Attrs.get_upd]; simp [Function.update, Ne.symm hk]

/-- A ready session starts a job at the engine. -/
theorem ci_addJob {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hcj : isJob (g.c i) = false)
    (k : Cat) (md : Mode) (l : ℕ)
    (hk : (k = .p ∧ md = .prefill ∧ 1 ≤ l ∧ 128 ∣ l) ∨ (k = .d ∧ md = .decode ∧ 1 ≤ l ∧ l ≤ 512))
    (M : Machine) (s' : Sess) (a b : List Job) (hab : a ++ b = m.jobs)
    (hsz : M.sess.size = m.sess.size)
    (hget : ∀ j, getS M j = if j = i then s' else getS m j)
    (hjobs : M.jobs = a ++ ⟨i, md, l, none⟩ :: b) (hready : M.ready = rest)
    (hdl : M.delays = m.delays) (hit : M.iter = m.iter) (hie : M.iterEnd = m.iterEnd)
    (hs' : Shape s' k ∧ s'.stack = [] ∧ s'.attr.get 10 = (L i).1 ∧ s'.attr.get 11 = (L i).2) :
    CI L (g.set i k l) M := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hkr : isReady k = false := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> rfl
  have hown := hI.owner_ne hcj
  have hgc : ∀ j, (g.set i k l).c j = if j = i then k else g.c j := DaiSarathi.Ghost.set_c g i k l
  have hgl : ∀ j, (g.set i k l).left j = if j = i then l else g.left j := DaiSarathi.Ghost.set_left g i k l
  have hmem : ∀ x, x ∈ M.jobs ↔ x ∈ m.jobs ∨ x = ⟨i, md, l, none⟩ := by
    intro x
    rw [hjobs, ← hab]
    simp only [List.mem_append, List.mem_cons]
    tauto
  have hnd' : rest.Nodup := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
  have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  have hsh0 : shareOf m.iter i = 0 := by
    have := hI.share i hin
    rw [hcj, if_neg (by simp)] at this
    omega
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
      shareP := fun j hj hjc => ?_
      tok := by rw [hit]; exact hI.tok }
  · rw [hsz] at hj
    rw [hget, hgc]
    split_ifs with h
    · subst h; exact ⟨hs'.1, hs'.2.1, hs'.2.2.1, hs'.2.2.2, (hI.sess j hj).2.2.2.2⟩
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
    have hp : List.Perm ((a ++ ⟨i, md, l, none⟩ :: b).map (·.owner)) (i :: (a ++ b).map (·.owner)) := by
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
    · subst h; rw [hsh0]; exact Nat.zero_le _
    · simp only [h, if_false]; exact hI.share j hj
  · rw [hsz] at hj
    rw [hit]
    by_cases h : j = i
    · subst h; rw [hsh0]; exact dvd_zero _
    · rw [hgc, if_neg h] at hjc; exact hI.shareP j hj hjc

/-- An arrived request sets `t0` and starts its prefill. -/
theorem ci_r1 {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    CI L (g.set i .p (L i).1) (exec D 10000 { m with ready := rest } i) ∧
      (exec D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec D 10000 { m with ready := rest } i) m ∧
      (exec D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, h10, h11, hfit⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    DaiSarathi.exec_set D _ m0 i 9 (fun x => x.now) Q2 (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl)]
  set s1 : Sess := { getS m0 i with attr := (getS m0 i).attr.upd 9 (evalE m0 i fun x => x.now), prog := Q2 }
    with hs1
  set m1 := setS m0 i s1 with hm1
  have hg1 : getS m1 i = s1 := KongSvf.getS_setS_self m0 s1 (by simpa using hisz)
  have a10 : s1.attr.get 10 = (L i).1 := (DaiSarathi.attr_upd10 _ _ _ (by decide)).trans h10
  have a11 : s1.attr.get 11 = (L i).2 := (attr_upd11 _ _ _ (by decide)).trans h11
  have hev : evalE m1 i (fun x => x.attr 10) = (L i).1 := by
    show (getS m1 i).attr.get 10 = _
    rw [hg1]; exact a10
  obtain ⟨a, b, hab, he⟩ := DaiSarathi.exec_runEngine' D 9998 m1 i .prefill (fun x => x.attr 10) Q3
    (by rw [hg1]; exact hst) (by rw [hg1]; rfl) (by rw [hev]; have := hfit.2.1; omega)
  rw [he, hev]
  set s2 : Sess := { getS m1 i with prog := Q3, status := .engine } with hs2
  refine ⟨ci_addJob hI hr (by rw [hc]; rfl) .p .prefill (L i).1
    (Or.inl ⟨rfl, rfl, by have := hfit.2.1; omega, hfit.1⟩)
    _ s2 a b hab (by simp [hm1, hm0]) (fun j => ?_) rfl rfl rfl rfl rfl ?_, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩,
    by simp [hm1, hm0]⟩
  · show getS (setS m1 i s2) j = _
    rw [DaiSarathi.getS_setS m1 s2 (by simpa [hm1, hm0] using hisz)]
    split_ifs with h
    · rfl
    · rw [hm1, DaiSarathi.getS_setS m0 s1 (by simpa [hm0] using hisz), if_neg h]; rfl
  · rw [hs2, hg1]
    exact ⟨⟨rfl, rfl⟩, hstk, a10, a11⟩

/-- A prefilled request starts its decode. -/
theorem ci_r3 {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r3) :
    CI L (g.set i .d (L i).2) (exec D 10000 { m with ready := rest } i) ∧
      (exec D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec D 10000 { m with ready := rest } i) m ∧
      (exec D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, h10, h11, hfit⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  have hev : evalE m0 i (fun x => x.attr 11) = (L i).2 := h11
  obtain ⟨a, b, hab, he⟩ := DaiSarathi.exec_runEngine' D 9999 m0 i .decode (fun x => x.attr 11) Q4
    (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl) (by rw [hev]; have := hfit.2.2.2.1; omega)
  rw [show (10000 : ℕ) = 9999 + 1 from rfl, he, hev]
  set s2 : Sess := { getS m0 i with prog := Q4, status := .engine } with hs2
  refine ⟨ci_addJob hI hr (by rw [hc]; rfl) .d .decode (L i).2
    (Or.inr ⟨rfl, rfl, hfit.2.2.2.1, hfit.2.2.2.2⟩)
    _ s2 a b hab (by simp [hm0]) (fun j => ?_) rfl rfl rfl rfl rfl ?_, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩,
    by simp [hm0]⟩
  · show getS (setS m0 i s2) j = _
    rw [DaiSarathi.getS_setS m0 s2 (by simpa [hm0] using hisz)]; rfl
  · rw [hs2, hg0]
    exact ⟨⟨rfl, rfl⟩, hstk, h10, h11⟩

/-- A decoded request observes its latency and ends. -/
theorem ci_r4 {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r4) :
    CI L (g.set i .e 0) (exec D 10000 { m with ready := rest } i) ∧
      (exec D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec D 10000 { m with ready := rest } i) m ∧
      (exec D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, h10, h11, hfit⟩ := hI.sess i hin
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
  have hs'a : s'.attr = (getS m i).attr := by rw [hs', hg1]; rfl
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
      shareP := fun j hj hjc => ?_
      tok := hI.tok }, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩, hsz⟩
  · rw [hsz] at hj
    rw [hget, hgc]
    split_ifs with h
    · subst h; exact ⟨rfl, rfl, by rw [hs'a]; exact h10, by rw [hs'a]; exact h11, hfit⟩
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
  · rw [hsz] at hj
    show 128 ∣ shareOf m.iter j
    rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; exact hI.shareP j hj hjc

/-- Draining keeps every request's tokens left. -/
def SameR (n : ℕ) (L : ℕ → ℕ × ℕ) (g g' : Ghost) : Prop := ∀ j < n, rem L g' j = rem L g j

theorem SameR.refl (n : ℕ) (L : ℕ → ℕ × ℕ) (g : Ghost) : SameR n L g g := fun _ _ => rfl

theorem SameR.trans {n : ℕ} {L : ℕ → ℕ × ℕ} {a b c : Ghost} (h1 : SameR n L a b) (h2 : SameR n L b c) :
    SameR n L a c :=
  fun j hj => (h2 j hj).trans (h1 j hj)

theorem rem_set (L : ℕ → ℕ × ℕ) (g : Ghost) (i : ℕ) (k : Cat) (l j : ℕ) (hj : j ≠ i) :
    rem L (g.set i k l) j = rem L g j := by
  simp only [rem, DaiSarathi.Ghost.set_c, DaiSarathi.Ghost.set_left, hj, if_false]

theorem SameR.set {n : ℕ} (L : ℕ → ℕ × ℕ) (g : Ghost) (i : ℕ) (k : Cat) (l : ℕ)
    (hr : rem L (g.set i k l) i = rem L g i) : SameR n L g (g.set i k l) := fun j _ => by
  by_cases h : j = i
  · subst h; exact hr
  · exact rem_set L g i k l j h

theorem WnC_congr {n : ℕ} {L : ℕ → ℕ × ℕ} {g g' : Ghost} (h : SameR n L g g') : WnC n L g' = WnC n L g :=
  Finset.sum_congr rfl fun j hj => h j (Finset.mem_range.mp hj)

/-- **Draining.** -/
theorem drain_ci {L : ℕ → ℕ × ℕ} : ∀ (f : ℕ) (g : Ghost) (m : Machine), CI L g m → m.ready.length ≤ f →
    ∃ g', CI L g' (drain D f m) ∧ (drain D f m).ready = [] ∧ Keeps (drain D f m) m ∧
      (drain D f m).sess.size = m.sess.size ∧ SameR m.sess.size L g g'
  | 0, g, m, hI, hf => by
    have hr : m.ready = [] := List.eq_nil_of_length_eq_zero (by omega)
    exact ⟨g, hI, by simp [drain, hr], DaiSarathi.Keeps.refl m, rfl, SameR.refl _ L g⟩
  | f + 1, g, m, hI, hf => by
    unfold drain
    split
    · rename_i hr
      exact ⟨g, hI, hr, DaiSarathi.Keeps.refl m, rfl, SameR.refl _ L g⟩
    · rename_i i rest hr
      have hlen : rest.length ≤ f := by rw [hr] at hf; simpa using hf
      obtain ⟨hin, hcr⟩ := hI.lt_of_ready hr
      have hsh := (hI.sess i hin).1
      obtain ⟨g1, hI1, hr1, hk1, hz1, hs1⟩ : ∃ g1, CI L g1 (exec D 10000 { m with ready := rest } i) ∧
          (exec D 10000 { m with ready := rest } i).ready = rest ∧
          Keeps (exec D 10000 { m with ready := rest } i) m ∧
          (exec D 10000 { m with ready := rest } i).sess.size = m.sess.size ∧ SameR m.sess.size L g g1 := by
        cases hc : g.c i with
        | r1 =>
          obtain ⟨h1, h2, h3, h4⟩ := ci_r1 hI hr hc
          exact ⟨_, h1, h2, h3, h4, SameR.set L g i .p _ (by simp [rem, DaiSarathi.Ghost.set_c,
            DaiSarathi.Ghost.set_left, hc])⟩
        | r3 =>
          obtain ⟨h1, h2, h3, h4⟩ := ci_r3 hI hr hc
          exact ⟨_, h1, h2, h3, h4, SameR.set L g i .d _ (by simp [rem, DaiSarathi.Ghost.set_c,
            DaiSarathi.Ghost.set_left, hc])⟩
        | r4 =>
          obtain ⟨h1, h2, h3, h4⟩ := ci_r4 hI hr hc
          exact ⟨_, h1, h2, h3, h4, SameR.set L g i .e 0 (by simp [rem, DaiSarathi.Ghost.set_c, hc])⟩
        | s0 => rw [hc] at hsh; exact hsh.elim
        | w => rw [hc] at hsh; exact hsh.elim
        | _ => rw [hc] at hcr; simp [isReady] at hcr
      obtain ⟨g2, hI2, hr2, hk2, hz2, hs2⟩ := drain_ci f g1 _ hI1 (by rw [hr1]; exact hlen)
      rw [hz1] at hs2
      exact ⟨g2, hI2, hr2, hk1.trans hk2, hz2.trans hz1, hs1.trans hs2⟩

/-- **Settling an instant** of at most 10 000 ready requests. -/
theorem settle_ci {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) (hf : m.ready.length ≤ 10000) :
    ∃ g', CI L g' (settle D m) ∧ (settle D m).ready = [] ∧ Keeps (settle D m) m ∧
      (settle D m).sess.size = m.sess.size ∧ SameR m.sess.size L g g' := by
  obtain ⟨g', h1, h2, h3, h4, h5⟩ := drain_ci 10000 g m hI hf
  have hs : settle D m = drain D 10000 m := by
    unfold settle
    rw [show (1000 : ℕ) = 999 + 1 from rfl, settleLoop]
    simp only [BariRad.admitAll_rad, h2, List.isEmpty_nil, ↓reduceIte]
  rw [hs]
  exact ⟨g', h1, h2, h3, h4, h5⟩

/-! ### Arrivals -/

theorem getS_inject (r : ℕ × ℕ) (m : Machine) (j : ℕ) :
    getS (inject arrived (attrs r) m) j =
      if j = m.sess.size then ⟨m.sess.size, ⟨attrs r, []⟩, 0, arrived, [], .ready, 0, 0⟩ else getS m j := by
  unfold getS inject
  simp only [Array.getD_eq_getD_getElem?, Array.getElem?_push]
  split_ifs with h
  · subst h; simp
  · rfl

/-- **One arrival.** -/
theorem ci_inject {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) (r : ℕ × ℕ) (hr : Fits r) :
    CI (Function.update L m.sess.size r) (g.set m.sess.size .r1 0) (inject arrived (attrs r) m) ∧
      (inject arrived (attrs r) m).sess.size = m.sess.size + 1 ∧
      (inject arrived (attrs r) m).ready = m.ready ++ [m.sess.size] ∧
      Keeps (inject arrived (attrs r) m) m ∧
      WnC (m.sess.size + 1) (Function.update L m.sess.size r) (g.set m.sess.size .r1 0) =
        WnC m.sess.size L g + (r.1 + r.2) := by
  set n := m.sess.size with hn
  set L' := Function.update L n r with hL'
  set M := inject arrived (attrs r) m with hM
  have hsz : M.sess.size = n + 1 := by simp [hM, inject, hn]
  have hget := getS_inject r m
  have hLn : L' n = r := by simp [hL']
  have hLj : ∀ j, j ≠ n → L' j = L j := fun j h => by simp [hL', h]
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
      shareP := fun j hj hjc => ?_
      tok := hI.tok }, hsz, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩, ?_⟩
  · rw [hsz] at hj
    rw [hget, hgc]
    by_cases h : j = n
    · rw [if_pos h, if_pos h]
      subst h
      rw [hLn]
      refine ⟨⟨rfl, rfl⟩, rfl, ?_, ?_, hr⟩ <;> simp [Attrs.get, attrs]
    · rw [if_neg h, if_neg h, hLj j h]; exact hI.sess j (by omega)
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
    · subst h; rw [if_pos rfl, if_pos rfl, DaiStable.shareOf_eq_zero hio]; exact Nat.zero_le _
    · rw [if_neg h, if_neg h]; exact hI.share j (by omega)
  · rw [hsz] at hj
    show 128 ∣ shareOf m.iter j
    rw [hgc] at hjc
    by_cases h : j = n
    · rw [if_pos h] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; exact hI.shareP j (by omega) hjc
  · unfold WnC
    rw [Finset.sum_range_succ]
    have h1 : ∑ i ∈ Finset.range n, rem L' (g.set n .r1 0) i = ∑ i ∈ Finset.range n, rem L g i := by
      refine Finset.sum_congr rfl fun j hj => ?_
      have hjn : j ≠ n := Nat.ne_of_lt (Finset.mem_range.mp hj)
      rw [rem_set L' g n .r1 0 j hjn]
      simp only [rem, hLj j hjn]
    have h2 : rem L' (g.set n .r1 0) n = r.1 + r.2 := by simp [rem, hgc, hLn]
    rw [h1, h2]

/-- **The arrivals of a slot.** -/
theorem ci_injects : ∀ (rs : List (ℕ × ℕ)) {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine}, CI L g m →
    (∀ r ∈ rs, Fits r) → ∃ L' g',
    CI L' g' ((rs.map attrs).foldl (fun m a => inject arrived a m) m) ∧
      ((rs.map attrs).foldl (fun m a => inject arrived a m) m).sess.size = m.sess.size + rs.length ∧
      ((rs.map attrs).foldl (fun m a => inject arrived a m) m).ready.length = m.ready.length + rs.length ∧
      Keeps ((rs.map attrs).foldl (fun m a => inject arrived a m) m) m ∧
      WnC (m.sess.size + rs.length) L' g' = WnC m.sess.size L g + (rs.map fun r => r.1 + r.2).sum
  | [], L, g, m, hI, _ => ⟨L, g, hI, rfl, rfl, DaiSarathi.Keeps.refl m, by simp⟩
  | r :: rs, L, g, m, hI, hf => by
    obtain ⟨c1, c2, c3, c4, c5⟩ := ci_inject hI r (hf r List.mem_cons_self)
    obtain ⟨L2, g2, h1, h2, h3, h4, h5⟩ := ci_injects rs c1 fun x hx => hf x (List.mem_cons_of_mem _ hx)
    simp only [List.map_cons, List.foldl_cons, List.length_cons, List.sum_cons]
    refine ⟨L2, g2, h1, by rw [h2, c2]; ring, by rw [h3, c3]; simp; ring, c4.trans h4, ?_⟩
    rw [c2] at h5
    rw [show m.sess.size + (rs.length + 1) = m.sess.size + 1 + rs.length by ring, h5, c5]
    ring

/-! ### An iteration ends -/

/-- **An iteration ends.** Its tokens leave the backlog; the requests that
finish are ready, at most one per token. -/
theorem ci_end {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) (hr : m.ready = []) {a qa : ℕ}
    (hie : m.iterEnd = some (a, qa)) :
    CI L (g.tick m.iter) (handle m a qa) ∧
      WnC m.sess.size L (g.tick m.iter) + tokSum m.iter = WnC m.sess.size L g ∧
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
  have hrem : ∀ j < n, rem L g' j + shareOf it j = rem L g j := by
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
  have hWn : WnC n L g' + tokSum it = WnC n L g := by
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
      shareP := fun j hj _ => by rw [hRit]; simp [shareOf]
      tok := by rw [hRit]; simp [tokSum] }, hWn, ?_, hRie, hRsz⟩
  · rw [hRsz] at hj
    rw [hgetR]
    by_cases hd : j ∈ done
    · rw [if_pos hd]
      obtain ⟨hs, hst, h10, h11, hfit⟩ := hI.sess j hj
      rcases hfinc j hj hd with ⟨hc, hc'⟩ | ⟨hc, hc'⟩
      · rw [hc] at hs; rw [hc']; exact ⟨⟨hs.1, rfl⟩, hst, h10, h11, hfit⟩
      · rw [hc] at hs; rw [hc']; exact ⟨⟨hs.1, rfl⟩, hst, h10, h11, hfit⟩
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
      have h2 := hI.shareP j hj hjc
      have : ¬ g.left j ≤ shareOf it j := fun h => hd ((hmemd j).mpr ⟨hj, hjj, h⟩)
      rw [hgl]
      exact ⟨by omega, Nat.dvd_sub h1.2 h2⟩
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

theorem ci_sum_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) (f : ℕ → ℕ)
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

/-- RAD's invariant `R` (`Serq/Papers/Bari.lean`) holds on an idle machine. -/
theorem ci_R {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) (hit : m.iter = []) : BariRad.R m := by
  refine BariRad.R.mk' (fun j hj => ?_) hI.jobsNodup (fun s hs => ?_) (fun s hs => ?_)
    (fun e he => by rw [hit] at he; simp at he) (fun e he => by rw [hit] at he; simp at he)
  · obtain ⟨h1, h2, h3, h4⟩ := hI.jobs j hj
    have hsh := (hI.sess _ h1).1
    rcases h2 with ⟨hc, hm⟩ | ⟨hc, hm⟩
    · have hl := hI.leftP _ h1 hc
      rw [hc] at hsh
      refine ⟨by rw [h4]; omega, fun _ => by rw [h4]; exact hl.2, hsh.2, by rw [hm]; decide, h3⟩
    · have hl := hI.leftD _ h1 hc
      rw [hc] at hsh
      refine ⟨by rw [h4]; omega, fun h => by rw [hm] at h; exact absurd h (by decide), hsh.2,
        by rw [hm]; decide, h3⟩
  · rw [DaiSarathi.sess_toList] at hs
    obtain ⟨i, hi, rfl⟩ := List.mem_map.mp hs
    obtain ⟨-, -, h10, -, hfit⟩ := hI.sess i (List.mem_range.mp hi)
    rw [h10]; exact hfit.1
  · rw [DaiSarathi.sess_toList] at hs
    obtain ⟨i, hi, rfl⟩ := List.mem_map.mp hs
    exact (hI.sess i (List.mem_range.mp hi)).2.1

/-- RAD's batch is full, or every resident decodes and gets one token. -/
theorem rad_full (m : Machine) (h : BariRad.R m) :
    tokSum (fillIter D (m.jobs.filter (serves D m)) 128) = 128 ∨
      ((∀ j ∈ m.jobs, j.mode = .decode) ∧
        tokSum (fillIter D (m.jobs.filter (serves D m)) 128) = m.jobs.length) := by
  set S := m.jobs.filter (serves D m) with hS
  have hFm : ∀ j ∈ S, j ∈ m.jobs := fun j hj => (List.mem_filter.mp hj).1
  by_cases hc : BariRad.decodeMode m
  · have hdec : ∀ j ∈ S, j.mode = .decode := fun j hj =>
      ((BariRad.serves_rad m j).mp (List.mem_filter.mp hj).2).1 hc
    have hw1 : ∀ j ∈ S, wantOf D j = 1 := fun j hj => by
      have := (h.jobs j (hFm j hj)).1
      simp only [wantOf, hdec j hj]; omega
    have hFd : S = m.jobs.filter (·.mode = .decode) := by
      rw [hS]
      apply List.filter_congr
      intro j _
      have := BariRad.serves_rad m j
      by_cases hd : j.mode = .decode
      · simp only [hd, decide_true]; exact this.mpr ⟨fun _ => hd, fun h' => absurd hc h'⟩
      · simp only [hd, decide_false]
        cases hs : serves D m j
        · rfl
        · exact absurd ((this.mp hs).1 hc) hd
    have hsum : (S.map (wantOf D)).sum = S.length := by
      rw [List.map_congr_left hw1]; simp
    rw [tokSum_fillIter, hsum]
    by_cases hl : 128 ≤ S.length
    · left; omega
    · right
      have hlen : S.length = m.jobs.length := by
        rcases hc with hc | hc
        · rw [hFd] at hl; omega
        · rw [hFd]; exact hc
      refine ⟨fun j hj => ?_, by omega⟩
      have hall : m.jobs.filter (fun j : Job => decide (j.mode = .decode)) = m.jobs :=
        List.Sublist.eq_of_length (List.filter_sublist) (by rw [← hFd]; exact hlen)
      have : j ∈ m.jobs.filter (fun j : Job => decide (j.mode = .decode)) := by rw [hall]; exact hj
      simpa using (List.mem_filter.mp this).2
  · left
    have hpre : ∀ j ∈ S, j.mode = .prefill := fun j hj => by
      have h1 := ((BariRad.serves_rad m j).mp (List.mem_filter.mp hj).2).2 hc
      have h2 := (h.jobs j (hFm j hj)).2.2.2.1
      cases hm : j.mode <;> simp_all
    have hne : S ≠ [] := by
      intro he
      apply hc
      right
      have : ∀ j ∈ m.jobs, j.mode = .decode := by
        intro j hj
        by_contra hd
        have : j ∈ S := List.mem_filter.mpr ⟨hj, (BariRad.serves_rad m j).mpr ⟨fun h' => absurd h' hc, fun _ => hd⟩⟩
        rw [he] at this; simp at this
      rw [List.filter_eq_self.mpr (by simpa using this)]
    obtain ⟨f, rest, hfr⟩ := List.exists_cons_of_ne_nil hne
    have hwf : wantOf D f = 128 := by
      have hj := h.jobs f (hFm f (by rw [hfr]; exact List.mem_cons_self))
      have hp := hpre f (by rw [hfr]; exact List.mem_cons_self)
      obtain ⟨c, hcf⟩ := hj.2.1 hp
      have : 0 < c := by rcases Nat.eq_zero_or_pos c with rfl | h0 <;> simp_all
      simp only [wantOf, hp, D, Claims.BariRad.deployment]
      simp; rw [hcf]; omega
    have hfill : fillIter D S 128 = [(f.owner, 128)] := by
      rw [hfr]; unfold fillIter; rw [hwf]; simp
    rw [hfill]; simp [tokSum]

/-- A list's entries for `i` are each a whole tile. -/
theorem dvd_shareOf (l : List (ℕ × ℕ)) (i : ℕ) (h : ∀ e ∈ l, e.1 = i → e.2 = 128) : 128 ∣ shareOf l i := by
  induction l with
  | nil => simp [shareOf]
  | cons e l ih =>
    rw [DaiSarathi.shareOf_cons]
    have := ih fun x hx => h x (List.mem_cons_of_mem _ hx)
    split_ifs with he
    · rw [h e List.mem_cons_self he]; exact Nat.dvd_add (dvd_refl _) this
    · simpa using this

theorem wantOf_le (j : Job) : wantOf D j ≤ j.left := by
  unfold wantOf
  split
  · exact min_le_right _ _
  · split
    · exact min_le_left _ _
    · exact le_rfl

/-- **An iteration starts** on an idle, settled engine. -/
theorem ci_start {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) (hr : m.ready = [])
    (hie : m.iterEnd = none) :
    SB L g (startIteration D m) ∧ (startIteration D m).sess.size = m.sess.size := by
  have hq : engineQueuesEmpty D m := fun p hp => by simp [pdef, D, Claims.BariRad.deployment] at hp
  have hg : ∀ j ∈ m.jobs, j.growing = none := fun j hj => (hI.jobs j hj).2.2.1
  have ha := assign_eq_fillIter_only D m hq hg m.preempts (m.jobs.length + 100000) 0 128 [] (by omega)
  simp only [List.drop_zero, List.nil_append] at ha
  have hvia : ((List.range D.pools.length).any fun p =>
      (pdef D p).viaEngine && !(pst m p).queue.isEmpty) = false := by
    simp [D, Claims.BariRad.deployment]
  have hb : D.budget = 128 := rfl
  have hR := ci_R hI (hI.iterNone hie)
  unfold startIteration
  -- the deployment has no `chunkLift`: every iteration runs it as it is
  simp only [iterDeployment_of_none _ (rfl : Claims.BariRad.deployment.chunkLift = none)]
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
  · have hne0 : m.jobs.isEmpty = false := by simpa using hjs
    simp only [hne0, Bool.not_false, ↓reduceIte]
    rw [hb, ha]
    set it := fillIter D (m.jobs.filter (serves D m)) 128 with hit
    have hfull := rad_full m hR
    rw [← hit] at hfull
    obtain ⟨hT, -⟩ := BariRad.rad_batch m hR
    rw [← hit] at hT
    have hne : it ≠ [] := by
      intro h0
      rw [h0] at hfull
      rcases hfull with h | ⟨-, h⟩
      · simp [tokSum] at h
      · simp [tokSum] at h; exact hjs (List.eq_nil_of_length_eq_zero h.symm)
    have hne' : it.isEmpty = false := by simpa using hne
    simp only [hne', Bool.not_false, Bool.true_or, ↓reduceIte]
    have htok : tokSum it ≤ 128 := by rw [hit, tokSum_fillIter]; exact min_le_left _ _
    have hmemit : ∀ e ∈ it, ∃ j ∈ m.jobs, e.1 = j.owner := fun e he => by
      obtain ⟨j, hj, h1, -⟩ := mem_fillIter D _ 128 e he
      exact ⟨j, (List.mem_filter.mp hj).1, h1⟩
    have hsh : ∀ i < m.sess.size, shareOf it i ≤ if isJob (g.c i) then g.left i else 0 := by
      intro i hi
      have h1 := DaiSarathi.shareOf_fillIter D (m.jobs.filter (serves D m)) 128 i
      rw [← hit] at h1
      have h2 : (((m.jobs.filter (serves D m)).filter fun j => decide (j.owner = i)).map (wantOf D)).sum ≤
          ((m.jobs.filter fun j => decide (j.owner = i)).map (wantOf D)).sum :=
        List.Sublist.sum_le_sum ((List.filter_sublist.filter _).map _) (fun _ _ => Nat.zero_le _)
      by_cases hj : isJob (g.c i) = true
      · rw [if_pos hj]
        obtain ⟨j, hjm, rfl⟩ := hI.jobsP i hi hj
        rw [DaiSarathi.filter_owner_eq hI.jobsNodup hjm] at h2
        simp only [List.map_cons, List.map_nil, List.sum_cons, List.sum_nil, Nat.add_zero] at h2
        have h3 := wantOf_le j
        rw [(hI.jobs j hjm).2.2.2] at h3
        omega
      · rw [if_neg hj]
        have : m.jobs.filter (fun x => decide (x.owner = i)) = [] := by
          rw [List.filter_eq_nil_iff]
          intro x hx
          simp only [decide_eq_true_eq]
          intro he
          obtain ⟨-, h2, -⟩ := hI.jobs x hx
          rw [he] at h2
          rcases h2 with ⟨hc, -⟩ | ⟨hc, -⟩ <;> rw [hc] at hj <;> simp [isJob] at hj
        rw [this, List.map_nil, List.sum_nil] at h2; omega
    have hshP : ∀ i < m.sess.size, g.c i = .p → 128 ∣ shareOf it i := by
      intro i hi hc
      obtain ⟨j, hjm, rfl⟩ := hI.jobsP i hi (by rw [hc]; rfl)
      have hmj : j.mode = .prefill := by
        rcases (hI.jobs j hjm).2.1 with ⟨-, hm⟩ | ⟨hc', -⟩
        · exact hm
        · rw [hc] at hc'; exact absurd hc' (by decide)
      apply dvd_shareOf
      intro e he heo
      rcases hT e he with h | h
      · exact h
      · have := h j hjm heo.symm
        rw [hmj] at this; exact absurd this (by decide)
    exact ⟨⟨{ hI with
        iterNone := fun h => by simp at h
        iterOwn := fun e he => by
          obtain ⟨j, hj, he1⟩ := hmemit e he
          rw [he1]; exact (hI.jobs j hj).1
        share := hsh
        shareP := hshP
        tok := htok }, hr, fun h => by simp at h, fun _ => ⟨rfl, hfull⟩⟩, by first | rfl | trivial⟩

/-! ### A slot -/

/-- After an event on an idle engine: settle, then start an iteration. -/
theorem ci_after_idle {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m)
    (hf : m.ready.length ≤ 10000) (hie : m.iterEnd = none) :
    ∃ g', SB L g' (afterEvent D m) ∧ (afterEvent D m).sess.size = m.sess.size ∧
      SameR m.sess.size L g g' := by
  obtain ⟨g', h1, h2, h3, h4, h5⟩ := settle_ci hI hf
  have hie' : (settle D m).iterEnd = none := h3.iterEnd.trans hie
  have hpend : pendingBy (settle D m) (settle D m).now = false := by
    simp [pendingBy, nextEvent, hie', h1.delays]
  have hA : afterEvent D m = startIteration D (settle D m) := by
    unfold afterEvent; simp [hie', hpend]
  obtain ⟨s1, s2⟩ := ci_start h1 h2 hie'
  rw [hA]
  exact ⟨g', s1, s2.trans h4, h5⟩

theorem after_busy (m : Machine) {a q : ℕ} (h : (settle D m).iterEnd = some (a, q)) :
    afterEvent D m = settle D m := by
  unfold afterEvent; simp [h]

/-- **A slot**: the arrivals, then the running iteration (if any) ends and
the next one starts. -/
theorem slot_sb {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hB : SB L g m) (rs : List (ℕ × ℕ))
    (hl : rs.length ≤ 10000) (hf : ∀ r ∈ rs, Fits r) :
    ∃ L' g', SB L' g' (slot rs m) ∧ (slot rs m).sess.size = m.sess.size + rs.length ∧
      WnC (m.sess.size + rs.length) L' g' + (if m.iterEnd.isSome then tokSum m.iter else 0) =
        WnC m.sess.size L g + (rs.map fun r => r.1 + r.2).sum := by
  obtain ⟨L1, g1, h1, h2, h3, h4, h5⟩ := ci_injects rs hB.toCI hf
  set m1 := (rs.map attrs).foldl (fun m a => inject arrived a m) m with hm1
  have hlen : m1.ready.length ≤ 10000 := by rw [h3, hB.rdy]; simpa using hl
  have hsl : slot rs m = if m.iterEnd.isSome then step D (afterEvent D m1) else afterEvent D m1 := rfl
  rcases hie : m.iterEnd with _ | ⟨a, qa⟩
  · have hie1 : m1.iterEnd = none := h4.iterEnd.trans hie
    obtain ⟨g2, s1, s2, s3⟩ := ci_after_idle h1 hlen hie1
    rw [hsl, hie]
    simp only [Option.isSome_none, Bool.false_eq_true, ↓reduceIte, Nat.add_zero]
    refine ⟨L1, g2, s1, by rw [s2, h2], ?_⟩
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
    have hz : m2.sess.size = m.sess.size + rs.length := c4.trans h2
    refine ⟨L1, g4, s1, by rw [s2, e5, hz], ?_⟩
    have hiter : m2.iter = m.iter := c3.iter.trans h4.iter
    rw [e5, hz] at s3
    rw [hz] at e2
    rw [h2] at c5
    rw [WnC_congr s3, ← hiter, e2, WnC_congr c5, h5]

/-- The chain starts from no request. -/
theorem empty_sb : SB (fun _ => (128, 1)) ⟨fun _ => .e, fun _ => 0⟩ empty := by
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
      shareP := fun i hi _ => by rw [hs] at hi; omega
      tok := by simp [empty, Exec.initial, tokSum] }, by simp [empty, Exec.initial], fun _ => rfl,
    fun h => by simp [empty, Exec.initial] at h⟩

/-- **Every state of the chain is at a slot's end.** -/
theorem reach_sb {m : Machine} (h : Reach m) : ∃ L g, SB L g m := by
  induction h with
  | empty => exact ⟨_, _, empty_sb⟩
  | slot rs hl hf _ ih =>
    obtain ⟨L, g, hB⟩ := ih
    obtain ⟨L', g', h1, -⟩ := slot_sb hB rs hl hf
    exact ⟨L', g', h1⟩

/-- At a slot's end the backlog is the ghost's tokens left. -/
theorem ci_backlog {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI L g m) (hr : m.ready = []) :
    backlog m = WnC m.sess.size L g := by
  unfold backlog WnC
  rw [← ci_sum_jobs hI (rem L g) fun i hi hj => ?_]
  · congr 1
    refine List.map_congr_left fun j hj => ?_
    obtain ⟨h1, h2, -, h4⟩ := hI.jobs j hj
    have h11 := (hI.sess _ h1).2.2.2.1
    rcases h2 with ⟨hc, hm⟩ | ⟨hc, hm⟩
    · simp [rem, hc, hm, h4, h11]
    · simp [rem, hc, hm, h4]
  · have hnr := hI.not_ready hr i hi
    unfold rem
    cases hc : g.c i <;> simp_all [isJob, isReady]

/-- A full batch serves 128 tokens and the arrivals bring their prompts and
outputs: the backlog after the slot. -/
theorem backlog_slot (x : State) (hx : ¬ F x) (rs : List (ℕ × ℕ)) (hl : rs.length ≤ 10000)
    (hf : ∀ r ∈ rs, Fits r) :
    backlog (slot rs x.1) + 128 = backlog x.1 + (rs.map fun r => r.1 + r.2).sum := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  simp only [F, not_or, not_lt] at hx
  have hbusy : x.1.iterEnd.isSome = true := Option.isSome_iff_ne_none.mpr hx.1
  obtain ⟨hb1, -⟩ := hB.busy hbusy
  have h128 : tokSum x.1.iter = 128 := by have := hB.tok; omega
  obtain ⟨L', g', s1, s2, s3⟩ := slot_sb hB rs hl hf
  rw [ci_backlog s1.toCI s1.rdy, ci_backlog hB.toCI hB.rdy, s2]
  rw [if_pos hbusy, h128] at s3
  omega

/-- `F` is small: RAD's batch is full unless every resident decodes and
fewer than 128 do (`optimal_tiling`), each with at most 512 tokens left. -/
theorem backlog_lt_of_F (x : State) (hx : F x) : backlog x.1 < 128 * 512 := by
  obtain ⟨L, g, hB⟩ := reach_sb x.2
  rcases hie : x.1.iterEnd with _ | ⟨a, q⟩
  · simp [backlog, hB.idle hie]
  · have hbusy : x.1.iterEnd.isSome = true := by rw [hie]; rfl
    have htok : x.1.last.stats.tokens < 128 := by
      rcases hx with hx | hx
      · rw [hie] at hx; simp at hx
      · exact hx
    obtain ⟨hb1, hb2⟩ := hB.busy hbusy
    rw [hb1] at htok
    rcases hb2 with h | ⟨hdec, hlen⟩
    · omega
    · have hle : ∀ v ∈ x.1.jobs.map (fun j => j.left + if j.mode = .prefill then (getS x.1 j.owner).attr.get 11 else 0),
          v ≤ 512 := by
        intro v hv
        obtain ⟨j, hj, rfl⟩ := List.mem_map.mp hv
        have hm := hdec j hj
        obtain ⟨h1, h2, -, h4⟩ := hB.jobs j hj
        have hc : g.c j.owner = .d := by
          rcases h2 with ⟨-, hm'⟩ | ⟨hc, -⟩
          · rw [hm] at hm'; exact absurd hm' (by decide)
          · exact hc
        have := (hB.leftD _ h1 hc).2
        simp only [hm, reduceCtorEq, if_false, Nat.add_zero]
        omega
      have := List.sum_le_card_nsmul _ 512 hle
      simp only [List.length_map, smul_eq_mul] at this
      unfold backlog
      omega

/-- Foster's drift condition, below capacity. -/
theorem drift {N : ℕ} (A : Arrivals N) (hA : A.load < 128) :
    Drift (kernel A) F (fun x => (backlog x.1 : ℝ)) (ε A) where
  nonneg _ := Nat.cast_nonneg _
  pos := by unfold ε; linarith
  drift x hx := by
    -- the expectation over the outcomes (a `rw` cannot abstract a `State` behind the
    -- coercion to `Machine`)
    have hE : (kernel A).apply (fun x => (backlog x.1 : ℝ)) x =
        ∑ o ∈ Finset.range (N + 1), A.p o * (backlog (slot (A.arr o) x.1) : ℝ) :=
      Kernel.apply_ofOutcomes _ _ _ _ _ _ _
    rw [hE]
    calc _ = ∑ o ∈ Finset.range (N + 1),
            (A.p o * backlog x.1 + A.p o * A.work o - 128 * A.p o) := by
          refine Finset.sum_congr rfl fun o _ => ?_
          have h1 := backlog_slot x hx (A.arr o) (A.small o) (A.fits o)
          have h2 : (backlog (slot (A.arr o) x.1) : ℝ) + 128 = backlog x.1 + A.work o := by
            unfold Arrivals.work; exact_mod_cast h1
          have h3 : (backlog (slot (A.arr o) x.1) : ℝ) = backlog x.1 + A.work o - 128 := by linarith
          rw [h3]; ring
      _ = (backlog x.1 : ℝ) * ∑ o ∈ Finset.range (N + 1), A.p o + A.load -
            128 * ∑ o ∈ Finset.range (N + 1), A.p o := by
          rw [Finset.sum_sub_distrib, Finset.sum_add_distrib, ← Finset.mul_sum, Arrivals.load,
            ← Finset.sum_mul]; ring
      _ ≤ (backlog x.1 : ℝ) - ε A := by
          rw [A.sum_one]; unfold ε; linarith

/-- Theorem 2 (Foster's half): from every state, the expected number of slots until the
batch is not full (or the engine idle) is at most `backlog / ε`. -/
theorem hitTime_le {N : ℕ} (A : Arrivals N) (hA : A.load < 128) (x : State) :
    ε A * hitTime (kernel A) F x ≤ backlog x.1 :=
  Foster.hitTime_le (drift A hA) x

/-- Theorem 2 (Foster's half): from every state of `F`, the expected return time to `F` is
finite. -/
theorem returnTime_le {N : ℕ} (A : Arrivals N) (hA : A.load < 128) (x : State) (hx : F x) :
    returnTime (kernel A) F x ≤ 1 + (kernel A).apply (fun y => (backlog y.1 : ℝ)) x / ε A :=
  Foster.returnTime_le_of_drift (drift A hA) x hx

/-- The expected hitting time is finite: the truncated expectations are
bounded and converge to `hitTime`. (`hitTime` is a supremum in ℝ, which
would read 0 were they unbounded, so `hitTime_le` alone does not say this.) -/
theorem hit_tendsto {N : ℕ} (A : Arrivals N) (hA : A.load < 128) (x : State) :
    Filter.Tendsto (fun n => hit (kernel A) F n x) Filter.atTop (nhds (hitTime (kernel A) F x)) :=
  Foster.hit_tendsto (drift A hA) x

/-- The theorem is not vacuous: one request of one tile makes a full batch
(Prefill Mode, a chunk of 128), a state outside `F`. -/
example : ¬ F ⟨slot [(128, 1)] empty,
    Reach.slot _ (by decide) (by intro r hr; simp at hr; subst hr; exact ⟨by decide, le_rfl, by decide, le_rfl, by decide⟩) .empty⟩ := by
  have h : (slot [(128, 1)] empty).iterEnd.isSome = true ∧
      (slot [(128, 1)] empty).last.stats.tokens = 128 := by
    decide +kernel
  intro hF
  change (slot [(128, 1)] empty).iterEnd = none ∨ (slot [(128, 1)] empty).last.stats.tokens < 128 at hF
  rcases hF with hF | hF
  · rw [hF] at h; exact absurd h.1 (by decide)
  · omega

/-- … and the load condition can hold: a request of one tile and one output
token in a slot with probability 1/2 is 64.5 tokens per slot. -/
example : ∃ A : Arrivals 1, A.load < 128 :=
  ⟨⟨fun o => if o = 0 then 1 / 2 else if o = 1 then 1 / 2 else 0,
      fun o => by split_ifs <;> norm_num,
      by simp [Finset.sum_range_succ]; norm_num,
      fun o => if o = 1 then [(128, 1)] else [],
      fun o => by split_ifs <;> simp,
      fun o r hr => by split_ifs at hr <;> simp at hr; subst hr; exact ⟨by decide, le_rfl, by decide, le_rfl, by decide⟩⟩,
    by simp [Arrivals.load, Arrivals.work, Finset.sum_range_succ]; norm_num⟩

end BariStable
end Papers
end SerqLang
