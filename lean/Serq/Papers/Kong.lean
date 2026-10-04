/-
# Kong, Qi, Ye, Zhou: Smallest Volume First

The claim of `examples/papers/kong_svf.sq` (arXiv 2606.22327, §3, the burst
case of Theorem 3.2: every request arrives at time 0, holds its peak
`p = s + o` from admission, decodes one token per step, and the pool admits
the waiting request of least volume while its peak fits), proved about the
executable semantics of that program, and with it Theorem 3.2 itself.

The proof is the paper's (Lemma A.2): every step a request `j` waits, the
request at the head of the queue does not fit, so the active peaks exceed
`M - P`; the active requests precede `j` in volume order and each holds its
peak for `o` steps. As an invariant of the run: for each waiting `j`,
`(M - P + 1) now + Σ_{i ≺ j} p_i rem_i ≤ Σ_{i ≺ j} p_i o_i`, where `rem_i` is
what request `i` has left to decode; at its admission `j` keeps the
certificate `(M - P + 1) W_j ≤ Σ_{i ≺ j} p_i o_i`.
-/
import Serq.Work
import Serq.Claims


namespace SerqLang
namespace Papers
namespace KongSvf

open Exec

local notation "Dk" => Claims.KongSvf.deployment
local notation "Pk" => Claims.KongSvf.prog

/-! ### The program -/

/-- After the hold: observe the latency and end. -/
def Kk : Prog := .observe 2 (fun x => x.now) .stop

/-- The body of the hold: decode `o` tokens. -/
def Bk : Prog := .run 0 .decode (fun x => x.attr 10) none .done

/-- The hold of the peak `s + o`. -/
def Hk : Prog := .hold [(0, fun x => x.attr 9 + x.attr 10, none)] none Bk none Kk

def volE : Env → ℕ := fun x => x.attr 9 * x.attr 10 + (x.attr 10 * x.attr 10 + x.attr 10) / 2

theorem prog_eq : Pk = .observe 0 (fun x => x.attr 10) (.observe 1 volE Hk) := rfl

theorem deployment_eq : Dk = ⟨[⟨20000, 1, false, some volE⟩], 1000000, 0, none, fun _ => 1, none⟩ := rfl

/-! ### One command -/

theorem getS_setS_self (m : Machine) {i : ℕ} (s : Sess) (hi : i < m.sess.size) :
    getS (setS m i s) i = s := by
  simp [getS, setS, Array.getD_eq_getD_getElem?, hi]

theorem exec_observe (D : Deployment) (f : ℕ) (m : Machine) (i n : ℕ) (e : Env → ℕ) (k : Prog)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .observe n e k) :
    exec D (f + 1) m i = exec D f { setS m i { getS m i with prog := k } with
      obs := (n, (getS m i).serial, m.now, evalE m i e) :: m.obs } i := by
  rw [exec]; simp [hs, hp]

theorem exec_hold (D : Deployment) (f : ℕ) (m : Machine) (i : ℕ)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = Hk) :
    exec D (f + 1) m i = enqueue m i false := by
  rw [exec]; simp [hs, hp, Hk]

/-! ### The state of a request -/

/-- Where a request is: before its first command (`s0`), waiting at the pool
(`q`), admitted with its body not yet run (`r2`), decoding (`a`), finished
decoding with its hold not yet released (`r1`), released and about to
observe and end (`x`), ended (`e`). -/
inductive Cat
  | s0 | q | r2 | a | r1 | x | e
  deriving DecidableEq

section
variable (w : Workload)

def oo (i : ℕ) : ℕ := w.attr i 10
def ss (i : ℕ) : ℕ := w.attr i 9
def pp (i : ℕ) : ℕ := ss w i + oo w i
def vv (i : ℕ) : ℕ := ss w i * oo w i + (oo w i * oo w i + oo w i) / 2

/-- The queue's order: by volume, ties by serial number. -/
def pr (i j : ℕ) : Prop := vv w i < vv w j ∨ (vv w i = vv w j ∧ i < j)

instance (i j : ℕ) : Decidable (pr w i j) := by unfold pr; infer_instance

/-- The hold frame of an admitted request. -/
def fr (i : ℕ) : Frame := .hold ⟨[(0, pp w i, 0)], false, none, Hk⟩ Kk

end

/-- The ghost state: each request's place, the work its decode has left, and
the latency it observed. -/
structure Ghost where
  c : ℕ → Cat
  left : ℕ → ℕ
  lat : ℕ → ℕ

/-- What request `i` has left to decode. -/
def rem (w : Workload) (g : Ghost) (i : ℕ) : ℕ :=
  match g.c i with
  | .a => g.left i
  | .r1 => 0
  | .x => 0
  | .e => 0
  | _ => oo w i

/-- Its admission time, while admitted and not ended. -/
def adm (w : Workload) (g : Ghost) (now i : ℕ) : ℕ :=
  match g.c i with
  | .e => g.lat i - oo w i
  | _ => now - (oo w i - rem w g i)

/-- The fields of session `i` in its place. -/
def Shape (w : Workload) (s : Sess) (i : ℕ) : Cat → Prop
  | .s0 => s.prog = Pk ∧ s.stack = [] ∧ s.status = .ready
  | .q => s.prog = Hk ∧ s.stack = [] ∧ s.status = .queued
  | .r2 => s.prog = Bk ∧ s.stack = [fr w i] ∧ s.status = .ready
  | .a => s.prog = .done ∧ s.stack = [fr w i] ∧ s.status = .engine
  | .r1 => s.prog = .done ∧ s.stack = [fr w i] ∧ s.status = .ready
  | .x => s.prog = .done ∧ s.stack = [] ∧ s.status = .ready
  | .e => s.status = .ended

def holding (k : Cat) : Prop := k = .r2 ∨ k = .a ∨ k = .r1

instance (k : Cat) : Decidable (holding k) := by unfold holding; infer_instance

/-- The invariant of an instant. -/
structure SInv (w : Workload) (g : Ghost) (m : Machine) : Prop where
  size : m.sess.size = w.init.length
  shape : ∀ i < w.init.length, Shape w (getS m i) i (g.c i)
  serial : ∀ i < w.init.length, (getS m i).serial = i
  attr9 : ∀ i < w.init.length, (getS m i).attr.get 9 = ss w i
  attr10 : ∀ i < w.init.length, (getS m i).attr.get 10 = oo w i
  pools : m.pools.length = 1
  entries : (pst m 0).entries = []
  queue : (pst m 0).queue.Pairwise (· < ·)
  queueMem : ∀ i, i ∈ (pst m 0).queue ↔ i < w.init.length ∧ g.c i = .q
  holders : (pst m 0).holders.Nodup
  holdersMem : ∀ i, i ∈ (pst m 0).holders ↔ i < w.init.length ∧ holding (g.c i)
  used : (pst m 0).used = ((pst m 0).holders.map (pp w)).sum
  jobs : ∀ j ∈ m.jobs, j.owner < w.init.length ∧ g.c j.owner = .a ∧ j.mode = .decode ∧
    j.growing = none ∧ j.left = g.left j.owner
  jobsA : ∀ i < w.init.length, g.c i = .a → ∃ j ∈ m.jobs, j.owner = i
  jobsNodup : (m.jobs.map (·.owner)).Nodup
  leftPos : ∀ i < w.init.length, g.c i = .a → 1 ≤ g.left i ∧ g.left i ≤ oo w i
  ready : m.ready.Nodup
  readyMem : ∀ i, i ∈ m.ready ↔ i < w.init.length ∧ (g.c i = .s0 ∨ g.c i = .r2 ∨ g.c i = .r1)
  s0 : (∃ i < w.init.length, g.c i = .s0) → ∀ i < w.init.length, g.c i = .s0 ∨ g.c i = .q
  s0order : ∀ j < w.init.length, ∀ k < w.init.length, g.c j = .q → g.c k = .s0 → j < k
  readyS0 : (∃ i < w.init.length, g.c i = .s0) → m.ready.Pairwise (· < ·)
  nowS0 : (∃ i < w.init.length, g.c i = .s0) → m.now = 0
  delays : m.delays = []
  obs0 : (values m 0).Perm (((List.range w.init.length).filter (g.c · ≠ .s0)).map (oo w))
  obs1 : (values m 1).Perm (((List.range w.init.length).filter (g.c · ≠ .s0)).map (vv w))
  obs2 : (values m 2).Perm (((List.range w.init.length).filter (g.c · = .e)).map g.lat)
  before : ∀ i < w.init.length, ∀ j < w.init.length, g.c j = .q → g.c i ≠ .q → g.c i ≠ .s0 → pr w i j
  pot : ∀ j < w.init.length, g.c j = .q →
    17501 * m.now + ∑ i ∈ (Finset.range w.init.length).filter (pr w · j), pp w i * rem w g i ≤
      ∑ i ∈ (Finset.range w.init.length).filter (pr w · j), pp w i * oo w i
  cert : ∀ i < w.init.length, g.c i ≠ .q → g.c i ≠ .s0 →
    oo w i - rem w g i ≤ m.now ∧ 17501 * adm w g m.now i ≤
      ∑ k ∈ (Finset.range w.init.length).filter (pr w · i), pp w k * oo w k
  latE : ∀ i < w.init.length, g.c i = .e → oo w i ≤ g.lat i ∧ g.lat i ≤ m.now

/-! ### Machine helpers -/

theorem pst_setPool_self (m : Machine) (s : PoolSt) (h : m.pools.length = 1) : pst (setPool m 0 s) 0 = s := by
  unfold pst setPool
  rcases m with ⟨_, _, _, pools, _⟩
  simp only at h ⊢
  match pools, h with
  | [_], _ => rfl

theorem pools_setPool (m : Machine) (s : PoolSt) : (setPool m 0 s).pools.length = m.pools.length := by
  simp [setPool]

theorem makeRoom_empty (b cap need f : ℕ) (s : PoolSt) (h : s.entries = []) : makeRoom b cap need f s = s := by
  cases f with
  | zero => rfl
  | succ f => simp [makeRoom, h]

@[simp] theorem setS_pools (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).pools = m.pools := rfl
@[simp] theorem setS_jobs (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).jobs = m.jobs := rfl
@[simp] theorem setS_ready (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).ready = m.ready := rfl
@[simp] theorem setS_obs (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).obs = m.obs := rfl
@[simp] theorem setS_now (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).now = m.now := rfl
@[simp] theorem setS_nextAdm (m : Machine) (i : ℕ) (s : Sess) : (setS m i s).nextAdm = m.nextAdm := rfl
@[simp] theorem setPool_sess (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).sess = m.sess := rfl
@[simp] theorem setPool_nextAdm (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).nextAdm = m.nextAdm := rfl
@[simp] theorem setPool_ready (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).ready = m.ready := rfl
@[simp] theorem setPool_obs (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).obs = m.obs := rfl
@[simp] theorem setPool_now (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).now = m.now := rfl
@[simp] theorem setPool_jobs (m : Machine) (p : ℕ) (s : PoolSt) : (setPool m p s).jobs = m.jobs := rfl

theorem getS_setPool (m : Machine) (p i : ℕ) (s : PoolSt) : getS (setPool m p s) i = getS m i := rfl

/-- The peak of session `h` as its attributes give it. -/
def peakOf (m : Machine) (h : ℕ) : ℕ := (getS m h).attr.get 9 + (getS m h).attr.get 10

/-- The pool after admitting `h`. -/
def admPool (m : Machine) (h : ℕ) : PoolSt :=
  { pst m 0 with used := (pst m 0).used + peakOf m h, holders := (pst m 0).holders ++ [h] }

/-- Session `h` after its admission. -/
def admSess (m : Machine) (h : ℕ) : Sess :=
  let s := getS m h
  ⟨s.serial, s.attr, 0, Bk, [.hold ⟨[(0, peakOf m h, 0)], false, none, Hk⟩ Kk], .ready, m.nextAdm, s.turnIx⟩

/-- The admission of a waiting request `h` of this program: it takes its peak
from the pool and is ready to run its body. -/
theorem admit_eq (m : Machine) (h : ℕ) (hp : (getS m h).prog = Hk) (hs : (getS m h).stack = [])
    (he : (pst m 0).entries = []) :
    admit Dk m h 0 =
      { setS (setPool m 0 (admPool m h)) h (admSess m h) with
        nextAdm := m.nextAdm + 1, ready := m.ready ++ [h] } := by
  unfold admit
  rw [hp]
  simp only [Hk, holdNeeds, List.map_cons, List.map_nil, List.foldl_cons, List.foldl_nil, admitPool]
  simp [makeRoom_empty _ _ _ _ _ he, roundUp, pdef, deployment_eq, evalE, env, getS_setPool, hs,
    admPool, admSess, peakOf, Hk]

/-! ### Selecting and admitting -/

theorem argminKey_mem (k : ℕ → ℕ) : ∀ (l : List ℕ) (h : ℕ), argminKey k l = some h → h ∈ l
  | [], _, hh => by simp [argminKey] at hh
  | x :: xs, h, hh => by
    unfold argminKey at hh
    split at hh
    · simp only [Option.some.injEq] at hh; subst hh; exact List.mem_cons_self
    · rename_i y hy
      split at hh <;> simp only [Option.some.injEq] at hh <;> subst hh
      · exact List.mem_cons_of_mem _ (argminKey_mem k xs y hy)
      · exact List.mem_cons_self

theorem argminKey_none (k : ℕ → ℕ) : ∀ (l : List ℕ), argminKey k l = none → l = []
  | [], _ => rfl
  | x :: xs, hh => by
    unfold argminKey at hh
    split at hh <;> [simp at hh; (split at hh <;> simp at hh)]

/-- On a strictly increasing list, the first element of least key precedes
every other element in (key, value) order. -/
theorem argminKey_least (k : ℕ → ℕ) : ∀ (l : List ℕ), l.Pairwise (· < ·) → ∀ h, argminKey k l = some h →
    ∀ j ∈ l, j ≠ h → k h < k j ∨ (k h = k j ∧ h < j)
  | [], _, _, hh => by simp [argminKey] at hh
  | x :: xs, hs, h, hh => by
    rw [List.pairwise_cons] at hs
    unfold argminKey at hh
    split at hh
    · rename_i hn
      simp only [Option.some.injEq] at hh
      subst hh
      have := argminKey_none k xs hn
      subst this
      intro j hj hne; simp at hj; exact absurd hj hne
    · rename_i y hy
      have hym := argminKey_mem k xs y hy
      have ih := argminKey_least k xs hs.2 y hy
      split at hh
      · rename_i hlt
        simp only [Option.some.injEq] at hh
        subst hh
        intro j hj hne
        rcases List.mem_cons.mp hj with rfl | hj
        · exact Or.inl hlt
        · exact ih j hj hne
      · rename_i hge
        simp only [Option.some.injEq] at hh
        subst hh
        intro j hj hne
        rcases List.mem_cons.mp hj with rfl | hj
        · exact absurd rfl hne
        · have hxj := hs.1 j hj
          by_cases hjy : j = y
          · subst hjy
            rcases Nat.lt_or_ge (k x) (k j) with h1 | h1
            · exact Or.inl h1
            · exact Or.inr ⟨by omega, hxj⟩
          · rcases ih j hj hjy with h1 | ⟨h1, _⟩
            · rcases Nat.lt_or_ge (k x) (k j) with h2 | h2
              · exact Or.inl h2
              · exact Or.inr ⟨by omega, hxj⟩
            · rcases Nat.lt_or_ge (k x) (k j) with h2 | h2
              · exact Or.inl h2
              · exact Or.inr ⟨by omega, hxj⟩

theorem selectHead_k (m : Machine) :
    selectHead Dk m 0 = (argminKey (fun i => evalE m i volE) (pst m 0).queue).map
      (fun h => (h, (pst m 0).queue.erase h)) := by
  simp only [selectHead, pdef, deployment_eq, List.getD_cons_zero]
  cases argminKey (fun i => evalE m i volE) (pst m 0).queue <;> rfl

theorem evalE_vol {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) {i : ℕ}
    (hi : i < w.init.length) : evalE m i volE = vv w i := by
  simp only [evalE, env, volE, hI.attr9 i hi, hI.attr10 i hi, vv]

theorem fits_k {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) {h : ℕ}
    (hh : h < w.init.length) (hp : (getS m h).prog = Hk) :
    fitsAll Dk m (holdNeeds m h 0 (getS m h).prog) = true ↔ (pst m 0).used + pp w h ≤ 20000 := by
  rw [hp]
  simp [fitsAll, holdNeeds, Hk, evalE, env, hI.attr9 h hh, hI.attr10 h hh, roundUp, pdef,
    deployment_eq, pp]

theorem queue_nodup {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) : (pst m 0).queue.Nodup :=
  hI.queue.imp (fun h => Nat.ne_of_lt h)

/-- One admission: the selected waiting request moves to `r2`. -/
theorem sinv_admit {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m)
    (hno : ∀ i < w.init.length, g.c i ≠ .s0) {h : ℕ}
    (hsel : argminKey (fun i => evalE m i volE) (pst m 0).queue = some h)
    (hfit : (pst m 0).used + pp w h ≤ 20000) :
    SInv w { g with c := Function.update g.c h .r2 }
      (admit Dk (setPool m 0 { pst m 0 with queue := (pst m 0).queue.erase h }) h 0) := by
  have hmq := argminKey_mem _ _ h hsel
  obtain ⟨hhn, hhq⟩ := (hI.queueMem h).mp hmq
  have hsh := hI.shape h hhn
  rw [hhq] at hsh
  obtain ⟨hp, hs, hst⟩ := hsh
  set M0 := setPool m 0 { pst m 0 with queue := (pst m 0).queue.erase h } with hM0
  have hgM0 : ∀ i, getS M0 i = getS m i := fun i => rfl
  have hpM0 : pst M0 0 = { pst m 0 with queue := (pst m 0).queue.erase h } := pst_setPool_self _ _ hI.pools
  rw [admit_eq M0 h (by rw [hgM0]; exact hp) (by rw [hgM0]; exact hs) (by rw [hpM0]; exact hI.entries)]
  have hhsz : h < m.sess.size := hI.size ▸ hhn
  have hpeak : peakOf M0 h = pp w h := by
    simp [peakOf, hgM0, hI.attr9 h hhn, hI.attr10 h hhn, pp]
  set M' := { setS (setPool M0 0 (admPool M0 h)) h (admSess M0 h) with
    nextAdm := M0.nextAdm + 1, ready := M0.ready ++ [h] } with hM'
  have hpl : M'.pools.length = 1 := by simp [hM', hM0, setPool, hI.pools]
  have hpst : pst M' 0 = admPool M0 h := pst_setPool_self _ _ (by simp [hM0, setPool, hI.pools])
  have hq' : (pst M' 0).queue = (pst m 0).queue.erase h := by rw [hpst]; simp [admPool, hpM0]
  have hh' : (pst M' 0).holders = (pst m 0).holders ++ [h] := by rw [hpst]; simp [admPool, hpM0]
  have hu' : (pst M' 0).used = (pst m 0).used + pp w h := by rw [hpst]; simp [admPool, hpM0, hpeak]
  have he' : (pst M' 0).entries = [] := by rw [hpst]; simp [admPool, hpM0, hI.entries]
  have hget : ∀ i, getS M' i = if i = h then admSess M0 h else getS m i := by
    intro i
    by_cases hi : i = h
    · subst hi; simp only [if_true]; exact getS_setS_self _ _ (by simpa [hM0, setPool] using hhsz)
    · simp only [hi, if_false]; exact getS_setS_ne _ _ (Ne.symm hi)
  have hsz : M'.sess.size = w.init.length := by simp [hM', hM0, setS, setPool, hI.size]
  set g' : Ghost := { g with c := Function.update g.c h .r2 } with hg'
  have hc' : ∀ i, g'.c i = if i = h then .r2 else g.c i := by
    intro i; simp only [hg', Function.update]; split_ifs <;> simp_all
  have hrem : ∀ i, rem w g' i = rem w g i := by
    intro i
    simp only [rem, hc']
    split_ifs with hi
    · subst hi; rw [hhq]
    · rfl
  have hadm : ∀ now i, adm w g' now i = adm w g now i := by
    intro now i
    simp only [adm, hrem, hc']
    split_ifs with hi
    · subst hi; rw [hhq]
    · rfl
  have hneS0 : ∀ i, g'.c i ≠ .s0 ∨ i ≥ w.init.length := by
    intro i
    rw [hc']
    split_ifs
    · exact Or.inl (by decide)
    · by_cases hi : i < w.init.length
      · exact Or.inl (hno i hi)
      · exact Or.inr (by omega)
  have hnoS0' : ¬ ∃ i < w.init.length, g'.c i = .s0 := by
    rintro ⟨i, hi, hc⟩
    rcases hneS0 i with h1 | h1
    · exact h1 hc
    · omega
  have hfilt : ∀ (p : Cat → Prop) [DecidablePred p], (p .q ↔ p .r2) →
      (List.range w.init.length).filter (fun i => p (g'.c i)) =
        (List.range w.init.length).filter (fun i => p (g.c i)) := by
    intro p _ h1
    apply List.filter_congr
    intro i _
    rw [hc']
    split_ifs with hi
    · subst hi; rw [hhq]; simp [h1]
    · rfl
  refine
    { size := hsz
      shape := ?_
      serial := ?_
      attr9 := ?_
      attr10 := ?_
      pools := hpl
      entries := he'
      queue := hq' ▸ hI.queue.sublist (List.erase_sublist)
      queueMem := ?_
      holders := ?_
      holdersMem := ?_
      used := ?_
      jobs := ?_
      jobsA := ?_
      jobsNodup := hI.jobsNodup
      leftPos := ?_
      ready := ?_
      readyMem := ?_
      s0 := fun h' => absurd h' hnoS0'
      s0order := fun j _ k hk _ hck => absurd ⟨k, hk, hck⟩ hnoS0'
      readyS0 := fun h' => absurd h' hnoS0'
      nowS0 := fun h' => absurd h' hnoS0'
      delays := hI.delays
      obs0 := ?_
      obs1 := ?_
      obs2 := ?_
      before := ?_
      pot := ?_
      cert := ?_
      latE := ?_ }
  · intro i hi
    rw [hget, hc']
    split_ifs with hih
    · subst hih
      refine ⟨rfl, ?_, rfl⟩
      simp [admSess, fr, hpeak]
    · exact hI.shape i hi
  · intro i hi; rw [hget]; split_ifs with hih
    · subst hih; simp [admSess, hgM0, hI.serial i hi]
    · exact hI.serial i hi
  · intro i hi; rw [hget]; split_ifs with hih
    · subst hih; simp [admSess, hgM0, hI.attr9 i hi]
    · exact hI.attr9 i hi
  · intro i hi; rw [hget]; split_ifs with hih
    · subst hih; simp [admSess, hgM0, hI.attr10 i hi]
    · exact hI.attr10 i hi
  · intro i
    rw [hq', List.Nodup.mem_erase_iff (queue_nodup hI), hI.queueMem, hc']
    by_cases hih : i = h <;> simp [hih]
  · rw [hh']
    refine List.nodup_append.mpr ⟨hI.holders, List.nodup_singleton h, ?_⟩
    intro a ha b hb hab
    simp at hb; subst hb; subst hab
    obtain ⟨_, hhold⟩ := (hI.holdersMem a).mp ha
    rw [hhq] at hhold; simp [holding] at hhold
  · intro i
    rw [hh', List.mem_append, hI.holdersMem, hc']
    by_cases hih : i = h
    · subst hih; simp [hhn, holding, hhq]
    · simp [hih]
  · rw [hu', hh', hI.used]; simp
  · intro j hj
    obtain ⟨a1, a2, a3, a4, a5⟩ := hI.jobs j hj
    have hne : j.owner ≠ h := fun he => by rw [he, hhq] at a2; exact absurd a2 (by decide)
    refine ⟨a1, by rw [hc', if_neg hne]; exact a2, a3, a4, a5⟩
  · intro i hi hca
    rw [hc'] at hca
    by_cases hih : i = h
    · rw [if_pos hih] at hca; exact absurd hca (by decide)
    · rw [if_neg hih] at hca; exact hI.jobsA i hi hca
  · intro i hi hca
    rw [hc'] at hca
    by_cases hih : i = h
    · rw [if_pos hih] at hca; exact absurd hca (by decide)
    · rw [if_neg hih] at hca; exact hI.leftPos i hi hca
  · show (M0.ready ++ [h]).Nodup
    refine List.nodup_append.mpr ⟨hI.ready, List.nodup_singleton h, ?_⟩
    intro a ha b hb hab
    simp at hb; subst hb; subst hab
    obtain ⟨_, hr⟩ := (hI.readyMem a).mp ha
    rw [hhq] at hr; simp at hr
  · intro i
    show i ∈ m.ready ++ [h] ↔ _
    rw [List.mem_append, hI.readyMem, hc']
    by_cases hih : i = h
    · subst hih; simp [hhn, hhq]
    · simp [hih]
  · rw [hfilt (· ≠ .s0) (by decide)]; exact hI.obs0
  · rw [hfilt (· ≠ .s0) (by decide)]; exact hI.obs1
  · rw [hfilt (· = .e) (by decide)]; exact hI.obs2
  · intro i hi j hj hcj hci hci0
    rw [hc'] at hcj hci hci0
    by_cases hjh : j = h
    · subst hjh; simp at hcj
    · rw [if_neg hjh] at hcj
      by_cases hih : i = h
      · subst hih
        have hjq : j ∈ (pst m 0).queue := (hI.queueMem j).mpr ⟨hj, hcj⟩
        have := argminKey_least _ _ hI.queue i hsel j hjq hjh
        rw [evalE_vol hI hhn, evalE_vol hI hj] at this
        exact this
      · rw [if_neg hih] at hci hci0
        exact hI.before i hi j hj hcj hci hci0
  · intro j hj hcj
    rw [hc'] at hcj
    by_cases hjh : j = h
    · subst hjh; simp at hcj
    · rw [if_neg hjh] at hcj
      simp only [hrem]
      exact hI.pot j hj hcj
  · intro i hi hcq hcs
    rw [hc'] at hcq hcs
    by_cases hih : i = h
    · subst hih
      have hp := hI.pot i hi hhq
      have hr : rem w g' i = oo w i := by rw [hrem]; simp [rem, hhq]
      refine ⟨by rw [hr]; simp, ?_⟩
      rw [hadm]
      simp only [adm, hhq, rem, Nat.sub_self, Nat.sub_zero]
      show 17501 * M'.now ≤ _
      exact le_trans (Nat.le_add_right _ _) hp
    · rw [if_neg hih] at hcq hcs
      rw [hrem, hadm]
      exact hI.cert i hi hcq hcs
  · intro i hi hce
    rw [hc'] at hce
    by_cases hih : i = h
    · rw [if_pos hih] at hce; exact absurd hce (by decide)
    · rw [if_neg hih] at hce; exact hI.latE i hi hce

end KongSvf
end Papers
end SerqLang
