/-
# The slot chain of a prefill-then-decode program

`examples/papers/dai_sarathi.sq` and `bari_rad.sq` run the same session once
a request has arrived: stamp its arrival, prefill its prompt, decode its
output, observe its latency. They differ in where the lengths come from
(constants, or the request's attributes) and in the engine's batch (the
greedy fill, or RAD's tiles). This module is what they share on the chain of
`Serq/Chain.lean`: the invariant of a slot's end and that one slot keeps it.

* `Model`: the deployment, the request's lengths as a function of its
  attributes (`len`), the lengths the chain admits (`Fits`), the prompt tile
  `u` (1 for Dai et al., 128 for Bari et al.) and the budget `B`.
* `CI`: the invariant of an instant; `SB`: of a slot's end, with what the
  engine's batch is (`Busy`), which each paper proves of its own engine
  (`Starts`).
* `slot_sb`: a slot keeps `SB`, and its arrivals add their tokens to the
  work left (`WnC`) while a running iteration's tokens leave it.
-/
import Serq.Chain
import Serq.Steps

namespace SerqLang
namespace Slot

open Exec

/-- A program whose arrived request prefills `(len a).1` tokens and decodes
`(len a).2`, `a` its attributes, on an engine with no pool. -/
structure Model where
  D : Deployment
  admit : ∀ m, admitAll D m = m
  len : (ℕ → ℕ) → ℕ × ℕ
  /-- Stamping the arrival (slot 9) does not change the lengths. -/
  len_set : ∀ a v, len (Function.update a 9 v) = len a
  Fits : ℕ × ℕ → Prop
  /-- Prompts are whole tiles of `u` tokens. -/
  u : ℕ
  fits : ∀ r, Fits r → 1 ≤ r.1 ∧ u ∣ r.1 ∧ 1 ≤ r.2
  B : ℕ

variable (M : Model)

/-! ### The program after an arrival -/

def Q4 : Prog := .observe 0 (fun x => x.now - x.attr 9) .stop
def Q3 : Prog := .run 0 .decode (fun x => (M.len x.attr).2) none Q4
def Q2 : Prog := .run 0 .prefill (fun x => (M.len x.attr).1) none (Q3 M)
def Q1 : Prog := .set 9 (fun x => x.now) (Q2 M)

/-- No request, the engine idle. -/
def empty : Machine := Exec.initial M.D 0 (fun _ _ => 0) (Q1 M) { init := [], computedSlot := some 8 }

/-! ### The state of a request -/

/-- Where an arrived request is: arrived (`r1`), prefilling (`p`), prefilled
(`r3`), decoding (`d`), decoded (`r4`), ended (`e`). -/
inductive Cat
  | r1 | p | r3 | d | r4 | e
  deriving DecidableEq

/-- Each request's place and the tokens its job has left. -/
structure Ghost where
  c : ℕ → Cat
  left : ℕ → ℕ

/-- In the ready list. -/
def isReady (k : Cat) : Bool := k = .r1 || k = .r3 || k = .r4
/-- A job at the engine. -/
def isJob (k : Cat) : Bool := k = .p || k = .d

/-- The update of a request's place and its job's tokens. -/
def Ghost.set (g : Ghost) (i : ℕ) (k : Cat) (l : ℕ) : Ghost :=
  ⟨Function.update g.c i k, Function.update g.left i l⟩

theorem Ghost.set_c (g : Ghost) (i : ℕ) (k : Cat) (l j : ℕ) :
    (g.set i k l).c j = if j = i then k else g.c j := by
  simp [Ghost.set, Function.update_apply]

theorem Ghost.set_left (g : Ghost) (i : ℕ) (k : Cat) (l j : ℕ) :
    (g.set i k l).left j = if j = i then l else g.left j := by
  simp [Ghost.set, Function.update_apply]

/-- The ghost after an iteration with batch `it`: every job loses its share,
and those it finishes are prefilled or decoded. -/
def Ghost.tick (g : Ghost) (it : List (ℕ × ℕ)) : Ghost :=
  ⟨fun i => if isJob (g.c i) = true ∧ g.left i ≤ shareOf it i then (if g.c i = .p then .r3 else .r4) else g.c i,
   fun i => g.left i - shareOf it i⟩

/-- What a session is, in each place an arrived request can be. -/
def Shape (s : Sess) : Cat → Prop
  | .r1 => s.prog = Q1 M ∧ s.status = .ready
  | .p => s.prog = Q3 M ∧ s.status = .engine
  | .r3 => s.prog = Q3 M ∧ s.status = .ready
  | .d => s.prog = Q4 ∧ s.status = .engine
  | .r4 => s.prog = Q4 ∧ s.status = .ready
  | .e => s.status = .ended

/-- The tokens request `i`, of lengths `L i`, has left to be served. -/
def rem (L : ℕ → ℕ × ℕ) (g : Ghost) (i : ℕ) : ℕ :=
  match g.c i with
  | .r1 => (L i).1 + (L i).2
  | .p => g.left i + (L i).2
  | .r3 => (L i).2
  | .d => g.left i
  | _ => 0

/-- The tokens arrived and not yet served, over the first `n` requests. -/
def WnC (n : ℕ) (L : ℕ → ℕ × ℕ) (g : Ghost) : ℕ := ∑ i ∈ Finset.range n, rem L g i

/-- The tokens the engine still has to serve: a prefill's left work and the
output it will then decode, a decode's left work. -/
def backlog (m : Machine) : ℕ :=
  (m.jobs.map fun j => j.left + if j.mode = .prefill then (M.len (getS m j.owner).attr.get).2 else 0).sum

/-! ### The invariant -/

/-- The invariant of an instant: request `i` has lengths `L i`. -/
structure CI (L : ℕ → ℕ × ℕ) (g : Ghost) (m : Machine) : Prop where
  sess : ∀ i < m.sess.size, Shape M (getS m i) (g.c i) ∧ (getS m i).stack = [] ∧
    M.len (getS m i).attr.get = L i ∧ M.Fits (L i)
  jobs : ∀ j ∈ m.jobs, j.owner < m.sess.size ∧
    ((g.c j.owner = .p ∧ j.mode = .prefill) ∨ (g.c j.owner = .d ∧ j.mode = .decode)) ∧
    j.growing = none ∧ j.left = g.left j.owner
  jobsP : ∀ i < m.sess.size, isJob (g.c i) = true → ∃ j ∈ m.jobs, j.owner = i
  jobsNodup : (m.jobs.map (·.owner)).Nodup
  leftP : ∀ i < m.sess.size, g.c i = .p → 1 ≤ g.left i ∧ M.u ∣ g.left i ∧ g.left i ≤ (L i).1
  leftD : ∀ i < m.sess.size, g.c i = .d → 1 ≤ g.left i ∧ g.left i ≤ (L i).2
  ready : m.ready.Nodup
  readyMem : ∀ i, i ∈ m.ready ↔ i < m.sess.size ∧ isReady (g.c i) = true
  delays : m.delays = []
  iterNone : m.iterEnd = none → m.iter = []
  iterOwn : ∀ e ∈ m.iter, e.1 < m.sess.size
  share : ∀ i < m.sess.size, shareOf m.iter i ≤ if isJob (g.c i) then g.left i else 0
  shareP : ∀ i < m.sess.size, g.c i = .p → M.u ∣ shareOf m.iter i
  tok : tokSum m.iter ≤ M.B

/-- At a slot's end: settled, an idle engine has no job, and a running
iteration's batch is what the engine makes of its residents (`Busy`). -/
structure SB (Busy : (ℕ → ℕ × ℕ) → Ghost → Machine → Prop) (L : ℕ → ℕ × ℕ) (g : Ghost) (m : Machine) :
    Prop extends CI M L g m where
  rdy : m.ready = []
  idle : m.iterEnd = none → m.jobs = []
  busy : m.iterEnd.isSome = true → Busy L g m

/-- What the engine's batch is: an iteration started on a settled, idle
engine is a slot's end. Each paper proves it of its own engine. -/
def Starts (Busy : (ℕ → ℕ × ℕ) → Ghost → Machine → Prop) : Prop :=
  ∀ {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine}, CI M L g m → m.ready = [] → m.iterEnd = none →
    SB M Busy L g (startIteration M.D m) ∧ (startIteration M.D m).sess.size = m.sess.size

variable {M}

theorem CI.lt_of_ready {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) : i < m.sess.size ∧ isReady (g.c i) = true :=
  (hI.readyMem i).mp (by rw [hr]; simp)

theorem CI.owner_ne {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {i : ℕ}
    (hc : isJob (g.c i) = false) : ∀ j ∈ m.jobs, j.owner ≠ i := by
  intro j hj he
  obtain ⟨-, h, -⟩ := hI.jobs j hj
  rw [he] at h
  rcases h with ⟨h, -⟩ | ⟨h, -⟩ <;> rw [h] at hc <;> simp [isJob] at hc

theorem CI.not_ready {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hr : m.ready = []) :
    ∀ i < m.sess.size, isReady (g.c i) = false := by
  intro i hi
  by_contra h
  have := (hI.readyMem i).mpr ⟨hi, by simpa using h⟩
  rw [hr] at this; simp at this

/-! ### One request's commands -/

/-- A ready request starts a job at the engine. -/
theorem ci_addJob {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hcj : isJob (g.c i) = false)
    (k : Cat) (md : Mode) (l : ℕ)
    (hk : (k = .p ∧ md = .prefill ∧ 1 ≤ l ∧ M.u ∣ l ∧ l ≤ (L i).1) ∨
      (k = .d ∧ md = .decode ∧ 1 ≤ l ∧ l ≤ (L i).2))
    (N : Machine) (s' : Sess) (a b : List Job) (hab : a ++ b = m.jobs)
    (hsz : N.sess.size = m.sess.size)
    (hget : ∀ j, getS N j = if j = i then s' else getS m j)
    (hjobs : N.jobs = a ++ ⟨i, md, l, none⟩ :: b) (hready : N.ready = rest)
    (hdl : N.delays = m.delays) (hit : N.iter = m.iter) (hie : N.iterEnd = m.iterEnd)
    (hs' : Shape M s' k ∧ s'.stack = [] ∧ M.len s'.attr.get = L i) :
    CI M L (g.set i k l) N := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hkr : isReady k = false := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> rfl
  have hown := hI.owner_ne hcj
  have hgc : ∀ j, (g.set i k l).c j = if j = i then k else g.c j := Ghost.set_c g i k l
  have hgl : ∀ j, (g.set i k l).left j = if j = i then l else g.left j := Ghost.set_left g i k l
  have hmem : ∀ x, x ∈ N.jobs ↔ x ∈ m.jobs ∨ x = ⟨i, md, l, none⟩ := by
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
    · subst h; exact ⟨hs'.1, hs'.2.1, hs'.2.2, (hI.sess j hj).2.2.2⟩
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
      rcases hk with ⟨-, -, h1, h2, h3⟩ | ⟨rfl, -⟩
      · exact ⟨h1, h2, h3⟩
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

/-- Stamping the arrival keeps the lengths. -/
theorem len_upd (s : Sess) (v : ℕ) : M.len (s.attr.upd 9 v).get = M.len s.attr.get := by
  rw [show (s.attr.upd 9 v).get = Function.update s.attr.get 9 v from funext fun j => Attrs.get_upd _ _ _ j,
    M.len_set]

/-- An arrived request stamps its arrival and starts its prefill. -/
theorem ci_r1 {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    CI M L (g.set i .p (L i).1) (exec M.D 10000 { m with ready := rest } i) ∧
      (exec M.D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec M.D 10000 { m with ready := rest } i) m ∧
      (exec M.D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, hlen, hfit⟩ := hI.sess i hin
  have hf := M.fits _ hfit
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    Exec.exec_set M.D _ m0 i 9 (fun x => x.now) (Q2 M) (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl)]
  set s1 : Sess := { getS m0 i with attr := (getS m0 i).attr.upd 9 (evalE m0 i fun x => x.now), prog := Q2 M }
    with hs1
  set m1 := setS m0 i s1 with hm1
  have hg1 : getS m1 i = s1 := Exec.getS_setS_self m0 s1 (by simpa using hisz)
  have hl1 : M.len s1.attr.get = L i := (len_upd (getS m0 i) _).trans hlen
  have hev : evalE m1 i (fun x => (M.len x.attr).1) = (L i).1 := by
    show (M.len (getS m1 i).attr.get).1 = _
    rw [hg1, hl1]
  obtain ⟨a, b, hab, he⟩ := Exec.exec_runEngine' M.D 9998 m1 i .prefill (fun x => (M.len x.attr).1) (Q3 M)
    (by rw [hg1]; exact hst) (by rw [hg1]; rfl) (by rw [hev]; omega)
  rw [he, hev]
  set s2 : Sess := { getS m1 i with prog := Q3 M, status := .engine } with hs2
  refine ⟨ci_addJob hI hr (by rw [hc]; rfl) .p .prefill (L i).1
    (Or.inl ⟨rfl, rfl, hf.1, hf.2.1, le_rfl⟩)
    _ s2 a b hab (by simp [hm1, hm0]) (fun j => ?_) rfl rfl rfl rfl rfl ?_, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩,
    by simp [hm1, hm0]⟩
  · show getS (setS m1 i s2) j = _
    rw [Exec.getS_setS m1 s2 (by simpa [hm1, hm0] using hisz)]
    split_ifs with h
    · rfl
    · rw [hm1, Exec.getS_setS m0 s1 (by simpa [hm0] using hisz), if_neg h]; rfl
  · rw [hs2, hg1]
    exact ⟨⟨rfl, rfl⟩, hstk, hl1⟩

/-- A prefilled request starts its decode. -/
theorem ci_r3 {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r3) :
    CI M L (g.set i .d (L i).2) (exec M.D 10000 { m with ready := rest } i) ∧
      (exec M.D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec M.D 10000 { m with ready := rest } i) m ∧
      (exec M.D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, hlen, hfit⟩ := hI.sess i hin
  have hf := M.fits _ hfit
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  have hev : evalE m0 i (fun x => (M.len x.attr).2) = (L i).2 := congrArg Prod.snd hlen
  obtain ⟨a, b, hab, he⟩ := Exec.exec_runEngine' M.D 9999 m0 i .decode (fun x => (M.len x.attr).2) Q4
    (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl) (by rw [hev]; omega)
  rw [show (10000 : ℕ) = 9999 + 1 from rfl, he, hev]
  set s2 : Sess := { getS m0 i with prog := Q4, status := .engine } with hs2
  refine ⟨ci_addJob hI hr (by rw [hc]; rfl) .d .decode (L i).2
    (Or.inr ⟨rfl, rfl, hf.2.2, le_rfl⟩)
    _ s2 a b hab (by simp [hm0]) (fun j => ?_) rfl rfl rfl rfl rfl ?_, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩,
    by simp [hm0]⟩
  · show getS (setS m0 i s2) j = _
    rw [Exec.getS_setS m0 s2 (by simpa [hm0] using hisz)]; rfl
  · rw [hs2, hg0]
    exact ⟨⟨rfl, rfl⟩, hstk, hlen⟩

/-- A decoded request observes its latency and ends. -/
theorem ci_r4 {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r4) :
    CI M L (g.set i .e 0) (exec M.D 10000 { m with ready := rest } i) ∧
      (exec M.D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec M.D 10000 { m with ready := rest } i) m ∧
      (exec M.D 10000 { m with ready := rest } i).sess.size = m.sess.size := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hin
  obtain ⟨hsh, hstk, hlen, hfit⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    Exec.exec_observe M.D _ m0 i 0 (fun x => x.now - x.attr 9) .stop (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl)]
  set m1 : Machine := { setS m0 i { getS m0 i with prog := .stop } with
    obs := (0, (getS m0 i).serial, m0.now, evalE m0 i fun x => x.now - x.attr 9) :: m0.obs } with hm1
  have hg1 : getS m1 i = { getS m0 i with prog := .stop } :=
    Exec.getS_setS_self m0 _ (by simpa using hisz)
  rw [Exec.exec_stop M.D _ m1 i (by rw [hg1]; exact hst) (by rw [hg1]) (by rw [hg1]; exact hstk)]
  set s' : Sess := { getS m1 i with status := .ended, stack := [] } with hs'
  have hget : ∀ j, getS (setS m1 i s') j = if j = i then s' else getS m j := by
    intro j
    rw [Exec.getS_setS m1 s' (by simpa [hm1, hm0] using hisz)]
    split_ifs with h
    · rfl
    · show getS (setS m0 i _) j = _
      rw [Exec.getS_setS m0 _ (by simpa [hm0] using hisz), if_neg h]; rfl
  have hs'a : s'.attr = (getS m i).attr := by rw [hs', hg1]; rfl
  have hgc : ∀ j, (g.set i .e 0).c j = if j = i then .e else g.c j := Ghost.set_c g i .e 0
  have hgl : ∀ j, (g.set i .e 0).left j = if j = i then 0 else g.left j := Ghost.set_left g i .e 0
  have hown := hI.owner_ne (i := i) (by rw [hc]; rfl)
  have hnd' : rest.Nodup := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
  have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  set N := setS m1 i s' with hN
  have hsz : N.sess.size = m.sess.size := by simp [hN, hm1, hm0]
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
    · subst h; exact ⟨rfl, rfl, by rw [hs'a]; exact hlen, hfit⟩
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
    show M.u ∣ shareOf m.iter j
    rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; exact hI.shareP j hj hjc

/-! ### Settling an instant -/

/-- Draining keeps every request's tokens left. -/
def SameR (n : ℕ) (L : ℕ → ℕ × ℕ) (g g' : Ghost) : Prop := ∀ j < n, rem L g' j = rem L g j

theorem SameR.refl (n : ℕ) (L : ℕ → ℕ × ℕ) (g : Ghost) : SameR n L g g := fun _ _ => rfl

theorem SameR.trans {n : ℕ} {L : ℕ → ℕ × ℕ} {a b c : Ghost} (h1 : SameR n L a b) (h2 : SameR n L b c) :
    SameR n L a c :=
  fun j hj => (h2 j hj).trans (h1 j hj)

theorem rem_set (L : ℕ → ℕ × ℕ) (g : Ghost) (i : ℕ) (k : Cat) (l j : ℕ) (hj : j ≠ i) :
    rem L (g.set i k l) j = rem L g j := by
  simp only [rem, Ghost.set_c, Ghost.set_left, hj, if_false]

theorem SameR.set {n : ℕ} (L : ℕ → ℕ × ℕ) (g : Ghost) (i : ℕ) (k : Cat) (l : ℕ)
    (hr : rem L (g.set i k l) i = rem L g i) : SameR n L g (g.set i k l) := fun j _ => by
  by_cases h : j = i
  · subst h; exact hr
  · exact rem_set L g i k l j h

theorem WnC_congr {n : ℕ} {L : ℕ → ℕ × ℕ} {g g' : Ghost} (h : SameR n L g g') : WnC n L g' = WnC n L g :=
  Finset.sum_congr rfl fun j hj => h j (Finset.mem_range.mp hj)

/-- One ready request runs its commands of the instant. -/
theorem ci_exec {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) : ∃ g1, CI M L g1 (exec M.D 10000 { m with ready := rest } i) ∧
      (exec M.D 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec M.D 10000 { m with ready := rest } i) m ∧
      (exec M.D 10000 { m with ready := rest } i).sess.size = m.sess.size ∧ SameR m.sess.size L g g1 := by
  have hcr := (hI.lt_of_ready hr).2
  cases hc : g.c i with
  | r1 =>
    obtain ⟨h1, h2, h3, h4⟩ := ci_r1 hI hr hc
    exact ⟨_, h1, h2, h3, h4, SameR.set L g i .p _ (by simp [rem, Ghost.set_c, Ghost.set_left, hc])⟩
  | r3 =>
    obtain ⟨h1, h2, h3, h4⟩ := ci_r3 hI hr hc
    exact ⟨_, h1, h2, h3, h4, SameR.set L g i .d _ (by simp [rem, Ghost.set_c, Ghost.set_left, hc])⟩
  | r4 =>
    obtain ⟨h1, h2, h3, h4⟩ := ci_r4 hI hr hc
    exact ⟨_, h1, h2, h3, h4, SameR.set L g i .e 0 (by simp [rem, Ghost.set_c, hc])⟩
  | _ => rw [hc] at hcr; simp [isReady] at hcr

/-- **Draining.** -/
theorem drain_ci {L : ℕ → ℕ × ℕ} : ∀ (f : ℕ) (g : Ghost) (m : Machine), CI M L g m → m.ready.length ≤ f →
    ∃ g', CI M L g' (drain M.D f m) ∧ (drain M.D f m).ready = [] ∧ Keeps (drain M.D f m) m ∧
      (drain M.D f m).sess.size = m.sess.size ∧ SameR m.sess.size L g g'
  | 0, g, m, hI, hf => by
    have hr : m.ready = [] := List.eq_nil_of_length_eq_zero (by omega)
    exact ⟨g, hI, by simp [drain, hr], Keeps.refl m, rfl, SameR.refl _ L g⟩
  | f + 1, g, m, hI, hf => by
    unfold drain
    split
    · rename_i hr
      exact ⟨g, hI, hr, Keeps.refl m, rfl, SameR.refl _ L g⟩
    · rename_i i rest hr
      have hlen : rest.length ≤ f := by rw [hr] at hf; simpa using hf
      obtain ⟨g1, hI1, hr1, hk1, hz1, hs1⟩ := ci_exec hI hr
      obtain ⟨g2, hI2, hr2, hk2, hz2, hs2⟩ := drain_ci f g1 _ hI1 (by rw [hr1]; exact hlen)
      rw [hz1] at hs2
      exact ⟨g2, hI2, hr2, hk1.trans hk2, hz2.trans hz1, hs1.trans hs2⟩

theorem settle_eq_drain {m : Machine} (h : (drain M.D (drainFuel m) m).ready = []) :
    settle M.D m = drain M.D (drainFuel m) m := by
  unfold settle
  rw [show (1000 : ℕ) = 999 + 1 from rfl, settleLoop]
  simp only [M.admit, h, List.isEmpty_nil, ↓reduceIte]

/-- **Settling an instant**: every ready request runs, however many. -/
theorem settle_ci {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) :
    ∃ g', CI M L g' (settle M.D m) ∧ (settle M.D m).ready = [] ∧ Keeps (settle M.D m) m ∧
      (settle M.D m).sess.size = m.sess.size ∧ SameR m.sess.size L g g' := by
  obtain ⟨g', h1, h2, h3, h4, h5⟩ := drain_ci (drainFuel m) g m hI (by unfold drainFuel; omega)
  rw [settle_eq_drain h2]
  exact ⟨g', h1, h2, h3, h4, h5⟩

/-! ### Arrivals -/

/-- The tokens a request of attributes `a` brings. -/
def Model.work (M : Model) (a : ℕ → ℕ) : ℕ := (M.len a).1 + (M.len a).2

theorem getS_inject (a : ℕ → ℕ) (m : Machine) (j : ℕ) :
    getS (inject (Q1 M) a m) j =
      if j = m.sess.size then ⟨m.sess.size, ⟨a, []⟩, 0, Q1 M, [], .ready, 0, 0⟩ else getS m j := by
  unfold getS inject
  simp only [Array.getD_eq_getD_getElem?, Array.getElem?_push]
  split_ifs with h
  · subst h; simp
  · rfl

/-- **One arrival.** -/
theorem ci_inject {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (a : ℕ → ℕ)
    (ha : M.Fits (M.len a)) :
    CI M (Function.update L m.sess.size (M.len a)) (g.set m.sess.size .r1 0) (inject (Q1 M) a m) ∧
      (inject (Q1 M) a m).sess.size = m.sess.size + 1 ∧
      (inject (Q1 M) a m).ready = m.ready ++ [m.sess.size] ∧
      Keeps (inject (Q1 M) a m) m ∧
      WnC (m.sess.size + 1) (Function.update L m.sess.size (M.len a)) (g.set m.sess.size .r1 0) =
        WnC m.sess.size L g + M.work a := by
  set n := m.sess.size with hn
  set L' := Function.update L n (M.len a) with hL'
  set N := inject (Q1 M) a m with hN
  have hsz : N.sess.size = n + 1 := by simp [hN, inject, hn]
  have hget := getS_inject (M := M) a m
  have hLn : L' n = M.len a := by simp [hL']
  have hLj : ∀ j, j ≠ n → L' j = L j := fun j h => by simp [hL', h]
  have hgc : ∀ j, (g.set n .r1 0).c j = if j = n then .r1 else g.c j := Ghost.set_c g n .r1 0
  have hgl : ∀ j, (g.set n .r1 0).left j = if j = n then 0 else g.left j := Ghost.set_left g n .r1 0
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
      exact ⟨⟨rfl, rfl⟩, rfl, congrArg M.len (funext fun k => by simp [Attrs.get]), ha⟩
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
    · rw [if_neg h] at hjc; rw [hgl, if_neg h, hLj j h]; exact hI.leftP j (by omega) hjc
  · rw [hsz] at hj
    rw [hgc] at hjc
    by_cases h : j = n
    · rw [if_pos h] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h, hLj j h]; exact hI.leftD j (by omega) hjc
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
    · subst h; rw [if_pos rfl, if_pos rfl, Exec.shareOf_eq_zero hio]; exact Nat.zero_le _
    · rw [if_neg h, if_neg h]; exact hI.share j (by omega)
  · rw [hsz] at hj
    show M.u ∣ shareOf m.iter j
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
    have h2 : rem L' (g.set n .r1 0) n = M.work a := by simp [rem, hgc, hLn, Model.work]
    rw [h1, h2]

/-- **The arrivals of a slot**, of attributes `as`. -/
theorem ci_injects : ∀ (as : List (ℕ → ℕ)) {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine}, CI M L g m →
    (∀ a ∈ as, M.Fits (M.len a)) → ∃ L' g',
    CI M L' g' (as.foldl (fun m a => inject (Q1 M) a m) m) ∧
      (as.foldl (fun m a => inject (Q1 M) a m) m).sess.size = m.sess.size + as.length ∧
      (as.foldl (fun m a => inject (Q1 M) a m) m).ready.length = m.ready.length + as.length ∧
      Keeps (as.foldl (fun m a => inject (Q1 M) a m) m) m ∧
      WnC (m.sess.size + as.length) L' g' = WnC m.sess.size L g + (as.map M.work).sum
  | [], L, g, m, hI, _ => ⟨L, g, hI, rfl, rfl, Keeps.refl m, by simp⟩
  | a :: as, L, g, m, hI, hf => by
    obtain ⟨c1, c2, c3, c4, c5⟩ := ci_inject hI a (hf a List.mem_cons_self)
    obtain ⟨L2, g2, h1, h2, h3, h4, h5⟩ := ci_injects as c1 fun x hx => hf x (List.mem_cons_of_mem _ hx)
    simp only [List.foldl_cons, List.length_cons, List.map_cons, List.sum_cons]
    refine ⟨L2, g2, h1, by rw [h2, c2]; ring, by rw [h3, c3]; simp; ring, c4.trans h4, ?_⟩
    rw [c2] at h5
    rw [show m.sess.size + (as.length + 1) = m.sess.size + 1 + as.length by ring, h5, c5]
    ring

/-! ### An iteration ends -/

/-- **An iteration ends.** Its tokens leave the backlog; the requests that
finish are ready, at most one per token. -/
theorem ci_end {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hr : m.ready = []) {a qa : ℕ}
    (hie : m.iterEnd = some (a, qa)) :
    CI M L (g.tick m.iter) (handle m a qa) ∧
      WnC m.sess.size L (g.tick m.iter) + tokSum m.iter = WnC m.sess.size L g ∧
      (handle m a qa).ready.length ≤ M.B ∧ (handle m a qa).iterEnd = none ∧
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
    rw [← Exec.sum_shareOf n it hI.iterOwn, ← Finset.sum_add_distrib]
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
  have hH : handle m a qa = Exec.readyAll done M1 := by
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
  obtain ⟨f1, f2, -, -, -, f6, -, f8, f9, f10, f11⟩ := Exec.readyAll_fields done M1 hdn hdb
  set R := Exec.readyAll done M1 with hR
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
    rw [← List.toFinset_card_of_nodup hdn, ← Exec.sum_shareOf n it hI.iterOwn]
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
      obtain ⟨hs, hst, hl, hfit⟩ := hI.sess j hj
      rcases hfinc j hj hd with ⟨hc, hc'⟩ | ⟨hc, hc'⟩
      · rw [hc] at hs; rw [hc']; exact ⟨⟨hs.1, rfl⟩, hst, hl, hfit⟩
      · rw [hc] at hs; rw [hc']; exact ⟨⟨hs.1, rfl⟩, hst, hl, hfit⟩
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
      exact ⟨by omega, Nat.dvd_sub h1.2.1 h2, by omega⟩
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
    show (m.ready ++ done).length ≤ M.B
    rw [hr, List.nil_append]
    exact hlen.trans hI.tok

/-! ### A slot -/

/-- A sum over the jobs is a sum over the requests that have one. -/
theorem ci_sum_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (f : ℕ → ℕ)
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

variable {Busy : (ℕ → ℕ × ℕ) → Ghost → Machine → Prop}

/-- After an event on an idle engine: settle, then start an iteration. -/
theorem ci_after_idle (hS : Starts M Busy) {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m)
    (hie : m.iterEnd = none) :
    ∃ g', SB M Busy L g' (afterEvent M.D m) ∧ (afterEvent M.D m).sess.size = m.sess.size ∧
      SameR m.sess.size L g g' := by
  obtain ⟨g', h1, h2, h3, h4, h5⟩ := settle_ci hI
  have hie' : (settle M.D m).iterEnd = none := h3.iterEnd.trans hie
  have hpend : pendingBy (settle M.D m) (settle M.D m).now = false := by
    simp [pendingBy, nextEvent, hie', h1.delays]
  have hA : afterEvent M.D m = startIteration M.D (settle M.D m) := by
    unfold afterEvent; simp [hie', hpend]
  obtain ⟨s1, s2⟩ := hS h1 h2 hie'
  rw [hA]
  exact ⟨g', s1, s2.trans h4, h5⟩

theorem after_busy (D : Deployment) (m : Machine) {a q : ℕ} (h : (settle D m).iterEnd = some (a, q)) :
    afterEvent D m = settle D m := by
  unfold afterEvent; simp [h]

/-- **A slot**: the arrivals, then the running iteration (if any) ends and
the next one starts. -/
theorem slot_sb (hS : Starts M Busy) {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hB : SB M Busy L g m)
    (as : List (ℕ → ℕ)) (hf : ∀ a ∈ as, M.Fits (M.len a)) :
    ∃ L' g', SB M Busy L' g' (slotL M.D (Q1 M) as m) ∧ (slotL M.D (Q1 M) as m).sess.size = m.sess.size + as.length ∧
      WnC (m.sess.size + as.length) L' g' + (if m.iterEnd.isSome then tokSum m.iter else 0) =
        WnC m.sess.size L g + (as.map M.work).sum := by
  obtain ⟨L1, g1, h1, h2, h3, h4, h5⟩ := ci_injects as hB.toCI hf
  set m1 := as.foldl (fun m a => inject (Q1 M) a m) m with hm1
  have hsl : slotL M.D (Q1 M) as m =
      if m.iterEnd.isSome then step M.D (afterEvent M.D m1) else afterEvent M.D m1 := rfl
  rcases hie : m.iterEnd with _ | ⟨a, qa⟩
  · have hie1 : m1.iterEnd = none := h4.iterEnd.trans hie
    obtain ⟨g2, s1, s2, s3⟩ := ci_after_idle hS h1 hie1
    rw [hsl, hie]
    simp only [Option.isSome_none, Bool.false_eq_true, ↓reduceIte, Nat.add_zero]
    refine ⟨L1, g2, s1, by rw [s2, h2], ?_⟩
    rw [h2] at s3
    rw [WnC_congr s3, h5]
  · have hie1 : m1.iterEnd = some (a, qa) := h4.iterEnd.trans hie
    obtain ⟨g2, c1, c2, c3, c4, c5⟩ := settle_ci h1
    set m2 := settle M.D m1 with hm2
    have hie2 : m2.iterEnd = some (a, qa) := c3.iterEnd.trans hie1
    have hA : afterEvent M.D m1 = m2 := after_busy M.D m1 hie2
    have hstep : step M.D m2 = afterEvent M.D (handle m2 a qa) := by
      unfold step
      rw [show nextEvent m2 = some (a, qa) by simp [nextEvent, hie2, c1.delays]]
    obtain ⟨e1, e2, e3, e4, e5⟩ := ci_end c1 c2 hie2
    obtain ⟨g4, s1, s2, s3⟩ := ci_after_idle hS e1 e4
    rw [hsl, hie]
    simp only [Option.isSome_some, ↓reduceIte]
    rw [hA, hstep]
    have hz : m2.sess.size = m.sess.size + as.length := c4.trans h2
    refine ⟨L1, g4, s1, by rw [s2, e5, hz], ?_⟩
    have hiter : m2.iter = m.iter := c3.iter.trans h4.iter
    rw [e5, hz] at s3
    rw [hz] at e2
    rw [h2] at c5
    rw [WnC_congr s3, ← hiter, e2, WnC_congr c5, h5]

/-- The running iteration lasts its cost: it ends `max 1 cost` after now. -/
def Dur (D : Deployment) (m : Machine) : Prop :=
  ∀ a q, m.iterEnd = some (a, q) → a = m.now + max 1 (D.cost m.last.stats)

theorem startIteration_dur (D : Deployment) (m : Machine) : Dur D (startIteration D m) := by
  intro a q h
  unfold startIteration at h ⊢
  simp only at h ⊢
  split at h
  · rename_i h1
    rw [if_pos h1]
    split at h
    · rename_i h2
      rw [if_pos h2]
      simp only [Option.some.injEq, Prod.mk.injEq] at h
      rw [← h.1]; rfl
    · simp at h
  · simp at h

theorem afterEvent_dur (D : Deployment) (m : Machine) (h : (settle D m).iterEnd = none) :
    Dur D (afterEvent D m) := by
  unfold afterEvent
  simp only [h, Option.isNone_none, Bool.true_and]
  split
  · exact startIteration_dur D _
  · intro a q h'; rw [h] at h'; cases h'

/-- **A slot ends** with an iteration that lasts its cost, or with an idle engine. -/
theorem slot_dur (hS : Starts M Busy) {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hB : SB M Busy L g m)
    (as : List (ℕ → ℕ)) (hf : ∀ a ∈ as, M.Fits (M.len a)) : Dur M.D (slotL M.D (Q1 M) as m) := by
  obtain ⟨L1, g1, h1, -, -, h4, -⟩ := ci_injects as hB.toCI hf
  set m1 := as.foldl (fun m a => inject (Q1 M) a m) m with hm1
  have hsl : slotL M.D (Q1 M) as m =
      if m.iterEnd.isSome then step M.D (afterEvent M.D m1) else afterEvent M.D m1 := rfl
  obtain ⟨g2, c1, c2, c3, -, -⟩ := settle_ci h1
  rcases hie : m.iterEnd with _ | ⟨a, qa⟩
  · rw [hsl, hie]
    simp only [Option.isSome_none, Bool.false_eq_true, ↓reduceIte]
    exact afterEvent_dur _ _ (c3.iterEnd.trans (h4.iterEnd.trans hie))
  · have hie2 : (settle M.D m1).iterEnd = some (a, qa) := c3.iterEnd.trans (h4.iterEnd.trans hie)
    have hstep : step M.D (settle M.D m1) = afterEvent M.D (handle (settle M.D m1) a qa) := by
      unfold step
      rw [show nextEvent (settle M.D m1) = some (a, qa) by simp [nextEvent, hie2, c1.delays]]
    obtain ⟨e1, -, -, e4, -⟩ := ci_end c1 c2 hie2
    obtain ⟨-, -, -, d3, -, -⟩ := settle_ci e1
    rw [hsl, hie]
    simp only [Option.isSome_some, ↓reduceIte]
    rw [after_busy M.D m1 hie2, hstep]
    exact afterEvent_dur _ _ (d3.iterEnd.trans e4)

/-- The chain starts from no request. -/
theorem empty_sb (L : ℕ → ℕ × ℕ) : SB M Busy L ⟨fun _ => .e, fun _ => 0⟩ (empty M) := by
  have hs : (empty M).sess.size = 0 := by simp [empty, Exec.initial]
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

/-- At a slot's end the backlog is the ghost's tokens left. -/
theorem ci_backlog {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hr : m.ready = []) :
    backlog M m = WnC m.sess.size L g := by
  unfold backlog WnC
  rw [← ci_sum_jobs hI (rem L g) fun i hi hj => ?_]
  · congr 1
    refine List.map_congr_left fun j hj => ?_
    obtain ⟨h1, h2, -, h4⟩ := hI.jobs j hj
    have hl := (hI.sess _ h1).2.2.1
    rcases h2 with ⟨hc, hm⟩ | ⟨hc, hm⟩
    · simp [rem, hc, hm, h4, hl]
    · simp [rem, hc, hm, h4]
  · have hnr := hI.not_ready hr i hi
    unfold rem
    cases hc : g.c i <;> simp_all [isJob, isReady]

/-- The backlog after a slot whose running batch served `b` tokens. -/
theorem backlog_slot (hS : Starts M Busy) {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hB : SB M Busy L g m)
    (hbusy : m.iterEnd.isSome = true) (as : List (ℕ → ℕ))
    (hf : ∀ a ∈ as, M.Fits (M.len a)) :
    backlog M (slotL M.D (Q1 M) as m) + tokSum m.iter = backlog M m + (as.map M.work).sum := by
  obtain ⟨L', g', s1, s2, s3⟩ := slot_sb hS hB as hf
  rw [ci_backlog s1.toCI s1.rdy, ci_backlog hB.toCI hB.rdy, s2]
  rw [if_pos hbusy] at s3
  exact s3

/-! ### The job list, exactly

`CI` says what each job is; for a chain on job lists (`DaiSim`, `BariSim`)
we follow the list itself. No pool admits, so every session keeps admission
number 0 (`A0`), and a `run` on the engine appends its job (`Exec.exec`
inserts behind the jobs of no later admission). -/

/-- Every session has admission number 0. -/
def A0 (m : Machine) : Prop := ∀ j, (getS m j).admSeq = 0

theorem A0_setS {m : Machine} (hA : A0 m) {i : ℕ} (s : Sess) (hs : s.admSeq = 0) (hi : i < m.sess.size) :
    A0 (setS m i s) := fun j => by
  rw [Exec.getS_setS m s hi]
  split_ifs
  · exact hs
  · exact hA j

theorem span_all {α : Type} (p : α → Bool) : ∀ l : List α, (∀ x ∈ l, p x = true) → l.span p = (l, [])
  | [], _ => rfl
  | x :: xs, h => by
    rw [List.span_eq_takeWhile_dropWhile, List.takeWhile_cons_of_pos (h x List.mem_cons_self),
      List.dropWhile_cons_of_pos (h x List.mem_cons_self)]
    have := span_all p xs fun y hy => h y (List.mem_cons_of_mem _ hy)
    rw [List.span_eq_takeWhile_dropWhile] at this
    simp only [Prod.mk.injEq] at this
    rw [this.1, this.2]

/-- An arrived request's prefill of its prompt joins the end of the job list. -/
theorem exec_r1_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hA : A0 m)
    {i : ℕ} {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    (exec M.D 10000 { m with ready := rest } i).jobs = m.jobs ++ [⟨i, .prefill, (L i).1, none⟩] ∧
      A0 (exec M.D 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, -, hlen, hfit⟩ := hI.sess i hin
  have hf := M.fits _ hfit
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    Exec.exec_set M.D _ m0 i 9 (fun x => x.now) (Q2 M) (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl)]
  set s1 : Sess := { getS m0 i with attr := (getS m0 i).attr.upd 9 (evalE m0 i fun x => x.now), prog := Q2 M }
    with hs1
  set m1 := setS m0 i s1 with hm1
  have hA1 : A0 m1 := A0_setS (fun j => hA j) s1 (hA i) (by simpa using hin)
  have hg1 : getS m1 i = s1 := Exec.getS_setS_self m0 s1 (by simpa using hin)
  have hev : evalE m1 i (fun x => (M.len x.attr).1) = (L i).1 := by
    show (M.len (getS m1 i).attr.get).1 = _
    rw [hg1, show M.len s1.attr.get = L i from (len_upd (getS m0 i) _).trans hlen]
  rw [Exec.exec_runEngine M.D 9998 m1 i .prefill (fun x => (M.len x.attr).1) (Q3 M) (by rw [hg1]; exact hst)
    (by rw [hg1]; rfl) (by rw [hev]; omega)]
  rw [span_all _ _ fun j _ => by simp [hA1 j.owner, hA1 i]]
  refine ⟨?_, fun j => ?_⟩
  · show m1.jobs ++ [⟨i, .prefill, evalE m1 i (fun x => (M.len x.attr).1), none⟩] = _
    rw [hev]; try rfl
  · show (getS (setS m1 i { getS m1 i with prog := Q3 M, status := .engine }) j).admSeq = 0
    exact A0_setS hA1 { getS m1 i with prog := Q3 M, status := .engine } (hA1 i)
      (by simpa [hm1, hm0] using hin) j

/-- A prefilled request's decode of its output joins the end of the job list. -/
theorem exec_r3_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hA : A0 m)
    {i : ℕ} {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r3) :
    (exec M.D 10000 { m with ready := rest } i).jobs = m.jobs ++ [⟨i, .decode, (L i).2, none⟩] ∧
      A0 (exec M.D 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, -, hlen, hfit⟩ := hI.sess i hin
  have hf := M.fits _ hfit
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  have hA0 : A0 m0 := fun j => hA j
  have hev : evalE m0 i (fun x => (M.len x.attr).2) = (L i).2 := congrArg Prod.snd hlen
  rw [show (10000 : ℕ) = 9999 + 1 from rfl,
    Exec.exec_runEngine M.D 9999 m0 i .decode (fun x => (M.len x.attr).2) Q4 (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl) (by rw [hev]; omega)]
  rw [span_all _ _ fun j _ => by simp [hA0 j.owner, hA0 i]]
  refine ⟨?_, fun j => ?_⟩
  · show m0.jobs ++ [⟨i, .decode, evalE m0 i (fun x => (M.len x.attr).2), none⟩] = _
    rw [hev]; try rfl
  · show (getS (setS m0 i { getS m0 i with prog := Q4, status := .engine }) j).admSeq = 0
    exact A0_setS hA0 { getS m0 i with prog := Q4, status := .engine } (hA0 i) (by simpa [hm0] using hin) j

/-- A decoded request leaves the job list as it is. -/
theorem exec_r4_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hA : A0 m)
    {i : ℕ} {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r4) :
    (exec M.D 10000 { m with ready := rest } i).jobs = m.jobs ∧
      A0 (exec M.D 10000 { m with ready := rest } i) := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  obtain ⟨hsh, hstk, -⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    Exec.exec_observe M.D _ m0 i 0 (fun x => x.now - x.attr 9) .stop (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl)]
  set m1 : Machine := { setS m0 i { getS m0 i with prog := .stop } with
    obs := (0, (getS m0 i).serial, m0.now, evalE m0 i fun x => x.now - x.attr 9) :: m0.obs } with hm1
  have hg1 : getS m1 i = { getS m0 i with prog := .stop } :=
    Exec.getS_setS_self m0 _ (by simpa using hin)
  have hA1 : A0 m1 := fun j => by
    show (getS (setS m0 i { getS m0 i with prog := .stop }) j).admSeq = 0
    exact A0_setS (m := m0) (fun j => hA j) { getS m0 i with prog := .stop } (hA i) (by simpa using hin) j
  rw [Exec.exec_stop M.D _ m1 i (by rw [hg1]; exact hst) (by rw [hg1]) (by rw [hg1]; exact hstk)]
  exact ⟨rfl, A0_setS hA1 _ (hA1 i) (by simpa [hm1, hm0] using hin)⟩

/-- The job a ready request adds to the list when it runs. -/
def contrib (L : ℕ → ℕ × ℕ) (g : Ghost) (i : ℕ) : List Job :=
  match g.c i with
  | .r1 => [⟨i, .prefill, (L i).1, none⟩]
  | .r3 => [⟨i, .decode, (L i).2, none⟩]
  | _ => []

/-- **Draining** appends each ready request's job, in the ready list's order. -/
theorem drain_jobs {L : ℕ → ℕ × ℕ} : ∀ (f : ℕ) (g : Ghost) (m : Machine), CI M L g m → A0 m →
    m.ready.length ≤ f →
    (drain M.D f m).jobs = m.jobs ++ m.ready.flatMap (contrib L g) ∧ A0 (drain M.D f m)
  | 0, g, m, hI, hA, hf => by
    have hr : m.ready = [] := List.eq_nil_of_length_eq_zero (by omega)
    exact ⟨by simp [drain, hr], hA⟩
  | f + 1, g, m, hI, hA, hf => by
    unfold drain
    split
    · rename_i hr
      exact ⟨by simp [hr], hA⟩
    · rename_i i rest hr
      have hlen : rest.length ≤ f := by rw [hr] at hf; simpa using hf
      have hcr := (hI.lt_of_ready hr).2
      have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
      obtain ⟨g1, hI1, hr1, hJ, hA1, hc1⟩ : ∃ g1, CI M L g1 (exec M.D 10000 { m with ready := rest } i) ∧
          (exec M.D 10000 { m with ready := rest } i).ready = rest ∧
          (exec M.D 10000 { m with ready := rest } i).jobs = m.jobs ++ contrib L g i ∧
          A0 (exec M.D 10000 { m with ready := rest } i) ∧ (∀ j, j ≠ i → g1.c j = g.c j) := by
        cases hc : g.c i with
        | r1 =>
          obtain ⟨h1, h2, -, -⟩ := ci_r1 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r1_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2, fun j hj => by simp [Ghost.set_c, hj]⟩
        | r3 =>
          obtain ⟨h1, h2, -, -⟩ := ci_r3 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r3_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2, fun j hj => by simp [Ghost.set_c, hj]⟩
        | r4 =>
          obtain ⟨h1, h2, -, -⟩ := ci_r4 hI hr hc
          obtain ⟨j1, j2⟩ := exec_r4_jobs hI hA hr hc
          exact ⟨_, h1, h2, by rw [j1]; simp [contrib, hc], j2, fun j hj => by simp [Ghost.set_c, hj]⟩
        | _ => rw [hc] at hcr; simp [isReady] at hcr
      obtain ⟨d1, d2⟩ := drain_jobs f g1 _ hI1 hA1 (by rw [hr1]; exact hlen)
      refine ⟨?_, d2⟩
      rw [d1, hJ, hr1, hr, List.flatMap_cons, List.append_assoc]
      congr 2
      exact List.flatMap_congr fun j hj => by
        have hji : j ≠ i := fun h => hir (h ▸ hj)
        simp only [contrib, hc1 j hji]

/-- **Settling an instant**: the ready requests' jobs join the list. -/
theorem settle_jobs {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) (hA : A0 m) :
    (settle M.D m).jobs = m.jobs ++ m.ready.flatMap (contrib L g) ∧ A0 (settle M.D m) := by
  have hf : m.ready.length ≤ drainFuel m := by unfold drainFuel; omega
  obtain ⟨g', h1, h2, -⟩ := drain_ci _ g m hI hf
  rw [settle_eq_drain h2]
  exact drain_jobs _ g m hI hA hf

/-- The machine after a slot's arrivals, of attributes `as`. -/
def injL (M : Model) (as : List (ℕ → ℕ)) (m : Machine) : Machine := as.foldl (fun m a => inject (Q1 M) a m) m

/-- A slot's arrivals: the job list is unchanged, the old sessions too, and
the new ones are ready, in order, with their attributes. -/
theorem injects_facts : ∀ (as : List (ℕ → ℕ)) (m : Machine), A0 m →
    (injL M as m).jobs = m.jobs ∧ (injL M as m).ready = m.ready ++ List.range' m.sess.size as.length ∧
    (injL M as m).sess.size = m.sess.size + as.length ∧ A0 (injL M as m) ∧
    (∀ j < m.sess.size, getS (injL M as m) j = getS m j) ∧
    (∀ t < as.length, getS (injL M as m) (m.sess.size + t) =
      ⟨m.sess.size + t, ⟨as.getD t (fun _ => 0), []⟩, 0, Q1 M, [], .ready, 0, 0⟩)
  | [], m, hA => ⟨rfl, by simp [injL], by simp [injL], hA, fun _ _ => rfl, fun t ht => by simp at ht⟩
  | a :: as, m, hA => by
    set m' := inject (Q1 M) a m with hm'
    have hsz : m'.sess.size = m.sess.size + 1 := by simp [hm', inject]
    have hA' : A0 m' := fun j => by
      rw [hm', getS_inject]; split_ifs
      · rfl
      · exact hA j
    have hg' : ∀ j, getS m' j = if j = m.sess.size then
        ⟨m.sess.size, ⟨a, []⟩, 0, Q1 M, [], .ready, 0, 0⟩ else getS m j := getS_inject a m
    have hI : injL M (a :: as) m = injL M as m' := rfl
    obtain ⟨h1, h2, h3, h4, h5, h6⟩ := injects_facts as m' hA'
    rw [hI]
    refine ⟨h1, ?_, by rw [h3, hsz]; simp; ring, h4, fun j hj => ?_, fun t ht => ?_⟩
    · rw [h2, hsz]
      show m.ready ++ [m.sess.size] ++ _ = _
      simp [List.range'_succ]
    · rw [h5 j (by rw [hsz]; omega), hg', if_neg (by omega)]
    · cases t with
      | zero =>
        rw [h5 _ (by rw [hsz]; omega), hg', if_pos (by omega)]
        simp
      | succ t =>
        have := h6 t (by simp at ht; omega)
        rw [hsz] at this
        rw [show m.sess.size + (t + 1) = m.sess.size + 1 + t by ring, this]
        simp

/-- A ready session at the start of its program has arrived. -/
theorem cat_r1 {L : ℕ → ℕ × ℕ} {g : Ghost} {m : Machine} (hI : CI M L g m) {j : ℕ}
    (hj : j < m.sess.size) (hp : (getS m j).prog = Q1 M) (hs : (getS m j).status = .ready) : g.c j = .r1 := by
  have hsh := (hI.sess j hj).1
  cases hc : g.c j <;> rw [hc] at hsh <;> simp_all [Shape, Q1, Q2, Q3, Q4]

theorem flatMap_single {α β : Type} (f : α → List β) (c : α → β) :
    ∀ l : List α, (∀ x ∈ l, f x = [c x]) → l.flatMap f = l.map c
  | [], _ => rfl
  | x :: xs, h => by
    rw [List.flatMap_cons, h x List.mem_cons_self,
      flatMap_single f c xs fun y hy => h y (List.mem_cons_of_mem _ hy)]
    rfl

theorem flatMap_ite {α β : Type} (f : α → List β) (p : α → Bool) (c : α → β) :
    ∀ l : List α, (∀ x ∈ l, f x = if p x then [c x] else []) → l.flatMap f = (l.filter p).map c
  | [], _ => rfl
  | x :: xs, h => by
    rw [List.flatMap_cons, h x List.mem_cons_self,
      flatMap_ite f p c xs fun y hy => h y (List.mem_cons_of_mem _ hy)]
    by_cases hx : p x <;> simp [hx]

theorem range_map {α β : Type} (d : α) (f : ℕ → β) (g : α → β) :
    ∀ (rs : List α) (n : ℕ), (∀ t < rs.length, f (n + t) = g (rs.getD t d)) →
      (List.range' n rs.length).map f = rs.map g
  | [], _, _ => rfl
  | r :: rs, n, h => by
    rw [List.length_cons, List.range'_succ, List.map_cons, List.map_cons]
    have h0 := h 0 (by simp)
    simp only [Nat.add_zero, List.getD_cons_zero] at h0
    rw [h0, range_map d f g rs (n + 1) fun t ht => by
      have := h (t + 1) (by simp; omega)
      rw [show n + (t + 1) = n + 1 + t by ring] at this
      simpa using this]

/-- An iteration ends: each job loses its share; the finished ones leave the
list and become ready, in the list's order. -/
theorem handle_facts {m : Machine} {a qa : ℕ} (hie : m.iterEnd = some (a, qa))
    (hn : (m.jobs.map (·.owner)).Nodup) (hA : A0 m) (hb : ∀ j ∈ m.jobs, j.owner < m.sess.size) :
    (handle m a qa).jobs =
        (m.jobs.map fun j => { j with left := j.left - shareOf m.iter j.owner }).filter (·.left ≠ 0) ∧
      (handle m a qa).ready = m.ready ++
        ((m.jobs.map fun j => { j with left := j.left - shareOf m.iter j.owner }).filter (·.left = 0)).map
          (·.owner) ∧
      A0 (handle m a qa) := by
  set jobs' := m.jobs.map fun j => { j with left := j.left - shareOf m.iter j.owner } with hjobs'
  set done := (jobs'.filter (·.left = 0)).map (·.owner) with hdone
  set M1 : Machine := { m with now := a, iterEnd := none, jobs := jobs'.filter (·.left ≠ 0), iter := [] }
    with hM1
  have hH : handle m a qa = Exec.readyAll done M1 := by
    unfold handle
    simp only [hie, if_true]
    rfl
  have hdn : done.Nodup := by
    refine List.Nodup.sublist ((List.filter_sublist).map _) ?_
    simpa [hjobs', List.map_map, Function.comp_def] using hn
  have hdb : ∀ i ∈ done, i < M1.sess.size := fun i hi => by
    obtain ⟨x, hx, rfl⟩ := List.mem_map.mp hi
    obtain ⟨x0, hx0, rfl⟩ := List.mem_map.mp (List.mem_filter.mp hx).1
    exact hb x0 hx0
  obtain ⟨f1, f2, -, -, -, -, -, -, -, -, f11⟩ := Exec.readyAll_fields done M1 hdn hdb
  rw [hH]
  refine ⟨f2, f1, fun j => ?_⟩
  rw [f11]
  split_ifs
  · exact hA j
  · exact hA j

end Slot
end SerqLang
