/-
# Dai et al., Theorem 2(b), pathwise

`claim bounded` of `examples/papers/dai_sarathi.sq`: arrivals one gap apart
bring one request's tokens (`W = 290 + 990`) per gap, which is exactly what
the engine serves in that time when its batches are full (`W · t_{b_max} =
b_max · gap`). Before every iteration the tokens that have arrived and not
been served are at most `(b_max + 1) W`, a bound that does not depend on
how many requests arrive (the family allows up to 500).
-/
import Serq.Papers.Dai
import Serq.Steps

namespace SerqLang
namespace Papers
namespace DaiSarathi

open Exec

local notation "Dd" => Claims.DaiSarathi.deployment
local notation "Pd" => Claims.DaiSarathi.prog

/-! ### The program -/

def P4 : Prog := .observe 0 (fun x => x.now - x.attr 9) .stop
def P3 : Prog := .run 0 .decode (fun _ => 990) none P4
def P2 : Prog := .run 0 .prefill (fun _ => 290) none P3
def P1 : Prog := .set 9 (fun x => x.now) P2

theorem prog_eq : Pd = .run 1 .plain (fun x => x.attr 10) none P1 := rfl

theorem deployment_eq : Dd = ⟨[], 128, 0, none, fun st => 1128 + 3547 * ((st.tokens + 127) / 128), none, none⟩ := rfl

/-! ### The state of a request -/

/-- Where a request is: before its first command (`s0`), waiting for its
arrival (`w`), arrived (`r1`), prefilling (`p`), prefilled (`r3`), decoding
(`d`), decoded (`r4`), ended (`e`). -/
inductive Cat
  | s0 | w | r1 | p | r3 | d | r4 | e
  deriving DecidableEq

/-- Each request's place and the tokens its job has left. -/
structure Ghost where
  c : ℕ → Cat
  left : ℕ → ℕ

/-- The tokens request `i` has left to be served, once arrived. -/
def rem (g : Ghost) (i : ℕ) : ℕ :=
  match g.c i with
  | .r1 => 1280
  | .p => g.left i + 990
  | .r3 => 990
  | .d => g.left i
  | _ => 0

/-- Not arrived yet. -/
def fresh (k : Cat) : Bool := k = .s0 || k = .w
/-- In the ready list. -/
def isReady (k : Cat) : Bool := k = .s0 || k = .r1 || k = .r3 || k = .r4
/-- A job at the engine. -/
def isJob (k : Cat) : Bool := k = .p || k = .d

def Shape (s : Sess) : Cat → Prop
  | .s0 => s.prog = Pd ∧ s.status = .ready
  | .w => s.prog = P1 ∧ ∃ u q, s.status = .delay u q
  | .r1 => s.prog = P1 ∧ s.status = .ready
  | .p => s.prog = P3 ∧ s.status = .engine
  | .r3 => s.prog = P3 ∧ s.status = .ready
  | .d => s.prog = P4 ∧ s.status = .engine
  | .r4 => s.prog = P4 ∧ s.status = .ready
  | .e => s.status = .ended

def arr (w : Workload) (i : ℕ) : ℕ := w.attr i 10

section
variable (w : Workload) (g : Ghost)

/-- The tokens arrived and not yet served. -/
def Wn : ℕ := ∑ i ∈ Finset.range w.init.length, rem g i
/-- The requests arrived. -/
def St : ℕ := ((Finset.range w.init.length).filter fun i => fresh (g.c i) = false).card
/-- The requests that arrive by `t` and have not been seen arriving. -/
def Fr (t : ℕ) : ℕ := ((Finset.range w.init.length).filter fun i => fresh (g.c i) = true ∧ arr w i ≤ t).card

end

/-- What one session carries. -/
def SessI (w : Workload) (s : Sess) (i : ℕ) (k : Cat) : Prop :=
  Shape s k ∧ s.stack = [] ∧ s.attr.base = w.attr i ∧ s.attr.get 10 = arr w i

/-- The invariant of an instant. -/
structure DInv (w : Workload) (g : Ghost) (m : Machine) : Prop where
  wl : m.wl = w
  size : m.sess.size = w.init.length
  sess : ∀ i < w.init.length, SessI w (getS m i) i (g.c i)
  jobs : ∀ j ∈ m.jobs, j.owner < w.init.length ∧
    ((g.c j.owner = .p ∧ j.mode = .prefill) ∨ (g.c j.owner = .d ∧ j.mode = .decode)) ∧
    j.growing = none ∧ j.left = g.left j.owner
  jobsP : ∀ i < w.init.length, isJob (g.c i) = true → ∃ j ∈ m.jobs, j.owner = i
  jobsNodup : (m.jobs.map (·.owner)).Nodup
  leftP : ∀ i < w.init.length, g.c i = .p → 1 ≤ g.left i ∧ g.left i ≤ 290
  leftD : ∀ i < w.init.length, g.c i = .d → 1 ≤ g.left i ∧ g.left i ≤ 990
  ready : m.ready.Nodup
  readyMem : ∀ i, i ∈ m.ready ↔ i < w.init.length ∧ isReady (g.c i) = true
  delaysOf : ∀ d ∈ m.delays, d.2.2 < w.init.length ∧ g.c d.2.2 = .w ∧
    (getS m d.2.2).status = .delay d.1 d.2.1 ∧ d.1 = arr w d.2.2 ∧ d.2.1 < m.nextDelay
  delaysW : ∀ i < w.init.length, g.c i = .w → ∃ q, (arr w i, q, i) ∈ m.delays
  delaysNodup : (m.delays.map (·.2.2)).Nodup
  sorted : m.delays.Pairwise (fun a b => a.1 ≤ b.1)
  future : ∀ d ∈ m.delays, m.now ≤ d.1
  past : ∀ i < w.init.length, fresh (g.c i) = false → arr w i ≤ m.now
  s0now : ∀ i < w.init.length, g.c i = .s0 → m.now = 0
  s0idle : ∀ i < w.init.length, g.c i = .s0 → m.iterEnd = none
  iterNone : m.iterEnd = none → m.iter = []
  iterOwn : ∀ e ∈ m.iter, e.1 < w.init.length
  share : ∀ i < w.init.length, shareOf m.iter i ≤ if isJob (g.c i) then g.left i else 0
  busySeq : ∀ e q, m.iterEnd = some (e, q) → ∀ d ∈ m.delays, d.2.1 ≠ q
  cons : 1280 * St w g + tokSum m.iter = m.served + Wn w g

/-! ### Machine helpers -/

/-- The update of a request's place and its job's tokens. -/
def Ghost.set (g : Ghost) (i : ℕ) (k : Cat) (l : ℕ) : Ghost :=
  ⟨Function.update g.c i k, Function.update g.left i l⟩

theorem Ghost.set_c (g : Ghost) (i : ℕ) (k : Cat) (l j : ℕ) :
    (g.set i k l).c j = if j = i then k else g.c j := by
  simp [Ghost.set, Function.update_apply]

theorem Ghost.set_left (g : Ghost) (i : ℕ) (k : Cat) (l j : ℕ) :
    (g.set i k l).left j = if j = i then l else g.left j := by
  simp [Ghost.set, Function.update_apply]

theorem rem_set (g : Ghost) (i : ℕ) (k : Cat) (l j : ℕ) (hj : j ≠ i) : rem (g.set i k l) j = rem g j := by
  simp only [rem, Ghost.set_c, Ghost.set_left, hj, if_false]

/-! ### The sums -/

theorem Wn_congr (w : Workload) {g g' : Ghost} (h : ∀ j < w.init.length, rem g' j = rem g j) :
    Wn w g' = Wn w g :=
  Finset.sum_congr rfl fun j hj => h j (Finset.mem_range.mp hj)

theorem St_congr (w : Workload) {g g' : Ghost} (h : ∀ j < w.init.length, fresh (g'.c j) = fresh (g.c j)) :
    St w g' = St w g := by
  unfold St; congr 1
  exact Finset.filter_congr fun j hj => by rw [h j (Finset.mem_range.mp hj)]

theorem Fr_congr (w : Workload) {g g' : Ghost} (h : ∀ j < w.init.length, fresh (g'.c j) = fresh (g.c j))
    (t : ℕ) : Fr w g' t = Fr w g t := by
  unfold Fr; congr 1
  exact Finset.filter_congr fun j hj => by rw [h j (Finset.mem_range.mp hj)]

/-! ### One request's commands -/

theorem DInv.lt_of_ready {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) : i < w.init.length ∧ isReady (g.c i) = true :=
  (hI.readyMem i).mp (by rw [hr]; simp)

theorem DInv.owner_ne {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {i : ℕ}
    (hc : isJob (g.c i) = false) : ∀ j ∈ m.jobs, j.owner ≠ i := by
  intro j hj he
  obtain ⟨-, h, -⟩ := hI.jobs j hj
  rw [he] at h
  rcases h with ⟨h, -⟩ | ⟨h, -⟩ <;> rw [h] at hc <;> simp [isJob] at hc

theorem DInv.delay_ne {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {i : ℕ}
    (hc : g.c i ≠ .w) : ∀ d ∈ m.delays, d.2.2 ≠ i := by
  intro d hd he
  exact hc (he ▸ (hI.delaysOf d hd).2.1)

/-- A ready request starts a job at the engine: it leaves the ready list,
its job joins the residents, nothing else changes. -/
theorem dinv_addJob {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hcf : fresh (g.c i) = false) (hcj : isJob (g.c i) = false)
    (k : Cat) (md : Mode) (L : ℕ)
    (hk : (k = .p ∧ md = .prefill ∧ 1 ≤ L ∧ L ≤ 290) ∨ (k = .d ∧ md = .decode ∧ 1 ≤ L ∧ L ≤ 990))
    (hrem : rem (g.set i k L) i = rem g i)
    (M : Machine) (s' : Sess) (a b : List Job) (hab : a ++ b = m.jobs)
    (hwl : M.wl = m.wl) (hsz : M.sess.size = m.sess.size)
    (hget : ∀ j, getS M j = if j = i then s' else getS m j)
    (hjobs : M.jobs = a ++ ⟨i, md, L, none⟩ :: b) (hready : M.ready = rest)
    (hdl : M.delays = m.delays) (hnd : M.nextDelay = m.nextDelay) (hnow : M.now = m.now)
    (hit : M.iter = m.iter) (hie : M.iterEnd = m.iterEnd) (hsv : M.served = m.served)
    (hs' : SessI w s' i k) :
    DInv w (g.set i k L) M := by
  obtain ⟨hin, hcr⟩ := hI.lt_of_ready hr
  have hkf : fresh k = false := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> rfl
  have hkj : isJob k = true := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> rfl
  have hkr : isReady k = false := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> rfl
  have hkw : k ≠ .w := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> decide
  have hks : k ≠ .s0 := by rcases hk with ⟨rfl, -⟩ | ⟨rfl, -⟩ <;> decide
  have hciw : g.c i ≠ .w := fun h => by rw [h] at hcf; simp [fresh] at hcf
  have hown := hI.owner_ne hcj
  have hdne := hI.delay_ne hciw
  have hgc : ∀ j, (g.set i k L).c j = if j = i then k else g.c j := Ghost.set_c g i k L
  have hgl : ∀ j, (g.set i k L).left j = if j = i then L else g.left j := Ghost.set_left g i k L
  have hmem : ∀ x, x ∈ M.jobs ↔ x ∈ m.jobs ∨ x = ⟨i, md, L, none⟩ := by
    intro x
    rw [hjobs, ← hab]
    simp only [List.mem_append, List.mem_cons]
    tauto
  have hnd' : (rest).Nodup := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
  have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  refine
    { wl := hwl.trans hI.wl
      size := hsz.trans hI.size
      sess := fun j hj => ?_
      jobs := fun x hx => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := ?_
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := hready ▸ hnd'
      readyMem := fun j => ?_
      delaysOf := fun d hd => ?_
      delaysW := fun j hj hjc => ?_
      delaysNodup := hdl ▸ hI.delaysNodup
      sorted := hdl ▸ hI.sorted
      future := fun d hd => hnow ▸ hI.future d (hdl ▸ hd)
      past := fun j hj hjf => ?_
      s0now := fun j hj hjc => ?_
      s0idle := fun j hj hjc => ?_
      iterNone := fun h => hit ▸ hI.iterNone (hie ▸ h)
      iterOwn := fun e he => hI.iterOwn e (hit ▸ he)
      share := fun j hj => ?_
      busySeq := fun e q h d hd => hI.busySeq e q (hie ▸ h) d (hdl ▸ hd)
      cons := ?_ }
  · rw [hget, hgc]
    split_ifs with h
    · subst h; exact hs'
    · exact hI.sess j hj
  · rcases (hmem x).mp hx with hx | rfl
    · obtain ⟨h1, h2, h3, h4⟩ := hI.jobs x hx
      have hne := hown x hx
      simp only [hgc, hgl, hne, if_false]
      exact ⟨h1, h2, h3, h4⟩
    · simp only [hgc, hgl, if_true]
      refine ⟨hin, ?_, by simp, by simp⟩
      rcases hk with ⟨rfl, rfl, -⟩ | ⟨rfl, rfl, -⟩
      · exact Or.inl ⟨rfl, rfl⟩
      · exact Or.inr ⟨rfl, rfl⟩
  · by_cases h : j = i
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
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; rw [hgl, if_pos rfl]
      rcases hk with ⟨-, -, h1, h2⟩ | ⟨rfl, -⟩
      · exact ⟨h1, h2⟩
      · exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftP j hj hjc
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; rw [hgl, if_pos rfl]
      rcases hk with ⟨rfl, -⟩ | ⟨-, -, h1, h2⟩
      · exact absurd hjc (by decide)
      · exact ⟨h1, h2⟩
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftD j hj hjc
  · rw [hready, hgc]
    by_cases h : j = i
    · subst h; simp only [if_true, hkr]; simp [hir]
    · simp only [h, if_false]
      rw [← hI.readyMem j, hr]
      simp [h]
  · have hd' : d ∈ m.delays := hdl ▸ hd
    obtain ⟨h1, h2, h3, h4, h5⟩ := hI.delaysOf d hd'
    have hne := hdne d hd'
    rw [hgc, if_neg hne, hget, if_neg hne, hnd]
    exact ⟨h1, h2, h3, h4, h5⟩
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc hkw
    · rw [if_neg h] at hjc; rw [hdl]; exact hI.delaysW j hj hjc
  · rw [hnow]
    rw [hgc] at hjf
    by_cases h : j = i
    · subst h; exact hI.past j hj hcf
    · rw [if_neg h] at hjf; exact hI.past j hj hjf
  · rw [hnow]
    rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc hks
    · rw [if_neg h] at hjc; exact hI.s0now j hj hjc
  · rw [hie]
    rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc hks
    · rw [if_neg h] at hjc; exact hI.s0idle j hj hjc
  · rw [hit, hgc, hgl]
    by_cases h : j = i
    · subst h
      have := hI.share j hj
      rw [hcj, if_neg (by simp)] at this
      omega
    · simp only [h, if_false]; exact hI.share j hj
  · rw [hit, hsv]
    have hSt : St w (g.set i k L) = St w g := St_congr w fun j _ => by
      rw [hgc]; split_ifs with h
      · subst h; rw [hkf, hcf]
      · rfl
    have hWn : Wn w (g.set i k L) = Wn w g := Wn_congr w fun j _ => by
      by_cases h : j = i
      · subst h; exact hrem
      · exact rem_set g i k L j h
    rw [hSt, hWn]; exact hI.cons

/-- An arrived request sets `t0` and starts its prefill. -/
theorem exec_r1 {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    DInv w (g.set i .p 290) (exec Dd 10000 { m with ready := rest } i) ∧
      (exec Dd 10000 { m with ready := rest } i).ready = rest ∧
      (exec Dd 10000 { m with ready := rest } i).delays = m.delays ∧
      (exec Dd 10000 { m with ready := rest } i).nextDelay = m.nextDelay ∧
      Keeps (exec Dd 10000 { m with ready := rest } i) m := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hI.size ▸ hin
  obtain ⟨hsh, hstk, hb, h10⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    exec_set Dd _ m0 i 9 (fun x => x.now) P2 (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl)]
  set s1 : Sess := { getS m0 i with attr := (getS m0 i).attr.upd 9 (evalE m0 i fun x => x.now), prog := P2 }
    with hs1
  set m1 := setS m0 i s1 with hm1
  have hg1 : getS m1 i = s1 := Exec.getS_setS_self m0 s1 (by simpa using hisz)
  obtain ⟨a, b, hab, he⟩ := exec_runEngine' Dd 9998 m1 i .prefill (fun _ => 290) P3
    (by rw [hg1]; exact hst) (by rw [hg1]; rfl) (by simp [evalE])
  rw [he]
  have hev : evalE m1 i (fun _ => 290) = 290 := rfl
  rw [hev]
  set s2 : Sess := { getS m1 i with prog := P3, status := .engine } with hs2
  refine ⟨dinv_addJob hI hr (by rw [hc]; rfl) (by rw [hc]; rfl) .p .prefill 290 (Or.inl ⟨rfl, rfl, by norm_num, le_rfl⟩)
    (by simp [rem, Ghost.set_c, Ghost.set_left, hc]) _ s2 a b hab rfl (by simp [hm1, hm0]) (fun j => ?_)
    rfl rfl rfl rfl rfl rfl rfl rfl ?_, rfl, rfl, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩⟩
  · show getS (setS m1 i s2) j = _
    rw [getS_setS m1 s2 (by simpa [hm1, hm0] using hisz)]
    split_ifs with h
    · rfl
    · rw [hm1, getS_setS m0 s1 (by simpa [hm0] using hisz), if_neg h]; rfl
  · rw [hs2, hg1, hs1]
    refine ⟨⟨rfl, rfl⟩, hstk, ?_, ?_⟩
    · simp only [hg0, Attrs.upd_base]; exact hb
    · simp only [hg0]; rw [Attrs.get_upd_ne _ _ (by norm_num)]; exact h10

/-- A prefilled request starts its decode. -/
theorem exec_r3 {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r3) :
    DInv w (g.set i .d 990) (exec Dd 10000 { m with ready := rest } i) ∧
      (exec Dd 10000 { m with ready := rest } i).ready = rest ∧
      (exec Dd 10000 { m with ready := rest } i).delays = m.delays ∧
      (exec Dd 10000 { m with ready := rest } i).nextDelay = m.nextDelay ∧
      Keeps (exec Dd 10000 { m with ready := rest } i) m := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hI.size ▸ hin
  obtain ⟨hsh, hstk, hb, h10⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  obtain ⟨a, b, hab, he⟩ := exec_runEngine' Dd 9999 m0 i .decode (fun _ => 990) P4
    (by rw [hg0]; exact hst) (by rw [hg0, hp]; rfl) (by simp [evalE])
  rw [show (10000 : ℕ) = 9999 + 1 from rfl, he]
  have hev : evalE m0 i (fun _ => 990) = 990 := rfl
  rw [hev]
  set s2 : Sess := { getS m0 i with prog := P4, status := .engine } with hs2
  refine ⟨dinv_addJob hI hr (by rw [hc]; rfl) (by rw [hc]; rfl) .d .decode 990 (Or.inr ⟨rfl, rfl, by norm_num, le_rfl⟩)
    (by simp [rem, Ghost.set_c, Ghost.set_left, hc]) _ s2 a b hab rfl (by simp [hm0]) (fun j => ?_)
    rfl rfl rfl rfl rfl rfl rfl rfl ?_, rfl, rfl, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩⟩
  · show getS (setS m0 i s2) j = _
    rw [getS_setS m0 s2 (by simpa [hm0] using hisz)]; rfl
  · rw [hs2, hg0]
    exact ⟨⟨rfl, rfl⟩, hstk, hb, h10⟩


/-- A decoded request observes its latency and ends. -/
theorem exec_r4 {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .r4) :
    DInv w (g.set i .e 0) (exec Dd 10000 { m with ready := rest } i) ∧
      (exec Dd 10000 { m with ready := rest } i).ready = rest ∧
      (exec Dd 10000 { m with ready := rest } i).delays = m.delays ∧
      (exec Dd 10000 { m with ready := rest } i).nextDelay = m.nextDelay ∧
      Keeps (exec Dd 10000 { m with ready := rest } i) m := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hI.size ▸ hin
  obtain ⟨hsh, hstk, hb, h10⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  rw [show (10000 : ℕ) = 9998 + 1 + 1 from rfl,
    Exec.exec_observe Dd _ m0 i 0 (fun x => x.now - x.attr 9) .stop (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl)]
  set m1 : Machine := { setS m0 i { getS m0 i with prog := .stop } with
    obs := (0, (getS m0 i).serial, m0.now, evalE m0 i fun x => x.now - x.attr 9) :: m0.obs } with hm1
  have hg1 : getS m1 i = { getS m0 i with prog := .stop } :=
    Exec.getS_setS_self m0 _ (by simpa using hisz)
  rw [Exec.exec_stop Dd _ m1 i (by rw [hg1]; exact hst) (by rw [hg1]) (by rw [hg1]; exact hstk)]
  set s' : Sess := { getS m1 i with status := .ended, stack := [] } with hs'
  have hget : ∀ j, getS (setS m1 i s') j = if j = i then s' else getS m j := by
    intro j
    rw [getS_setS m1 s' (by simpa [hm1, hm0] using hisz)]
    split_ifs with h
    · rfl
    · show getS (setS m0 i _) j = _
      rw [getS_setS m0 _ (by simpa [hm0] using hisz), if_neg h]; rfl
  have hgc : ∀ j, (g.set i .e 0).c j = if j = i then .e else g.c j := Ghost.set_c g i .e 0
  have hgl : ∀ j, (g.set i .e 0).left j = if j = i then 0 else g.left j := Ghost.set_left g i .e 0
  have hown := hI.owner_ne (i := i) (by rw [hc]; rfl)
  have hdne := hI.delay_ne (i := i) (by rw [hc]; decide)
  have hnd' : rest.Nodup := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
  have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  set M := setS m1 i s' with hM
  have hjobs : M.jobs = m.jobs := rfl
  refine ⟨{
      wl := hI.wl
      size := by simp [hM, hm1, hm0, hI.size]
      sess := fun j hj => ?_
      jobs := fun x hx => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := hI.jobsNodup
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := hnd'
      readyMem := fun j => ?_
      delaysOf := fun d hd => ?_
      delaysW := fun j hj hjc => ?_
      delaysNodup := hI.delaysNodup
      sorted := hI.sorted
      future := hI.future
      past := fun j hj hjf => ?_
      s0now := fun j hj hjc => ?_
      s0idle := fun j hj hjc => ?_
      iterNone := hI.iterNone
      iterOwn := hI.iterOwn
      share := fun j hj => ?_
      busySeq := hI.busySeq
      cons := ?_ }, rfl, rfl, rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩⟩
  · rw [hget, hgc]
    split_ifs with h
    · subst h; exact ⟨rfl, rfl, by simp [hs', hg1, hg0, hb], by simp [hs', hg1, hg0, h10]⟩
    · exact hI.sess j hj
  · obtain ⟨h1, h2, h3, h4⟩ := hI.jobs x (hjobs ▸ hx)
    have hne := hown x (hjobs ▸ hx)
    simp only [hgc, hgl, hne, if_false]
    exact ⟨h1, h2, h3, h4⟩
  · by_cases h : j = i
    · subst h; rw [hgc, if_pos rfl] at hjj; exact absurd hjj (by decide)
    · rw [hgc, if_neg h] at hjj; exact hI.jobsP j hj hjj
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftP j hj hjc
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftD j hj hjc
  · show j ∈ rest ↔ _
    rw [hgc]
    by_cases h : j = i
    · subst h; simp [hir, isReady]
    · simp only [h, if_false]
      rw [← hI.readyMem j, hr]
      simp [h]
  · obtain ⟨h1, h2, h3, h4, h5⟩ := hI.delaysOf d hd
    have hne := hdne d hd
    rw [hgc, if_neg hne, hget, if_neg hne]
    exact ⟨h1, h2, h3, h4, h5⟩
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; exact hI.delaysW j hj hjc
  · rw [hgc] at hjf
    by_cases h : j = i
    · subst h; exact hI.past j hj (by rw [hc]; rfl)
    · rw [if_neg h] at hjf; exact hI.past j hj hjf
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; exact hI.s0now j hj hjc
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; exact hI.s0idle j hj hjc
  · show shareOf m.iter j ≤ _
    rw [hgc, hgl]
    by_cases h : j = i
    · subst h
      have := hI.share j hj
      rw [hc] at this
      simp [isJob] at this
      simp [this]
    · simp only [h, if_false]; exact hI.share j hj
  · show 1280 * St w (g.set i .e 0) + tokSum m.iter = m.served + Wn w (g.set i .e 0)
    have hSt : St w (g.set i .e 0) = St w g := St_congr w fun j _ => by
      rw [hgc]; split_ifs with h
      · subst h; rw [hc]; rfl
      · rfl
    have hWn : Wn w (g.set i .e 0) = Wn w g := Wn_congr w fun j _ => by
      by_cases h : j = i
      · subst h; simp [rem, hgc, hc]
      · exact rem_set g i .e 0 j h
    rw [hSt, hWn]; exact hI.cons

/-- A request at its first command starts waiting for its arrival. -/
theorem dinv_wait {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .s0)
    (M : Machine) (s' : Sess) (hwl : M.wl = m.wl) (hsz : M.sess.size = m.sess.size)
    (hget : ∀ j, getS M j = if j = i then s' else getS m j)
    (hjobs : M.jobs = m.jobs) (hready : M.ready = rest)
    (hdl : M.delays = insertDelay (arr w i, m.nextDelay, i) m.delays) (hnd : M.nextDelay = m.nextDelay + 1)
    (hnow : M.now = m.now) (hit : M.iter = m.iter) (hie : M.iterEnd = m.iterEnd) (hsv : M.served = m.served)
    (hs' : SessI w s' i .w) (hst' : s'.status = .delay (arr w i) m.nextDelay) :
    DInv w (g.set i .w 0) M := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hnow0 := hI.s0now i hin hc
  have hidle := hI.s0idle i hin hc
  set x : ℕ × ℕ × ℕ := (arr w i, m.nextDelay, i) with hx
  have hgc : ∀ j, (g.set i .w 0).c j = if j = i then .w else g.c j := Ghost.set_c g i .w 0
  have hgl : ∀ j, (g.set i .w 0).left j = if j = i then 0 else g.left j := Ghost.set_left g i .w 0
  have hown := hI.owner_ne (i := i) (by rw [hc]; rfl)
  have hdne := hI.delay_ne (i := i) (by rw [hc]; decide)
  have hnd' : rest.Nodup := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
  have hir : i ∉ rest := by have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  have hmemd : ∀ d, d ∈ M.delays ↔ d = x ∨ d ∈ m.delays := fun d => by rw [hdl]; exact mem_insertDelay x d _
  refine {
      wl := hwl.trans hI.wl
      size := hsz.trans hI.size
      sess := fun j hj => ?_
      jobs := fun y hy => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := hjobs ▸ hI.jobsNodup
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := hready ▸ hnd'
      readyMem := fun j => ?_
      delaysOf := fun d hd => ?_
      delaysW := fun j hj hjc => ?_
      delaysNodup := ?_
      sorted := hdl ▸ sorted_insertDelay x _ hI.sorted
      future := fun d hd => by rw [hnow, hnow0]; exact Nat.zero_le _
      past := fun j hj hjf => ?_
      s0now := fun j hj hjc => hnow.trans hnow0
      s0idle := fun j hj hjc => hie.trans hidle
      iterNone := fun h => hit ▸ hI.iterNone (hie ▸ h)
      iterOwn := fun e he => hI.iterOwn e (hit ▸ he)
      share := fun j hj => ?_
      busySeq := fun e q h => by rw [hie, hidle] at h; simp at h
      cons := ?_ }
  · rw [hget, hgc]
    split_ifs with h
    · subst h; exact hs'
    · exact hI.sess j hj
  · rw [hjobs] at hy
    obtain ⟨h1, h2, h3, h4⟩ := hI.jobs y hy
    have hne := hown y hy
    simp only [hgc, hgl, hne, if_false]
    exact ⟨h1, h2, h3, h4⟩
  · rw [hjobs]
    by_cases h : j = i
    · subst h; rw [hgc, if_pos rfl] at hjj; exact absurd hjj (by decide)
    · rw [hgc, if_neg h] at hjj; exact hI.jobsP j hj hjj
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftP j hj hjc
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftD j hj hjc
  · rw [hready, hgc]
    by_cases h : j = i
    · subst h; simp [hir, isReady]
    · simp only [h, if_false]
      rw [← hI.readyMem j, hr]
      simp [h]
  · rw [hnd]
    rcases (hmemd d).mp hd with rfl | hd
    · simp only [hx]
      rw [hgc, if_pos rfl, hget, if_pos rfl]
      exact ⟨hin, by simp, hst', by simp, Nat.lt_succ_self _⟩
    · obtain ⟨h1, h2, h3, h4, h5⟩ := hI.delaysOf d hd
      have hne := hdne d hd
      rw [hgc, if_neg hne, hget, if_neg hne]
      exact ⟨h1, h2, h3, h4, by omega⟩
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; exact ⟨m.nextDelay, (hmemd _).mpr (Or.inl rfl)⟩
    · rw [if_neg h] at hjc
      obtain ⟨q, hq⟩ := hI.delaysW j hj hjc
      exact ⟨q, (hmemd _).mpr (Or.inr hq)⟩
  · rw [hdl]
    refine ((insertDelay_perm x _).map _).nodup_iff.mpr ?_
    refine List.nodup_cons.mpr ⟨fun hm => ?_, hI.delaysNodup⟩
    obtain ⟨d, hd, hdo⟩ := List.mem_map.mp hm
    exact hdne d hd hdo
  · rw [hgc] at hjf
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjf; exact absurd hjf (by decide)
    · rw [if_neg h] at hjf; rw [hnow]; exact hI.past j hj hjf
  · rw [hit, hgc, hgl]
    by_cases h : j = i
    · subst h
      have := hI.share j hj
      rw [hc] at this
      simp [isJob] at this
      simp [this]
    · simp only [h, if_false]; exact hI.share j hj
  · rw [hit, hsv]
    have hSt : St w (g.set i .w 0) = St w g := St_congr w fun j _ => by
      rw [hgc]; split_ifs with h
      · subst h; rw [hc]; rfl
      · rfl
    have hWn : Wn w (g.set i .w 0) = Wn w g := Wn_congr w fun j _ => by
      by_cases h : j = i
      · subst h; simp [rem, hgc, hc]
      · exact rem_set g i .w 0 j h
    rw [hSt, hWn]; exact hI.cons

/-- A request runs its first command: it waits for its arrival. -/
theorem exec_s0 {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m)
    (harr : ∀ i < w.init.length, 0 < arr w i) {i : ℕ} {rest : List ℕ}
    (hr : m.ready = i :: rest) (hc : g.c i = .s0) :
    DInv w (g.set i .w 0) (exec Dd 10000 { m with ready := rest } i) ∧
      (exec Dd 10000 { m with ready := rest } i).ready = rest ∧
      Keeps (exec Dd 10000 { m with ready := rest } i) m := by
  obtain ⟨hin, -⟩ := hI.lt_of_ready hr
  have hisz : i < m.sess.size := hI.size ▸ hin
  obtain ⟨hsh, hstk, hb, h10⟩ := hI.sess i hin
  rw [hc] at hsh
  obtain ⟨hp, hst⟩ := hsh
  have hnow := hI.s0now i hin hc
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun _ => rfl
  have hev : evalE m0 i (fun x => x.attr 10) = arr w i := by
    simp only [evalE, env, hg0]; exact h10
  rw [show (10000 : ℕ) = 9999 + 1 from rfl,
    exec_runDelay Dd _ m0 i 1 .plain (fun x => x.attr 10) P1 (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl) (by rw [hev]; exact (harr i hin).ne') (by norm_num), hev]
  have hn0 : m0.now = 0 := hnow
  rw [hn0, Nat.zero_add]
  refine ⟨dinv_wait hI hr hc _ { getS m0 i with prog := P1, status := .delay (arr w i) m0.nextDelay }
    rfl (by simp [hm0]) (fun j => ?_) rfl rfl rfl rfl rfl rfl rfl rfl ⟨⟨rfl, _, _, rfl⟩, hstk, hb, h10⟩ rfl,
    rfl, ⟨rfl, rfl, rfl, rfl, rfl⟩⟩
  show getS (setS m0 i _) j = _
  rw [getS_setS m0 _ (by simpa [hm0] using hisz)]; rfl


/-- What draining the ready requests does to the ghost: nobody's tokens
left, nor whether it has arrived, change. -/
def Same3 (n : ℕ) (g g' : Ghost) : Prop :=
  ∀ j < n, rem g' j = rem g j ∧ fresh (g'.c j) = fresh (g.c j) ∧ (g.c j ≠ .s0 → g'.c j ≠ .s0)

theorem Same3.refl (n : ℕ) (g : Ghost) : Same3 n g g := fun _ _ => ⟨rfl, rfl, id⟩

theorem Same3.trans {n : ℕ} {a b c : Ghost} (h1 : Same3 n a b) (h2 : Same3 n b c) : Same3 n a c :=
  fun j hj => ⟨(h2 j hj).1.trans (h1 j hj).1, (h2 j hj).2.1.trans (h1 j hj).2.1,
    fun h => (h2 j hj).2.2 ((h1 j hj).2.2 h)⟩

theorem Same3.set {n : ℕ} (g : Ghost) (i : ℕ) (k : Cat) (l : ℕ) (hr : rem (g.set i k l) i = rem g i)
    (hf : fresh k = fresh (g.c i)) (hk : k ≠ .s0) : Same3 n g (g.set i k l) := by
  intro j _
  by_cases h : j = i
  · subst h
    refine ⟨hr, by rw [Ghost.set_c, if_pos rfl, hf], fun _ => by rw [Ghost.set_c, if_pos rfl]; exact hk⟩
  · exact ⟨rem_set g i k l j h, by rw [Ghost.set_c, if_neg h], fun h' => by rw [Ghost.set_c, if_neg h]; exact h'⟩

/-- **Draining.** Every ready request runs until it waits, holds a job or
ends; nothing else changes, and with nobody at its first command the delays
do not either. -/
theorem drain_dinv {w : Workload} (harr : ∀ i < w.init.length, 0 < arr w i) :
    ∀ (f : ℕ) (g : Ghost) (m : Machine), DInv w g m → m.ready.length ≤ f →
      ∃ g', DInv w g' (drain Dd f m) ∧ (drain Dd f m).ready = [] ∧ Keeps (drain Dd f m) m ∧
        Same3 w.init.length g g' ∧
        ((∀ j < w.init.length, g.c j ≠ .s0) →
          (drain Dd f m).delays = m.delays ∧ (drain Dd f m).nextDelay = m.nextDelay)
  | 0, g, m, hI, hf => by
    have hr : m.ready = [] := List.eq_nil_of_length_eq_zero (by omega)
    exact ⟨g, hI, by simp [drain, hr], Keeps.refl m, Same3.refl _ g, fun _ => ⟨rfl, rfl⟩⟩
  | f + 1, g, m, hI, hf => by
    unfold drain
    split
    · rename_i hr
      exact ⟨g, hI, hr, Keeps.refl m, Same3.refl _ g, fun _ => ⟨rfl, rfl⟩⟩
    · rename_i i rest hr
      have hlen : rest.length ≤ f := by rw [hr] at hf; simpa using hf
      obtain ⟨hin, hcr⟩ := hI.lt_of_ready hr
      obtain ⟨g1, hI1, hr1, hk1, hs1, hd1⟩ : ∃ g1, DInv w g1 (exec Dd 10000 { m with ready := rest } i) ∧
          (exec Dd 10000 { m with ready := rest } i).ready = rest ∧
          Keeps (exec Dd 10000 { m with ready := rest } i) m ∧ Same3 w.init.length g g1 ∧
          ((∀ j < w.init.length, g.c j ≠ .s0) →
            (exec Dd 10000 { m with ready := rest } i).delays = m.delays ∧
            (exec Dd 10000 { m with ready := rest } i).nextDelay = m.nextDelay) := by
        cases hc : g.c i with
        | s0 =>
          obtain ⟨h1, h2, h3⟩ := exec_s0 hI harr hr hc
          exact ⟨_, h1, h2, h3, Same3.set g i .w 0 (by simp [rem, Ghost.set_c, hc]) (by rw [hc]; rfl)
            (by decide), fun h => absurd hc (h i hin)⟩
        | r1 =>
          obtain ⟨h1, h2, h3, h4, h5⟩ := exec_r1 hI hr hc
          exact ⟨_, h1, h2, h5, Same3.set g i .p 290 (by simp [rem, Ghost.set_c, Ghost.set_left, hc])
            (by rw [hc]; rfl) (by decide), fun _ => ⟨h3, h4⟩⟩
        | r3 =>
          obtain ⟨h1, h2, h3, h4, h5⟩ := exec_r3 hI hr hc
          exact ⟨_, h1, h2, h5, Same3.set g i .d 990 (by simp [rem, Ghost.set_c, Ghost.set_left, hc])
            (by rw [hc]; rfl) (by decide), fun _ => ⟨h3, h4⟩⟩
        | r4 =>
          obtain ⟨h1, h2, h3, h4, h5⟩ := exec_r4 hI hr hc
          exact ⟨_, h1, h2, h5, Same3.set g i .e 0 (by simp [rem, Ghost.set_c, hc])
            (by rw [hc]; rfl) (by decide), fun _ => ⟨h3, h4⟩⟩
        | _ => rw [hc] at hcr; simp [isReady] at hcr
      obtain ⟨g2, hI2, hr2, hk2, hs2, hd2⟩ := drain_dinv harr f g1 _ hI1 (by rw [hr1]; exact hlen)
      refine ⟨g2, hI2, hr2, hk1.trans hk2, hs1.trans hs2, fun h => ?_⟩
      obtain ⟨e1, e2⟩ := hd1 h
      obtain ⟨e3, e4⟩ := hd2 fun j hj => (hs1 j hj).2.2 (h j hj)
      exact ⟨e3.trans e1, e4.trans e2⟩


theorem admitAll_dai (m : Machine) : admitAll Dd m = m := by
  simp [admitAll, Claims.DaiSarathi.deployment]

theorem DInv.ready_short {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) :
    m.ready.length ≤ w.init.length :=
  Exec.length_le_of_nodup_lt hI.ready fun x hx => ((hI.readyMem x).mp hx).1

/-- **Settling an instant.** -/
theorem settle_dinv {w : Workload} (harr : ∀ i < w.init.length, 0 < arr w i) (hn : w.init.length ≤ 500)
    {g : Ghost} {m : Machine} (hI : DInv w g m) :
    ∃ g', DInv w g' (settle Dd m) ∧ (settle Dd m).ready = [] ∧ Keeps (settle Dd m) m ∧
      Same3 w.init.length g g' ∧
      ((∀ j < w.init.length, g.c j ≠ .s0) → (settle Dd m).delays = m.delays ∧ (settle Dd m).nextDelay = m.nextDelay) := by
  obtain ⟨g', h1, h2, h3, h4, h5⟩ := drain_dinv harr 10000 g m hI (by have := hI.ready_short; omega)
  have hs : settle Dd m = drain Dd 10000 m := by
    unfold settle
    rw [show (1000 : ℕ) = 999 + 1 from rfl, settleLoop]
    simp only [admitAll_dai, h2, List.isEmpty_nil, ↓reduceIte]
  rw [hs]
  exact ⟨g', h1, h2, h3, h4, h5⟩


/-! ### The batch -/

/-- The sum of `f` over the owners of the jobs is the sum over the requests,
if `f` vanishes off the jobs. -/
theorem sum_jobs {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) (f : ℕ → ℕ)
    (hf : ∀ i < w.init.length, isJob (g.c i) = false → f i = 0) :
    (m.jobs.map fun j => f j.owner).sum = ∑ i ∈ Finset.range w.init.length, f i := by
  have h1 : (m.jobs.map fun j => f j.owner) = (m.jobs.map (·.owner)).map f := by simp [List.map_map, Function.comp_def]
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

/-- What a request's job wants from an iteration. -/
def wantG (g : Ghost) (i : ℕ) : ℕ :=
  match g.c i with
  | .p => g.left i
  | .d => 1
  | _ => 0

theorem want_job {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) {j : Job} (hj : j ∈ m.jobs) :
    wantOf Dd j = wantG g j.owner := by
  obtain ⟨h1, h2, -, h4⟩ := hI.jobs j hj
  rcases h2 with ⟨hc, hm⟩ | ⟨hc, hm⟩
  · simp [wantOf, wantG, hm, hc, h4, Claims.DaiSarathi.deployment]
  · have := (hI.leftD _ h1 hc).1
    simp only [wantOf, wantG, hm, hc, h4]; omega

theorem demand_eq {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) :
    (m.jobs.map (wantOf Dd)).sum = ∑ i ∈ Finset.range w.init.length, wantG g i := by
  rw [← sum_jobs hI (wantG g) fun i _ h => ?_]
  · congr 1; exact List.map_congr_left fun j hj => want_job hI hj
  · unfold wantG; split <;> simp_all [isJob]


/-! ### Arrivals and time -/

/-- The workloads of the claim: requests one gap apart. -/
structure Fam (w : Workload) : Prop where
  len : w.init.length ≤ 500
  slot : w.arriveSlot = some 10
  gap : ∀ i < w.init.length, arr w i = (i + 1) * 46750

theorem fam_of_family {w : Workload} (hw : Claims.DaiSarathi.family_bounded w) : Fam w :=
  ⟨hw.1, hw.2.2.2.2.1, hw.2.2.2.2.2⟩

theorem Fam.pos {w : Workload} (hF : Fam w) : ∀ i < w.init.length, 0 < arr w i := fun i hi => by
  rw [hF.gap i hi]; omega

/-- **An iteration's worth of arrivals.** If nobody still to arrive is due
by `s`, at most one is due in the iteration from `s`, and only if a gap
boundary falls in it: the arrivals and the phase together grow by at most
the iteration's length. -/
theorem fr_step {w : Workload} {g : Ghost} (hF : Fam w) {s : ℕ}
    (hfut : ∀ i < w.init.length, fresh (g.c i) = true → s < arr w i) :
    46750 * Fr w g (s + 4675) + (s + 4675) % 46750 ≤ s % 46750 + 4675 := by
  have hsub : (Finset.range w.init.length).filter (fun i => fresh (g.c i) = true ∧ arr w i ≤ s + 4675) ⊆
      {s / 46750} := by
    intro i hi
    simp only [Finset.mem_filter, Finset.mem_range] at hi
    have h1 := hfut i hi.1 hi.2.1
    have h2 := hi.2.2
    rw [hF.gap i hi.1] at h1 h2
    simp only [Finset.mem_singleton]
    omega
  have hle : Fr w g (s + 4675) ≤ 1 := (Finset.card_le_card hsub).trans (by simp)
  have hwrap : 0 < Fr w g (s + 4675) → 46750 ≤ s % 46750 + 4675 := by
    intro hpos
    obtain ⟨i, hi⟩ := Finset.card_pos.mp hpos
    simp only [Finset.mem_filter, Finset.mem_range] at hi
    have h1 := hfut i hi.1 hi.2.1
    have h2 := hi.2.2
    rw [hF.gap i hi.1] at h1 h2
    omega
  rcases Nat.eq_zero_or_pos (Fr w g (s + 4675)) with h0 | h0
  · rw [h0]; omega
  · have := hwrap h0
    have : Fr w g (s + 4675) = 1 := by omega
    rw [this]; omega

theorem fr_zero {w : Workload} {g : Ghost} {s : ℕ}
    (hfut : ∀ i < w.init.length, fresh (g.c i) = true → s < arr w i) : Fr w g s = 0 := by
  unfold Fr
  rw [Finset.card_eq_zero, Finset.filter_eq_empty_iff]
  intro i hi h
  have := hfut i (Finset.mem_range.mp hi) h.1
  omega

/-- The claim's `arrived`: the requests whose arrival time has come. -/
theorem arrived_eq {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) (t : ℕ) :
    (m.sess.toList.filter fun s => decide (s.attr.base 10 ≤ t)).length =
      ((Finset.range w.init.length).filter fun i => arr w i ≤ t).card := by
  rw [sess_toList, List.filter_map, List.length_map, hI.size, ← card_range_filter]
  congr 1
  apply List.filter_congr
  intro i hi
  have hi' := List.mem_range.mp hi
  simp only [Function.comp_apply, (hI.sess i hi').2.2.1]
  rfl


/-! ### The potential -/

/-- Between events: the invariant, and the potential of the running
iteration (projected to its end) or of the idle engine (now), with the
claim for the running iteration. -/
structure Pre (w : Workload) (g : Ghost) (m : Machine) : Prop extends DInv w g m where
  potB : ∀ e q, m.iterEnd = some (e, q) → m.now ≤ e ∧
    46750 * (Wn w g + 1280 * Fr w g e) + 1280 * (e % 46750) ≤ 46750 * (165120 + tokSum m.iter)
  potI : m.iterEnd = none → 46750 * (Wn w g + 1280 * Fr w g m.now) + 1280 * (m.now % 46750) ≤ 46750 * 165120
  good : m.iterEnd.isSome = true → m.last.arrived * 1280 ≤ m.last.served + 165120

/-- At an event boundary: settled, and an idle engine has nothing to do or
an event due now. -/
structure Bnd (w : Workload) (g : Ghost) (m : Machine) : Prop extends Pre w g m where
  rdy : m.ready = []
  idleW : m.iterEnd = none → Wn w g = 0 ∨ pendingBy m m.now = true

theorem DInv.not_ready {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) (hr : m.ready = []) :
    ∀ i < w.init.length, isReady (g.c i) = false := by
  intro i hi
  by_contra h
  have := (hI.readyMem i).mpr ⟨hi, by simpa using h⟩
  rw [hr] at this; simp at this

/-- After settling, the engine's batch is at most its demand plus what
the residents keep: a prefill keeps its decode, a decode all but one token. -/
theorem wn_le {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) (hr : m.ready = []) :
    Wn w g ≤ ∑ i ∈ Finset.range w.init.length, wantG g i +
      990 * ∑ i ∈ Finset.range w.init.length, (if isJob (g.c i) then 1 else 0) ∧
    ∑ i ∈ Finset.range w.init.length, (if isJob (g.c i) then 1 else 0) ≤
      ∑ i ∈ Finset.range w.init.length, wantG g i := by
  have hnr := hI.not_ready hr
  constructor
  · rw [Wn, Finset.mul_sum, ← Finset.sum_add_distrib]
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

theorem wn_zero {w : Workload} {g : Ghost} {m : Machine} (hI : DInv w g m) (hr : m.ready = [])
    (hj : m.jobs = []) : Wn w g = 0 := by
  have hnr := hI.not_ready hr
  apply Finset.sum_eq_zero
  intro i hi
  have hi' := Finset.mem_range.mp hi
  have h1 := hnr i hi'
  unfold rem
  cases hc : g.c i <;> simp [hc, isReady] at h1 ⊢
  · obtain ⟨j, hj', -⟩ := hI.jobsP i hi' (by rw [hc]; rfl); rw [hj] at hj'; simp at hj'
  · obtain ⟨j, hj', -⟩ := hI.jobsP i hi' (by rw [hc]; rfl); rw [hj] at hj'; simp at hj'

theorem cost_eq (st : IterStats) (h1 : 1 ≤ st.tokens) (h2 : st.tokens ≤ 128) :
    max 1 (Claims.DaiSarathi.deployment.cost st) = 4675 := by
  show max 1 (1128 + 3547 * ((st.tokens + 127) / 128)) = 4675
  have : (st.tokens + 127) / 128 = 1 := by omega
  rw [this]; rfl

/-- **An iteration starts.** Its claim holds, and the potential projected to
its end is within the bound plus its batch: a full batch serves what an
iteration's arrivals and phase can add, and a partial one leaves at most a
decode's worth for each of fewer than 128 residents. -/
theorem start_bnd {w : Workload} (hF : Fam w) {g : Ghost} {m : Machine} (hI : DInv w g m)
    (hr : m.ready = []) (hie : m.iterEnd = none) (hp : pendingBy m m.now = false)
    (hpot : 46750 * (Wn w g + 1280 * Fr w g m.now) + 1280 * (m.now % 46750) ≤ 46750 * 165120) :
    Bnd w g (startIteration Dd m) := by
  have hnr := hI.not_ready hr
  have hfut : ∀ i < w.init.length, fresh (g.c i) = true → m.now < arr w i := by
    intro i hi hf
    have h1 := hnr i hi
    cases hc : g.c i <;> simp [hc, fresh, isReady] at hf h1
    obtain ⟨q, hq⟩ := hI.delaysW i hi hc
    exact not_pending hie hp hI.sorted _ hq
  have hFr0 := fr_zero (g := g) hfut
  rw [hFr0] at hpot
  have hq : engineQueuesEmpty Dd m := fun p hp => by simp [pdef, Claims.DaiSarathi.deployment] at hp
  have hg : ∀ j ∈ m.jobs, j.growing = none := fun j hj => (hI.jobs j hj).2.2.1
  have ha := assign_eq_fillIter Dd rfl m hq hg m.preempts (m.jobs.length + 100000) 0 128 [] (by omega)
  simp only [List.drop_zero, List.nil_append] at ha
  have hvia : ((List.range Claims.DaiSarathi.deployment.pools.length).any fun p =>
      (pdef Dd p).viaEngine && !(pst m p).queue.isEmpty) = false := by
    simp [Claims.DaiSarathi.deployment]
  have hb : Claims.DaiSarathi.deployment.budget = 128 := rfl
  have hiter0 : m.iter = [] := hI.iterNone hie
  have hcons := hI.cons
  rw [hiter0] at hcons
  simp only [tokSum, List.map_nil, List.sum_nil, Nat.add_zero] at hcons
  unfold startIteration
  -- the deployment has no `chunkAt`: every iteration runs it as it is
  simp only [iterDeployment_of_none _ (rfl : Claims.DaiSarathi.deployment.chunkAt = none)]
  rw [hvia, Bool.or_false]
  by_cases hjs : m.jobs = []
  · have he0 : m.jobs.isEmpty = true := by simp [hjs]
    simp only [he0, Bool.not_true, Bool.false_eq_true, ↓reduceIte]
    have hW := wn_zero hI hr hjs
    refine ⟨⟨{ hI with
        s0idle := fun _ _ _ => rfl
        iterNone := fun _ => rfl
        iterOwn := fun e he => by simp at he
        share := fun i hi => by simp [shareOf]
        busySeq := fun e q h => by simp at h
        cons := by show 1280 * St w g + tokSum [] = m.served + Wn w g; simpa [tokSum] using hcons },
        fun e q h => by simp at h,
        fun _ => by rw [hFr0]; exact hpot, fun h => by simp at h⟩, hr, fun _ => Or.inl hW⟩
  · obtain ⟨j0, js, hjs'⟩ := List.exists_cons_of_ne_nil hjs
    have hne0 : m.jobs.isEmpty = false := by simpa using hjs
    simp only [hne0, Bool.not_false, ↓reduceIte]
    rw [hb, ha]
    set it := fillIter Dd m.jobs 128 with hit
    have hj0 : j0 ∈ m.jobs := by rw [hjs']; exact List.mem_cons_self
    have hw0 : 0 < wantOf Dd j0 := by
      rw [want_job hI hj0]
      obtain ⟨h1, h2, -⟩ := hI.jobs j0 hj0
      rcases h2 with ⟨hc, -⟩ | ⟨hc, -⟩
      · simp [wantG, hc]; exact (hI.leftP _ h1 hc).1
      · simp [wantG, hc]
    have hne : it ≠ [] := by rw [hit, hjs']; exact fillIter_ne_nil Dd j0 js 128 hw0 (by norm_num)
    have hne' : it.isEmpty = false := by simpa using hne
    simp only [hne', Bool.not_false, Bool.true_or, ↓reduceIte]
    -- the batch
    have hk : tokSum it = min 128 (∑ i ∈ Finset.range w.init.length, wantG g i) := by
      rw [hit, tokSum_fillIter, demand_eq hI]
    set k := tokSum it with hkdef
    have hk1 : 1 ≤ k := by
      rw [hk]
      have : wantG g j0.owner ≤ ∑ i ∈ Finset.range w.init.length, wantG g i :=
        Finset.single_le_sum (fun _ _ => Nat.zero_le _) (Finset.mem_range.mpr (hI.jobs j0 hj0).1)
      rw [← want_job hI hj0] at this
      omega
    have hk2 : k ≤ 128 := by rw [hk]; exact min_le_left _ _
    have hst : (iterStats Dd { m with iter := it }).tokens = k := iterStats_tokens Dd _
    rw [cost_eq _ (by rw [hst]; exact hk1) (by rw [hst]; exact hk2)]
    have hmemit : ∀ e ∈ it, ∃ j ∈ m.jobs, e.1 = j.owner := fun e he => by
      obtain ⟨j, hj, h1, -⟩ := mem_fillIter Dd _ 128 e he
      exact ⟨j, hj, h1⟩
    have hsh : ∀ i < w.init.length, shareOf it i ≤ if isJob (g.c i) then g.left i else 0 := by
      intro i hi
      have h1 := shareOf_fillIter Dd m.jobs 128 i
      by_cases hj : isJob (g.c i) = true
      · rw [if_pos hj]
        obtain ⟨j, hjm, rfl⟩ := hI.jobsP i hi hj
        rw [filter_owner_eq hI.jobsNodup hjm] at h1
        simp only [List.map_cons, List.map_nil, List.sum_cons, List.sum_nil, Nat.add_zero] at h1
        rw [want_job hI hjm] at h1
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
    -- the claim
    have hgood : (m.sess.toList.filter fun s => decide (s.attr.base 10 ≤ m.now)).length * 1280 ≤
        m.served + 165120 := by
      rw [arrived_eq hI]
      have hc : ((Finset.range w.init.length).filter fun i => arr w i ≤ m.now).card = St w g := by
        unfold St; congr 1
        apply Finset.filter_congr
        intro i hi
        have hi' := Finset.mem_range.mp hi
        constructor
        · intro h; by_contra hf; simp only [Bool.not_eq_false] at hf; have := hfut i hi' hf; omega
        · intro h; exact hI.past i hi' h
      rw [hc]
      omega
    refine ⟨⟨{ hI with
        delaysOf := fun d hd => by
          obtain ⟨h1, h2, h3, h4, h5⟩ := hI.delaysOf d hd
          exact ⟨h1, h2, h3, h4, by show d.2.1 < m.nextDelay + 1; omega⟩
        s0idle := fun i hi hc => absurd (hnr i hi) (by rw [hc]; decide)
        iterNone := fun h => by simp at h
        iterOwn := fun e he => by
          obtain ⟨j, hj, he1⟩ := hmemit e he
          rw [he1]; exact (hI.jobs j hj).1
        share := hsh
        busySeq := fun e q h d hd => by
          simp only [Option.some.injEq, Prod.mk.injEq] at h
          have := (hI.delaysOf d hd).2.2.2.2
          show d.2.1 ≠ q
          omega
        cons := by show 1280 * St w g + k = m.served + k + Wn w g; omega }, ?_, fun h => by simp at h, ?_⟩,
      hr, fun h => by simp at h⟩
    · intro e q h
      simp only [Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, -⟩ := h
      refine ⟨Nat.le_add_right _ _, ?_⟩
      have hstep := fr_step hF hfut (g := g)
      obtain ⟨hW1, hW2⟩ := wn_le hI hr
      show 46750 * (Wn w g + 1280 * Fr w g (m.now + 4675)) + 1280 * ((m.now + 4675) % 46750) ≤
        46750 * (165120 + k)
      by_cases hfull : k = 128
      · rw [hfull]; omega
      · have : k = ∑ i ∈ Finset.range w.init.length, wantG g i := by rw [hk] at hfull ⊢; omega
        have hmod := Nat.mod_lt m.now (show 0 < 46750 by norm_num)
        omega
    · intro _
      show (iterRec Dd { m with iter := it } _).arrived * 1280 ≤ (iterRec Dd { m with iter := it } _).served + 165120
      simp only [iterRec, hI.wl, hF.slot]
      exact hgood


theorem Pre.transfer {w : Workload} {g g' : Ghost} {m m' : Machine} (h : Pre w g m) (hI : DInv w g' m')
    (hk : Keeps m' m) (hs : Same3 w.init.length g g') : Pre w g' m' := by
  have hW : Wn w g' = Wn w g := Wn_congr w fun j hj => (hs j hj).1
  have hFr : ∀ t, Fr w g' t = Fr w g t := Fr_congr w fun j hj => (hs j hj).2.1
  refine ⟨hI, fun e q he => ?_, fun he => ?_, fun he => ?_⟩
  · rw [hk.iterEnd] at he
    rw [hW, hFr, hk.now, hk.iter]; exact h.potB e q he
  · rw [hk.iterEnd] at he
    rw [hW, hFr, hk.now]; exact h.potI he
  · rw [hk.iterEnd] at he
    rw [hk.last]; exact h.good he

/-- **After an event.** Settling keeps the potential; an idle engine with
nothing due starts an iteration. -/
theorem after_bnd {w : Workload} (hF : Fam w) {g : Ghost} {m : Machine} (h : Pre w g m) :
    ∃ g', Bnd w g' (afterEvent Dd m) := by
  obtain ⟨g', hI1, hr1, hk1, hs1, -⟩ := settle_dinv hF.pos hF.len h.toDInv
  have hP1 := h.transfer hI1 hk1 hs1
  unfold afterEvent
  simp only
  split
  · rename_i hc
    simp only [Bool.and_eq_true, Option.isNone_iff_eq_none, Bool.not_eq_true'] at hc
    exact ⟨g', start_bnd hF hI1 hr1 hc.1 hc.2 (hP1.potI hc.1)⟩
  · rename_i hc
    refine ⟨g', hP1, hr1, fun hie => ?_⟩
    right
    simp only [Bool.and_eq_true, Option.isNone_iff_eq_none, Bool.not_eq_true', not_and] at hc
    have := hc hie
    simpa using this

/-! ### An arrival -/

theorem handle_arr {m : Machine} {u qu i : ℕ} {rest : List (ℕ × ℕ × ℕ)} (hdl : m.delays = (u, qu, i) :: rest)
    (hne : m.iterEnd ≠ some (u, qu)) (hst : (getS m i).status = .delay u qu) :
    handle m u qu = { setS { m with now := u, delays := rest } i { getS m i with status := .ready } with
      ready := m.ready ++ [i] } := by
  unfold handle
  simp only [hne, if_false, hdl]
  split_ifs with h
  · rfl
  · exact absurd hst h

/-- **An arrival.** The request becomes ready; its tokens join the backlog
and leave the arrivals still due, so the potential of a running iteration
does not move; an idle engine with nothing to do had nobody in the system,
and starts from one request's tokens. -/
theorem arr_pre {w : Workload} (hF : Fam w) {g : Ghost} {m : Machine} (hB : Bnd w g m)
    {u qu i : ℕ} {rest : List (ℕ × ℕ × ℕ)} (hdl : m.delays = (u, qu, i) :: rest)
    (hbusy : ∀ a qa, m.iterEnd = some (a, qa) → u ≤ a) :
    Pre w (g.set i .r1 0) (handle m u qu) := by
  have hI := hB.toDInv
  have hnr := hI.not_ready hB.rdy
  have hhead : (u, qu, i) ∈ m.delays := by rw [hdl]; exact List.mem_cons_self
  obtain ⟨hin, hcw, hst, hu, -⟩ := hI.delaysOf _ hhead
  simp only at hin hcw hst hu
  have hne : m.iterEnd ≠ some (u, qu) := fun h => hI.busySeq u qu h _ hhead rfl
  have hisz : i < m.sess.size := hI.size ▸ hin
  have hnow : m.now ≤ u := hI.future _ hhead
  have hsorted := hI.sorted
  rw [hdl] at hsorted
  have hrest : ∀ d ∈ rest, u ≤ d.1 := (List.pairwise_cons.mp hsorted).1
  have hnd := hI.delaysNodup
  rw [hdl] at hnd
  simp only [List.map_cons, List.nodup_cons] at hnd
  have hrest_i : ∀ d ∈ rest, d.2.2 ≠ i := fun d hd he => hnd.1 (List.mem_map.mpr ⟨d, hd, he⟩)
  set g' := g.set i .r1 0 with hg'
  have hgc : ∀ j, g'.c j = if j = i then .r1 else g.c j := Ghost.set_c g i .r1 0
  have hgl : ∀ j, g'.left j = if j = i then 0 else g.left j := Ghost.set_left g i .r1 0
  have hown := hI.owner_ne (i := i) (by rw [hcw]; rfl)
  have hfi : fresh (g.c i) = true := by rw [hcw]; rfl
  -- the sums
  have hSt : St w g' = St w g + 1 := by
    unfold St
    exact card_update hin _ _ (by simp [hfi]) (by simp [hgc, fresh]) fun j hj => by simp [hgc, hj]
  have hWn : Wn w g' = Wn w g + 1280 := by
    have := sum_update hin (rem g) (rem g') fun j hj => rem_set g i .r1 0 j hj
    have h1 : rem g i = 0 := by simp [rem, hcw]
    have h2 : rem g' i = 1280 := by simp [rem, hgc]
    rw [h1, h2] at this
    unfold Wn
    omega
  have hFr : ∀ t, u ≤ t → Fr w g t = Fr w g' t + 1 := by
    intro t ht
    unfold Fr
    exact card_update hin _ _ (by simp [hgc, fresh]) (by simp [hfi, hu ▸ ht]) fun j hj => by simp [hgc, hj]
  rw [handle_arr hdl hne hst, hB.rdy]
  suffices H : ∀ M : Machine, M.wl = m.wl → M.sess.size = m.sess.size →
      (∀ j, getS M j = if j = i then { getS m i with status := .ready } else getS m j) →
      M.jobs = m.jobs → M.ready = [i] → M.delays = rest → M.nextDelay = m.nextDelay → M.now = u →
      M.iter = m.iter → M.iterEnd = m.iterEnd → M.served = m.served → M.last = m.last → Pre w g' M by
    exact H _ rfl (by simp) (fun j => getS_setS _ _ hisz j) rfl (by simp) rfl rfl rfl rfl rfl rfl rfl
  intro M hwl hsz hget hjobs hready hdl' hnd' hnow' hit hie hsv hlast
  refine ⟨{
      wl := hwl.trans hI.wl
      size := hsz.trans hI.size
      sess := fun j hj => ?_
      jobs := fun x hx => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := hjobs ▸ hI.jobsNodup
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := by rw [hready]; simp
      readyMem := fun j => ?_
      delaysOf := fun d hd => ?_
      delaysW := fun j hj hjc => ?_
      delaysNodup := hdl' ▸ hnd.2
      sorted := hdl' ▸ (List.pairwise_cons.mp hsorted).2
      future := fun d hd => by rw [hnow']; exact hrest d (hdl' ▸ hd)
      past := fun j hj hjf => ?_
      s0now := fun j hj hjc => ?_
      s0idle := fun j hj hjc => ?_
      iterNone := fun h => hit ▸ hI.iterNone (hie ▸ h)
      iterOwn := fun e he => hI.iterOwn e (hit ▸ he)
      share := fun j hj => ?_
      busySeq := fun e q h d hd => hI.busySeq e q (hie ▸ h) d (by rw [hdl]; exact List.mem_cons_of_mem _ (hdl' ▸ hd))
      cons := ?_ }, ?_, ?_, ?_⟩
  · rw [hget, hgc]
    split_ifs with h
    · subst h
      have hs := hI.sess j hj
      rw [hcw] at hs
      obtain ⟨⟨hp, -⟩, h2, h3, h4⟩ := hs
      exact ⟨⟨hp, rfl⟩, h2, h3, h4⟩
    · exact hI.sess j hj
  · rw [hjobs] at hx
    obtain ⟨h1, h2, h3, h4⟩ := hI.jobs x hx
    have hne := hown x hx
    simp only [hgc, hgl, hne, if_false]
    exact ⟨h1, h2, h3, h4⟩
  · rw [hjobs]
    by_cases h : j = i
    · subst h; rw [hgc, if_pos rfl] at hjj; exact absurd hjj (by decide)
    · rw [hgc, if_neg h] at hjj; exact hI.jobsP j hj hjj
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftP j hj hjc
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; rw [hgl, if_neg h]; exact hI.leftD j hj hjc
  · rw [hready, hgc]
    by_cases h : j = i
    · subst h; simp [hin, isReady]
    · rw [if_neg h, List.mem_singleton]
      constructor
      · intro h'; exact absurd h' h
      · rintro ⟨hj, hr⟩; rw [hnr j hj] at hr; exact absurd hr (by decide)
  · rw [hdl'] at hd
    obtain ⟨h1, h2, h3, h4, h5⟩ := hI.delaysOf d (by rw [hdl]; exact List.mem_cons_of_mem _ hd)
    have hne := hrest_i d hd
    rw [hgc, if_neg hne, hget, if_neg hne, hnd']
    exact ⟨h1, h2, h3, h4, h5⟩
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc
      obtain ⟨q, hq⟩ := hI.delaysW j hj hjc
      rw [hdl] at hq
      rcases List.mem_cons.mp hq with he | hq
      · simp only [Prod.mk.injEq] at he; exact absurd he.2.2 h
      · exact ⟨q, hdl' ▸ hq⟩
  · rw [hnow']
    rw [hgc] at hjf
    by_cases h : j = i
    · subst h; omega
    · rw [if_neg h] at hjf; exact (hI.past j hj hjf).trans hnow
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; have := hnr j hj; rw [hjc] at this; exact absurd this (by decide)
  · rw [hgc] at hjc
    by_cases h : j = i
    · subst h; rw [if_pos rfl] at hjc; exact absurd hjc (by decide)
    · rw [if_neg h] at hjc; have := hnr j hj; rw [hjc] at this; exact absurd this (by decide)
  · rw [hit, hgc, hgl]
    by_cases h : j = i
    · subst h
      have := hI.share j hj
      rw [hcw] at this
      simp [isJob] at this
      simp [this]
    · simp only [h, if_false]; exact hI.share j hj
  · rw [hit, hsv]
    have := hI.cons
    rw [hSt, hWn]; omega
  -- the potential
  · intro e q he
    rw [hie] at he
    have hle := hbusy e q he
    obtain ⟨-, hp⟩ := hB.potB e q he
    refine ⟨hnow' ▸ hle, ?_⟩
    have := hFr e hle
    rw [hWn, hit]; omega
  · intro hie'
    rw [hie] at hie'
    rw [hnow']
    have hp := hB.potI hie'
    rcases hB.idleW hie' with hW0 | hpend
    · -- nobody in the system: one request's tokens, nobody else due by `u`
      have hz : Fr w g' u = 0 := by
        apply fr_zero
        intro j hj hjf
        rw [hgc] at hjf
        by_cases h : j = i
        · subst h; simp [fresh] at hjf
        · rw [if_neg h] at hjf
          have hcj : g.c j = .w := by
            have := hnr j hj
            cases hc : g.c j <;> simp [hc, fresh, isReady] at hjf this ⊢
          obtain ⟨q, hq⟩ := hI.delaysW j hj hcj
          rw [hdl] at hq
          rcases List.mem_cons.mp hq with he | hq
          · simp only [Prod.mk.injEq] at he; exact absurd he.2.2 h
          · have h1 := hrest _ hq
            simp only at h1
            have h2 := hF.gap j hj
            have h3 := hF.gap i hin
            rcases Nat.lt_or_ge u (arr w j) with h4 | h4
            · exact h4
            · exfalso
              have : arr w j = arr w i := by omega
              rw [h2, h3] at this
              exact h (by omega)
      rw [hz, hWn, hW0]
      have := Nat.mod_lt u (show 0 < 46750 by norm_num)
      omega
    · -- an event due now: this arrival, at the same instant
      have hu' : u ≤ m.now := by
        unfold pendingBy nextEvent at hpend
        rw [hie', hdl] at hpend
        simpa using hpend
      have hun : u = m.now := le_antisymm hu' hnow
      have := hFr m.now (by omega)
      rw [hun, hWn]
      omega
  · intro h; rw [hlast]; rw [hie] at h; exact hB.good h


/-! ### The end of an iteration -/

/-- The ghost after the batch `it`: each job loses its share, and a job
that reaches the end of its run is done with it. -/
def Ghost.tick (g : Ghost) (it : List (ℕ × ℕ)) : Ghost :=
  ⟨fun i => if isJob (g.c i) = true ∧ g.left i ≤ shareOf it i then (if g.c i = .p then .r3 else .r4) else g.c i,
   fun i => g.left i - shareOf it i⟩

/-- **An iteration ends.** Its tokens leave the backlog; the potential
projected to its end becomes the idle engine's, now. -/
theorem end_pre {w : Workload} {g : Ghost} {m : Machine} (hB : Bnd w g m) {a qa : ℕ}
    (hie : m.iterEnd = some (a, qa)) (hfut : ∀ d ∈ m.delays, a ≤ d.1) :
    Pre w (g.tick m.iter) (handle m a qa) := by
  have hI := hB.toDInv
  have hnr := hI.not_ready hB.rdy
  obtain ⟨hna, hpot⟩ := hB.potB a qa hie
  set it := m.iter with hit
  set g' := g.tick it with hg'
  have hgc : ∀ j, g'.c j =
      if isJob (g.c j) = true ∧ g.left j ≤ shareOf it j then (if g.c j = .p then .r3 else .r4) else g.c j :=
    fun _ => rfl
  have hgl : ∀ j, g'.left j = g.left j - shareOf it j := fun _ => rfl
  -- shares
  have hshj : ∀ j < w.init.length, isJob (g.c j) = true → shareOf it j ≤ g.left j := by
    intro j hj hjj; have := hI.share j hj; rw [if_pos hjj] at this; exact this
  have hsh0 : ∀ j < w.init.length, isJob (g.c j) = false → shareOf it j = 0 := by
    intro j hj hjj; exact Nat.le_zero.mp (by simpa [hjj] using hI.share j hj)
  have hcj : ∀ j < w.init.length, ¬ (isJob (g.c j) = true ∧ g.left j ≤ shareOf it j) → g'.c j = g.c j :=
    fun j _ h => by rw [hgc, if_neg h]
  have hrem : ∀ j < w.init.length, rem g' j + shareOf it j = rem g j := by
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
  have hfr : ∀ j < w.init.length, fresh (g'.c j) = fresh (g.c j) := by
    intro j hj
    rw [hgc]
    split_ifs with h1 h2
    · obtain ⟨h1, -⟩ := h1; rw [h2]; rfl
    · obtain ⟨h1, -⟩ := h1; cases hc : g.c j <;> simp [hc, isJob, fresh] at h1 ⊢
    · rfl
  have hWn : Wn w g' + tokSum it = Wn w g := by
    unfold Wn
    rw [← sum_shareOf w.init.length it hI.iterOwn, ← Finset.sum_add_distrib]
    exact Finset.sum_congr rfl fun j hj => hrem j (Finset.mem_range.mp hj)
  have hSt : St w g' = St w g := St_congr w hfr
  have hFr : ∀ t, Fr w g' t = Fr w g t := Fr_congr w hfr
  -- the machine
  unfold handle
  simp only [hie, if_true]
  unfold endIteration
  simp only
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
  have hfin : ∀ x0 ∈ m.jobs, (x0.left - shareOf it x0.owner = 0 ↔ g.left x0.owner ≤ shareOf it x0.owner) := by
    intro x0 hx0; rw [(hI.jobs x0 hx0).2.2.2]; omega
  have hmemd : ∀ j, j ∈ done ↔ j < w.init.length ∧ isJob (g.c j) = true ∧ g.left j ≤ shareOf it j := by
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
  have hdb : ∀ i ∈ done, i < M1.sess.size := fun i hi => by
    show i < m.sess.size
    rw [hI.size]; exact ((hmemd i).mp hi).1
  obtain ⟨f1, f2, -, -, f5, f6, f7, f8, f9, f10, f11⟩ := Exec.readyAll_fields done M1 hdn hdb
  obtain ⟨o1, o2, o3⟩ := readyAll_other done M1
  change Pre w g' (Exec.readyAll done M1)
  set R := Exec.readyAll done M1 with hR
  have hgetR : ∀ j, getS R j = if j ∈ done then { getS m j with status := .ready } else getS m j := f11
  have hfinc : ∀ j < w.init.length, j ∈ done → (g.c j = .p ∧ g'.c j = .r3) ∨ (g.c j = .d ∧ g'.c j = .r4) := by
    intro j hj hd
    obtain ⟨-, hjj, hl⟩ := (hmemd j).mp hd
    rw [hgc, if_pos ⟨hjj, hl⟩]
    cases hc : g.c j <;> simp [hc, isJob] at hjj ⊢
  have hnfin : ∀ j < w.init.length, j ∉ done → g'.c j = g.c j := by
    intro j hj hd
    exact hcj j hj fun h => hd ((hmemd j).mpr ⟨hj, h⟩)
  have hR1 : R.jobs = jobs'.filter (·.left ≠ 0) := f2
  have hRit : R.iter = [] := f8
  have hRie : R.iterEnd = none := f9
  have hRnow : R.now = a := f5
  have hRdl : R.delays = m.delays := f6
  refine ⟨{
      wl := f7.trans hI.wl
      size := f10.trans hI.size
      sess := fun j hj => ?_
      jobs := fun x hx => ?_
      jobsP := fun j hj hjj => ?_
      jobsNodup := ?_
      leftP := fun j hj hjc => ?_
      leftD := fun j hj hjc => ?_
      ready := by rw [f1]; simpa [hM1, hM0, hB.rdy] using hdn
      readyMem := fun j => ?_
      delaysOf := fun d hd => ?_
      delaysW := fun j hj hjc => ?_
      delaysNodup := by rw [hRdl]; exact hI.delaysNodup
      sorted := by rw [hRdl]; exact hI.sorted
      future := fun d hd => by rw [hRnow]; rw [hRdl] at hd; exact hfut d hd
      past := fun j hj hjf => ?_
      s0now := fun j hj hjc => ?_
      s0idle := fun j hj hjc => ?_
      iterNone := fun _ => hRit
      iterOwn := fun e he => by rw [hRit] at he; simp at he
      share := fun j hj => by rw [hRit]; simp [shareOf]
      busySeq := fun e q h => by rw [hRie] at h; simp at h
      cons := ?_ }, fun e q h => by rw [hRie] at h; simp at h, fun _ => ?_, fun h => by rw [hRie] at h; simp at h⟩
  · rw [hgetR]
    by_cases hd : j ∈ done
    · rw [if_pos hd]
      rcases hfinc j hj hd with ⟨hc, hc'⟩ | ⟨hc, hc'⟩
      · have hs := hI.sess j hj; rw [hc] at hs
        rw [hc']; exact ⟨⟨hs.1.1, rfl⟩, hs.2.1, hs.2.2.1, hs.2.2.2⟩
      · have hs := hI.sess j hj; rw [hc] at hs
        rw [hc']; exact ⟨⟨hs.1.1, rfl⟩, hs.2.1, hs.2.2.1, hs.2.2.2⟩
    · rw [if_neg hd, hnfin j hj hd]; exact hI.sess j hj
  · rw [hR1] at hx
    obtain ⟨hx, hl⟩ := List.mem_filter.mp hx
    obtain ⟨x0, hx0, rfl⟩ := (hj' x).mp hx
    obtain ⟨h1, h2, h3, h4⟩ := hI.jobs x0 hx0
    have hnd : x0.owner ∉ done := fun hd => by
      have := ((hmemd _).mp hd).2.2
      have := (hfin x0 hx0).mpr this
      simp at hl; omega
    refine ⟨h1, by rw [hnfin _ h1 hnd]; exact h2, h3, ?_⟩
    show x0.left - shareOf it x0.owner = g'.left x0.owner
    rw [hgl, h4]
  · by_cases hd : j ∈ done
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
  · by_cases hd : j ∈ done
    · rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] at hjc <;> simp at hjc
    · rw [hnfin j hj hd] at hjc
      have hjj : isJob (g.c j) = true := by rw [hjc]; rfl
      have h1 := hI.leftP j hj hjc
      have : ¬ g.left j ≤ shareOf it j := fun h => hd ((hmemd j).mpr ⟨hj, hjj, h⟩)
      rw [hgl]; omega
  · by_cases hd : j ∈ done
    · rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] at hjc <;> simp at hjc
    · rw [hnfin j hj hd] at hjc
      have hjj : isJob (g.c j) = true := by rw [hjc]; rfl
      have h1 := hI.leftD j hj hjc
      have : ¬ g.left j ≤ shareOf it j := fun h => hd ((hmemd j).mpr ⟨hj, hjj, h⟩)
      rw [hgl]; omega
  · rw [f1]
    show j ∈ m.ready ++ done ↔ _
    rw [hB.rdy, List.nil_append]
    constructor
    · intro hd
      have hj := ((hmemd j).mp hd).1
      refine ⟨hj, ?_⟩
      rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] <;> rfl
    · rintro ⟨hj, hr⟩
      by_contra hd
      rw [hnfin j hj hd, hnr j hj] at hr
      exact absurd hr (by decide)
  · rw [hRdl] at hd
    obtain ⟨h1, h2, h3, h4, h5⟩ := hI.delaysOf d hd
    have hnd : d.2.2 ∉ done := fun h => by
      have := ((hmemd _).mp h).2.1; rw [h2] at this; exact absurd this (by decide)
    refine ⟨h1, by rw [hnfin _ h1 hnd]; exact h2, by rw [hgetR, if_neg hnd]; exact h3, h4, ?_⟩
    rw [o1]; exact h5
  · rw [hRdl]
    by_cases hd : j ∈ done
    · rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] at hjc <;> simp at hjc
    · rw [hnfin j hj hd] at hjc; exact hI.delaysW j hj hjc
  · rw [hRnow]
    rw [hfr j hj] at hjf
    exact (hI.past j hj hjf).trans hna
  · by_cases hd : j ∈ done
    · rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] at hjc <;> simp at hjc
    · rw [hnfin j hj hd] at hjc; have := hnr j hj; rw [hjc] at this; exact absurd this (by decide)
  · by_cases hd : j ∈ done
    · rcases hfinc j hj hd with ⟨-, hc'⟩ | ⟨-, hc'⟩ <;> rw [hc'] at hjc <;> simp at hjc
    · rw [hnfin j hj hd] at hjc; have := hnr j hj; rw [hjc] at this; exact absurd this (by decide)
  · rw [hRit, o2, hSt]
    have := hI.cons
    rw [← hit] at this
    show 1280 * St w g + tokSum [] = m.served + Wn w g'
    simp only [tokSum, List.map_nil, List.sum_nil, Nat.add_zero]
    omega
  · rw [hRnow, hFr]
    show 46750 * (Wn w g' + 1280 * Fr w g a) + 1280 * (a % 46750) ≤ 46750 * 165120
    omega


/-! ### The run -/

/-- **One event.** -/
theorem step_bnd {w : Workload} (hF : Fam w) {g : Ghost} {m : Machine} (hB : Bnd w g m) :
    ∃ g', Bnd w g' (step Dd m) := by
  have hs := hB.sorted
  unfold step
  rcases hie : m.iterEnd with _ | ⟨a, qa⟩ <;> rcases hdl : m.delays with _ | ⟨⟨u, qu, i⟩, rest⟩
  · have : nextEvent m = none := by unfold nextEvent; rw [hie, hdl]; rfl
    rw [this]; exact ⟨g, hB⟩
  · have : nextEvent m = some (u, qu) := by unfold nextEvent; rw [hie, hdl]; rfl
    rw [this]
    exact after_bnd hF (arr_pre hF hB hdl fun a qa h => by rw [hie] at h; simp at h)
  · have : nextEvent m = some (a, qa) := by unfold nextEvent; rw [hie, hdl]; rfl
    rw [this]
    exact after_bnd hF (end_pre hB hie fun d hd => by rw [hdl] at hd; simp at hd)
  · rw [hdl] at hs
    have hrest : ∀ d ∈ rest, u ≤ d.1 := (List.pairwise_cons.mp hs).1
    by_cases hc : u < a ∨ (u = a ∧ qu < qa)
    · have : nextEvent m = some (u, qu) := by unfold nextEvent; rw [hie, hdl]; simp [hc]
      rw [this]
      refine after_bnd hF (arr_pre hF hB hdl fun a' qa' h => ?_)
      rw [hie] at h; simp only [Option.some.injEq, Prod.mk.injEq] at h
      obtain ⟨rfl, -⟩ := h; omega
    · have : nextEvent m = some (a, qa) := by unfold nextEvent; rw [hie, hdl]; simp [hc]
      rw [this]
      refine after_bnd hF (end_pre hB hie fun d hd => ?_)
      rw [hdl] at hd
      rcases List.mem_cons.mp hd with rfl | hd
      · simp only; omega
      · have := hrest d hd; omega

/-- At the start every request is about to run its first command. -/
def g0 : Ghost := ⟨fun _ => .s0, fun _ => 0⟩

theorem pre_initial {w : Workload} (hF : Fam w) : Pre w g0 (Exec.initial Dd w.init.length w.attr Pd w) := by
  have hget : ∀ i < w.init.length, getS (Exec.initial Dd w.init.length w.attr Pd w) i =
      ⟨i, ⟨w.attr i, []⟩, 0, Pd, [], .ready, 0, 0⟩ := by
    intro i hi
    simp [getS, Exec.initial, Array.getD_eq_getD_getElem?, hi]
  have hW : Wn w g0 = 0 := Finset.sum_eq_zero fun i _ => rfl
  have hSt : St w g0 = 0 := by
    unfold St; rw [Finset.card_eq_zero, Finset.filter_eq_empty_iff]; intro i _; simp [g0, fresh]
  have hFr : Fr w g0 0 = 0 := fr_zero fun i hi _ => hF.pos i hi
  refine ⟨{
      wl := rfl
      size := by simp [Exec.initial]
      sess := fun i hi => ?_
      jobs := fun j hj => by simp [Exec.initial] at hj
      jobsP := fun i _ h => by simp [g0, isJob] at h
      jobsNodup := by simp [Exec.initial]
      leftP := fun i _ h => by simp [g0] at h
      leftD := fun i _ h => by simp [g0] at h
      ready := by simp [Exec.initial, List.nodup_range]
      readyMem := fun i => by simp [Exec.initial, g0, isReady]
      delaysOf := fun d hd => by simp [Exec.initial] at hd
      delaysW := fun i _ h => by simp [g0] at h
      delaysNodup := by simp [Exec.initial]
      sorted := by simp [Exec.initial]
      future := fun d hd => by simp [Exec.initial] at hd
      past := fun i _ h => by simp [g0, fresh] at h
      s0now := fun _ _ _ => rfl
      s0idle := fun _ _ _ => rfl
      iterNone := fun _ => rfl
      iterOwn := fun e he => by simp [Exec.initial] at he
      share := fun i _ => by simp [Exec.initial, shareOf]
      busySeq := fun e q h => by simp [Exec.initial] at h
      cons := by simp [Exec.initial, hW, hSt, tokSum] }, fun e q h => by simp [Exec.initial] at h,
    fun _ => ?_, fun h => by simp [Exec.initial] at h⟩
  · rw [hget i hi]
    exact ⟨⟨rfl, rfl⟩, rfl, rfl, by simp [Attrs.get, arr]⟩
  · show 46750 * (Wn w g0 + 1280 * Fr w g0 0) + 1280 * (0 % 46750) ≤ 46750 * 165120
    rw [hW, hFr]; norm_num

/-- **Every machine of every path is at a boundary.** -/
theorem reach_bnd {w : Workload} (hF : Fam w) {m : Machine} (hr : Reach Dd w Pd m) : ∃ g, Bnd w g m := by
  induction hr with
  | start => exact after_bnd hF (pre_initial hF)
  | step _ ih =>
    obtain ⟨g, hB⟩ := ih
    exact step_bnd hF hB

/-- **Theorem 2(b), pathwise.** Before every iteration of every path, the
tokens arrived and not yet served are at most `(b_max + 1) W`. -/
theorem bounded : Claims.DaiSarathi.bounded := by
  intro w hw m hr hs
  obtain ⟨g, hB⟩ := reach_bnd (fam_of_family hw) hr
  exact hB.good hs

end DaiSarathi
end Papers
end SerqLang
