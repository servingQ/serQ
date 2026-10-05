/-
# What a session runs is a part of its program

On every path of a program `P`, every session's current statement, every
continuation on its stack and every hold it may re-execute is a
sub-program of `P` (`Sub P`), and every job at the engine was started by a
`run` statement of `P` (`Inv`, `reach_inv`). A claim about a program reads
its text through this: a program without `growing` never grows a hold, a
program that sets a slot nowhere never changes it.

Key definitions: `Exec.Sub`, `Exec.Inv`.
Key theorems: `Exec.reach_inv` (every reachable machine satisfies `Inv`),
`Exec.jobs_not_growing`.
-/
import Serq.Claim

namespace SerqLang
namespace Exec

variable (D : Deployment)

/-- The sub-programs of `P`: `P` and what its statements continue with or
contain. -/
inductive Sub (P : Prog) : Prog → Prop
  | refl : Sub P P
  | turn {k} : Sub P (.turn k) → Sub P k
  | set {s e k} : Sub P (.set s e k) → Sub P k
  | observe {n e k} : Sub P (.observe n e k) → Sub P k
  | run {s md w g k} : Sub P (.run s md w g k) → Sub P k
  | holdBody {ps r b c k} : Sub P (.hold ps r b c k) → Sub P b
  | holdK {ps r b c k} : Sub P (.hold ps r b c k) → Sub P k
  | yes {p a b k} : Sub P (.branch p a b k) → Sub P a
  | no {p a b k} : Sub P (.branch p a b k) → Sub P b
  | branchK {p a b k} : Sub P (.branch p a b k) → Sub P k
  | loopBody {b} : Sub P (.loop b) → Sub P b

/-- A frame of the stack holds sub-programs. -/
def FrameOK (P : Prog) : Frame → Prop
  | .seq k => Sub P k
  | .hold h k => Sub P h.stmt ∧ Sub P k
  | .loop b => Sub P b

def SessOK (P : Prog) (s : Sess) : Prop := Sub P s.prog ∧ ∀ f ∈ s.stack, FrameOK P f

/-- A job was started by a `run` of the engine in `P`. -/
def JobOK (P : Prog) (j : Job) : Prop := ∃ w k, Sub P (.run 0 j.mode w j.growing k)

/-- The invariant reads the sessions and the jobs only, so a machine that
differs in other fields satisfies it by definition. -/
def InvSJ (P : Prog) (ss : Array Sess) (js : List Job) : Prop :=
  (∀ s ∈ ss.toList, SessOK P s) ∧ ∀ j ∈ js, JobOK P j

abbrev Inv (P : Prog) (m : Machine) : Prop := InvSJ P m.sess m.jobs

theorem Inv.sess {P : Prog} {m : Machine} (h : Inv P m) : ∀ s ∈ m.sess.toList, SessOK P s := h.1

theorem Inv.jobs {P : Prog} {m : Machine} (h : Inv P m) : ∀ j ∈ m.jobs, JobOK P j := h.2

/-! ### Sessions and jobs of a machine -/

theorem getS_mem (m : Machine) {i : ℕ} (h : i < m.sess.size) : getS m i ∈ m.sess.toList := by
  unfold getS
  rw [Array.getD_eq_getD_getElem?, Array.getElem?_eq_getElem h]
  simp

theorem Inv.sessOK {P : Prog} {m : Machine} (h : Inv P m) {i : ℕ} (hi : i < m.sess.size) :
    SessOK P (getS m i) := h.sess _ (getS_mem m hi)

theorem Inv.of_eq {P : Prog} {m m' : Machine} (h : Inv P m) (h1 : m'.sess = m.sess)
    (h2 : m'.jobs = m.jobs) : Inv P m' :=
  ⟨fun s hs => h.1 s (h1 ▸ hs), fun j hj => h.2 j (h2 ▸ hj)⟩

theorem Inv.upd {P : Prog} {m : Machine} (h : Inv P m) (i : ℕ) (s : Sess)
    (hs : i < m.sess.size → SessOK P s) : Inv P (setS m i s) := by
  refine ⟨fun x hx => ?_, h.2⟩
  simp only [Exec.setS] at hx
  rw [Array.toList_setIfInBounds] at hx
  by_cases hi : i < m.sess.size
  · rcases List.mem_or_eq_of_mem_set hx with hx | rfl
    · exact h.sess x hx
    · exact hs hi
  · rw [List.set_eq_of_length_le (by simpa using hi)] at hx
    exact h.sess x hx

@[simp] theorem setS_size (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).sess.size = m.sess.size := by
  simp [Exec.setS]

/-- A session obtained from session `i` by changing its program and stack. -/
theorem Inv.upd' {P : Prog} {m : Machine} (h : Inv P m) (i : ℕ) (s : Sess)
    (hs : SessOK P (getS m i) → SessOK P s) : Inv P (setS m i s) :=
  h.upd i s fun hi => hs (h.sessOK hi)

theorem inv_nextAdm_ready {P : Prog} {m : Machine} {a : ℕ} {r : List ℕ} (h : Inv P m) :
    Inv P { m with nextAdm := a, ready := r } := h

theorem Inv.setS_attr {P : Prog} {m : Machine} (h : Inv P m) (i : ℕ) (a : Attrs) :
    Inv P (setS m i { getS m i with attr := a }) :=
  h.upd' i _ fun ho => ⟨ho.1, ho.2⟩

theorem inv_setPool {P : Prog} {m : Machine} (h : Inv P m) (p : ℕ) (s : PoolSt) :
    Inv P (setPool m p s) := h.of_eq rfl rfl

/-! ### The commands of an instant -/

/-- A fold that keeps a projection keeps it. -/
theorem proj_foldl {α γ : Type} (κ : Machine → γ) (f : Machine → α → Machine)
    (hf : ∀ m a, κ (f m a) = κ m) : ∀ (l : List α) (m : Machine), κ (l.foldl f m) = κ m
  | [], _ => rfl
  | a :: l, m => (proj_foldl κ f hf l (f m a)).trans (hf m a)

/-- The sessions and the jobs. -/
def skey (m : Machine) : Array Sess × List Job := (m.sess, m.jobs)

theorem Inv.of_skey {P : Prog} {m m' : Machine} (h : Inv P m) (hk : skey m' = skey m) : Inv P m' := by
  simp only [skey, Prod.mk.injEq] at hk
  exact h.of_eq hk.1 hk.2

theorem skey_release (m : Machine) (i : ℕ) (hr : HoldRec) : skey (release D m i hr) = skey m := by
  unfold release
  apply proj_foldl
  intro m ⟨p, alloc, pos⟩
  simp only
  repeat' split
  all_goals rfl

theorem skey_admitPool (i serial : ℕ) (cache : Option (Env → ℕ)) (r? : Option ℕ)
    (acc : Machine × List (ℕ × ℕ × ℕ) × ℕ) (x : ℕ × ℕ × ℕ) :
    skey (admitPool D i serial cache r? acc x).1 = skey acc.1 := by
  unfold admitPool
  simp only
  show skey (Exec.setPool _ _ _) = _
  split_ifs <;> rfl

theorem proj_foldl2 {α β γ : Type} (κ : Machine → γ) (f : Machine × β → α → Machine × β)
    (hf : ∀ acc a, κ (f acc a).1 = κ acc.1) :
    ∀ (l : List α) (acc : Machine × β), κ (l.foldl f acc).1 = κ acc.1
  | [], _ => rfl
  | a :: l, acc => (proj_foldl2 κ f hf l (f acc a)).trans (hf acc a)

theorem inv_admit {P : Prog} (m : Machine) (i left : ℕ) (h : Inv P m) : Inv P (admit D m i left) := by
  unfold admit
  split
  · rename_i stmt ps reuse body cache k hp
    simp only
    generalize hr : List.foldl _ (m, [], 0) _ = r
    have hk : skey r.1 = skey m := hr ▸ proj_foldl2 skey _ (skey_admitPool D i _ _ _) _ (m, [], 0)
    have h1 : Inv P r.1 := h.of_skey hk
    have hs : (getS r.1 i) = getS m i := by
      simp only [skey, Prod.mk.injEq] at hk; simp [Exec.getS, hk.1]
    apply inv_nextAdm_ready
    refine h1.upd' i _ fun ho => ?_
    rw [hs] at ho ⊢
    obtain ⟨hsub, hst⟩ := ho
    rw [hp] at hsub
    refine ⟨Sub.holdBody hsub, fun f hf => ?_⟩
    rcases List.mem_cons.mp hf with rfl | hf
    · exact ⟨hp ▸ hsub, Sub.holdK hsub⟩
    · exact hst f hf
  · exact h

theorem inv_enqueue {P : Prog} (m : Machine) (i : ℕ) (front : Bool) (h : Inv P m) :
    Inv P (enqueue m i front) := by
  unfold enqueue
  split
  · exact (inv_setPool h _ _).upd' i _ (fun ho => ho)
  · exact h

theorem inv_admitHeads {P : Prog} (p : ℕ) :
    ∀ (f : ℕ) (m : Machine), Inv P m → Inv P (admitHeads D p f m)
  | 0, _, h => h
  | f + 1, m, h => by
    unfold admitHeads
    split
    · split
      · exact inv_admitHeads p f _ (inv_admit D _ _ _ (inv_setPool h _ _))
      · exact h
    · exact h

theorem inv_admitAll {P : Prog} (m : Machine) (h : Inv P m) : Inv P (admitAll D m) := by
  unfold admitAll
  suffices ∀ (l : List ℕ) (m : Machine), Inv P m →
      Inv P (l.foldl (fun m p => if (pdef D p).viaEngine then m else admitHeads D p 1000 m) m) from
    this _ m h
  intro l
  induction l with
  | nil => exact fun _ h => h
  | cons p l ih =>
    intro m h
    apply ih
    dsimp only
    split
    · exact h
    · exact inv_admitHeads D p 1000 m h

theorem inv_endSession {P : Prog} (m : Machine) (i : ℕ) (h : Inv P m) : Inv P (endSession D m i) := by
  unfold endSession
  simp only
  have hk := proj_foldl skey (fun m f => match f with
    | .hold hr _ => release D m i hr
    | _ => m) (fun m f => by split <;> first | exact skey_release D m i _ | rfl) (getS m i).stack m
  have h1 := h.of_skey hk
  refine h1.upd' i _ fun ho => ⟨ho.1, fun f hf => by simp at hf⟩

theorem lt_of_ready (m : Machine) {i : ℕ} (h : (getS m i).status = .ready) : i < m.sess.size := by
  by_contra hi
  unfold getS at h
  simp [Array.getD_eq_getD_getElem?, hi] at h

theorem inv_exec {P : Prog} : ∀ (f : ℕ) (m : Machine) (i : ℕ), Inv P m → Inv P (exec D f m i)
  | 0, _, _, h => h
  | f + 1, m, i, h => by
    unfold exec
    simp only
    split
    · exact h
    · rename_i hrd
      have ho0 : SessOK P (getS m i) := h.sessOK (lt_of_ready m (by simpa using hrd))
      split
      · -- done
        split
        · exact h.upd' i _ fun ho => ⟨ho.1, ho.2⟩
        · rename_i k st hst
          refine inv_exec f _ i (h.upd' i _ fun ho => ⟨?_, fun g hg => ho.2 g (by simp [hst, hg])⟩)
          have := ho.2 (.seq k) (by simp [hst])
          exact this
        · rename_i hr k st hst
          have h1 : Inv P (setS m i { getS m i with stack := st }) :=
            h.upd' i _ fun ho => ⟨ho.1, fun g hg => ho.2 g (by simp [hst, hg])⟩
          have hk : Sub P k := (ho0.2 (.hold hr k) (by simp [hst])).2
          refine inv_exec f _ i ?_
          refine Inv.upd' ?_ i _ fun ho => ⟨hk, ho.2⟩
          refine inv_admitAll D _ (Inv.upd' ?_ i _ fun ho => ⟨ho.1, ho.2⟩)
          exact (h1.of_skey (skey_release D _ i hr))
        · rename_i body st hst
          refine inv_exec f _ i (h.upd' i _ fun ho => ⟨?_, fun g hg => ?_⟩)
          · exact ho.2 (.loop body) (by simp [hst])
          · rcases List.mem_cons.mp hg with rfl | hg
            · exact ho.2 (.loop body) (by simp [hst])
            · exact ho.2 g (by simp [hst, hg])
      · exact inv_endSession D m i h
      · rename_i k hp
        have hk : Sub P k := Sub.turn (hp ▸ ho0.1)
        repeat' split
        all_goals exact inv_exec f _ i (h.upd' i _ fun ho => ⟨hk, ho.2⟩)
      · rename_i slot e k hp
        exact inv_exec f _ i (h.upd' i _ fun ho => ⟨Sub.set (hp ▸ ho.1), ho.2⟩)
      · rename_i n e k hp
        exact inv_exec f _ i (h.upd' i _ fun ho => ⟨Sub.observe (hp ▸ ho.1), ho.2⟩)
      · rename_i p a b k hp
        refine inv_exec f _ i (h.upd' i _ fun ho => ⟨?_, fun g hg => ?_⟩)
        · split
          · exact Sub.yes (hp ▸ ho.1)
          · exact Sub.no (hp ▸ ho.1)
        · rcases List.mem_cons.mp hg with rfl | hg
          · exact Sub.branchK (hp ▸ ho.1)
          · exact ho.2 g hg
      · rename_i body hp
        refine inv_exec f _ i (h.upd' i _ fun ho => ⟨Sub.loopBody (hp ▸ ho.1), fun g hg => ?_⟩)
        rcases List.mem_cons.mp hg with rfl | hg
        · exact Sub.loopBody (hp ▸ ho.1)
        · exact ho.2 g hg
      · rename_i st md w g k hp
        split
        · exact h.upd' i _ fun ho => ⟨Sub.run (hp ▸ ho.1), ho.2⟩
        · split
          · exact h.upd' i _ fun ho => ⟨Sub.run (hp ▸ ho.1), ho.2⟩
          · rename_i hst0
            have hst : st = 0 := by simpa using hst0
            subst hst
            refine ⟨fun x hx => ?_, fun j hj => ?_⟩
            · exact (h.upd' i { getS m i with prog := k, status := .engine }
                fun ho => ⟨Sub.run (hp ▸ ho0.1), ho.2⟩).1 x hx
            · have hsp := List.span_eq_takeWhile_dropWhile (p := fun j => decide ((getS m j.owner).admSeq ≤ (getS m i).admSeq)) (l := m.jobs)
              simp only [hsp, List.mem_append, List.mem_cons] at hj
              rcases hj with hj | rfl | hj
              · exact h.2 j ((List.takeWhile_sublist _).subset hj)
              · exact ⟨w, k, hp ▸ ho0.1⟩
              · exact h.2 j ((List.dropWhile_suffix _).subset hj)
      · exact inv_enqueue m i false h

theorem inv_drain {P : Prog} : ∀ (f : ℕ) (m : Machine), Inv P m → Inv P (drain D f m)
  | 0, _, h => h
  | f + 1, m, h => by
    unfold drain
    split
    · exact h
    · rename_i i q _
      exact inv_drain f _ (inv_exec D 10000 { m with ready := q } i h)

theorem inv_settleLoop {P : Prog} : ∀ (f : ℕ) (m : Machine), Inv P m → Inv P (settleLoop D f m)
  | 0, _, h => h
  | f + 1, m, h => by
    unfold settleLoop
    simp only
    have h1 := inv_admitAll D _ (inv_drain D (drainFuel m) m h)
    split
    · exact h1
    · exact inv_settleLoop f _ h1

theorem inv_settle {P : Prog} (m : Machine) (h : Inv P m) : Inv P (settle D m) :=
  inv_settleLoop D 1000 m h

/-! ### The engine -/

theorem mapHold_frames (p : ℕ) (g : ℕ × ℕ → ℕ × ℕ) {P : Prog} :
    ∀ (fs : List Frame) (b : Bool), (∀ f ∈ fs, FrameOK P f) → ∀ f ∈ mapHold.go p g fs b, FrameOK P f
  | [], _, _ => by simp [mapHold.go]
  | .hold hr k :: fs, false, h => by
    unfold mapHold.go
    split
    · intro f hf
      rcases List.mem_cons.mp hf with rfl | hf
      · exact h (.hold hr k) List.mem_cons_self
      · exact mapHold_frames p g fs true (fun f hf => h f (List.mem_cons_of_mem _ hf)) f hf
    · intro f hf
      rcases List.mem_cons.mp hf with rfl | hf
      · exact h _ List.mem_cons_self
      · exact mapHold_frames p g fs false (fun f hf => h f (List.mem_cons_of_mem _ hf)) f hf
  | .hold hr k :: fs, true, h => by
    unfold mapHold.go
    intro f hf
    rcases List.mem_cons.mp hf with rfl | hf
    · exact h _ List.mem_cons_self
    · exact mapHold_frames p g fs true (fun f hf => h f (List.mem_cons_of_mem _ hf)) f hf
  | .seq k :: fs, b, h => by
    unfold mapHold.go
    intro f hf
    rcases List.mem_cons.mp hf with rfl | hf
    · exact h _ List.mem_cons_self
    · exact mapHold_frames p g fs b (fun f hf => h f (List.mem_cons_of_mem _ hf)) f hf
  | .loop k :: fs, b, h => by
    unfold mapHold.go
    intro f hf
    rcases List.mem_cons.mp hf with rfl | hf
    · exact h _ List.mem_cons_self
    · exact mapHold_frames p g fs b (fun f hf => h f (List.mem_cons_of_mem _ hf)) f hf

theorem sessOK_mapHold {P : Prog} (s : Sess) (p : ℕ) (g : ℕ × ℕ → ℕ × ℕ) (h : SessOK P s) :
    SessOK P (mapHold s p g) :=
  ⟨h.1, mapHold_frames p g s.stack false h.2⟩

theorem inv_unwind {P : Prog} (p v : ℕ) :
    ∀ (fs : List Frame) (m : Machine), (∀ f ∈ fs, FrameOK P f) → Inv P m →
      Inv P (preemptVictim.unwind D p v m fs)
  | [], _, _, h => h
  | .hold hr k :: fs, m, hf, h => by
    unfold preemptVictim.unwind
    simp only
    have h1 := h.of_skey (skey_release D m v { hr with grown := true })
    split
    · exact inv_enqueue _ v true (Inv.upd' h1 v _ fun ho =>
        ⟨(hf _ List.mem_cons_self).1, fun f hf' => hf f (List.mem_cons_of_mem _ hf')⟩)
    · exact inv_unwind p v fs _ (fun f hf' => hf f (List.mem_cons_of_mem _ hf')) h1
  | .seq _ :: fs, m, hf, h => by
    unfold preemptVictim.unwind
    exact inv_unwind p v fs m (fun f hf' => hf f (List.mem_cons_of_mem _ hf')) h
  | .loop _ :: fs, m, hf, h => by
    unfold preemptVictim.unwind
    exact inv_unwind p v fs m (fun f hf' => hf f (List.mem_cons_of_mem _ hf')) h

theorem inv_preemptVictim {P : Prog} (m : Machine) (p : ℕ) (h : Inv P m) :
    Inv P (preemptVictim D m p).1 := by
  unfold preemptVictim
  split
  · exact h
  · rename_i v _
    simp only
    by_cases hv : v < m.sess.size
    · have ho := h.sessOK hv
      apply inv_unwind D p v _ _ ho.2
      exact ⟨(h.setS_attr v _).1, fun j hj => h.2 j (List.mem_of_mem_filter hj)⟩
    · have hd : getS m v = ⟨v, ⟨fun _ => 0, []⟩, 0, .stop, [], .ended, 0, 0⟩ := by
        unfold getS; simp [Array.getD_eq_getD_getElem?, hv]
      apply inv_unwind D p v _ _ (by rw [hd]; simp)
      exact ⟨(h.setS_attr v _).1, fun j hj => h.2 j (List.mem_of_mem_filter hj)⟩

theorem inv_grow {P : Prog} : ∀ (f : ℕ) (m : Machine) (i p d : ℕ), Inv P m → Inv P (grow D f m i p d).1
  | 0, _, _, _, _, h => h
  | f + 1, m, i, p, d, h => by
    unfold grow
    split
    · exact h
    · simp only
      split
      · exact (inv_setPool h _ _).upd' i _ fun ho => sessOK_mapHold _ _ _ ho
      · have h1 := inv_preemptVictim D m p h
        split
        · rename_i m' heq; rw [heq] at h1; exact h1
        · rename_i m' v heq
          rw [heq] at h1
          split
          · exact h1
          · exact inv_grow f m' i p d h1

theorem inv_admitVia {P : Prog} (m : Machine) (left : ℕ) (h : Inv P m) : Inv P (admitVia D m left).1 := by
  unfold admitVia
  split
  · exact h
  · split
    · split
      · exact inv_drain D _ _ (inv_admit D _ _ _ (inv_setPool h _ _))
      · exact h
    · exact h

theorem inv_assign {P : Prog} (pre0 : ℕ) :
    ∀ (f : ℕ) (m : Machine) (idx left : ℕ), Inv P m → Inv P (assign D f m idx left pre0)
  | 0, _, _, _, h => h
  | f + 1, m, idx, left, h => by
    unfold assign
    split
    · split
      · split
        · rename_i m' heq
          have h1 := inv_admitVia D m left h
          rw [heq] at h1
          exact inv_assign pre0 f m' idx left h1
        · rename_i m' heq
          have h1 := inv_admitVia D m left h
          rw [heq] at h1
          exact h1
      · exact h
    · rename_i j _
      split
      · exact inv_assign pre0 f m (idx + 1) left h
      · simp only
        split
        · exact inv_assign pre0 f m (idx + 1) left h
        · split
          · try simp only
            split
            · exact h
            · exact inv_assign pre0 f _ (idx + 1) _ h
          · split
            · exact inv_assign pre0 f m (idx + 1) left h
            · rename_i p _ _ a x _
              rcases hr : (if x + min (wantOf D j) left > a then
                  grow D (D.pools.length * 100000) m j.owner p (x + min (wantOf D j) left - a)
                  else (m, true)) with ⟨m', ok⟩
              have h1 : Inv P m' := by
                have : Inv P (if x + min (wantOf D j) left > a then
                  grow D (D.pools.length * 100000) m j.owner p (x + min (wantOf D j) left - a)
                  else (m, true)).1 := by
                  split
                  · exact inv_grow D _ _ _ _ _ h
                  · exact h
                rw [hr] at this
                exact this
              try simp only [hr]
              split
              · have h2 : Inv P (setS m' j.owner (mapHold (getS m' j.owner) p fun (a, x) => (a, x + min (wantOf D j) left))) :=
                  h1.upd' _ _ fun ho => sessOK_mapHold _ _ _ ho
                split
                · exact h2
                · exact inv_assign pre0 f _ (idx + 1) _ h2
              · exact inv_assign pre0 f m' idx left h1

theorem inv_startIteration {P : Prog} (m : Machine) (h : Inv P m) : Inv P (startIteration D m) := by
  unfold startIteration
  simp only
  split
  · have h1 := inv_assign (iterDeployment D m) m.preempts (m.jobs.length + 100000)
      { m with iter := [] } 0 D.budget h
    split
    · exact h1
    · exact h1
  · exact h

theorem inv_endIteration {P : Prog} (m : Machine) (h : Inv P m) : Inv P (endIteration m) := by
  unfold endIteration
  simp only
  suffices ∀ (l : List ℕ) (m : Machine), Inv P m →
      Inv P (l.foldl (fun m i => { setS m i { getS m i with status := .ready } with ready := m.ready ++ [i] }) m) by
    apply this
    refine ⟨h.1, fun j hj => ?_⟩
    obtain ⟨j0, hj0, rfl⟩ := List.mem_map.mp (List.mem_of_mem_filter hj)
    exact h.2 j0 hj0
  intro l
  induction l with
  | nil => exact fun _ h => h
  | cons i l ih =>
    intro m h
    exact ih _ (h.upd' i _ fun ho => ⟨ho.1, ho.2⟩)

theorem inv_afterEvent {P : Prog} (m : Machine) (h : Inv P m) : Inv P (afterEvent D m) := by
  unfold afterEvent
  simp only
  have h1 := inv_settle D m h
  split
  · exact inv_startIteration D _ h1
  · exact h1

theorem inv_step {P : Prog} (m : Machine) (h : Inv P m) : Inv P (step D m) := by
  unfold step
  split
  · exact h
  · apply inv_afterEvent
    unfold handle; simp only
    split
    · exact inv_endIteration _ h
    · split
      · split
        · exact h.upd' _ _ fun ho => ⟨ho.1, ho.2⟩
        · exact h
      · exact h

theorem inv_initial (w : Workload) (P : Prog) :
    Inv P (Exec.initial D w.init.length w.attr P w) := by
  refine ⟨fun s hs => ?_, fun j hj => by simp [Exec.initial] at hj⟩
  simp only [Exec.initial, List.toList_toArray, List.mem_map, List.mem_range] at hs
  obtain ⟨i, _, rfl⟩ := hs
  exact ⟨Sub.refl, fun f hf => by simp at hf⟩

theorem inv_handle {P : Prog} (m : Machine) (t q : ℕ) (h : Inv P m) : Inv P (handle m t q) := by
  unfold handle; simp only
  split
  · exact inv_endIteration _ h
  · split
    · split
      · exact h.upd' _ _ fun ho => ⟨ho.1, ho.2⟩
      · exact h
    · exact h

/-- **Every session runs a part of its program.** -/
theorem reach_sub {w : Workload} {P : Prog} {m : Machine} (hr : Reach D w P m) : Inv P m := by
  induction hr with
  | start =>
    exact inv_afterEvent D _ (inv_initial D w P)
  | step _ ih => exact inv_step D _ ih

/-- Whether a program grows a hold. -/
def Route.grows : Prog → Bool
  | .done => false
  | .stop => false
  | .turn k => Route.grows k
  | .set _ _ k => Route.grows k
  | .observe _ _ k => Route.grows k
  | .run _ _ _ g k => g.isSome || Route.grows k
  | .hold _ _ b _ k => Route.grows b || Route.grows k
  | .branch _ a b k => Route.grows a || Route.grows b || Route.grows k
  | .loop b => Route.grows b

theorem grows_sub {P q : Prog} (h : Sub P q) (hg : Route.grows P = false) : Route.grows q = false := by
  induction h with
  | refl => exact hg
  | turn _ ih => simpa [Route.grows] using ih
  | set _ ih => simpa [Route.grows] using ih
  | observe _ ih => simpa [Route.grows] using ih
  | run _ ih => simp [Route.grows] at ih; exact ih.2
  | holdBody _ ih => simp [Route.grows] at ih; exact ih.1
  | holdK _ ih => simp [Route.grows] at ih; exact ih.2
  | yes _ ih => simp [Route.grows] at ih; exact ih.1.1
  | no _ ih => simp [Route.grows] at ih; exact ih.1.2
  | branchK _ ih => simp [Route.grows] at ih; exact ih.2
  | loopBody _ ih => simpa [Route.grows] using ih

/-- A program without `growing` has no growing job. -/
theorem jobs_not_growing {w : Workload} {P : Prog} {m : Machine} (hr : Reach D w P m)
    (hg : Route.grows P = false) : ∀ j ∈ m.jobs, j.growing = none := by
  intro j hj
  obtain ⟨w', k, hs⟩ := (reach_sub D hr).2 j hj
  have := grows_sub hs hg
  simp [Route.grows] at this
  exact Option.not_isSome_iff_eq_none.mp (by simp [this.1])

end Exec
end SerqLang
