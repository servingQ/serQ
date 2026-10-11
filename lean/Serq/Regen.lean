/-
# Dead cache entries do not change what a live session sees

The core of the regeneration conjecture of `docs/design/stochastic-model.md`
(Proposition 5): at a configuration with no live session, every cache entry
belongs to an ended session, and every entry a future session releases is
younger than all of them. Under LRU such *dead* entries are evicted before
any *live* one, so the live entries that an eviction removes — and the
tail blocks of the last one — are the same whether or not the dead entries
are there. This module proves that statement for one pool's eviction loop
(`Exec.makeRoom`):

* `makeRoom_fuel`: given more fuel than cached units, the loop's result
  does not depend on the fuel (it stops at the same point);
* `makeRoom_dead_irrelevant`: for a set of owners `dead` whose entries all
  come strictly before every other entry, the live entries left by
  `makeRoom` on `s` are exactly the entries left by `makeRoom` on `s`
  without its dead entries.

What is not proved here: that the rest of the kernel (admission, release,
the step engine) reads nothing of the dead entries but through `makeRoom`,
which is what lifts this lemma to Proposition 5's path statement.

Key theorems: `makeRoom_fuel`, `makeRoom_dead_irrelevant`.
-/
import Serq.Exec

namespace SerqLang
namespace Exec

/-- `d` comes strictly before `g` in eviction order (an older release, or
the same release time and an earlier release number). -/
def Entry.strictBefore (d g : Entry) : Prop :=
  d.last < g.last ∨ (d.last = g.last ∧ d.seq < g.seq)

theorem Entry.not_before_of_strictBefore {d g : Entry} (h : d.strictBefore g) : ¬ g.before d := by
  unfold Entry.strictBefore at h; unfold Entry.before; omega

/-- The entries of `s` whose owner is not in `dead`. -/
def liveEntries (dead : List ℕ) (s : PoolSt) : List Entry :=
  s.entries.filter fun e => decide (e.owner ∉ dead)

/-- `s` without its dead entries. -/
def live (dead : List ℕ) (s : PoolSt) : PoolSt := { s with entries := liveEntries dead s }

/-- Every dead entry of `s` comes strictly before every live one. -/
def DeadFirst (dead : List ℕ) (s : PoolSt) : Prop :=
  ∀ d ∈ s.entries, d.owner ∈ dead → ∀ g ∈ s.entries, g.owner ∉ dead → d.strictBefore g

/-! ### What one eviction does to the shape of the entries -/

theorem evictOne_holders (b : ℕ) (s : PoolSt) : (evictOne b s).holders = s.holders := by
  unfold evictOne; split <;> rfl

theorem evictOne_queue (b : ℕ) (s : PoolSt) : (evictOne b s).queue = s.queue := by
  unfold evictOne; split <;> rfl

/-- Every entry after an eviction is an entry of before, possibly shrunk:
same owner, release time and release number. -/
theorem evictOne_shape (b : ℕ) (s : PoolSt) :
    ∀ g ∈ (evictOne b s).entries,
      ∃ g0 ∈ s.entries, g.owner = g0.owner ∧ g.last = g0.last ∧ g.seq = g0.seq := by
  intro g hg
  unfold evictOne at hg
  cases hl : lru s.entries with
  | none => rw [hl] at hg; exact ⟨g, hg, rfl, rfl, rfl⟩
  | some e =>
    rw [hl] at hg
    simp only at hg
    split_ifs at hg with h0
    · exact ⟨g, List.mem_of_mem_filter hg, rfl, rfl, rfl⟩
    · rcases List.mem_cons.mp hg with rfl | hg
      · exact ⟨e, lru_mem hl, rfl, rfl, rfl⟩
      · exact ⟨g, List.mem_of_mem_filter hg, rfl, rfl, rfl⟩

theorem makeRoom_shape (b cap need : ℕ) :
    ∀ (f : ℕ) (s : PoolSt) (g : Entry), g ∈ (makeRoom b cap need f s).entries →
      ∃ g0 ∈ s.entries, g.owner = g0.owner ∧ g.last = g0.last ∧ g.seq = g0.seq
  | 0, s, g, hg => ⟨g, hg, rfl, rfl, rfl⟩
  | f + 1, s, g, hg => by
    unfold makeRoom at hg
    split_ifs at hg with hc
    · exact ⟨g, hg, rfl, rfl, rfl⟩
    · obtain ⟨g1, hg1, h1, h2, h3⟩ := makeRoom_shape b cap need f (evictOne b s) g hg
      obtain ⟨g0, hg0, k1, k2, k3⟩ := evictOne_shape b s g1 hg1
      exact ⟨g0, hg0, h1.trans k1, h2.trans k2, h3.trans k3⟩

theorem DeadFirst.evictOne {dead : List ℕ} {s : PoolSt} (h : DeadFirst dead s) (b : ℕ) :
    DeadFirst dead (evictOne b s) := by
  intro d hd hdo g hg hgo
  obtain ⟨d0, hd0, d1, d2, d3⟩ := evictOne_shape b s d hd
  obtain ⟨g0, hg0, g1, g2, g3⟩ := evictOne_shape b s g hg
  have := h d0 hd0 (d1 ▸ hdo) g0 hg0 (g1 ▸ hgo)
  unfold Entry.strictBefore at *
  omega

/-! ### The fuel does not matter once there is enough of it -/

theorem makeRoom_fuel (b cap need : ℕ) :
    ∀ (f g : ℕ) (s : PoolSt), cachedTotal s < f → cachedTotal s < g →
      (∀ e ∈ s.entries, 0 < e.size) →
      makeRoom b cap need f s = makeRoom b cap need g s
  | 0, _, _, hf, _, _ => by omega
  | _ + 1, 0, _, _, hg, _ => by omega
  | f + 1, g + 1, s, hf, hg, hpos => by
    unfold makeRoom
    split_ifs with hc
    · rfl
    · push_neg at hc
      have hlt := evictOne_lt b s hc.2 hpos
      exact makeRoom_fuel b cap need f g (evictOne b s) (by omega) (by omega)
        (evictOne_pos b s hpos)

/-! ### Dead entries are evicted first and leave the live ones alone -/

/-- Evicting from a dead entry does not change the live entries. -/
theorem liveEntries_evictOne_dead {dead : List ℕ} {s : PoolSt} {e : Entry} (b : ℕ)
    (hl : lru s.entries = some e) (he : e.owner ∈ dead) :
    liveEntries dead (evictOne b s) = liveEntries dead s := by
  have hsub : ∀ l : List Entry,
      (l.filter fun f => ¬ (f.owner = e.owner ∧ f.seq = e.seq)).filter
        (fun g => decide (g.owner ∉ dead)) = l.filter fun g => decide (g.owner ∉ dead) := by
    intro l
    rw [List.filter_filter]
    apply List.filter_congr
    intro g _
    by_cases hg : g.owner ∈ dead
    · simp [hg]
    · have : ¬ (g.owner = e.owner ∧ g.seq = e.seq) := fun h => hg (h.1 ▸ he)
      simp [hg, this]
  unfold liveEntries evictOne
  rw [hl]
  simp only
  split_ifs with h0
  · exact hsub _
  · rw [List.filter_cons_of_neg (by simpa using he)]
    exact hsub _

/-- If the least recently released entry is live, no entry is dead. -/
theorem no_dead_of_lru_live {dead : List ℕ} {s : PoolSt} {e : Entry}
    (h : DeadFirst dead s) (hl : lru s.entries = some e) (he : e.owner ∉ dead) :
    ∀ g ∈ s.entries, g.owner ∉ dead := by
  intro g hg hgd
  have h1 := h g hg hgd e (lru_mem hl) he
  have h2 := lru_before hl g hg
  exact Entry.not_before_of_strictBefore h1 h2

theorem liveEntries_eq_self {dead : List ℕ} {s : PoolSt} (h : ∀ g ∈ s.entries, g.owner ∉ dead) :
    liveEntries dead s = s.entries := by
  unfold liveEntries
  exact List.filter_eq_self.mpr fun g hg => by simpa using h g hg

theorem cachedTotal_live_le (dead : List ℕ) (s : PoolSt) :
    cachedTotal (live dead s) ≤ cachedTotal s :=
  sum_filter_le _ s.entries

theorem live_pos {dead : List ℕ} {s : PoolSt} (hpos : ∀ e ∈ s.entries, 0 < e.size) :
    ∀ e ∈ (live dead s).entries, 0 < e.size :=
  fun e he => hpos e (List.mem_of_mem_filter he)

/-- **Dead entries are irrelevant to the live ones.** With more fuel than
cached units, positive entries and every dead entry strictly before every
live one, the live entries `makeRoom` leaves on `s` are the entries it
leaves on `s` without its dead entries. -/
theorem makeRoom_dead_irrelevant (b cap need : ℕ) (dead : List ℕ) :
    ∀ (f : ℕ) (s : PoolSt), cachedTotal s < f → (∀ e ∈ s.entries, 0 < e.size) →
      DeadFirst dead s →
      liveEntries dead (makeRoom b cap need f s)
        = (makeRoom b cap need f (live dead s)).entries
  | 0, _, hf, _, _ => by omega
  | f + 1, s, hf, hpos, hdead => by
    by_cases hc : s.used + cachedTotal s + need ≤ cap ∨ s.entries = []
    · -- nothing is evicted on either side
      have hc' : (live dead s).used + cachedTotal (live dead s) + need ≤ cap
          ∨ (live dead s).entries = [] := by
        rcases hc with hc | hc
        · left
          have := cachedTotal_live_le dead s
          show s.used + cachedTotal (live dead s) + need ≤ cap
          omega
        · right
          show liveEntries dead s = []
          unfold liveEntries; rw [hc]; rfl
      rw [makeRoom, if_pos hc, makeRoom, if_pos hc']
      rfl
    · have hne : s.entries ≠ [] := fun h => hc (Or.inr h)
      obtain ⟨e, hl⟩ := lru_some hne
      by_cases he : e.owner ∈ dead
      · -- the eviction takes a dead block: the live entries do not move
        have hlive : liveEntries dead (evictOne b s) = liveEntries dead s :=
          liveEntries_evictOne_dead b hl he
        have hlt := evictOne_lt b s hne hpos
        have ih := makeRoom_dead_irrelevant b cap need dead f (evictOne b s) (by omega)
          (evictOne_pos b s hpos) (hdead.evictOne b)
        have hlive' : live dead (evictOne b s) = live dead s := by
          unfold live
          rw [hlive]
          cases s with
          | mk used entries holders queue =>
            simp only [evictOne_used, evictOne_holders, evictOne_queue]
        rw [makeRoom, if_neg hc, ih, hlive']
        -- the right-hand side has one more unit of fuel than it needs
        have hsum : cachedTotal (live dead s) ≤ cachedTotal (evictOne b s) := by
          rw [← hlive']
          exact cachedTotal_live_le dead (evictOne b s)
        exact congrArg PoolSt.entries
          (makeRoom_fuel b cap need f (f + 1) (live dead s) (by omega) (by omega) (live_pos hpos))
      · -- the least recently released entry is live: there are no dead entries
        have hnone := no_dead_of_lru_live hdead hl he
        have hself : live dead s = s := by
          unfold live
          rw [liveEntries_eq_self hnone]
        rw [hself]
        apply liveEntries_eq_self
        intro g hg hgd
        obtain ⟨g0, hg0, h1, _, _⟩ := makeRoom_shape b cap need (f + 1) s g hg
        exact hnone g0 hg0 (h1 ▸ hgd)

end Exec
end SerqLang
