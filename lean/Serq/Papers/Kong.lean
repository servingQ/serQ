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
  wl : m.wl = w
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
    { wl := hI.wl
      size := hsz
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

/-- `g'` comes from `g` by admissions only. -/
def Adm (g g' : Ghost) : Prop :=
  (∀ i, g'.c i = g.c i ∨ (g.c i = .q ∧ g'.c i = .r2)) ∧ g'.left = g.left ∧ g'.lat = g.lat

theorem Adm.refl (g : Ghost) : Adm g g := ⟨fun _ => Or.inl rfl, rfl, rfl⟩

theorem Adm.trans {a b c : Ghost} (h1 : Adm a b) (h2 : Adm b c) : Adm a c := by
  refine ⟨fun i => ?_, h2.2.1.trans h1.2.1, h2.2.2.trans h1.2.2⟩
  rcases h2.1 i with h | ⟨hb, hc⟩ <;> rcases h1.1 i with h' | ⟨ha, hb'⟩
  · exact Or.inl (h.trans h')
  · exact Or.inr ⟨ha, h ▸ hb'⟩
  · exact Or.inr ⟨h' ▸ hb, hc⟩
  · rw [hb] at hb'; exact absurd hb' (by decide)

/-- The pool admits nobody more: its least-volume waiting request does not fit. -/
def Blocked (w : Workload) (m : Machine) : Prop :=
  ∀ h, argminKey (fun i => evalE m i volE) (pst m 0).queue = some h → 20000 < (pst m 0).used + pp w h

theorem sinv_admitHeads (w : Workload) : ∀ (f : ℕ) (g : Ghost) (m : Machine), SInv w g m →
    (∀ i < w.init.length, g.c i ≠ .s0) →
    ∃ g', Adm g g' ∧ SInv w g' (admitHeads Dk 0 f m) ∧
      (∃ L, (admitHeads Dk 0 f m).ready = m.ready ++ L ∧ (L = [] → admitHeads Dk 0 f m = m)) ∧
      ((pst m 0).queue.length < f → Blocked w (admitHeads Dk 0 f m))
  | 0, g, m, hI, _ => ⟨g, Adm.refl g, hI, ⟨[], by simp [admitHeads], fun _ => rfl⟩, fun h => by omega⟩
  | f + 1, g, m, hI, hno => by
    unfold admitHeads
    rw [selectHead_k]
    rcases hsel : argminKey (fun i => evalE m i volE) (pst m 0).queue with _ | h
    · simp only [Option.map_none]
      refine ⟨g, Adm.refl g, hI, ⟨[], by simp, by simp⟩, fun _ h' hh => ?_⟩
      rw [hsel] at hh; exact absurd hh (by simp)
    · simp only [Option.map_some]
      have hmq := argminKey_mem _ _ h hsel
      obtain ⟨hhn, hhq⟩ := (hI.queueMem h).mp hmq
      have hp : (getS m h).prog = Hk := by have := hI.shape h hhn; rw [hhq] at this; exact this.1
      split
      · rename_i hfit
        have hfit' := (fits_k hI hhn hp).mp hfit
        have hI1 := sinv_admit hI hno hsel hfit'
        have hs0 : (getS m h).stack = [] := by have := hI.shape h hhn; rw [hhq] at this; exact this.2.1
        have hM0e : (pst (setPool m 0 { pst m 0 with queue := (pst m 0).queue.erase h }) 0).entries = [] := by
          rw [pst_setPool_self _ _ hI.pools]; exact hI.entries
        have hadmeq := admit_eq (setPool m 0 { pst m 0 with queue := (pst m 0).queue.erase h }) h hp hs0 hM0e
        set M1 := admit Dk (setPool m 0 { pst m 0 with queue := (pst m 0).queue.erase h }) h 0 with hM1
        have hno1 : ∀ i < w.init.length, (Function.update g.c h .r2) i ≠ .s0 := by
          intro i hi
          by_cases hih : i = h
          · subst hih; simp
          · simp [Function.update, hih, hno i hi]
        obtain ⟨g', hadm, hI', ⟨L, hL, _⟩, hblk⟩ := sinv_admitHeads w f _ M1 hI1 hno1
        have hr1 : M1.ready = m.ready ++ [h] := by
          rw [hadmeq]
          rfl
        have hq1 : (pst M1 0).queue.length + 1 = (pst m 0).queue.length := by
          have hp1 : pst M1 0 = admPool (setPool m 0 { pst m 0 with queue := (pst m 0).queue.erase h }) h := by
            rw [hadmeq]; exact pst_setPool_self _ _ (by simp [setPool, hI.pools])
          rw [hp1]
          simp only [admPool, pst_setPool_self _ _ hI.pools]
          rw [List.length_erase_of_mem hmq]
          have : 0 < (pst m 0).queue.length := List.length_pos_of_mem hmq
          omega
        refine ⟨g', ?_, hI', ⟨h :: L, by rw [hL, hr1]; simp, fun h' => by simp at h'⟩, fun hl => hblk (by omega)⟩
        refine (show Adm g { g with c := Function.update g.c h .r2 } from ⟨fun i => ?_, rfl, rfl⟩).trans hadm
        by_cases hih : i = h
        · subst hih; exact Or.inr ⟨hhq, by simp⟩
        · exact Or.inl (by simp [Function.update, hih])
      · rename_i hfit
        refine ⟨g, Adm.refl g, hI, ⟨[], by simp, fun _ => rfl⟩, fun _ h' hh => ?_⟩
        rw [hsel] at hh
        simp only [Option.some.injEq] at hh
        subst hh
        have := (fits_k hI hhn hp).not.mp hfit
        omega

theorem admitAll_k (m : Machine) : admitAll Dk m = admitHeads Dk 0 1000 m := by
  simp [admitAll, deployment_eq, pdef]

/-! ### The commands of a ready request -/

/-- A filtered list over `range n` whose predicate becomes true at one more
index `i < n`. -/
theorem perm_filter_add {n i : ℕ} (hi : i < n) (p p' : ℕ → Bool) (f : ℕ → ℕ)
    (hpi : p i = false) (hp'i : p' i = true) (hrest : ∀ j, j ≠ i → p' j = p j) :
    (((List.range n).filter p').map f).Perm (f i :: ((List.range n).filter p).map f) := by
  induction n with
  | zero => omega
  | succ n ih =>
    simp only [List.range_succ, List.filter_append, List.map_append, List.filter_cons, List.filter_nil]
    by_cases hin : i = n
    · subst hin
      have heq : (List.range i).filter p' = (List.range i).filter p := by
        apply List.filter_congr
        intro j hj
        exact hrest j (by simp at hj; omega)
      rw [heq, hp'i, hpi]
      simp only [if_true, List.map_cons, List.map_nil, Bool.false_eq_true, if_false, List.append_nil]
      exact List.perm_append_singleton _ _
    · have hlt : i < n := by omega
      rw [hrest n (Ne.symm hin)]
      have := ih hlt
      by_cases hpn : p n = true
      · simp only [hpn, if_true, List.map_cons, List.map_nil]
        exact (this.append_right _).trans (by simp)
      · simp only [hpn, Bool.false_eq_true, if_false, List.map_nil, List.append_nil]
        exact this

theorem values_cons (m : Machine) (k i t v : ℕ) (name : ℕ) :
    values { m with obs := (k, i, t, v) :: m.obs } name =
      if k = name then v :: values m name else values m name := by
  simp only [values, List.filter_cons]
  split_ifs with h1 h2 h2 <;> simp_all

/-- The update of a request's place. -/
def Ghost.set (g : Ghost) (i : ℕ) (k : Cat) : Ghost := { g with c := Function.update g.c i k }

@[simp] theorem Ghost.set_c (g : Ghost) (i : ℕ) (k : Cat) (j : ℕ) :
    (g.set i k).c j = if j = i then k else g.c j := by
  simp [Ghost.set, Function.update]

@[simp] theorem Ghost.set_left (g : Ghost) (i : ℕ) (k : Cat) : (g.set i k).left = g.left := rfl
@[simp] theorem Ghost.set_lat (g : Ghost) (i : ℕ) (k : Cat) : (g.set i k).lat = g.lat := rfl

theorem rem_le (w : Workload) (g : Ghost) {i : ℕ} (hl : g.c i = .a → g.left i ≤ oo w i) : rem w g i ≤ oo w i := by
  unfold rem; split <;> simp_all

/-- An initial request observes its output length and volume and queues. -/
theorem sinv_pop_s0 {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) {i : ℕ}
    {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .s0) :
    SInv w (g.set i .q) (exec Dk 10000 { m with ready := rest } i) := by
  have hin : i < w.init.length := ((hI.readyMem i).mp (by rw [hr]; simp)).1
  have hisz : i < m.sess.size := hI.size ▸ hin
  have hsh := hI.shape i hin
  rw [hc] at hsh
  obtain ⟨hp, hs, hst⟩ := hsh
  have hex : ∃ k < w.init.length, g.c k = .s0 := ⟨i, hin, hc⟩
  have hnow := hI.nowS0 hex
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun j => rfl
  rw [show (10000 : ℕ) = 9997 + 3 from rfl]
  rw [exec_observe Dk _ m0 i 0 (fun x => x.attr 10) (.observe 1 volE Hk) (by rw [hg0]; exact hst)
    (by rw [hg0, hp, prog_eq])]
  set m1 : Machine := { setS m0 i { getS m0 i with prog := .observe 1 volE Hk } with
    obs := (0, (getS m0 i).serial, m0.now, evalE m0 i fun x => x.attr 10) :: m0.obs } with hm1
  have hg1 : getS m1 i = { getS m i with prog := .observe 1 volE Hk } :=
    getS_setS_self _ _ (by simpa [hm0] using hisz)
  rw [exec_observe Dk _ m1 i 1 volE Hk (by rw [hg1]; exact hst) (by rw [hg1])]
  set m2 : Machine := { setS m1 i { getS m1 i with prog := Hk } with
    obs := (1, (getS m1 i).serial, m1.now, evalE m1 i volE) :: m1.obs } with hm2
  have hg2 : getS m2 i = { getS m i with prog := Hk } := by
    rw [show getS m2 i = { getS m1 i with prog := Hk } from getS_setS_self _ _ (by simpa [hm1, hm0] using hisz),
      hg1]
  rw [exec_hold Dk _ m2 i (by rw [hg2]; exact hst) (by rw [hg2])]
  unfold enqueue
  rw [hg2]
  simp only [Hk, Bool.false_eq_true, ↓reduceIte]
  set M2 := setPool m2 0 { pst m2 0 with queue := (pst m2 0).queue ++ [i] } with hM2
  have hpl2 : m2.pools.length = 1 := by simp [hm2, hm1, hm0, hI.pools]
  have hpst2 : pst m2 0 = pst m 0 := rfl
  have hpst : pst (setS M2 i { getS M2 i with status := .queued }) 0 =
      { pst m 0 with queue := (pst m 0).queue ++ [i] } := by
    show pst M2 0 = _
    exact pst_setPool_self _ _ hpl2
  have hgM2 : ∀ j, getS M2 j = getS m2 j := fun j => rfl
  have hget : ∀ j, getS (setS M2 i { getS M2 i with status := .queued }) j =
      if j = i then { getS m i with prog := Hk, status := .queued } else getS m j := by
    intro j
    by_cases hj : j = i
    · subst hj; simp only [if_true]
      rw [getS_setS_self _ _ (by simpa [hM2, hm2, hm1, hm0] using hisz), hgM2, hg2]
    · simp only [hj, if_false]
      rw [getS_setS_ne _ _ (Ne.symm hj), hgM2]
      show getS (setS m1 i _) j = _
      rw [getS_setS_ne _ _ (Ne.symm hj)]
      show getS (setS m0 i _) j = _
      rw [getS_setS_ne _ _ (Ne.symm hj)]
      exact hg0 j
  have hser : (getS m i).serial = i := hI.serial i hin
  have hobs : (setS M2 i { getS M2 i with status := .queued }).obs =
      (1, i, 0, vv w i) :: (0, i, 0, oo w i) :: m.obs := by
    show m2.obs = _
    have h2 : m2.obs = (1, (getS m1 i).serial, m1.now, evalE m1 i volE) ::
        (0, (getS m0 i).serial, m0.now, evalE m0 i fun x => x.attr 10) :: m.obs := rfl
    have e1 : evalE m1 i volE = vv w i := by
      simp only [evalE, env, hg1]; simp [volE, hI.attr9 i hin, hI.attr10 i hin, vv]
    have e0 : evalE m0 i (fun x => x.attr 10) = oo w i := by
      simp only [evalE, env, hg0]; simp [hI.attr10 i hin, oo]
    have hn1 : m1.now = 0 := hnow
    have hn0 : m0.now = 0 := hnow
    rw [h2, e1, e0, hg1, hg0, hn1, hn0]
    simp [hser]
  set R := setS M2 i { getS M2 i with status := .queued } with hR
  have hRsz : R.sess.size = w.init.length := by simp [hR, hM2, hm2, hm1, hm0, setS, setPool, hI.size]
  have hRpl : R.pools.length = 1 := by simp [hR, hM2, setPool, hpl2]
  have hc' : ∀ j, (g.set i .q).c j = if j = i then .q else g.c j := fun j => Ghost.set_c g i .q j
  have hrem : ∀ j, rem w (g.set i .q) j = rem w g j := by
    intro j
    simp only [rem, hc']
    split_ifs with hj
    · subst hj; rw [hc]
    · rfl
  have hadm : ∀ now j, adm w (g.set i .q) now j = adm w g now j := by
    intro now j; simp only [adm, hrem, hc']
    split_ifs with hj
    · subst hj; rw [hc]
    · rfl
  -- every request is initial or waiting
  have hall : ∀ j < w.init.length, g.c j = .s0 ∨ g.c j = .q := hI.s0 hex
  have hall' : ∀ j < w.init.length, (g.set i .q).c j = .s0 ∨ (g.set i .q).c j = .q := by
    intro j hj; rw [hc']; split_ifs; · exact Or.inr rfl
    · exact hall j hj
  have hrs : rest.Pairwise (· < ·) := by
    have := hI.readyS0 hex; rw [hr] at this; exact (List.pairwise_cons.mp this).2
  have hirest : ∀ k ∈ rest, i < k := by
    have := hI.readyS0 hex; rw [hr] at this; exact (List.pairwise_cons.mp this).1
  have hrnd : i ∉ rest := by
    have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  refine
    { wl := hI.wl
      size := hRsz
      shape := ?_
      serial := ?_
      attr9 := ?_
      attr10 := ?_
      pools := hRpl
      entries := by rw [hpst]; exact hI.entries
      queue := ?_
      queueMem := ?_
      holders := by rw [hpst]; exact hI.holders
      holdersMem := ?_
      used := by rw [hpst]; exact hI.used
      jobs := ?_
      jobsA := ?_
      jobsNodup := hI.jobsNodup
      leftPos := ?_
      ready := by show rest.Nodup; have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
      readyMem := ?_
      s0 := fun _ => hall'
      s0order := ?_
      readyS0 := fun _ => hrs
      nowS0 := fun _ => hnow
      delays := hI.delays
      obs0 := ?_
      obs1 := ?_
      obs2 := ?_
      before := ?_
      pot := ?_
      cert := ?_
      latE := ?_ }
  · intro j hj
    rw [hget, hc']
    split_ifs with hji
    · exact ⟨rfl, hs, rfl⟩
    · exact hI.shape j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hser
    · exact hI.serial j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.attr9 j hj
    · exact hI.attr9 j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.attr10 j hj
    · exact hI.attr10 j hj
  · rw [hpst]
    refine List.pairwise_append.mpr ⟨hI.queue, List.pairwise_singleton _ _, ?_⟩
    intro a ha b hb
    simp at hb; subst hb
    obtain ⟨han, haq⟩ := (hI.queueMem a).mp ha
    exact hI.s0order a han b hin haq hc
  · intro j
    rw [hpst]
    simp only [List.mem_append, List.mem_singleton, hI.queueMem, hc']
    by_cases hji : j = i
    · subst hji; simp [hin]
    · simp [hji]
  · intro j
    rw [hpst, hI.holdersMem, hc']
    by_cases hji : j = i
    · subst hji; simp [holding, hc]
    · simp [hji]
  · intro jb hjb
    obtain ⟨a1, a2, a3, a4, a5⟩ := hI.jobs jb hjb
    have hne : jb.owner ≠ i := fun he => by rw [he, hc] at a2; exact absurd a2 (by decide)
    exact ⟨a1, by rw [hc', if_neg hne]; exact a2, a3, a4, a5⟩
  · intro j hj hca
    rw [hc'] at hca
    by_cases hji : j = i
    · rw [if_pos hji] at hca; exact absurd hca (by decide)
    · rw [if_neg hji] at hca; exact hI.jobsA j hj hca
  · intro j hj hca
    rw [hc'] at hca
    by_cases hji : j = i
    · rw [if_pos hji] at hca; exact absurd hca (by decide)
    · rw [if_neg hji] at hca; exact hI.leftPos j hj hca
  · intro j
    show j ∈ rest ↔ _
    rw [hc']
    have := hI.readyMem j
    rw [hr, List.mem_cons] at this
    by_cases hji : j = i
    · subst hji; simp [hrnd]
    · simp only [hji, if_false, false_or] at this ⊢; exact this
  · intro j hj k hk hcj hck
    rw [hc'] at hcj hck
    by_cases hki : k = i
    · rw [if_pos hki] at hck; exact absurd hck (by decide)
    · rw [if_neg hki] at hck
      have hkr : k ∈ rest := by
        have := (hI.readyMem k).mpr ⟨hk, Or.inl hck⟩
        rw [hr] at this; rcases List.mem_cons.mp this with h' | h'
        · exact absurd h' hki
        · exact h'
      by_cases hji : j = i
      · subst hji; exact hirest k hkr
      · rw [if_neg hji] at hcj; exact hI.s0order j hj k hk hcj hck
  · rw [show values R 0 = oo w i :: values m 0 by
      simp only [values, hobs, List.filter_cons]; simp]
    exact (hI.obs0.cons _).trans (perm_filter_add hin (fun j => decide (g.c j ≠ .s0))
      (fun j => decide ((g.set i .q).c j ≠ .s0)) (oo w) (by simp [hc]) (by simp)
      (fun j hj => by simp [hj])).symm
  · rw [show values R 1 = vv w i :: values m 1 by
      simp only [values, hobs, List.filter_cons]; simp]
    exact (hI.obs1.cons _).trans (perm_filter_add hin (fun j => decide (g.c j ≠ .s0))
      (fun j => decide ((g.set i .q).c j ≠ .s0)) (vv w) (by simp [hc]) (by simp)
      (fun j hj => by simp [hj])).symm
  · rw [show values R 2 = values m 2 by simp only [values, hobs, List.filter_cons]; simp]
    have : (List.range w.init.length).filter (fun j => (g.set i .q).c j = .e) =
        (List.range w.init.length).filter (fun j => g.c j = .e) := by
      apply List.filter_congr; intro j _; rw [hc']; split_ifs with hji
      · subst hji; simp [hc]
      · rfl
    rw [this]; exact hI.obs2
  · intro a ha j hj hcj hca hca0
    rcases hall' a ha with h' | h'
    · exact absurd h' hca0
    · exact absurd h' hca
  · intro j hj hcj
    simp only [hrem]
    show 17501 * m.now + _ ≤ _
    rw [hnow, Nat.mul_zero, Nat.zero_add]
    apply Finset.sum_le_sum
    intro k hk
    apply Nat.mul_le_mul_left
    have hkn : k < w.init.length := by simp at hk; exact hk.1
    exact rem_le w g fun h' => (hI.leftPos k hkn h').2
  · intro a ha hca hca0
    rcases hall' a ha with h' | h'
    · exact absurd h' hca0
    · exact absurd h' hca
  · intro a ha hce
    rcases hall' a ha with h' | h' <;> rw [h'] at hce <;> exact absurd hce (by decide)

theorem exec_runEngine (D : Deployment) (f : ℕ) (m : Machine) (i : ℕ) (md : Mode) (e : Env → ℕ) (k : Prog)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .run 0 md e none k) (hw : evalE m i e ≠ 0) :
    exec D (f + 1) m i =
      { setS m i { getS m i with prog := k, status := .engine } with
        jobs := (m.jobs.span fun j => (getS m j.owner).admSeq ≤ (getS m i).admSeq).1 ++
          ⟨i, md, evalE m i e, none⟩ :: (m.jobs.span fun j => (getS m j.owner).admSeq ≤ (getS m i).admSeq).2 } := by
  rw [exec]; simp [hs, hp, hw]

theorem span_mem {α : Type} (p : α → Bool) (l : List α) (x : α) :
    x ∈ (l.span p).1 ++ (l.span p).2 ↔ x ∈ l := by
  rw [List.span_eq_takeWhile_dropWhile, List.takeWhile_append_dropWhile]

/-- An admitted request starts its decode: a job of `o` tokens. -/
theorem sinv_pop_r2 {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m)
    (ho : ∀ i < w.init.length, 1 ≤ oo w i) {i : ℕ}
    {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r2) :
    SInv w { g with c := Function.update g.c i .a, left := Function.update g.left i (oo w i) }
      (exec Dk 10000 { m with ready := rest } i) := by
  have hin : i < w.init.length := ((hI.readyMem i).mp (by rw [hr]; simp)).1
  have hisz : i < m.sess.size := hI.size ▸ hin
  have hsh := hI.shape i hin
  rw [hc] at hsh
  obtain ⟨hp, hs, hst⟩ := hsh
  have hno : ¬ ∃ k < w.init.length, g.c k = .s0 := fun hex => by
    rcases hI.s0 hex i hin with h' | h' <;> rw [hc] at h' <;> exact absurd h' (by decide)
  set m0 : Machine := { m with ready := rest } with hm0
  have hg0 : ∀ j, getS m0 j = getS m j := fun j => rfl
  have hwk : evalE m0 i (fun x => x.attr 10) = oo w i := by
    simp only [evalE, env, hg0]; simp [hI.attr10 i hin, oo]
  rw [show (10000 : ℕ) = 9999 + 1 from rfl,
    exec_runEngine Dk _ m0 i .decode (fun x => x.attr 10) .done (by rw [hg0]; exact hst)
      (by rw [hg0, hp]; rfl) (by rw [hwk]; exact Nat.pos_iff_ne_zero.mp (ho i hin))]
  rw [hwk]
  set P := fun j : Job => decide ((getS m0 j.owner).admSeq ≤ (getS m0 i).admSeq) with hP
  set R : Machine := { setS m0 i { getS m0 i with prog := .done, status := .engine } with
    jobs := (m0.jobs.span P).1 ++ ⟨i, .decode, oo w i, none⟩ :: (m0.jobs.span P).2 } with hR
  set g' : Ghost := { g with c := Function.update g.c i .a, left := Function.update g.left i (oo w i) }
  have hc' : ∀ j, g'.c j = if j = i then .a else g.c j := by
    intro j; simp only [g', Function.update]; split_ifs <;> simp_all
  have hl' : ∀ j, g'.left j = if j = i then oo w i else g.left j := by
    intro j; simp only [g', Function.update]; split_ifs <;> simp_all
  have hrem : ∀ j, rem w g' j = rem w g j := by
    intro j; simp only [rem, hc']
    split_ifs with hj
    · subst hj; rw [hc]; simp [hl']
    · split <;> simp_all
  have hadm : ∀ now j, adm w g' now j = adm w g now j := by
    intro now j; simp only [adm, hrem, hc']
    split_ifs with hj
    · subst hj; rw [hc]
    · rfl
  have hget : ∀ j, getS R j = if j = i then { getS m i with prog := .done, status := .engine } else getS m j := by
    intro j
    by_cases hj : j = i
    · subst hj; simp only [if_true]; exact getS_setS_self _ _ (by simpa [hm0] using hisz)
    · simp only [hj, if_false]; exact getS_setS_ne _ _ (Ne.symm hj)
  have hjobs : ∀ x, x ∈ R.jobs ↔ x ∈ m.jobs ∨ x = ⟨i, .decode, oo w i, none⟩ := by
    intro x
    show x ∈ (m0.jobs.span P).1 ++ ⟨i, .decode, oo w i, none⟩ :: (m0.jobs.span P).2 ↔ _
    rw [List.mem_append, List.mem_cons, ← span_mem P m0.jobs x, List.mem_append]
    tauto
  have hiown : ∀ jb ∈ m.jobs, jb.owner ≠ i := fun jb hjb he => by
    have := (hI.jobs jb hjb).2.1; rw [he, hc] at this; exact absurd this (by decide)
  have hrnd : i ∉ rest := by
    have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  have hfilt : ∀ (p : Cat → Prop) [DecidablePred p], (p .r2 ↔ p .a) →
      (List.range w.init.length).filter (fun j => p (g'.c j)) =
        (List.range w.init.length).filter (fun j => p (g.c j)) := by
    intro p _ h1
    apply List.filter_congr
    intro j _
    rw [hc']
    split_ifs with hj
    · subst hj; rw [hc]; simp [h1]
    · rfl
  have hno' : ¬ ∃ k < w.init.length, g'.c k = .s0 := by
    rintro ⟨k, hk, hck⟩
    rw [hc'] at hck
    split_ifs at hck
    exact hno ⟨k, hk, hck⟩
  refine
    { wl := hI.wl
      size := by simp [hR, hm0, setS, hI.size]
      shape := ?_
      serial := ?_
      attr9 := ?_
      attr10 := ?_
      pools := hI.pools
      entries := hI.entries
      queue := hI.queue
      queueMem := ?_
      holders := hI.holders
      holdersMem := ?_
      used := hI.used
      jobs := ?_
      jobsA := ?_
      jobsNodup := ?_
      leftPos := ?_
      ready := by show rest.Nodup; have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
      readyMem := ?_
      s0 := fun h' => absurd h' hno'
      s0order := fun j _ k hk _ hck => absurd ⟨k, hk, hck⟩ hno'
      readyS0 := fun h' => absurd h' hno'
      nowS0 := fun h' => absurd h' hno'
      delays := hI.delays
      obs0 := by rw [hfilt (· ≠ .s0) (by decide)]; exact hI.obs0
      obs1 := by rw [hfilt (· ≠ .s0) (by decide)]; exact hI.obs1
      obs2 := by rw [hfilt (· = .e) (by decide)]; exact hI.obs2
      before := ?_
      pot := ?_
      cert := ?_
      latE := ?_ }
  · intro j hj
    rw [hget, hc']
    split_ifs with hji
    · subst hji; exact ⟨rfl, hs, rfl⟩
    · exact hI.shape j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.serial j hj
    · exact hI.serial j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.attr9 j hj
    · exact hI.attr9 j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.attr10 j hj
    · exact hI.attr10 j hj
  · intro j; show j ∈ (pst m 0).queue ↔ _; rw [hI.queueMem, hc']
    by_cases hji : j = i
    · subst hji; simp [hc]
    · simp [hji]
  · intro j; show j ∈ (pst m 0).holders ↔ _; rw [hI.holdersMem, hc']
    by_cases hji : j = i
    · subst hji; simp [holding, hc, hin]
    · simp [hji]
  · intro jb hjb
    rcases (hjobs jb).mp hjb with hjb | rfl
    · obtain ⟨a1, a2, a3, a4, a5⟩ := hI.jobs jb hjb
      have hne := hiown jb hjb
      exact ⟨a1, by rw [hc', if_neg hne]; exact a2, a3, a4, by rw [hl', if_neg hne]; exact a5⟩
    · exact ⟨hin, by rw [hc', if_pos rfl], rfl, rfl, by rw [hl', if_pos rfl]⟩
  · intro j hj hca
    rw [hc'] at hca
    by_cases hji : j = i
    · subst hji; exact ⟨_, (hjobs _).mpr (Or.inr rfl), rfl⟩
    · rw [if_neg hji] at hca
      obtain ⟨jb, hjb, hjo⟩ := hI.jobsA j hj hca
      exact ⟨jb, (hjobs jb).mpr (Or.inl hjb), hjo⟩
  · show ((m0.jobs.span P).1 ++ ⟨i, .decode, oo w i, none⟩ :: (m0.jobs.span P).2).map (·.owner) |>.Nodup
    have hperm : (((m0.jobs.span P).1 ++ ⟨i, .decode, oo w i, none⟩ :: (m0.jobs.span P).2).map (·.owner)).Perm
        (i :: m.jobs.map (·.owner)) := by
      rw [List.map_append, List.map_cons]
      refine List.perm_middle.trans (List.Perm.cons _ ?_)
      rw [← List.map_append, List.span_eq_takeWhile_dropWhile, List.takeWhile_append_dropWhile]
    refine hperm.nodup_iff.mpr (List.nodup_cons.mpr ⟨fun hm => ?_, hI.jobsNodup⟩)
    obtain ⟨jb, hjb, hjo⟩ := List.mem_map.mp hm
    exact hiown jb hjb hjo
  · intro j hj hca
    rw [hc'] at hca
    by_cases hji : j = i
    · subst hji; rw [hl', if_pos rfl]; exact ⟨ho j hj, le_rfl⟩
    · rw [if_neg hji] at hca; rw [hl', if_neg hji]; exact hI.leftPos j hj hca
  · intro j
    show j ∈ rest ↔ _
    rw [hc']
    have := hI.readyMem j
    rw [hr, List.mem_cons] at this
    by_cases hji : j = i
    · subst hji; simp [hrnd]
    · simp only [hji, if_false, false_or] at this ⊢; exact this
  · intro a ha j hj hcj hca hca0
    rw [hc'] at hcj hca hca0
    by_cases hji : j = i
    · rw [if_pos hji] at hcj; exact absurd hcj (by decide)
    · rw [if_neg hji] at hcj
      by_cases hai : a = i
      · subst hai; exact hI.before a ha j hj hcj (by rw [hc]; decide) (by rw [hc]; decide)
      · rw [if_neg hai] at hca hca0; exact hI.before a ha j hj hcj hca hca0
  · intro j hj hcj
    rw [hc'] at hcj
    by_cases hji : j = i
    · rw [if_pos hji] at hcj; exact absurd hcj (by decide)
    · rw [if_neg hji] at hcj
      simp only [hrem]
      exact hI.pot j hj hcj
  · intro a ha hca hca0
    rw [hrem, hadm]
    rw [hc'] at hca hca0
    by_cases hai : a = i
    · subst hai; exact hI.cert a ha (by rw [hc]; decide) (by rw [hc]; decide)
    · rw [if_neg hai] at hca hca0; exact hI.cert a ha hca hca0
  · intro a ha hce
    rw [hc'] at hce
    by_cases hai : a = i
    · rw [if_pos hai] at hce; exact absurd hce (by decide)
    · rw [if_neg hai] at hce; exact hI.latE a ha hce

theorem exec_stop (D : Deployment) (f : ℕ) (m : Machine) (i : ℕ)
    (hs : (getS m i).status = .ready) (hp : (getS m i).prog = .stop) (hst : (getS m i).stack = []) :
    exec D (f + 1) m i = setS m i { getS m i with status := .ended, stack := [] } := by
  rw [exec]; simp [hs, hp, endSession, hst]

/-- The pool after request `i` gives back `p`. -/
def relPool (m : Machine) (i p : ℕ) : PoolSt :=
  { pst m 0 with used := (pst m 0).used - p, holders := (pst m 0).holders.filter (· ≠ i) }

/-- The release of request `i`'s hold: its peak goes back to the pool. -/
theorem release_k (m : Machine) (i : ℕ) (hpl : m.pools.length = 1) (p : ℕ) :
    release Dk m i ⟨[(0, p, 0)], false, none, Hk⟩ = setPool m 0 (relPool m i p) := by
  simp [release, relPool]

theorem sum_filter_ne {l : List ℕ} (hn : l.Nodup) {i : ℕ} (hi : i ∈ l) (f : ℕ → ℕ) :
    ((l.filter (· ≠ i)).map f).sum + f i = (l.map f).sum := by
  induction l with
  | nil => simp at hi
  | cons a l ih =>
    rw [List.nodup_cons] at hn
    by_cases hai : a = i
    · subst hai
      have : l.filter (· ≠ a) = l := List.filter_eq_self.mpr (fun x hx => by
        simp only [ne_eq, decide_eq_true_eq]; rintro rfl; exact hn.1 hx)
      rw [List.filter_cons_of_neg (by simp), this, List.map_cons, List.sum_cons, Nat.add_comm]
    · have hi' : i ∈ l := by
        rcases List.mem_cons.mp hi with h | h
        · exact absurd h.symm hai
        · exact h
      rw [List.filter_cons_of_pos (by simpa using hai), List.map_cons, List.sum_cons, List.map_cons,
        List.sum_cons, Nat.add_assoc, ih hn.2 hi']

/-- The release of a finished request: it leaves the pool's holders and is
about to observe its latency (`x`). -/
theorem sinv_release {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) (hcs : w.computedSlot = some 8)
    {i : ℕ} {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    let m0 : Machine := { m with ready := rest }
    let M1 := release Dk (setS m0 i { getS m0 i with stack := [] }) i ⟨[(0, pp w i, 0)], false, none, Hk⟩
    SInv w (g.set i .x) (setS M1 i { getS M1 i with attr := (getS M1 i).attr.upd 8 0 }) := by
  intro m0 M1
  have hin : i < w.init.length := ((hI.readyMem i).mp (by rw [hr]; simp)).1
  have hisz : i < m.sess.size := hI.size ▸ hin
  have hsh := hI.shape i hin
  rw [hc] at hsh
  obtain ⟨hp, hs, hst⟩ := hsh
  have hno : ¬ ∃ k < w.init.length, g.c k = .s0 := fun hex => by
    rcases hI.s0 hex i hin with h' | h' <;> rw [hc] at h' <;> exact absurd h' (by decide)
  have hM1 : M1 = setPool (setS m0 i { getS m0 i with stack := [] }) 0 (relPool m i (pp w i)) :=
    release_k _ i (by simp [m0, hI.pools]) _
  set R := setS M1 i { getS M1 i with attr := (getS M1 i).attr.upd 8 0 } with hR
  have hgM1 : getS M1 i = { getS m i with stack := [] } := by
    rw [hM1, getS_setPool]; exact getS_setS_self _ _ (by simpa [m0] using hisz)
  have hget : ∀ j, getS R j = if j = i then { getS m i with stack := [], attr := (getS m i).attr.upd 8 0 }
      else getS m j := by
    intro j
    by_cases hj : j = i
    · subst hj; simp only [if_true]
      rw [hR, getS_setS_self _ _ (by rw [hM1]; simpa [m0, setPool] using hisz), hgM1]
    · simp only [hj, if_false]
      rw [hR, getS_setS_ne _ _ (Ne.symm hj), hM1, getS_setPool, getS_setS_ne _ _ (Ne.symm hj)]
      rfl
  have hpst : pst R 0 = relPool m i (pp w i) := by
    show pst M1 0 = _
    rw [hM1]; exact pst_setPool_self _ _ (by simp [m0, hI.pools])
  have hihold : i ∈ (pst m 0).holders := (hI.holdersMem i).mpr ⟨hin, by rw [hc]; simp [holding]⟩
  have hsum := sum_filter_ne hI.holders hihold (pp w)
  have hc' : ∀ j, (g.set i .x).c j = if j = i then .x else g.c j := fun j => Ghost.set_c g i .x j
  have hrem : ∀ j, rem w (g.set i .x) j = rem w g j := by
    intro j; simp only [rem, hc']
    split_ifs with hj
    · subst hj; rw [hc]
    · rfl
  have hadm : ∀ now j, adm w (g.set i .x) now j = adm w g now j := by
    intro now j; simp only [adm, hrem, hc']
    split_ifs with hj
    · subst hj; rw [hc]
    · rfl
  have hno' : ¬ ∃ k < w.init.length, (g.set i .x).c k = .s0 := by
    rintro ⟨k, hk, hck⟩
    rw [hc'] at hck
    split_ifs at hck
    exact hno ⟨k, hk, hck⟩
  have hfilt : ∀ (p : Cat → Prop) [DecidablePred p], (p .r1 ↔ p .x) →
      (List.range w.init.length).filter (fun j => p ((g.set i .x).c j)) =
        (List.range w.init.length).filter (fun j => p (g.c j)) := by
    intro p _ h1
    apply List.filter_congr
    intro j _
    rw [hc']
    split_ifs with hj
    · subst hj; rw [hc]; simp [h1]
    · rfl
  have hrnd : i ∉ rest := by
    have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).1
  have hiown : ∀ jb ∈ m.jobs, jb.owner ≠ i := fun jb hjb he => by
    have := (hI.jobs jb hjb).2.1; rw [he, hc] at this; exact absurd this (by decide)
  have h8 : ∀ (a : Attrs) (k : ℕ), k = 9 ∨ k = 10 → (a.upd 8 0).get k = a.get k := by
    intro a k hk; rw [Attrs.get_upd]; rcases hk with rfl | rfl <;> simp [Function.update]
  refine
    { wl := hI.wl
      size := by simp [hR, hM1, m0, setS, setPool, hI.size]
      shape := ?_
      serial := ?_
      attr9 := ?_
      attr10 := ?_
      pools := by simp [hR, hM1, m0, setPool, hI.pools]
      entries := by rw [hpst]; exact hI.entries
      queue := by rw [hpst]; exact hI.queue
      queueMem := ?_
      holders := by rw [hpst]; exact hI.holders.filter _
      holdersMem := ?_
      used := by rw [hpst]; simp only [relPool]; rw [hI.used]; omega
      jobs := ?_
      jobsA := ?_
      jobsNodup := hI.jobsNodup
      leftPos := ?_
      ready := by show rest.Nodup; have := hI.ready; rw [hr] at this; exact (List.nodup_cons.mp this).2
      readyMem := ?_
      s0 := fun h' => absurd h' hno'
      s0order := fun j _ k hk _ hck => absurd ⟨k, hk, hck⟩ hno'
      readyS0 := fun h' => absurd h' hno'
      nowS0 := fun h' => absurd h' hno'
      delays := hI.delays
      obs0 := by rw [hfilt (· ≠ .s0) (by decide)]; exact hI.obs0
      obs1 := by rw [hfilt (· ≠ .s0) (by decide)]; exact hI.obs1
      obs2 := by rw [hfilt (· = .e) (by decide)]; exact hI.obs2
      before := ?_
      pot := ?_
      cert := ?_
      latE := ?_ }
  · intro j hj
    rw [hget, hc']
    split_ifs with hji
    · subst hji; exact ⟨hp, rfl, hst⟩
    · exact hI.shape j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.serial j hj
    · exact hI.serial j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; simp only; rw [h8 _ 9 (Or.inl rfl)]; exact hI.attr9 j hj
    · exact hI.attr9 j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; simp only; rw [h8 _ 10 (Or.inr rfl)]; exact hI.attr10 j hj
    · exact hI.attr10 j hj
  · intro j; rw [hpst]; simp only [relPool]; rw [hI.queueMem, hc']
    by_cases hji : j = i
    · subst hji; simp [hc]
    · simp [hji]
  · intro j; rw [hpst]; simp only [relPool, List.mem_filter, hI.holdersMem, hc']
    by_cases hji : j = i
    · subst hji; simp [holding]
    · simp [hji]
  · intro jb hjb
    obtain ⟨a1, a2, a3, a4, a5⟩ := hI.jobs jb hjb
    exact ⟨a1, by rw [hc', if_neg (hiown jb hjb)]; exact a2, a3, a4, a5⟩
  · intro j hj hca
    rw [hc'] at hca
    by_cases hji : j = i
    · rw [if_pos hji] at hca; exact absurd hca (by decide)
    · rw [if_neg hji] at hca; exact hI.jobsA j hj hca
  · intro j hj hca
    rw [hc'] at hca
    by_cases hji : j = i
    · rw [if_pos hji] at hca; exact absurd hca (by decide)
    · rw [if_neg hji] at hca; exact hI.leftPos j hj hca
  · intro j
    show j ∈ rest ↔ _
    rw [hc']
    have := hI.readyMem j
    rw [hr, List.mem_cons] at this
    by_cases hji : j = i
    · subst hji; simp [hrnd]
    · simp only [hji, if_false, false_or] at this ⊢; exact this
  · intro a ha j hj hcj hca hca0
    rw [hc'] at hcj hca hca0
    by_cases hji : j = i
    · rw [if_pos hji] at hcj; exact absurd hcj (by decide)
    · rw [if_neg hji] at hcj
      by_cases hai : a = i
      · subst hai; exact hI.before a ha j hj hcj (by rw [hc]; decide) (by rw [hc]; decide)
      · rw [if_neg hai] at hca hca0; exact hI.before a ha j hj hcj hca hca0
  · intro j hj hcj
    rw [hc'] at hcj
    by_cases hji : j = i
    · rw [if_pos hji] at hcj; exact absurd hcj (by decide)
    · rw [if_neg hji] at hcj
      simp only [hrem]
      exact hI.pot j hj hcj
  · intro a ha hca hca0
    rw [hrem, hadm]
    rw [hc'] at hca hca0
    by_cases hai : a = i
    · subst hai; exact hI.cert a ha (by rw [hc]; decide) (by rw [hc]; decide)
    · rw [if_neg hai] at hca hca0; exact hI.cert a ha hca hca0
  · intro a ha hce
    rw [hc'] at hce
    by_cases hai : a = i
    · rw [if_pos hai] at hce; exact absurd hce (by decide)
    · rw [if_neg hai] at hce; exact hI.latE a ha hce

/-- A released request observes its latency and ends. -/
theorem sinv_finish {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) {i : ℕ}
    (hin : i < w.init.length) (hc : g.c i = .x) :
    SInv w { g with c := Function.update g.c i .e, lat := Function.update g.lat i m.now }
      (exec Dk 9999 (setS m i { getS m i with prog := Kk }) i) := by
  have hisz : i < m.sess.size := hI.size ▸ hin
  have hsh := hI.shape i hin
  rw [hc] at hsh
  obtain ⟨hp, hs, hst⟩ := hsh
  set m1 := setS m i { getS m i with prog := Kk } with hm1
  have hg1 : getS m1 i = { getS m i with prog := Kk } := getS_setS_self _ _ hisz
  rw [show (9999 : ℕ) = 9997 + 1 + 1 from rfl,
    exec_observe Dk _ m1 i 2 (fun x => x.now) .stop (by rw [hg1]; exact hst) (by rw [hg1]; rfl)]
  set m2 : Machine := { setS m1 i { getS m1 i with prog := .stop } with
    obs := (2, (getS m1 i).serial, m1.now, evalE m1 i fun x => x.now) :: m1.obs } with hm2
  have hg2 : getS m2 i = { getS m i with prog := .stop } := by
    rw [show getS m2 i = { getS m1 i with prog := .stop } from getS_setS_self _ _ (by simpa [hm1] using hisz),
      hg1]
  rw [exec_stop Dk _ m2 i (by rw [hg2]; exact hst) (by rw [hg2]) (by rw [hg2]; exact hs)]
  set R := setS m2 i { getS m2 i with status := .ended, stack := [] } with hR
  have hget : ∀ j, getS R j = if j = i then { getS m i with prog := .stop, status := .ended, stack := [] }
      else getS m j := by
    intro j
    by_cases hj : j = i
    · subst hj; simp only [if_true]
      rw [hR, getS_setS_self _ _ (by simpa [hm2, hm1] using hisz), hg2]
    · simp only [hj, if_false]
      rw [hR, getS_setS_ne _ _ (Ne.symm hj)]
      show getS (setS m1 i _) j = _
      rw [getS_setS_ne _ _ (Ne.symm hj), hm1, getS_setS_ne _ _ (Ne.symm hj)]
  have hobs : R.obs = (2, i, m.now, m.now) :: m.obs := by
    show m2.obs = _
    have : m2.obs = (2, (getS m1 i).serial, m1.now, evalE m1 i fun x => x.now) :: m.obs := rfl
    rw [this, hg1]
    simp [hI.serial i hin, evalE, env, hm1]
  set g' : Ghost := { g with c := Function.update g.c i .e, lat := Function.update g.lat i m.now } with hg'
  have hc' : ∀ j, g'.c j = if j = i then .e else g.c j := by
    intro j; simp only [hg', Function.update]; split_ifs <;> simp_all
  have hlat' : ∀ j, g'.lat j = if j = i then m.now else g.lat j := by
    intro j; simp only [hg', Function.update]; split_ifs <;> simp_all
  have hrem : ∀ j, rem w g' j = rem w g j := by
    intro j; simp only [rem, hc']
    split_ifs with hj
    · subst hj; rw [hc]
    · rfl
  have hcx := hI.cert i hin (by rw [hc]; decide) (by rw [hc]; decide)
  have hremi : rem w g i = 0 := by simp [rem, hc]
  have hadm : ∀ j, adm w g' m.now j = adm w g m.now j := by
    intro j; simp only [adm, hrem, hc']
    split_ifs with hj
    · subst hj; rw [hc]; simp [hlat', hremi]
    · rw [hlat', if_neg hj]
  have hno : ¬ ∃ k < w.init.length, g.c k = .s0 := fun hex => by
    rcases hI.s0 hex i hin with h' | h' <;> rw [hc] at h' <;> exact absurd h' (by decide)
  have hno' : ¬ ∃ k < w.init.length, g'.c k = .s0 := by
    rintro ⟨k, hk, hck⟩
    rw [hc'] at hck
    split_ifs at hck
    exact hno ⟨k, hk, hck⟩
  have hfilt : ∀ (p : Cat → Prop) [DecidablePred p], (p .x ↔ p .e) →
      (List.range w.init.length).filter (fun j => p (g'.c j)) =
        (List.range w.init.length).filter (fun j => p (g.c j)) := by
    intro p _ h1
    apply List.filter_congr
    intro j _
    rw [hc']
    split_ifs with hj
    · subst hj; rw [hc]; simp [h1]
    · rfl
  have hiown : ∀ jb ∈ m.jobs, jb.owner ≠ i := fun jb hjb he => by
    have := (hI.jobs jb hjb).2.1; rw [he, hc] at this; exact absurd this (by decide)
  refine
    { wl := hI.wl
      size := by simp [hR, hm2, hm1, setS, hI.size]
      shape := ?_
      serial := ?_
      attr9 := ?_
      attr10 := ?_
      pools := hI.pools
      entries := hI.entries
      queue := hI.queue
      queueMem := ?_
      holders := hI.holders
      holdersMem := ?_
      used := hI.used
      jobs := ?_
      jobsA := ?_
      jobsNodup := hI.jobsNodup
      leftPos := ?_
      ready := hI.ready
      readyMem := ?_
      s0 := fun h' => absurd h' hno'
      s0order := fun j _ k hk _ hck => absurd ⟨k, hk, hck⟩ hno'
      readyS0 := fun h' => absurd h' hno'
      nowS0 := fun h' => absurd h' hno'
      delays := hI.delays
      obs0 := ?_
      obs1 := ?_
      obs2 := ?_
      before := ?_
      pot := ?_
      cert := ?_
      latE := ?_ }
  · intro j hj
    rw [hget, hc']
    split_ifs with hji
    · rfl
    · exact hI.shape j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.serial j hj
    · exact hI.serial j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.attr9 j hj
    · exact hI.attr9 j hj
  · intro j hj; rw [hget]; split_ifs with hji
    · subst hji; exact hI.attr10 j hj
    · exact hI.attr10 j hj
  · intro j; show j ∈ (pst m 0).queue ↔ _; rw [hI.queueMem, hc']
    by_cases hji : j = i
    · subst hji; simp [hc]
    · simp [hji]
  · intro j; show j ∈ (pst m 0).holders ↔ _; rw [hI.holdersMem, hc']
    by_cases hji : j = i
    · subst hji; simp [holding, hc]
    · simp [hji]
  · intro jb hjb
    obtain ⟨a1, a2, a3, a4, a5⟩ := hI.jobs jb hjb
    exact ⟨a1, by rw [hc', if_neg (hiown jb hjb)]; exact a2, a3, a4, a5⟩
  · intro j hj hca
    rw [hc'] at hca
    by_cases hji : j = i
    · rw [if_pos hji] at hca; exact absurd hca (by decide)
    · rw [if_neg hji] at hca; exact hI.jobsA j hj hca
  · intro j hj hca
    rw [hc'] at hca
    by_cases hji : j = i
    · rw [if_pos hji] at hca; exact absurd hca (by decide)
    · rw [if_neg hji] at hca; exact hI.leftPos j hj hca
  · intro j
    show j ∈ m.ready ↔ _
    rw [hI.readyMem, hc']
    by_cases hji : j = i
    · subst hji; simp [hc]
    · simp [hji]
  · rw [show values R 0 = values m 0 by simp only [values, hobs, List.filter_cons]; simp,
      hfilt (· ≠ .s0) (by decide)]; exact hI.obs0
  · rw [show values R 1 = values m 1 by simp only [values, hobs, List.filter_cons]; simp,
      hfilt (· ≠ .s0) (by decide)]; exact hI.obs1
  · rw [show values R 2 = m.now :: values m 2 by simp only [values, hobs, List.filter_cons]; simp]
    have hmap : ((List.range w.init.length).filter (fun j => g.c j = .e)).map g'.lat =
        ((List.range w.init.length).filter (fun j => g.c j = .e)).map g.lat := by
      apply List.map_congr_left
      intro j hj
      have : j ≠ i := by
        rintro rfl; simp [hc] at hj
      rw [hlat', if_neg this]
    have := perm_filter_add hin (fun j => decide (g.c j = .e)) (fun j => decide (g'.c j = .e)) g'.lat
      (by simp [hc]) (by simp [hc']) (fun j hj => by simp [hc', hj])
    rw [hmap] at this
    refine Eq.subst (motive := fun x => List.Perm (x :: values m 2) _) (show g'.lat i = m.now by
      rw [hlat', if_pos rfl]) ?_
    exact ((hI.obs2.cons _).trans this.symm)
  · intro a ha j hj hcj hca hca0
    rw [hc'] at hcj hca hca0
    by_cases hji : j = i
    · rw [if_pos hji] at hcj; exact absurd hcj (by decide)
    · rw [if_neg hji] at hcj
      by_cases hai : a = i
      · subst hai; exact hI.before a ha j hj hcj (by rw [hc]; decide) (by rw [hc]; decide)
      · rw [if_neg hai] at hca hca0; exact hI.before a ha j hj hcj hca hca0
  · intro j hj hcj
    rw [hc'] at hcj
    by_cases hji : j = i
    · rw [if_pos hji] at hcj; exact absurd hcj (by decide)
    · rw [if_neg hji] at hcj
      simp only [hrem]
      exact hI.pot j hj hcj
  · intro a ha hca hca0
    show _ ∧ 17501 * adm w g' m.now a ≤ _
    rw [hrem, hadm]
    rw [hc'] at hca hca0
    by_cases hai : a = i
    · subst hai; exact hI.cert a ha (by rw [hc]; decide) (by rw [hc]; decide)
    · rw [if_neg hai] at hca hca0; exact hI.cert a ha hca hca0
  · intro a ha hce
    rw [hc'] at hce
    by_cases hai : a = i
    · subst hai
      rw [hlat', if_pos rfl]
      refine ⟨?_, le_rfl⟩
      have := hcx.1; rw [hremi] at this; omega
    · rw [if_neg hai] at hce; rw [hlat', if_neg hai]; exact hI.latE a ha hce

theorem attr_match {w : Workload} (M1 : Machine) (g : Ghost) (i : ℕ) (hwl : M1.wl = w)
    (hcs : w.computedSlot = some 8)
    (h : SInv w g (setS M1 i { getS M1 i with attr := (getS M1 i).attr.upd 8 0 })) :
    SInv w g (setS M1 i { getS M1 i with attr := match M1.wl.computedSlot with
      | some c => (getS M1 i).attr.upd c 0
      | none => (getS M1 i).attr }) := by
  simp only [hwl, hcs]; exact h

/-- After the release: the pool admits what now fits, then the request
observes its latency and ends. -/
theorem sinv_after_release {w : Workload} {g : Ghost} {i : ℕ} (hin : i < w.init.length) (hc : g.c i = .r1)
    (hno : ∀ k < w.init.length, g.c k ≠ .s0) (M1 : Machine) (A : Attrs)
    (h1 : SInv w (g.set i .x) (setS M1 i { getS M1 i with attr := A })) :
    ∃ g', SInv w g' (exec Dk 9999 (setS (admitAll Dk (setS M1 i { getS M1 i with attr := A })) i
        { getS (admitAll Dk (setS M1 i { getS M1 i with attr := A })) i with prog := Kk }) i) ∧ g'.c i = .e ∧
      (∀ j, j ≠ i → g'.c j = g.c j ∨ (g.c j = .q ∧ g'.c j = .r2)) ∧ g'.left = g.left := by
  rw [admitAll_k]
  have hno1 : ∀ k < w.init.length, (g.set i .x).c k ≠ .s0 := by
    intro k hk hck
    rw [Ghost.set_c] at hck
    split_ifs at hck
    exact hno k hk hck
  obtain ⟨g2, hadm, hI2, -, -⟩ := sinv_admitHeads w 1000 _ _ h1 hno1
  have hc2 : g2.c i = .x := by
    rcases hadm.1 i with h' | ⟨h', -⟩ <;> rw [Ghost.set_c, if_pos rfl] at h'
    · exact h'
    · exact absurd h' (by decide)
  refine ⟨_, sinv_finish hI2 hin hc2, by simp, fun j hj => ?_, hadm.2.1⟩
  simp only [Function.update, hj, dite_false]
  rcases hadm.1 j with h' | ⟨h', h''⟩ <;> rw [Ghost.set_c, if_neg hj] at h'
  · exact Or.inl h'
  · exact Or.inr ⟨h', h''⟩

/-- A finished request releases its peak, the pool admits what now fits, and
the request observes its latency and ends. -/
theorem sinv_pop_r1 {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) (hcs : w.computedSlot = some 8)
    {i : ℕ} {rest : List ℕ} (hr : m.ready = i :: rest) (hc : g.c i = .r1) :
    ∃ g', SInv w g' (exec Dk 10000 { m with ready := rest } i) ∧ g'.c i = .e ∧
      (∀ j, j ≠ i → g'.c j = g.c j ∨ (g.c j = .q ∧ g'.c j = .r2)) ∧ g'.left = g.left := by
  have hin : i < w.init.length := ((hI.readyMem i).mp (by rw [hr]; simp)).1
  have hsh := hI.shape i hin
  rw [hc] at hsh
  obtain ⟨hp, hs, hst⟩ := hsh
  have hno : ∀ k < w.init.length, g.c k ≠ .s0 := fun k hk hck => by
    rcases hI.s0 ⟨k, hk, hck⟩ i hin with h' | h' <;> rw [hc] at h' <;> exact absurd h' (by decide)
  have h1 := sinv_release hI hcs hr hc
  simp only at h1
  rw [show (10000 : ℕ) = 9999 + 1 from rfl, exec]
  have hst' : (getS { m with ready := rest } i).status = .ready := hst
  have hp' : (getS { m with ready := rest } i).prog = .done := hp
  have hs' : (getS { m with ready := rest } i).stack = [fr w i] := hs
  simp only [hst', hp', hs', fr, ne_eq, not_true_eq_false, if_false]
  simp only [hst', hp', hs'] at h1
  exact sinv_after_release hin hc hno _ _ (attr_match _ (g.set i .x) i hI.wl hcs h1)

/-! ### Settling an instant -/

/-- How much a request has left to do within an instant. -/
def wt : Cat → ℕ
  | .s0 => 4
  | .q => 3
  | .r2 => 2
  | .r1 => 2
  | .x => 1
  | .a => 0
  | .e => 0

def mu (w : Workload) (g : Ghost) : ℕ := ∑ j ∈ Finset.range w.init.length, wt (g.c j)

/-- Requests not yet admitted. -/
def nu (w : Workload) (g : Ghost) : ℕ :=
  ∑ j ∈ Finset.range w.init.length, if g.c j = .s0 ∨ g.c j = .q then 1 else 0

theorem mu_le {w : Workload} {g g' : Ghost} (h : ∀ j < w.init.length, wt (g'.c j) ≤ wt (g.c j)) :
    mu w g' ≤ mu w g :=
  Finset.sum_le_sum fun j hj => h j (Finset.mem_range.mp hj)

theorem mu_lt {w : Workload} {g g' : Ghost} (h : ∀ j < w.init.length, wt (g'.c j) ≤ wt (g.c j))
    {i : ℕ} (hi : i < w.init.length) (hlt : wt (g'.c i) < wt (g.c i)) : mu w g' < mu w g :=
  Finset.sum_lt_sum (fun j hj => h j (Finset.mem_range.mp hj)) ⟨i, Finset.mem_range.mpr hi, hlt⟩

theorem mu_bound (w : Workload) (g : Ghost) : mu w g ≤ 4 * w.init.length := by
  unfold mu
  calc ∑ j ∈ Finset.range w.init.length, wt (g.c j) ≤ ∑ _j ∈ Finset.range w.init.length, 4 :=
        Finset.sum_le_sum fun j _ => by cases g.c j <;> simp [wt]
    _ = 4 * w.init.length := by simp [mul_comm]

/-- Not yet admitted (`s0`, `q`) only from not yet admitted. -/
def Unadm (g g' : Ghost) (n : ℕ) : Prop :=
  ∀ j < n, (g'.c j = .s0 ∨ g'.c j = .q) → (g.c j = .s0 ∨ g.c j = .q)

theorem nu_le {w : Workload} {g g' : Ghost} (h : Unadm g g' w.init.length) : nu w g' ≤ nu w g :=
  Finset.sum_le_sum fun j hj => by
    have := h j (Finset.mem_range.mp hj)
    split_ifs <;> simp_all

theorem nu_lt {w : Workload} {g g' : Ghost} (h : Unadm g g' w.init.length) {i : ℕ} (hi : i < w.init.length)
    (h1 : g.c i = .q) (h2 : g'.c i = .r2) : nu w g' < nu w g :=
  Finset.sum_lt_sum (fun j hj => by
    have := h j (Finset.mem_range.mp hj)
    split_ifs <;> simp_all) ⟨i, Finset.mem_range.mpr hi, by simp [h1, h2]⟩

theorem nu_bound (w : Workload) (g : Ghost) : nu w g ≤ w.init.length := by
  unfold nu
  calc ∑ j ∈ Finset.range w.init.length, (if g.c j = .s0 ∨ g.c j = .q then 1 else 0)
        ≤ ∑ _j ∈ Finset.range w.init.length, 1 := Finset.sum_le_sum fun j _ => by split_ifs <;> simp
    _ = w.init.length := by simp

/-- The family's facts the proof uses. -/
structure Fam (w : Workload) : Prop where
  len : w.init.length ≤ 500
  cs : w.computedSlot = some 8
  peak : ∀ i < w.init.length, pp w i ≤ 2500
  out : ∀ i < w.init.length, 1 ≤ oo w i

theorem sinv_drain {w : Workload} (hF : Fam w) :
    ∀ (f : ℕ) (g : Ghost) (m : Machine), SInv w g m → (∀ j < w.init.length, g.c j ≠ .x) → mu w g < f →
      ∃ g', SInv w g' (drain Dk f m) ∧ (drain Dk f m).ready = [] ∧ (∀ j < w.init.length, g'.c j ≠ .x) ∧
        (∀ j < w.init.length, wt (g'.c j) ≤ wt (g.c j)) ∧ Unadm g g' w.init.length
  | 0, _, _, _, _, h => absurd h (Nat.not_lt_zero _)
  | f + 1, g, m, hI, hx, hmu => by
    unfold drain
    split
    · rename_i hr
      exact ⟨g, hI, hr, hx, fun _ _ => le_rfl, fun _ _ h => h⟩
    · rename_i i rest hr
      have hin : i < w.init.length := ((hI.readyMem i).mp (by rw [hr]; simp)).1
      have hcat := ((hI.readyMem i).mp (by rw [hr]; simp)).2
      -- the request's step, with what it does to the ghost
      obtain ⟨g1, hI1, hx1, hwt1, hun1, hlt1⟩ : ∃ g1, SInv w g1 (exec Dk 10000 { m with ready := rest } i) ∧
          (∀ j < w.init.length, g1.c j ≠ .x) ∧ (∀ j < w.init.length, wt (g1.c j) ≤ wt (g.c j)) ∧
          Unadm g g1 w.init.length ∧ wt (g1.c i) < wt (g.c i) := by
        rcases hcat with hc | hc | hc
        · refine ⟨_, sinv_pop_s0 hI hr hc, fun j _ => ?_, fun j _ => ?_, fun j _ => ?_, ?_⟩
          · simp only [Ghost.set_c]; split_ifs
            · decide
            · exact hx j ‹_›
          · simp only [Ghost.set_c]; split_ifs with h
            · subst h; rw [hc]; decide
            · exact le_rfl
          · simp only [Ghost.set_c]; split_ifs with h
            · subst h; intro; exact Or.inl hc
            · exact id
          · simp [hc, wt]
        · refine ⟨_, sinv_pop_r2 hI hF.out hr hc, fun j _ => ?_, fun j _ => ?_, fun j _ => ?_, ?_⟩
          · simp only [Function.update_apply]; split_ifs
            · decide
            · exact hx j ‹_›
          · simp only [Function.update_apply]; split_ifs with h
            · subst h; rw [hc]; decide
            · exact le_rfl
          · simp only [Function.update_apply]; split_ifs with h
            · intro h'; simp at h'
            · exact id
          · simp [hc, wt]
        · obtain ⟨g1, hI1, hci, hj, -⟩ := sinv_pop_r1 hI hF.cs hr hc
          refine ⟨g1, hI1, fun j hj' => ?_, fun j hj' => ?_, fun j hj' => ?_, by rw [hci, hc]; decide⟩
          · by_cases hji : j = i
            · rw [hji, hci]; decide
            · rcases hj j hji with h | ⟨_, h⟩
              · rw [h]; exact hx j hj'
              · rw [h]; decide
          · by_cases hji : j = i
            · rw [hji, hci, hc]; decide
            · rcases hj j hji with h | ⟨h1, h2⟩
              · rw [h]
              · rw [h1, h2]; decide
          · by_cases hji : j = i
            · rw [hji, hci]; intro h; simp at h
            · rcases hj j hji with h | ⟨h1, h2⟩
              · rw [h]; exact id
              · intro _; exact Or.inr h1
      have hmu1 : mu w g1 < mu w g := mu_lt hwt1 hin hlt1
      obtain ⟨g2, hI2, hr2, hx2, hwt2, hun2⟩ := sinv_drain hF f g1 _ hI1 hx1 (by omega)
      exact ⟨g2, hI2, hr2, hx2, fun j hj => (hwt2 j hj).trans (hwt1 j hj),
        fun j hj h => hun1 j hj (hun2 j hj h)⟩

theorem length_le_of_nodup_lt {l : List ℕ} (hn : l.Nodup) {n : ℕ} (h : ∀ x ∈ l, x < n) : l.length ≤ n := by
  rw [← List.toFinset_card_of_nodup hn]
  calc l.toFinset.card ≤ (Finset.range n).card :=
        Finset.card_le_card fun x hx => Finset.mem_range.mpr (h x (List.mem_toFinset.mp hx))
    _ = n := Finset.card_range n

theorem queue_short {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) (hF : Fam w) :
    (pst m 0).queue.length < 1000 := by
  have := length_le_of_nodup_lt (queue_nodup hI) (n := w.init.length)
    (fun x hx => ((hI.queueMem x).mp hx).1)
  have := hF.len
  omega

theorem sinv_settleLoop {w : Workload} (hF : Fam w) :
    ∀ (f : ℕ) (g : Ghost) (m : Machine), SInv w g m → (∀ j < w.init.length, g.c j ≠ .x) →
      mu w g < 10000 → nu w g < f →
      ∃ g', SInv w g' (settleLoop Dk f m) ∧ (settleLoop Dk f m).ready = [] ∧ Blocked w (settleLoop Dk f m) ∧
        (∀ j < w.init.length, g'.c j = .q ∨ g'.c j = .a ∨ g'.c j = .e) ∧ Unadm g g' w.init.length
  | 0, _, _, _, _, _, h => absurd h (Nat.not_lt_zero _)
  | f + 1, g, m, hI, hx, hmu, hnu => by
    unfold settleLoop
    simp only
    obtain ⟨g1, hI1, hr1, hx1, hwt1, hun1⟩ := sinv_drain hF 10000 g m hI hx hmu
    have hno1 : ∀ j < w.init.length, g1.c j ≠ .s0 := fun j hj hc =>
      by have := (hI1.readyMem j).mpr ⟨hj, Or.inl hc⟩; rw [hr1] at this; simp at this
    rw [admitAll_k]
    obtain ⟨g2, hadm, hI2, ⟨L, hL, hL0⟩, hblk⟩ := sinv_admitHeads w 1000 g1 _ hI1 hno1
    have hun2 : Unadm g1 g2 w.init.length := fun j _ h => by
      rcases hadm.1 j with h' | ⟨h', _⟩
      · rw [← h']; exact h
      · exact Or.inr h'
    have hcat : ∀ j < w.init.length, g2.c j ≠ .s0 ∧ g2.c j ≠ .x ∧
        (g2.c j = .r2 ∨ g2.c j = .r1 → j ∈ L) := by
      intro j hj
      refine ⟨fun hc => ?_, fun hc => ?_, fun hc => ?_⟩
      · rcases hadm.1 j with h' | ⟨h', _⟩
        · exact hno1 j hj (h' ▸ hc)
        · rw [hc] at *; simp_all
      · rcases hadm.1 j with h' | ⟨h', h''⟩
        · exact hx1 j hj (h' ▸ hc)
        · rw [hc] at h''; exact absurd h'' (by decide)
      · have := (hI2.readyMem j).mpr ⟨hj, Or.inr hc⟩
        rw [hL, hr1] at this; simpa using this
    split
    · rename_i hemp
      have hrd : (admitHeads Dk 0 1000 (drain Dk 10000 m)).ready = [] := by simpa using hemp
      refine ⟨g2, hI2, hrd, hblk (queue_short hI1 hF), fun j hj => ?_, fun j hj h => hun1 j hj (hun2 j hj h)⟩
      have hLe : L = [] := by rw [hr1] at hL; simpa [hrd] using hL.symm
      obtain ⟨h1, h2, h3⟩ := hcat j hj
      cases hcj : g2.c j <;> simp_all
    · rename_i hne
      have hLne : L ≠ [] := by
        intro hLe; rw [hLe, hr1] at hL; simp [hL] at hne
      obtain ⟨j, hjL⟩ := List.exists_mem_of_ne_nil L hLne
      have hj2 : j ∈ (admitHeads Dk 0 1000 (drain Dk 10000 m)).ready := by rw [hL]; simp [hjL]
      obtain ⟨hjn, hjc⟩ := (hI2.readyMem j).mp hj2
      have hq : g1.c j = .q ∧ g2.c j = .r2 := by
        rcases hadm.1 j with h' | h'
        · exfalso
          have : j ∈ (drain Dk 10000 m).ready := (hI1.readyMem j).mpr ⟨hjn, h' ▸ hjc⟩
          rw [hr1] at this; simp at this
        · exact h'
      have hnu2 : nu w g2 < nu w g1 := nu_lt hun2 hjn hq.1 hq.2
      have hnu1 : nu w g1 ≤ nu w g := nu_le hun1
      have hmu2 : mu w g2 ≤ mu w g1 := mu_le fun j hj => by
        rcases hadm.1 j with h' | ⟨h', h''⟩
        · rw [h']
        · rw [h', h'']; decide
      have hmu1 : mu w g1 ≤ mu w g := mu_le hwt1
      obtain ⟨g3, hI3, hr3, hb3, hc3, hun3⟩ := sinv_settleLoop hF f g2 _ hI2
        (fun j hj => (hcat j hj).2.1) (by omega) (by omega)
      exact ⟨g3, hI3, hr3, hb3, hc3, fun j hj h => hun1 j hj (hun2 j hj (hun3 j hj h))⟩

/-! ### Between instants -/

/-- `SInv` reads only these fields. -/
theorem SInv.congr {w : Workload} {g : Ghost} {m m' : Machine} (h : SInv w g m) (h1 : m'.wl = m.wl)
    (h2 : m'.sess = m.sess) (h3 : m'.pools = m.pools) (h4 : m'.jobs = m.jobs) (h5 : m'.ready = m.ready)
    (h6 : m'.obs = m.obs) (h7 : m'.now = m.now) (h8 : m'.delays = m.delays) : SInv w g m' := by
  have hg : ∀ i, getS m' i = getS m i := fun i => by simp [getS, h2]
  have hp : pst m' 0 = pst m 0 := by simp [pst, h3]
  have hv : ∀ k, values m' k = values m k := fun k => by simp [values, h6]
  exact
    { wl := h1.trans h.wl
      size := h2 ▸ h.size
      shape := fun i hi => hg i ▸ h.shape i hi
      serial := fun i hi => hg i ▸ h.serial i hi
      attr9 := fun i hi => hg i ▸ h.attr9 i hi
      attr10 := fun i hi => hg i ▸ h.attr10 i hi
      pools := h3 ▸ h.pools
      entries := hp ▸ h.entries
      queue := hp ▸ h.queue
      queueMem := fun i => hp ▸ h.queueMem i
      holders := hp ▸ h.holders
      holdersMem := fun i => hp ▸ h.holdersMem i
      used := hp ▸ h.used
      jobs := h4 ▸ h.jobs
      jobsA := h4 ▸ h.jobsA
      jobsNodup := h4 ▸ h.jobsNodup
      leftPos := h.leftPos
      ready := h5 ▸ h.ready
      readyMem := h5 ▸ h.readyMem
      s0 := h.s0
      s0order := h.s0order
      readyS0 := h5 ▸ h.readyS0
      nowS0 := h7 ▸ h.nowS0
      delays := h8.trans h.delays
      obs0 := (hv 0) ▸ h.obs0
      obs1 := (hv 1) ▸ h.obs1
      obs2 := (hv 2) ▸ h.obs2
      before := h.before
      pot := h7 ▸ h.pot
      cert := h7 ▸ h.cert
      latE := h7 ▸ h.latE }

/-- After an instant has settled: nobody is ready, every request waits,
decodes or has ended, and if some request waits the active peaks exceed
`M - P`. -/
structure Settled (w : Workload) (g : Ghost) (m : Machine) : Prop where
  inv : SInv w g m
  ready : m.ready = []
  cats : ∀ j < w.init.length, g.c j = .q ∨ g.c j = .a ∨ g.c j = .e
  full : (∃ j < w.init.length, g.c j = .q) → 17501 ≤ (pst m 0).used

/-- At an event boundary: settled, and the engine runs an iteration of one
token for each decoding request, ending one clock unit from now, exactly
when some request decodes. -/
structure Bnd (w : Workload) (g : Ghost) (m : Machine) : Prop extends Settled w g m where
  running : (∃ j < w.init.length, g.c j = .a) →
    (∃ s, m.iterEnd = some (m.now + 1, s)) ∧ m.iter = m.jobs.map fun j => (j.owner, 1)
  idle : (¬ ∃ j < w.init.length, g.c j = .a) → m.iterEnd = none

theorem full_of_blocked {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) (hF : Fam w)
    (hb : Blocked w m) : (∃ j < w.init.length, g.c j = .q) → 17501 ≤ (pst m 0).used := by
  rintro ⟨j, hj, hcj⟩
  have hjq : j ∈ (pst m 0).queue := (hI.queueMem j).mpr ⟨hj, hcj⟩
  rcases hsel : argminKey (fun i => evalE m i volE) (pst m 0).queue with _ | h
  · have := argminKey_none _ _ hsel; rw [this] at hjq; simp at hjq
  · have := hb h hsel
    have hhn := ((hI.queueMem h).mp (argminKey_mem _ _ h hsel)).1
    have := hF.peak h hhn
    omega

theorem settle_settled {w : Workload} (hF : Fam w) {g : Ghost} {m : Machine} (hI : SInv w g m)
    (hx : ∀ j < w.init.length, g.c j ≠ .x) :
    ∃ g', Settled w g' (settle Dk m) ∧ Unadm g g' w.init.length := by
  obtain ⟨g', hI', hr, hb, hc, hun⟩ := sinv_settleLoop hF 1000 g m hI hx
    (by have := mu_bound w g; have := hF.len; omega) (by have := nu_bound w g; have := hF.len; omega)
  exact ⟨g', ⟨hI', hr, hc, full_of_blocked hI' hF hb⟩, hun⟩

theorem jobs_nonempty_iff {w : Workload} {g : Ghost} {m : Machine} (hI : SInv w g m) :
    m.jobs ≠ [] ↔ ∃ j < w.init.length, g.c j = .a := by
  constructor
  · intro h
    obtain ⟨jb, hjb⟩ := List.exists_mem_of_ne_nil _ h
    exact ⟨jb.owner, (hI.jobs jb hjb).1, (hI.jobs jb hjb).2.1⟩
  · rintro ⟨j, hj, hca⟩ he
    obtain ⟨jb, hjb, -⟩ := hI.jobsA j hj hca
    rw [he] at hjb; simp at hjb

/-- The iteration starts: one token for every decoding request. -/
theorem start_bnd {w : Workload} {g : Ghost} {m : Machine} (hF : Fam w) (h : Settled w g m)
    (hie : m.iterEnd = none) : Bnd w g (startIteration Dk m) := by
  have hI := h.inv
  have hq : engineQueuesEmpty Dk m := fun p hp => by
    exfalso; revert hp; rcases p with _ | p <;> simp [pdef, deployment_eq]
  have hg : ∀ j ∈ m.jobs, j.growing = none := fun j hj => (hI.jobs j hj).2.2.2.1
  have hbud : Claims.KongSvf.deployment.budget = 1000000 := rfl
  have ha := assign_eq_fillIter Dk rfl m hq hg m.preempts (m.jobs.length + 100000) 0 1000000 [] (by omega)
  simp only [List.drop_zero, List.nil_append] at ha
  have hw1 : ∀ j ∈ m.jobs, wantOf Dk j = 1 := fun j hj => by
    have h1 := hI.jobs j hj
    have h2 := (hI.leftPos _ h1.1 h1.2.1).1
    simp only [wantOf, h1.2.2.1, h1.2.2.2.2]; omega
  have hlen : m.jobs.length < 1000000 := by
    have := length_le_of_nodup_lt hI.jobsNodup (n := w.init.length) (fun x hx => by
      obtain ⟨jb, hjb, rfl⟩ := List.mem_map.mp hx; exact (hI.jobs jb hjb).1)
    simp only [List.length_map] at this
    have := hF.len
    omega
  have hfill := fillIter_ones Dk m.jobs 1000000 hw1 hlen
  have hvia : ((List.range Claims.KongSvf.deployment.pools.length).any fun p =>
      (pdef Claims.KongSvf.deployment p).viaEngine && !(pst m p).queue.isEmpty) = false := by
    simp [pdef, deployment_eq]
  unfold startIteration
  rw [hvia, Bool.or_false]
  by_cases hj : m.jobs = []
  · have hna : ¬ ∃ j < w.init.length, g.c j = .a := fun he => (jobs_nonempty_iff hI).mpr he hj
    simp only [hj, List.isEmpty_nil, Bool.not_true, Bool.false_eq_true, ↓reduceIte]
    exact ⟨⟨hI.congr rfl rfl rfl hj.symm rfl rfl rfl rfl, h.ready, h.cats, h.full⟩,
      fun he => absurd he hna, fun _ => rfl⟩
  · have hya : ∃ j < w.init.length, g.c j = .a := (jobs_nonempty_iff hI).mp hj
    have hne : (m.jobs.isEmpty) = false := by simpa using hj
    simp only [hne, Bool.not_false, ↓reduceIte]
    rw [hbud, ha, hfill]
    have hne2 : (m.jobs.map fun j => (j.owner, 1)).isEmpty = false := by simpa using hj
    simp only [hne2, Bool.not_false, Bool.true_or, ↓reduceIte]
    refine ⟨⟨hI.congr rfl rfl rfl rfl rfl rfl rfl rfl, h.ready, h.cats, h.full⟩,
      fun _ => ⟨⟨m.nextDelay, by simp [deployment_eq]⟩, rfl⟩, fun hn => absurd hya hn⟩

end KongSvf
end Papers
end SerqLang
