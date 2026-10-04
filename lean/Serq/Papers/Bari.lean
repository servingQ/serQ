/-
# Bari, Hegde, de Veciana: optimal scheduling for LLM inference

The claims of `examples/papers/bari_rad.sq` (arXiv 2508.01002: one inference
node running RAD with `b_col = b_row = b_red = 128`, the batch time (7)
without its attention terms, time in µs), proved about the executable
semantics of that program.

* `token_rate`, Theorem 1: before every iteration the node has served at
  most one token per `t_Lin / b_col + 1 / c_nLin` µs, under any scheduler.
  It is `Exec.served_rate` with the tiled cost: a batch of `b ≤ b_col`
  tokens lasts `t_Lin ⌈b / b_col⌉ + b / c_nLin ≥ b (t_Lin / b_col + 1 / c_nLin)`.
-/
import Serq.Claims

namespace SerqLang
namespace Papers

open Exec

/-- The tiled cost's rate: a batch of `b ≤ bcol` tokens lasts at least
`b (tlin / bcol + tnl)`. -/
theorem tiled_rate (tlin tnl bcol b : ℕ) (hb : b ≤ bcol) (hpos : 0 < bcol) :
    b * (tlin + tnl * bcol) ≤ bcol * max 1 (tlin * ((b + (bcol - 1)) / bcol) + tnl * b) := by
  rcases Nat.eq_zero_or_pos b with rfl | hb0
  · simp
  have hq : 1 ≤ (b + (bcol - 1)) / bcol := (Nat.le_div_iff_mul_le hpos).mpr (by omega)
  calc b * (tlin + tnl * bcol) = b * tlin + bcol * (tnl * b) := by ring
    _ ≤ bcol * tlin + bcol * (tnl * b) := Nat.add_le_add_right (Nat.mul_le_mul_right _ hb) _
    _ ≤ bcol * (tlin * ((b + (bcol - 1)) / bcol)) + bcol * (tnl * b) := by
        apply Nat.add_le_add_right
        apply Nat.mul_le_mul_left
        exact Nat.le_mul_of_pos_right _ hq
    _ = bcol * (tlin * ((b + (bcol - 1)) / bcol) + tnl * b) := by ring
    _ ≤ bcol * max 1 (tlin * ((b + (bcol - 1)) / bcol) + tnl * b) :=
        Nat.mul_le_mul_left _ (le_max_right _ _)

namespace BariRad

theorem token_rate : Claims.BariRad.token_rate :=
  served_rate _ (fun st h => tiled_rate 4000 5 128 st.tokens h (by norm_num)) _ _

end BariRad

end Papers
end SerqLang
