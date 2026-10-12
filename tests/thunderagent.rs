//! ThunderAgent's router (`examples/multi-turn/thunderagent.sq`,
//! `docs/design/thunderagent.md`).
//!
//! The claim the program is there to show: with more agents than the
//! replicas' KV holds, admitting programs against the KV capacity (TR mode)
//! keeps a returning program's prefix in the cache, and the replicas then
//! serve more turns than when every program is let in (default mode), where
//! the caches thrash. The same seed draws the same agents in both modes.

use serq::run_program;

/// 32 agents' contexts are several times the two replicas' 64 000 tokens
/// each: a context averages about 6 000 + 9 × 1 300 ≈ 18 000 tokens. The
/// bounds sit below what the run gives at seed 1 (hit 0.52 against 0.015,
/// turns 1.24 times): the claim is the direction, not the value.
#[test]
fn admitting_programs_by_capacity_keeps_their_prefixes_and_serves_more_turns() {
    let run = |tr: &str| run_program("thunderagent", &[tr, "N=32"], Some(1), Some(2000.0));
    let (on, off) = (run("tr=1"), run("tr=0"));
    let hit = |r: &serq::Report| r.observe("hit").unwrap().mean;
    assert!(
        hit(&on) > 0.4 && hit(&off) < 0.05,
        "hit rate: tr {} default {}",
        hit(&on),
        hit(&off)
    );
    assert!(
        on.turns as f64 > 1.1 * off.turns as f64,
        "turns: tr {} default {}",
        on.turns,
        off.turns
    );
}

#[test]
fn with_room_for_every_agent_the_two_modes_serve_alike() {
    // Four agents, two to a replica, hold about 2 × 18 000 = 36 000 of its
    // 64 000 tokens: TR mode lets every one in and has nothing to hold back.
    let run = |tr: &str| run_program("thunderagent", &[tr, "N=4"], Some(1), Some(2000.0));
    let (on, off) = (run("tr=1"), run("tr=0"));
    let ratio = on.turns as f64 / off.turns as f64;
    assert!(
        (0.9..1.1).contains(&ratio),
        "turns: tr {} default {}",
        on.turns,
        off.turns
    );
}
