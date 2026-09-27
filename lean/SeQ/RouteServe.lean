/-
# Serving order of a step engine: admission order is decode-first

A decode-first replica serves prefill "from the token budget the decode
batch leaves": decoding turns first, prefill after. vLLM
v1 serves its running requests in admission order (`scheduler.py`,
`running` list) and gives each `min(remaining, budget left)` tokens. The
two rules coincide whenever every decoding resident precedes every
prefilling one. This module proves that a greedy step preserves that
shape when a prefill may take the whole budget left (no per-request chunk
cap), so that with FIFO admission (new residents are prefilling and join
at the end) admission order *is* decode-first; and that a per-request
chunk cap (`long_prefill_token_threshold`) breaks it.

A resident is its remaining prefill work, `0` for a decoding one. One
greedy step (`serve`) hands a decoding resident one token and a
prefilling one `min(r, left)` (or `min(r, c, left)` with a cap `c`).

Key theorems: `serve_preserves_shape`, `decodeFirst_eq_self`,
`serve_eq_decode_first`, `chunk_cap_breaks_shape`.
-/
import Mathlib.Tactic

namespace SeQ
namespace RouteLang
namespace Serve

/-- Residents in the shape "decoding (0) before prefilling (> 0)". -/
def Shape : List ℕ → Prop
  | [] => True
  | 0 :: l => Shape l
  | (_ + 1) :: l => ∀ x ∈ l, 0 < x

/-- One greedy step with `left` tokens: new remaining work. -/
def serve : ℕ → List ℕ → List ℕ
  | _, [] => []
  | left, 0 :: l => 0 :: serve (left - 1) l
  | left, (r + 1) :: l => (r + 1 - min (r + 1) left) :: serve (left - min (r + 1) left) l

/-- The same step with a per-request cap `c` on a prefill's tokens. -/
def serveCap (c : ℕ) : ℕ → List ℕ → List ℕ
  | _, [] => []
  | left, 0 :: l => 0 :: serveCap c (left - 1) l
  | left, (r + 1) :: l =>
    (r + 1 - min (min (r + 1) c) left) :: serveCap c (left - min (min (r + 1) c) left) l

/-- Decode-first order: decoding residents first, each group in its order. -/
def decodeFirst (l : List ℕ) : List ℕ := l.filter (· = 0) ++ l.filter (· ≠ 0)

theorem serve_zero_left (l : List ℕ) (h : ∀ x ∈ l, 0 < x) : serve 0 l = l := by
  induction l with
  | nil => rfl
  | cons x l ih =>
    obtain ⟨r, rfl⟩ : ∃ r, x = r + 1 := ⟨x - 1, by have := h x (by simp); omega⟩
    simp only [serve, Nat.min_zero, Nat.sub_zero]
    rw [ih fun y hy => h y (by simp [hy])]

theorem serve_pos (left : ℕ) (l : List ℕ) (h : ∀ x ∈ l, 0 < x) :
    Shape (serve left l) := by
  induction l generalizing left with
  | nil => trivial
  | cons x l ih =>
    obtain ⟨r, rfl⟩ : ∃ r, x = r + 1 := ⟨x - 1, by have := h x (by simp); omega⟩
    have hl : ∀ y ∈ l, 0 < y := fun y hy => h y (by simp [hy])
    simp only [serve]
    by_cases hle : r + 1 ≤ left
    · -- the prefill completes: it becomes a decoding resident
      rw [Nat.min_eq_left hle, Nat.sub_self]
      exact ih (left - (r + 1)) hl
    · -- it takes the whole budget: nothing is left for the rest
      push_neg at hle
      rw [Nat.min_eq_right hle.le, Nat.sub_self]
      obtain ⟨k, hk⟩ : ∃ k, r + 1 - left = k + 1 := ⟨r - left, by omega⟩
      rw [hk, serve_zero_left l hl]
      exact hl

/-- A greedy step keeps decoding residents in front of prefilling ones. -/
theorem serve_preserves_shape (left : ℕ) (l : List ℕ) (h : Shape l) : Shape (serve left l) := by
  induction l generalizing left with
  | nil => trivial
  | cons x l ih =>
    cases x with
    | zero => exact ih (left - 1) h
    | succ r => exact serve_pos left ((r + 1) :: l) (by
        intro y hy
        rcases List.mem_cons.mp hy with rfl | hy
        · omega
        · exact h y hy)

theorem decodeFirst_eq_self (l : List ℕ) (h : Shape l) : decodeFirst l = l := by
  induction l with
  | nil => rfl
  | cons x l ih =>
    cases x with
    | zero =>
      have := ih h
      unfold decodeFirst at this ⊢
      simp only [List.filter_cons, decide_true, if_true, ne_eq, not_true_eq_false, decide_false,
        List.cons_append]
      simp only [ne_eq, decide_not] at this
      simpa using this
    | succ r =>
      simp only [Shape] at h
      have h0 : l.filter (· = 0) = [] := by
        rw [List.filter_eq_nil_iff]; intro y hy; have := h y hy; simp; omega
      have h1 : l.filter (· ≠ 0) = l := by
        rw [List.filter_eq_self]; intro y hy; have := h y hy; simp; omega
      unfold decodeFirst
      simp only [List.filter_cons, h0, h1]
      simp

/-- In the shape (which FIFO admission keeps: residents join prefilling,
at the end), serving in admission order *is* serving decode-first. -/
theorem serve_eq_decode_first (left : ℕ) (l : List ℕ) (h : Shape l) :
    serve left l = serve left (decodeFirst l) := by
  rw [decodeFirst_eq_self l h]

/-- Joining at the end as a prefilling resident keeps the shape. -/
theorem shape_append_prefill (l : List ℕ) (r : ℕ) (h : Shape l) : Shape (l ++ [r + 1]) := by
  induction l with
  | nil => simp [Shape]
  | cons x l ih =>
    cases x with
    | zero => exact ih h
    | succ q =>
      simp only [Shape, List.cons_append] at h ⊢
      intro y hy
      rcases List.mem_append.mp hy with hy | hy
      · exact h y hy
      · simp at hy; omega

/-- A per-request cap breaks it: two prefills of 5 and 3 tokens, budget 8,
cap 4: the second finishes (decodes) while the first still prefills. -/
theorem chunk_cap_breaks_shape : Shape [5, 3] ∧ ¬ Shape (serveCap 4 8 [5, 3]) := by
  refine ⟨?_, ?_⟩
  · simp [Shape]
  · -- the step leaves [1, 0]: a prefill ahead of a decode
    have : serveCap 4 8 [5, 3] = [1, 0] := by decide
    rw [this]
    simp [Shape]

end Serve
end RouteLang
end SeQ
