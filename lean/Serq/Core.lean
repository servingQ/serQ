/-
# serQ: the serving-deployment language, formally

serQ is the language in which a serving deployment is a program: a
*deployment* of memory pools and stages, a *workload*, and a *route* that
every session follows (serQ `docs/language.md`; the Rust interpreter, crate `serq`, is the
reference implementation, this module is the formal one; see `docs/lean.md`). This
module formalises:

* the syntax of routes (`Route Env V`) and the well-formedness condition
  under which the semantics is well defined (every loop body runs a stage:
  `Route.wf`), with the surface syntax `[route| … ]` inside Lean;
* the rates of the stage kinds: a serial stage serves its head at rate 1,
  a shared stage splits `φ(n)` equally (`sharedRate_sum`: the shares add up
  to the capacity), an external stage serves everyone at rate 1;
* the semantics of one memory pool (`PoolState`, `Step`): the queue,
  admission with eviction of cached prefixes (the guard counts allocated
  units only, so a cached prefix never blocks), release with caching of
  `ℓ ≤ c` units, dropping a prefix and leaving. `Step.admit` is the
  admission of a hold with a `cache` clause, which consumes the session's
  own prefix; a hold without the clause leaves it (serQ IR 10, modelled in
  `Serq/Exec.lean`'s `admit`) and is, for this pool model, an admission that
  finds no entry of its own. The memory invariant
  `allocated + cached ≤ capacity` is preserved by every step
  (`Step.invariant`), as is the non-negativity of every entry
  (`Step.nonneg`).

Key theorems: `sharedRate_sum`, `total_evictUntil_le`, `Step.invariant`,
`Step.nonneg`, `admit_guard_units_only`.
-/
import Mathlib.Tactic

namespace SerqLang

/-! ## Syntax -/

/-- Session attributes: the workload's turn attributes and the hit
indicator, as a real vector indexed by attribute slot. -/
abbrev Attr := ℕ → ℝ

/-- Stage kinds (Definition L1:def:syntax; `step` is the iterating engine
of the colocated replica, Rust `stage s : step { budget; cost }`). -/
inductive StageKind
  /-- one job at a time, in arrival order, at rate 1 (a prefill instance) -/
  | serial
  /-- every job at once, throughput `φ n` split equally (decode, a link) -/
  | shared (φ : ℕ → ℝ)
  /-- every job on its own at rate 1, no waiting (a tool call) -/
  | external
  /-- an engine that runs iterations of `budget` tokens (chunked prefill
  with continuous batching) -/
  | step (budget : ℝ)

/-- How a job runs at a stage: plain work at rate 1 (`fifo`, `ps`,
`delay`), or prefill / decode tokens at a `step` stage. -/
inductive Mode
  | plain
  | prefill
  | decode
  deriving DecidableEq, Repr

/-- One pool of a `hold`: the pool, the units allocated, and optionally the
units that must fit for the admission (`fits`, e.g. the whole prompt while
only its first chunk is allocated). -/
abbrev HoldPool (Env V : Type) := ℕ × (Env → V) × Option (Env → V)

/-- Routes: the statements a session executes, over an environment type
`Env` (what an expression may read: the session's attributes, and in the
executable semantics also observables such as the cache and the engine's
budget) and a value type `V` (`ℝ` for the paper's models, `ℕ` for the
executable semantics on the step clock). Every statement carries its
continuation `k`.

* `hold ps reuse body cache k` waits until every pool of `ps` has room,
  consumes the session's own cached prefix (at most `reuse` of it; the rest
  stays cached, unusable, until evicted), allocates, runs `body` holding
  the units, releases them keeping `cache` units cached, and continues
  with `k` (the scoped form of the lecture's `admit`/`free`);
* `run s mode w g k` joins stage `s` with `w` units of work; `g` is the pool
  whose hold the job grows as it advances (`growing`);
* `set`, `observe` assign an attribute and record a measurement. -/
inductive Route (Env V : Type)
  | done
  | stop
  | turn (k : Route Env V)
  | set (slot : ℕ) (e : Env → V) (k : Route Env V)
  | observe (name : ℕ) (e : Env → V) (k : Route Env V)
  | run (stage : ℕ) (mode : Mode) (work : Env → V) (growing : Option ℕ) (k : Route Env V)
  | hold (pools : List (HoldPool Env V)) (reuse : Option (Env → V)) (body : Route Env V)
      (cache : Option (Env → V)) (k : Route Env V)
  | branch (p : Env → V) (yes no : Route Env V) (k : Route Env V)
  | loop (body : Route Env V)
  | whileLoop (p : Env → V) (body k : Route Env V)

namespace Route

variable {Env V : Type}

/-- Whether a route runs some stage (a `run` occurs in it). -/
def hasRun : Route Env V → Bool
  | done => false
  | stop => false
  | turn k => hasRun k
  | set _ _ k => hasRun k
  | observe _ _ k => hasRun k
  | run _ _ _ _ _ => true
  | hold _ _ body _ k => hasRun body || hasRun k
  | branch _ a b k => hasRun a || hasRun b || hasRun k
  | loop body => hasRun body
  | whileLoop _ body k => hasRun body || hasRun k

/-- Well formed: every loop body runs a stage, so that between two flow
steps a session executes finitely many commands (the condition under
which the semantics of Definition L1:def:semantics is well defined). -/
def wf : Route Env V → Bool
  | done => true
  | stop => true
  | turn k => wf k
  | set _ _ k => wf k
  | observe _ _ k => wf k
  | run _ _ _ _ k => wf k
  | hold _ _ body _ k => wf body && wf k
  | branch _ a b k => wf a && wf b && wf k
  | loop body => hasRun body && wf body
  | whileLoop _ body k => hasRun body && wf body && wf k

end Route

/-! ## Program helpers -/

/-- A single-pool hold, as the lecture writes it. -/
def hold1 {Env V : Type} (m : ℕ) (c : Env → V) (body : Route Env V) (l : Env → V)
    (k : Route Env V) : Route Env V :=
  .hold [(m, c, none)] none body (some l) k

/-- A plain run, as the lecture writes it. -/
def run1 {Env V : Type} (s : ℕ) (w : Env → V) (k : Route Env V) : Route Env V :=
  .run s .plain w none k

/-! ## Surface syntax inside Lean

serQ programs can be written in their own syntax inside Lean: a syntax
category `route` with the statements of the language, elaborated by
macros into the `Route` inductive. No hand-written lexer or parser is
needed on the formal side (the Rust crate keeps its own parser for the
simulator's CLI); the same term is the object of the proofs. Expressions
are ordinary Lean terms in the environment `x`. -/

declare_syntax_cat route (behavior := symbol)
declare_syntax_cat routePool

-- `&"kw"` keeps words non-reserved where they are also field names
syntax num "(" term ")" (&"fits" "(" term ")")? : routePool
syntax &"done" : route
syntax &"stop" : route
syntax &"turn" ";" route : route
syntax &"set" num "=" term ";" route : route
syntax &"observe" num "=" term ";" route : route
syntax &"run" num "(" term ")" ";" route : route
syntax &"run" num &"prefill" "(" term ")" (&"growing" num)? ";" route : route
syntax &"run" num &"decode" "(" term ")" (&"growing" num)? ";" route : route
syntax &"hold" routePool,+ (&"reuse" "(" term ")")? "{" route "}" (&"cache" "(" term ")")? ";"
  route : route
syntax &"branch" "(" term ")" "{" route "}" &"else" "{" route "}" ";" route : route
syntax &"loop" "{" route "}" : route
syntax &"while" "(" term ")" "{" route "}" ";" route : route
syntax "[route|" route "]" : term
syntax "[routePool|" routePool "]" : term

open Lean in
macro_rules
  | `([routePool| $m:num ( $c ) $[fits ( $f )]?]) => do
      let x := mkIdent `x
      match f with
      | some f => `(($m, (fun $x => $c), some (fun $x => $f)))
      | none => `(($m, (fun $x => $c), none))

open Lean in
macro_rules
  | `([route| done]) => `(Route.done)
  | `([route| stop]) => `(Route.stop)
  | `([route| turn ; $k]) => `(Route.turn [route| $k])
  | `([route| set $n = $e ; $k]) =>
      let x := mkIdent `x
      `(Route.set $n (fun $x => $e) [route| $k])
  | `([route| observe $n = $e ; $k]) =>
      let x := mkIdent `x
      `(Route.observe $n (fun $x => $e) [route| $k])
  | `([route| run $s:num ( $w ) ; $k]) =>
      let x := mkIdent `x
      `(Route.run $s Mode.plain (fun $x => $w) none [route| $k])
  | `([route| run $s:num prefill ( $w ) $[growing $g:num]? ; $k]) => do
      let x := mkIdent `x
      let gr ← match g with
        | some g => `(some $g)
        | none => `(none)
      `(Route.run $s Mode.prefill (fun $x => $w) $gr [route| $k])
  | `([route| run $s:num decode ( $w ) $[growing $g:num]? ; $k]) => do
      let x := mkIdent `x
      let gr ← match g with
        | some g => `(some $g)
        | none => `(none)
      `(Route.run $s Mode.decode (fun $x => $w) $gr [route| $k])
  | `([route| hold $ps:routePool,* $[reuse ( $r )]? { $body } $[cache ( $l )]? ; $k]) => do
      let x := mkIdent `x
      let pools ← ps.getElems.mapM fun p => `([routePool| $p])
      let ru ← match r with
        | some r => `(some (fun $x => $r))
        | none => `(none)
      let ca ← match l with
        | some l => `(some (fun $x => $l))
        | none => `(none)
      `(Route.hold [$pools,*] $ru [route| $body] $ca [route| $k])
  | `([route| branch ( $p ) { $a } else { $b } ; $k]) =>
      let x := mkIdent `x
      `(Route.branch (fun $x => $p) [route| $a] [route| $b] [route| $k])
  | `([route| loop { $body }]) => `(Route.loop [route| $body])
  | `([route| while ($p) { $body }; $k]) =>
      let x := mkIdent `x
      `(Route.whileLoop (fun $x => $p) [route| $body] [route| $k])

/-! ## Stage rates (Eq. L1:eq:rates) -/

/-- The rate at which a job at position `i` (0 = first) of a stage with `n`
jobs works off its remaining work. -/
noncomputable def rate (k : StageKind) (n i : ℕ) : ℝ :=
  match k with
  | .serial => if i = 0 then 1 else 0
  | .shared φ => φ n / n
  | .external => 1
  | .step budget => budget / n

/-- A serial stage works at rate 1 in total: only its head moves. -/
theorem serialRate_sum (n : ℕ) (hn : 0 < n) :
    ∑ i ∈ Finset.range n, rate .serial n i = 1 := by
  rw [Finset.sum_eq_single 0]
  · simp [rate]
  · intro i _ hi
    simp [rate, hi]
  · intro h
    exact absurd (Finset.mem_range.mpr hn) h

/-- A shared stage delivers its whole capacity `φ n`: the equal shares add
up (processor sharing is work conserving). -/
theorem sharedRate_sum (φ : ℕ → ℝ) (n : ℕ) (hn : 0 < n) :
    ∑ _i ∈ Finset.range n, rate (.shared φ) n _i = φ n := by
  simp only [rate, Finset.sum_const, Finset.card_range, nsmul_eq_mul]
  have : (n : ℝ) ≠ 0 := by exact_mod_cast hn.ne'
  field_simp

/-! ## One pool: configuration and commands -/

/-- Entries of a pool: `(session, units)`. -/
abbrev Entries := List (ℕ × ℝ)

/-- Units held by a list of entries. -/
def total (l : Entries) : ℝ := (l.map Prod.snd).sum

/-- All entries non-negative. -/
def AllNonneg (l : Entries) : Prop := ∀ e ∈ l, 0 ≤ e.2

/-- Remove the entry of a session (the first, if several). -/
def eraseSession (r : ℕ) (l : Entries) : Entries := l.eraseP (fun e => decide (e.1 = r))

/-- Delete entries from the front (the eviction order) until at most `b`
units remain: `evict_E(C, b)` of Definition L1:def:semantics. -/
noncomputable def evictUntil : Entries → ℝ → Entries
  | [], _ => []
  | e :: l, b => if total (e :: l) ≤ b then e :: l else evictUntil l b

/-- An eviction order: a rearrangement of the cache (LRU, shortest first,
the priced orders of §3). Only its permutation property matters here. -/
structure EvictionOrder where
  order : Entries → Entries
  perm : ∀ l, (order l).Perm l

/-- The state of one pool of capacity `cap`: the allocations of admitted
sessions, the cached prefixes, and the admission queue with the units
each waiting session asked for. -/
structure PoolState where
  cap : ℝ
  alloc : Entries
  cache : Entries
  queue : Entries

/-- The memory invariant (L1:eq:invariant): `|h_m| + |C_m| ≤ M`. -/
def PoolState.Invariant (P : PoolState) : Prop := total P.alloc + total P.cache ≤ P.cap

def PoolState.Nonneg (P : PoolState) : Prop := AllNonneg P.alloc ∧ AllNonneg P.cache

/-- The commands on a pool (Definition L1:def:semantics, with the scoped
release of v2 and `leave` for a session's end). -/
inductive Step (E : EvictionOrder) : PoolState → PoolState → Prop
  /-- `[Queue]`: a session asks for `c` units and joins the queue. -/
  | queue (P : PoolState) (r : ℕ) (c : ℝ) (hc : 0 ≤ c) :
      Step E P { P with queue := P.queue ++ [(r, c)] }
  /-- `[Admit]`: the head of the queue is admitted when its units fit next
  to the allocated ones (cached prefixes do not count); its own prefix is
  consumed and the others are evicted, in order, until the allocation and
  the cache fit. -/
  | admit (P : PoolState) (r : ℕ) (c : ℝ) (q : Entries)
      (hq : P.queue = (r, c) :: q) (hc : 0 ≤ c) (hfit : total P.alloc + c ≤ P.cap) :
      Step E P { P with
        alloc := P.alloc ++ [(r, c)]
        cache := evictUntil (E.order (eraseSession r P.cache)) (P.cap - total P.alloc - c)
        queue := q }
  /-- `[Free]`: a session holding `c` units releases them and keeps `ℓ ≤ c`
  cached. -/
  | free (P : PoolState) (r : ℕ) (c ℓ : ℝ) (hr : (r, c) ∈ P.alloc) (hl : 0 ≤ ℓ) (hlc : ℓ ≤ c) :
      Step E P { P with
        alloc := P.alloc.erase (r, c)
        cache := eraseSession r P.cache ++ [(r, ℓ)] }
  /-- `[Drop]`: a session discards its cached prefix (a forced miss). -/
  | drop (P : PoolState) (r : ℕ) :
      Step E P { P with cache := eraseSession r P.cache }
  /-- `[End]`: a session leaves; its allocation and its prefix go with it. -/
  | leave (P : PoolState) (r : ℕ) :
      Step E P { P with alloc := eraseSession r P.alloc, cache := eraseSession r P.cache }

/-! ### Lemmas on totals -/

@[simp] theorem total_nil : total [] = 0 := rfl

@[simp] theorem total_cons (e : ℕ × ℝ) (l : Entries) : total (e :: l) = e.2 + total l := rfl

theorem total_append (l₁ l₂ : Entries) : total (l₁ ++ l₂) = total l₁ + total l₂ := by
  simp [total, List.map_append, List.sum_append]

theorem total_perm {l₁ l₂ : Entries} (h : l₁.Perm l₂) : total l₁ = total l₂ :=
  (h.map Prod.snd).sum_eq

theorem total_nonneg {l : Entries} (h : AllNonneg l) : 0 ≤ total l := by
  induction l with
  | nil => simp
  | cons e l ih =>
    have he : 0 ≤ e.2 := h e (List.mem_cons_self ..)
    have hl : AllNonneg l := fun x hx => h x (List.mem_cons_of_mem _ hx)
    rw [total_cons]
    linarith [ih hl]

theorem nonneg_of_cons {e : ℕ × ℝ} {l : Entries} (h : AllNonneg (e :: l)) : AllNonneg l :=
  fun x hx => h x (List.mem_cons_of_mem _ hx)

theorem nonneg_append {l₁ l₂ : Entries} (h₁ : AllNonneg l₁) (h₂ : AllNonneg l₂) : AllNonneg (l₁ ++ l₂) := by
  intro e he
  rcases List.mem_append.mp he with h | h
  · exact h₁ e h
  · exact h₂ e h

theorem nonneg_perm {l₁ l₂ : Entries} (h : l₁.Perm l₂) (hl : AllNonneg l₁) : AllNonneg l₂ :=
  fun e he => hl e (h.mem_iff.mpr he)

/-- Erasing a session's entry removes at most its units. -/
theorem total_eraseSession_le (r : ℕ) {l : Entries} (h : AllNonneg l) :
    total (eraseSession r l) ≤ total l := by
  induction l with
  | nil => simp [eraseSession]
  | cons e l ih =>
    have he : 0 ≤ e.2 := h e (List.mem_cons_self ..)
    have hl : AllNonneg l := nonneg_of_cons h
    unfold eraseSession
    rw [List.eraseP_cons]
    by_cases hr : e.1 = r
    · have hd : decide (e.1 = r) = true := decide_eq_true hr
      rw [hd, if_pos rfl, total_cons]
      linarith
    · have hd : decide (e.1 = r) = false := decide_eq_false hr
      rw [hd, if_neg (by decide), total_cons, total_cons]
      have := ih hl
      unfold eraseSession at this
      linarith

theorem nonneg_eraseSession (r : ℕ) {l : Entries} (h : AllNonneg l) : AllNonneg (eraseSession r l) :=
  fun e he => h e (List.eraseP_sublist.subset he)

/-- Erasing the entry of a session holding `c` units frees exactly `c`. -/
theorem total_erase_of_mem {r : ℕ} {c : ℝ} {l : Entries} (hm : (r, c) ∈ l) :
    total (l.erase (r, c)) = total l - c := by
  have h := total_perm (List.perm_cons_erase hm)
  rw [total_cons] at h
  linarith

theorem nonneg_erase (e : ℕ × ℝ) {l : Entries} (h : AllNonneg l) : AllNonneg (l.erase e) :=
  fun x hx => h x (List.erase_sublist.subset hx)

/-- After eviction at most `b` units remain (when `b ≥ 0`). -/
theorem total_evictUntil_le {l : Entries} (h : AllNonneg l) {b : ℝ} (hb : 0 ≤ b) :
    total (evictUntil l b) ≤ b := by
  induction l with
  | nil => simpa [evictUntil]
  | cons e l ih =>
    unfold evictUntil
    split_ifs with hle
    · exact hle
    · exact ih (nonneg_of_cons h)

theorem nonneg_evictUntil {l : Entries} (h : AllNonneg l) (b : ℝ) : AllNonneg (evictUntil l b) := by
  induction l with
  | nil => simpa [evictUntil] using h
  | cons e l ih =>
    unfold evictUntil
    split_ifs
    · exact h
    · exact ih (nonneg_of_cons h)

/-! ### The invariants are preserved -/

theorem Step.nonneg {E : EvictionOrder} {P Q : PoolState} (h : Step E P Q) (hP : P.Nonneg) :
    Q.Nonneg := by
  unfold PoolState.Nonneg at *
  obtain ⟨ha, hc⟩ := hP
  cases h with
  | queue r c hc' => exact ⟨ha, hc⟩
  | admit r c q hq hc' hfit =>
    refine ⟨nonneg_append ha ?_, ?_⟩
    · intro e he
      simp at he
      rw [he]
      exact hc'
    · exact nonneg_evictUntil (nonneg_perm (E.perm _).symm (nonneg_eraseSession r hc)) _
  | free r c ℓ hr hl hlc =>
    refine ⟨nonneg_erase _ ha, nonneg_append (nonneg_eraseSession r hc) ?_⟩
    intro e he
    simp at he
    rw [he]
    exact hl
  | drop r => exact ⟨ha, nonneg_eraseSession r hc⟩
  | leave r => exact ⟨nonneg_eraseSession r ha, nonneg_eraseSession r hc⟩

/-- The memory invariant `|h| + |C| ≤ M` is preserved by every command
(Definition L1:def:semantics, "every reachable configuration satisfies
the memory invariant"). -/
theorem Step.invariant {E : EvictionOrder} {P Q : PoolState} (h : Step E P Q)
    (hP : P.Nonneg) (hI : P.Invariant) : Q.Invariant := by
  unfold PoolState.Nonneg at hP
  obtain ⟨ha, hc⟩ := hP
  unfold PoolState.Invariant at *
  cases h with
  | queue r c hc' => simpa using hI
  | admit r c q hq hc' hfit =>
    simp only [total_append, total_cons, total_nil, add_zero]
    have hb : 0 ≤ P.cap - total P.alloc - c := by linarith
    have hev := total_evictUntil_le
      (nonneg_perm (E.perm _).symm (nonneg_eraseSession r hc)) hb
    linarith
  | free r c ℓ hr hl hlc =>
    simp only [total_append, total_cons, total_nil, add_zero]
    have h1 := total_erase_of_mem hr
    have h2 := total_eraseSession_le r hc
    linarith
  | drop r =>
    have := total_eraseSession_le r hc
    simp only
    linarith
  | leave r =>
    have h1 := total_eraseSession_le r ha
    have h2 := total_eraseSession_le r hc
    simp only
    linarith

/-- The admission guard counts allocated units only: a pool whose cache is
full still admits a request that fits next to the allocations (the cache
is evicted instead). Definition L1:def:semantics, `[Admit]`. -/
theorem admit_guard_units_only (E : EvictionOrder) (P : PoolState) (r : ℕ) (c : ℝ) (q : Entries)
    (hq : P.queue = (r, c) :: q) (hc : 0 ≤ c) (hfit : total P.alloc + c ≤ P.cap) :
    ∃ Q, Step E P Q ∧ Q.alloc = P.alloc ++ [(r, c)] ∧ Q.queue = q :=
  ⟨_, Step.admit P r c q hq hc hfit, rfl, rfl⟩

end SerqLang
