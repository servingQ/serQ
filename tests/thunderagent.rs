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
/// bounds sit below what the run gives at seed 1 (hit 0.63 against 0.05,
/// turns 1.35 times): the claim is the direction, not the value.
#[test]
fn admitting_programs_by_capacity_keeps_their_prefixes_and_serves_more_turns() {
    let run = |tr: &str| run_program("thunderagent", &[tr, "N=32"], Some(1), Some(2000.0));
    let (on, off) = (run("tr=1"), run("tr=0"));
    let hit = |r: &serq::Report| r.observe("hit").unwrap().mean;
    assert!(
        hit(&on) > 0.4 && hit(&off) < 0.1,
        "hit rate: tr {} default {}",
        hit(&on),
        hit(&off)
    );
    assert!(
        on.turns as f64 > 1.3 * off.turns as f64,
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

/// At the paper's scale (`tools/thunderagent/sweep.py --preset paper`, one
/// seed, a shorter run), the two causes `docs/design/thunderagent.md` gives
/// for the paper's larger numbers: a recompute that costs the square of
/// its context (`g`) is what makes default mode lose, and a system prompt
/// every program shares (`S`) is what keeps paused prefixes and gives
/// default mode its hits.
fn paper(tr: &str, more: &[&str]) -> serq::Report {
    let mut sets = vec![
        tr,
        "N=96",
        "blocks=31250",
        "a=1.6e-5",
        "omega=0.015",
        "beta=1.4e-8",
    ];
    sets.extend_from_slice(more);
    run_program("thunderagent", &sets, Some(1), Some(1500.0))
}

fn token_hit(r: &serq::Report) -> f64 {
    let total = |k: &str| {
        let o = r.observe(k).unwrap();
        o.mean * o.count as f64
    };
    total("reused_tokens") / (total("reused_tokens") + total("prefill_tokens"))
}

#[test]
fn the_square_of_a_recompute_is_what_default_mode_loses() {
    let ratio = |g: &str| {
        let (on, off) = (paper("tr=1", &[g]), paper("tr=0", &[g]));
        on.turns as f64 / off.turns as f64
    };
    let (linear, square) = (ratio("g=0"), ratio("g=3.3e-9"));
    // 1.29 and 1.67 at this horizon, with no warm-up; 1.30 and 2.01 in the
    // sweep, which warms up for 500 s of 3 000
    assert!(
        linear < 1.4 && square > 1.25 * linear,
        "ratio: g = 0 {linear}, g = 3.3e-9 {square}"
    );
}

/// The router counts the shared prompt in every program's tokens, and vLLM
/// stores it once: the KV that leaves free keeps paused programs' prefixes.
#[test]
fn a_shared_system_prompt_keeps_paused_prefixes_and_gives_default_mode_its_hits() {
    let hit = |tr: &str, more: &[&str]| {
        let mut sets = vec!["g=3.3e-9"];
        sets.extend_from_slice(more);
        token_hit(&paper(tr, &sets))
    };
    let (alone, shared) = (hit("tr=1", &["S=0"]), hit("tr=1", &["S=3968"]));
    let once = hit("tr=1", &["S=3968", "count_shared=0"]);
    let default = hit("tr=0", &["S=3968"]);
    // the overcount is what keeps them: counted once, the prompt's own
    // hits remain and the paused prefixes are evicted
    assert!(
        alone < 0.75 && shared > 0.85 && once < shared - 0.1 && default > 0.2,
        "TR hit: S 0 {alone}, S 3968 {shared}, counted once {once}; default hit {default}"
    );
}

/// `w`, ThunderAgent's acting weight: a program calling a tool gives back
/// (1 - w) of its tokens and grows them back at its next request, which
/// pauses others. The claim is the mechanism: at `w` 0.5 more of the turns
/// come from paused programs, and fewer turns are served than at the
/// code's default, 1 (docs/design/thunderagent.md, item 6).
#[test]
fn counting_idle_programs_for_less_pauses_more_and_serves_less() {
    let run = |w: &str| paper("tr=1", &["g=3.3e-9", "S=3968", w]);
    let resumed =
        |r: &serq::Report| r.observe("resume_wait").map_or(0, |o| o.count) as f64 / r.turns as f64;
    let (whole, half) = (run("w=1"), run("w=0.5"));
    assert!(
        resumed(&half) > 2.0 * resumed(&whole) && whole.turns > half.turns,
        "resumed/turn: w 1 {}, w 0.5 {}; turns: {} against {}",
        resumed(&whole),
        resumed(&half),
        whole.turns,
        half.turns
    );
}

/// The router lets a paused program in at its scheduler's tick, on the
/// replica with the most room. With a tick of 50 ms a waiting program is
/// let in as soon as room appears, wherever it appears: more of the turns
/// come from paused programs, and fewer turns are served than at the
/// code's 5 s (docs/design/thunderagent.md, item 1).
#[test]
fn a_paused_program_comes_back_at_the_routers_tick() {
    let run = |dt: &str| paper("tr=1", &["g=3.3e-9", "S=3968", dt]);
    let resumed =
        |r: &serq::Report| r.observe("resume_wait").map_or(0, |o| o.count) as f64 / r.turns as f64;
    let (tick, fast) = (run("dt=5"), run("dt=0.05"));
    assert!(
        resumed(&fast) > 1.5 * resumed(&tick) && tick.turns > fast.turns,
        "resumed/turn: dt 5 {}, dt 0.05 {}; turns: {} against {}",
        resumed(&tick),
        resumed(&fast),
        tick.turns,
        fast.turns
    );
}

/// The router's estimate may overshoot the prompt; it counts no more than
/// a replica holds, so a program whose estimate is past the capacity is
/// still let in (at the example's scale `est` 0.2 once ended the run on a
/// lease grown past the cap). The error costs some turns, not most.
#[test]
fn an_estimate_past_the_capacity_counts_the_capacity() {
    let run = |est: &str| {
        run_program(
            "thunderagent",
            &["tr=1", "N=32", est],
            Some(1),
            Some(2000.0),
        )
    };
    let (exact, off) = (run("est=0"), run("est=0.2"));
    assert!(
        off.turns as f64 > 0.8 * exact.turns as f64,
        "turns: exact {}, est 0.2 {}",
        exact.turns,
        off.turns
    );
}
