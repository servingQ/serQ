//! Tests for `serq draw`: the projection, the geometry and the writers.
//!
//! The assertions are on the `Figure` and on the projected `Net`, not on the
//! rendered bytes, for the same reason `tsncd` unit-tests its geometry rather
//! than its images: a coordinate that moved is a diagnosable failure, and a
//! diff of two SVGs is not. The golden files at the bottom guard the writers.

mod common;

use serq::ir::Program;
use serq::view::deployment::{self, End};
use serq::view::figure::{BoxStyle, Figure, StationKind};
use serq::{compile_source, program_path};

const PROGRAMS: [&str; 10] = [
    "mg1",
    "ps",
    "closed",
    "replica",
    "routing",
    "vllm",
    "vllm_request",
    "llmd_nixl_pull",
    "vllm_nixl_push",
    "thunderagent",
];

fn program(name: &str) -> Program {
    example(&program_path(name))
}

/// The program at `path` with the workload `examples/pairs.json` runs it
/// with (`docs/design/paired-workload.md`).
fn example(path: &std::path::Path) -> Program {
    let ov = common::horizon(10.0);
    let src = common::main_source(&std::fs::read_to_string(path).unwrap());
    match serq::paired_workload(path) {
        Some(w) => common::compile(&src, &std::fs::read_to_string(w).unwrap(), &ov),
        None => compile_source(&src, &ov),
    }
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn stage(p: &Program, name: &str) -> usize {
    p.stage_index(name)
        .unwrap_or_else(|| panic!("no stage {name}"))
}

fn pool(p: &Program, name: &str) -> usize {
    p.pool_index(name)
        .unwrap_or_else(|| panic!("no pool {name}"))
}

fn pools_of(p: &Program, net: &deployment::Net, stage_name: &str) -> Vec<String> {
    let i = net
        .node_of(stage(p, stage_name))
        .expect("stage is on the session");
    net.nodes[i]
        .pools
        .iter()
        .map(|&x| p.pools[x].name.clone())
        .collect()
}

/// Every text the figure of `name` writes.
fn texts(name: &str) -> Vec<String> {
    deployment::figure(&program(name))
        .items
        .into_iter()
        .filter_map(|i| match i {
            serq::view::figure::Item::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

/// A step stage is drawn in an engine's words, whichever form the program
/// is written in (#413): `llmd_nixl_pull.sq`'s engines are drawn with
/// `tokens cap`; a pool an engine admits is on it, its memory on its
/// device, and a memory it also admits (`D.kv`, `on D.gpu`) says so among
/// its options.
#[test]
fn a_step_stage_is_drawn_in_an_engines_words() {
    let t = texts("llmd_nixl_pull");
    let has = |s: &str| t.iter().any(|x| x == s);
    for s in [
        "tokens cap 8192",
        "pool P.reqs on P",
        "pool D.reqs on D",
        "pool P.kv on P's device",
        "pool D.kv on D's device",
    ] {
        assert!(has(s), "no {s:?} in {t:?}");
    }
    // `D.kv`, once in each of the two decoders' boxes
    let admitted: Vec<&String> = t.iter().filter(|x| x.contains("admitted by")).collect();
    assert_eq!(admitted.len(), 2, "{admitted:?}");
    assert!(
        admitted.iter().all(|a| a.ends_with("admitted by D")),
        "{admitted:?}"
    );
    assert!(
        !t.iter()
            .any(|x| x.contains("admit via") || x.starts_with("budget"))
    );
}

// --- the station kinds -----------------------------------------------------

/// Each stage kind has its glyph: `mg1.sq` is one FIFO server, `ps.sq` one
/// processor-sharing server, `llmd_nixl_pull.sq` has a step engine on each
/// side and a processor-sharing NIC on each, and a delay stage is a delay.
#[test]
fn stations_take_their_stage_kind() {
    let kinds = |name: &str| -> Vec<StationKind> {
        let p = program(name);
        deployment::project(&p)
            .nodes
            .iter()
            .map(|n| n.kind)
            .collect()
    };
    assert_eq!(kinds("mg1"), [StationKind::Fifo]);
    assert_eq!(kinds("ps"), [StationKind::Ps]);
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    for (name, kind) in [
        ("P", StationKind::Step),
        ("D", StationKind::Step),
        ("P.nic", StationKind::Ps),
        ("D.nic", StationKind::Ps),
    ] {
        let i = net
            .node_of(stage(&p, name))
            .expect("stage is on the session");
        assert_eq!(net.nodes[i].kind, kind, "{name}");
    }
    let (p, net) = shape("run A (cost(A, 1)); end;");
    let i = net.node_of(stage(&p, "A")).expect("A is on the session");
    assert_eq!(net.nodes[i].kind, StationKind::Delay);
}

// --- the projection ---------------------------------------------------------

/// Two runs at the same stage in a row are two visits, not a flow between
/// stations: `vllm.sq` prefills and decodes at one engine.
#[test]
fn no_self_edges() {
    for name in PROGRAMS {
        let p = program(name);
        let net = deployment::project(&p);
        for e in &net.edges {
            assert_ne!(e.from, e.to, "{name}: self edge at {:?}", e.from);
        }
    }
}

/// Every station of every program's figure has an arrow in (the arrival, or
/// a flow to it) and an arrow out (a flow from it, or an exit). A station no
/// request reaches does not link (#617), and a request that reaches one
/// goes on from it; the figure shows both. A run over two NICs
/// (`vllm_nixl_push.sq`, whose NICs are in no instance) was a bracket with
/// no arrow between its stations, so the source's NIC had none out and the
/// reader's none in.
#[test]
fn every_station_has_an_arrow_in_and_out() {
    let mut files = vec![];
    let mut dirs = vec![
        std::path::PathBuf::from("examples"),
        "docs/tutorial/programs".into(),
    ];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let path = e.unwrap().path();
            // an instance file sets a program's inputs, a workload's file is
            // the client a program runs with, and half of a pair is a
            // program only with its other half (below): no programs
            if path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|n| n != "instances" && n != "workloads" && n != "paired")
            {
                dirs.push(path);
            } else if path.extension().is_some_and(|x| x == "sq") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(files.len() > 30, "found {} programs", files.len());
    for path in files {
        let p = example(&path);
        if let Some((node, which)) = deployment::figure(&p).unjoined() {
            panic!("{}: station {node} has no arrow {which}", path.display());
        }
        deployment::checked_figure(&p).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    // every deployment under every workload (docs/design/paired-workload.md)
    let sq = |d: &str| {
        let mut v: Vec<_> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "sq"))
            .collect();
        v.sort();
        v
    };
    for d in sq("examples/paired/deployments") {
        for w in sq("examples/paired/workloads") {
            let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap();
            let p = serq::compile_paired(&read(&d), &read(&w), ("d", "w"), &common::horizon(10.0))
                .unwrap_or_else(|e| panic!("{} with {}: {e}", d.display(), w.display()));
            deployment::checked_figure(&p)
                .unwrap_or_else(|e| panic!("{} with {}: {e}", d.display(), w.display()));
        }
    }
}

/// `serq check` and `serq draw` refuse a figure with a station cut off,
/// naming it: a figure with a station and nothing going out of it.
#[test]
fn a_station_with_no_arrow_out_is_named() {
    use serq::view::figure::{Item, StationKind, pt};
    let mut f = Figure {
        width: 100.0,
        height: 40.0,
        title: String::new(),
        items: vec![],
    };
    f.push(Item::Station {
        rect: serq::view::figure::Rect::new(40.0, 10.0, 20.0, 20.0),
        kind: StationKind::Ps,
        text: "PS".into(),
        stage: Some("E.nic".into()),
        node: "E.nic".into(),
    });
    f.arrival(vec![pt(0.0, 20.0), pt(40.0, 20.0)], "E.nic".into());
    assert_eq!(f.unjoined(), Some(("E.nic".to_string(), "out")));
    f.exit(vec![pt(60.0, 20.0), pt(100.0, 20.0)], "E.nic".into());
    assert_eq!(f.unjoined(), None);
}

/// A chain of guards that moves nobody must not multiply the paths out.
/// `routing.sq` has five sibling `if (policy == k)` blocks.
#[test]
fn guard_chains_do_not_multiply_edges() {
    let p = program("routing");
    let net = deployment::project(&p);
    assert_eq!(net.nodes.len(), 3, "the decision, link, rep[j]");
    assert!(
        net.edges.len() <= 8,
        "{} edges is a blow-up",
        net.edges.len()
    );
    for e in &net.edges {
        let len = e.label.as_ref().map_or(0, String::len);
        assert!(len < 40, "unreadable edge label: {:?}", e.label);
    }
}

/// A `choose` names an index, so it belongs to the station whose reference
/// reads that attribute - `rep[j]`, not whatever station comes next.
#[test]
fn choose_annotates_the_station_it_selects() {
    let p = program("routing");
    let net = deployment::project(&p);
    let rep = net.node_of(stage(&p, "rep")).unwrap();
    assert!(
        net.nodes[rep]
            .note
            .as_deref()
            .is_some_and(|n| n.contains("choose j"))
    );
    let link = net.node_of(stage(&p, "link")).unwrap();
    assert!(net.nodes[link].note.is_none(), "the link is not chosen");
}

/// The deployment `src`, with the workload's file `workload`.
fn compile(src: &str, workload: &str) -> Program {
    common::compile(&common::main_source(src), workload, &common::horizon(10.0))
        .expect("the fixture compiles")
}

/// `grow` advances the *innermost* hold holding the pool, so a `growing` run
/// nested inside another hold still belongs to the outer one. Walking only
/// `Run`/`Branch`/`Loop` misses it and the figure claims a prefix cache on
/// pools the interpreter never caches in. The nested hold is written as a
/// queue's entry: an engine's request holds only the pools it takes.
#[test]
fn growing_is_found_through_nested_holds() {
    let p = compile(
        r#"
        queue llm : prefill {
          device gpu { kv cap 100000; }
          pool kv on gpu { } pool reqs { cap 8; } pool gate { cap 4; }
          engine on gpu { tokens cap 512; scheduler { advance running; admit waiting while (running.preempted == 0); } worker { forward (1e-3); } }
          prefill (n) {
            hold kv (cost(kv, 32)), reqs (cost(reqs, 1)) {
              hold gate (cost(gate, 1)) { run llm prefill (cost(llm, n)) growing kv; }
            } cache (cost(kv, reqs, n + o));
          }
        }
        server (n, hidden o) { llm.prefill (n); }

"#,
        "workload { arrive poisson(0.2); turn { set n = 100; set o = 2; } send (n, o);
          session { turn; end; } }",
    );
    let net = deployment::project(&p);
    assert_eq!(
        net.cached,
        vec![pool(&p, "llm.kv")],
        "reqs is never cached in"
    );
}

/// A pool held around two stations with an unheld one between them gets one
/// enclosure per run, not one box swallowing the station in the middle.
#[test]
fn disjoint_holds_of_one_pool_get_separate_enclosures() {
    let p = compile(
        r#"
        pool kv { cap 100; }
        stage s1 : fifo; stage s2 : fifo; stage s3 : delay; stage s4 : fifo; stage s5 : fifo;
        server {
          hold kv (cost(kv, 1)) { run s1 (cost(s1, 1)); run s2 (cost(s2, 1)); }
          run s3 (cost(s3, 1));
          hold kv (cost(kv, 1)) { run s4 (cost(s4, 1)); run s5 (cost(s5, 1)); }
        }

"#,
        POISSON_SLOW,
    );
    let net = deployment::project(&p);
    let f = deployment::layout(&p, &net);
    assert_eq!(f.boxes(BoxStyle::Enclosure).len(), 2);
    // and the station in the middle is in neither
    let middle = f.stations()[2].0;
    assert!(
        f.boxes(BoxStyle::Enclosure)
            .iter()
            .all(|b| !b.contains(&middle))
    );
}

/// A workload of single requests, one every five seconds.
const POISSON_SLOW: &str = "workload { arrive poisson(0.2); session { turn; end; } }";

/// A workload of single requests, one a second.
const POISSON: &str = "workload { arrive poisson(1); session { turn; end; } }";

/// One request.
const ONE: &str = "workload { arrive batch(1); session { turn; end; } }";

/// A name with the member indices a stacked instance writes taken out:
/// `P.nic[1]` is `P.nic`.
fn family(name: &str) -> String {
    let mut out = String::new();
    let mut rest = name;
    while let Some(k) = rest.find('[') {
        let close = rest[k..].find(']').map(|c| k + c);
        match close {
            Some(c) if c > k + 1 && rest[k + 1..c].bytes().all(|b| b.is_ascii_digit()) => {
                out.push_str(&rest[..k]);
                rest = &rest[c + 1..];
            }
            _ => {
                out.push_str(&rest[..=k]);
                rest = &rest[k + 1..];
            }
        }
    }
    out + rest
}

/// Feedback edges, early exits and second entry points all need a lane below
/// the station row, and must not be given the same one.
#[test]
fn lanes_below_the_row_do_not_collide() {
    for name in PROGRAMS {
        let p = program(name);
        let f = deployment::figure(&p);
        // an arrow drawn for each member of a stacked instance is one arrow
        // (`P[0]`, `P[1]` are `P`'s), and its members share its lane
        let mut keyed: Vec<(String, f64)> = f
            .items
            .iter()
            .filter_map(|i| match i {
                serq::view::figure::Item::Edge { pts, role, .. } if pts.len() >= 3 => {
                    Some((family(&format!("{role:?}")), pts[1].y))
                }
                _ => None,
            })
            .collect();
        keyed.dedup();
        let mut ys: Vec<f64> = keyed.into_iter().map(|(_, y)| y).collect();
        ys.sort_by(f64::total_cmp);
        for w in ys.windows(2) {
            assert!(w[1] - w[0] > 1.0 || w[1] == w[0], "{name}: lanes at {w:?}");
        }
        let unique = {
            let mut v = ys.clone();
            v.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            v.len()
        };
        assert_eq!(unique, ys.len(), "{name}: two routed edges share a lane");
    }
}

/// A session that opens with a branch decides before its first station, as
/// a loop's body can: it starts at one decision, which sends it to either,
/// so one arrival arrow and one arrival label are drawn.
#[test]
fn a_session_that_decides_first_starts_at_a_decision() {
    let p = compile(
        r#"
        stage s1 : fifo; stage s2 : fifo;
        server (a) { if (a) { run s1 (cost(s1, 1)); } else { run s2 (cost(s2, 1)); }
        }

"#,
        "workload { arrive poisson(1); init { set a = 1; } send (a); session { turn; end; } }",
    );
    let net = deployment::project(&p);
    let d = decision(&net).expect("a decision");
    let arrivals: Vec<_> = net
        .edges
        .iter()
        .filter(|e| e.from == End::Arrival)
        .collect();
    assert_eq!(arrivals.len(), 1);
    assert_eq!(arrivals[0].to, d);
    for st in ["s1", "s2"] {
        assert!(has(&net, &p, d, st).is_some(), "{st}");
    }
    let f = deployment::layout(&p, &net);
    let labels = f
        .items
        .iter()
        .filter(|i| matches!(i, serq::view::figure::Item::Text { text, .. } if text.starts_with("new sessions")))
        .count();
    assert_eq!(labels, 1, "the arrival label is drawn once");
}

/// `pow()` parses `atom() '^' unary()`, so a `let` folded to a negative value
/// has to be bracketed on the left of `^` or the output re-parses as a
/// different tree.
#[test]
fn negative_constants_reparse() {
    let p = compile(
        r#"
        let k = 0 - 2;
        stage s : fifo;
        server (a) { observe o = k ^ a; run s (cost(s, 1));
        }

"#,
        "workload { arrive batch(1); turn { set a = 2; } send (a); session { turn; end; } }",
    );
    let printed = p
        .blocks
        .iter()
        .flatten()
        .find_map(|s| match s {
            serq::ir::CStmt::Observe(_, e) => Some(p.show_expr(e)),
            _ => None,
        })
        .expect("the observe is there");
    assert_eq!(printed, "(-2) ^ a");
    // and it means what it says
    let round = serq::frontend::parser::parse_expr(&printed).expect("re-parses");
    assert!(matches!(
        round,
        serq::frontend::ast::Expr::Binary(serq::frontend::ast::BinOp::Pow, ..)
    ));
}

/// `release_hold` caches the growing run's position when a hold grew, and the
/// allocation otherwise, so a hold with `growing` caches in that pool alone.
/// `replica.sq` has no `growing` and really does keep a unit of `reqs`.
#[test]
fn cache_targets_follow_the_release_rule() {
    let p = program("vllm");
    let net = deployment::project(&p);
    assert_eq!(net.cached, vec![pool(&p, "kv")], "growing kv, so not reqs");

    let p = program("replica");
    let net = deployment::project(&p);
    let mut cached = net.cached.clone();
    cached.sort_unstable();
    let mut want = vec![pool(&p, "reqs"), pool(&p, "kv")];
    want.sort_unstable();
    assert_eq!(cached, want);
}

/// A pool held across two stations, and one held inside it at the first.
const ACROSS: &str = "pool live { cap 2; } pool kv { cap 9; }
        stage A : fifo; stage B : fifo;
        server { hold live (cost(live, 1)) { hold kv (cost(kv, 1)) { run A (cost(A, 1)); } run B (cost(B, 1)); }
        }
        ";

/// Pools held around every visit to a stage, and only those.
#[test]
fn nested_holds_nest() {
    let p = program("replica");
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "llm"), ["live", "reqs", "kv"]);
    let p = compile(ACROSS, POISSON);
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "A"), ["live", "kv"]);
    assert_eq!(pools_of(&p, &net, "B"), ["live"], "outside the inner hold");
}

/// A pool held at one station alone is that station's: drawn in its frame,
/// not in a dashed box. "Alone" is what the program's holds reach, not
/// what stands beside the station in the row.
#[test]
fn pools_held_at_one_station_are_drawn_in_it() {
    let queues = |f: &Figure| {
        f.items
            .iter()
            .filter(|i| matches!(i, serq::view::figure::Item::Queue { .. }))
            .count()
    };
    let p = program("vllm");
    let net = deployment::project(&p);
    let engine = net.node_of(stage(&p, "vllm")).unwrap();
    assert_eq!(
        net.resident_pools(engine),
        [pool(&p, "reqs"), pool(&p, "kv")]
    );
    let f = deployment::layout(&p, &net);
    assert_eq!(f.boxes(BoxStyle::Enclosure).len(), 0);
    assert_eq!(f.boxes(BoxStyle::Frame).len(), 1);
    assert_eq!(queues(&f), 0, "a frame draws no queue");

    // `replica.sq` serves a request inside the workload's `live`, and holds
    // its `reqs` and `kv` around the engine: all three at the engine alone
    let p = program("replica");
    let net = deployment::project(&p);
    let engine = net.node_of(stage(&p, "llm")).unwrap();
    assert_eq!(
        net.resident_pools(engine),
        [pool(&p, "live"), pool(&p, "reqs"), pool(&p, "kv")]
    );
    let f = deployment::layout(&p, &net);
    assert_eq!(f.boxes(BoxStyle::Enclosure).len(), 0);

    // `live` held across two stations is a box, and the hold that waits in
    // it does so ahead of the box; `kv`, at `A` alone, is `A`'s
    let p = compile(ACROSS, POISSON);
    let net = deployment::project(&p);
    let a = net.node_of(stage(&p, "A")).unwrap();
    assert_eq!(net.resident_pools(a), [pool(&p, "kv")]);
    let f = deployment::layout(&p, &net);
    assert_eq!(f.boxes(BoxStyle::Enclosure).len(), 1, "live");
    assert_eq!(queues(&f), 1, "live's");

    // the prefiller's KV stays leased through the read and the decoder's is
    // taken before it, but a read between instances is drawn as the arrow
    // between them (`P.kv[i] → D.kv[j]`): each KV is its engine's
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    let prefill = net.node_of(stage(&p, "P")).unwrap();
    let decode = net.node_of(stage(&p, "D")).unwrap();
    assert_eq!(
        net.resident_pools(prefill),
        [pool(&p, "P.reqs"), pool(&p, "P.kv")]
    );
    assert_eq!(
        net.resident_pools(decode),
        [pool(&p, "D.kv"), pool(&p, "D.reqs")]
    );
    let f = deployment::layout(&p, &net);
    assert_eq!(f.boxes(BoxStyle::Enclosure).len(), 0);
}

/// Two holds of one pool at stations side by side are two frames, not one
/// box that reads as one hold across both.
#[test]
fn separate_holds_side_by_side_are_two_frames() {
    let p = compile(
        r#"
        pool a { cap 10; } stage s1 : fifo; stage s2 : fifo;
        server { hold a (cost(a, 1)) { run s1 (cost(s1, 1)); } hold a (cost(a, 1)) { run s2 (cost(s2, 1)); }
        }

"#,
        POISSON_SLOW,
    );
    let net = deployment::project(&p);
    assert_eq!(net.resident_pools(0), [pool(&p, "a")]);
    assert_eq!(net.resident_pools(1), [pool(&p, "a")]);
    let f = deployment::layout(&p, &net);
    assert_eq!(f.boxes(BoxStyle::Enclosure).len(), 0);
    assert_eq!(f.boxes(BoxStyle::Frame).len(), 2);
}

/// A hold that reaches other stations is not one station's, even where the
/// figure draws the pool at that station only: `s1` and `s2` are also
/// visited without it, so the pool is drawn at `s3` alone.
#[test]
fn a_hold_across_stations_is_no_stations_own() {
    let p = compile(
        r#"
        pool a { cap 10; } stage s1 : fifo; stage s2 : fifo; stage s3 : fifo;
        server { run s1 (cost(s1, 1)); run s2 (cost(s2, 1)); hold a (cost(a, 1)) { run s1 (cost(s1, 1)); run s3 (cost(s3, 1)); run s2 (cost(s2, 1)); }
        }

"#,
        POISSON_SLOW,
    );
    let net = deployment::project(&p);
    let s3 = net.node_of(stage(&p, "s3")).unwrap();
    assert_eq!(net.drawn_pools(s3), [pool(&p, "a")]);
    assert!(net.resident_pools(s3).is_empty());
}

// --- geometry ---------------------------------------------------------------

fn no_overlapping_enclosures(f: &Figure, name: &str) {
    let boxes = f.boxes(BoxStyle::Enclosure);
    let stations = f.stations();
    for (i, a) in boxes.iter().enumerate() {
        for b in boxes.iter().skip(i + 1) {
            let nested = a.contains(b) || b.contains(a);
            // two enclosures may cross where a station sits in both: the
            // program holds both pools there (a KV transfer between
            // instances holds the source until the link run is done and the
            // destination from before it)
            let shared = stations.iter().any(|(r, _)| a.contains(r) && b.contains(r));
            assert!(
                !a.overlaps(b) || nested || shared,
                "{name}: enclosures cross: {a:?} {b:?}"
            );
        }
    }
}

/// A `release` in one arm of a branch does not reach the other, and after
/// the branch the pool encloses a station only if both arms still hold it.
#[test]
fn a_release_in_one_arm_does_not_reach_the_other() {
    let p = compile(
        "pool p { cap 10; } stage s1 : delay; stage s2 : delay; stage s3 : delay;
        server (c) { hold p (cost(p, 1)) { if (c) { release p; run s1 (cost(s1, 1)); } else { run s2 (cost(s2, 1)); } run s3 (cost(s3, 1)); }
        }
        ",
        "workload { arrive batch(1); init { set c = 1; } send (c); session { turn; end; } }",
    );
    let net = deployment::project(&p);
    assert!(pools_of(&p, &net, "s1").is_empty());
    assert_eq!(pools_of(&p, &net, "s2"), ["p"]);
    assert!(pools_of(&p, &net, "s3").is_empty());
}

/// A leased pool stays on the stations after its hold, until the
/// `release` that takes it.
#[test]
fn a_lease_keeps_the_pool_on_the_stations_until_its_release() {
    let p = compile(
        "pool p { cap 10; } stage s1 : delay; stage s2 : delay; stage s3 : delay;
        server { hold p (cost(p, 1)) { run s1 (cost(s1, 1)); } lease p (inf); run s2 (cost(s2, 1)); release p; run s3 (cost(s3, 1));
        }
        ",
        ONE,
    );
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "s1"), ["p"]);
    assert_eq!(pools_of(&p, &net, "s2"), ["p"]);
    assert!(pools_of(&p, &net, "s3").is_empty());
}

/// A pool both arms of a branch hold stays held after it, whichever hold
/// took it in each: here a lease grown in one arm (`grow` outside every
/// hold grows the session's lease, docs/language.md, Pools) and held anew
/// in the other, as ThunderAgent's router does (#568).
#[test]
fn a_pool_both_arms_hold_stays_held_whichever_hold_took_it() {
    let shapes = [
        // a lease grown in one arm, held anew in the other
        "if (turn_no > 1) { grow p (cost(p, 1)); } else { hold p (cost(p, 1)) { run s1 (cost(s1, 1)); } lease p (inf); }",
        // a lease each arm takes by a hold of its own
        "if (turn_no > 1) { hold p (cost(p, 1)) { run s1 (cost(s1, 1)); } lease p (inf); } else { hold p (cost(p, 2)) { run s3 (cost(s3, 1)); } lease p (inf); }",
    ];
    for shape in shapes {
        // a stage no statement runs does not link: `s3` only where it runs
        let s3 = if shape.contains("s3") {
            "stage s3 : delay;"
        } else {
            ""
        };
        let p = compile(
            &format!(
                "pool p {{ cap 10; }} stage s1 : delay; stage s2 : delay; {s3}
            server (turn_no) {{ {shape} run s2 (cost(s2, 1)); }}
            "
            ),
            "workload { arrive batch(1); send (turn_no); session { turn; turn; } }",
        );
        let net = deployment::project(&p);
        assert_eq!(pools_of(&p, &net, "s2"), ["p"], "{shape}");
    }
}

/// A loop that leases its pool anew in each pass leaves it held after it,
/// as the branch does.
#[test]
fn a_pool_leased_anew_in_a_loop_stays_held_after_it() {
    let p = compile(
        "pool p { cap 10; } stage s1 : delay; stage s2 : delay; stage s3 : delay;
        server {
          set m = 0;
          hold p (cost(p, 1)) { run s1 (cost(s1, 1)); } lease p (inf);
          while (m < 2) { set m = m + 1; release p; hold p (cost(p, 1)) { run s3 (cost(s3, 1)); } lease p (inf); }
          run s2 (cost(s2, 1));
        }
        ",
        "workload { arrive batch(1); session { turn; } }",
    );
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "s2"), ["p"]);
}

/// A lease one arm grows and the other shrinks is held after the branch:
/// either acts on the session's lease (ThunderAgent's estimate, which a
/// request may raise or lower).
#[test]
fn a_lease_grown_in_one_arm_and_shrunk_in_the_other_stays_held() {
    let p = compile(
        "pool p { cap 10; } stage s1 : delay; stage s2 : delay;
        server (m) {
          hold p (cost(p, 2)) { run s1 (cost(s1, 1)); } lease p (inf);
          if (m > 0) { grow p (cost(p, 1)); } else { shrink p (cost(p, 1)); }
          run s2 (cost(s2, 1));
        }
        ",
        "workload { arrive batch(1); init { set m = 1; } send (m); session { turn; } }",
    );
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "s2"), ["p"]);
}

/// `examples/multi-turn/thunderagent.sq`: the router's `prog`, which a
/// program holds as a lease it grows, encloses the replica (#568), and the
/// programs waiting for the router's tick are drawn before it.
#[test]
fn thunderagents_router_pool_is_drawn() {
    let t = texts("thunderagent");
    assert!(t.iter().any(|x| x == "pool prog"), "{t:?}");
    assert!(
        t.iter()
            .any(|x| x.contains("preempt leases by (reasoning, prompt)")),
        "{t:?}"
    );
    assert!(t.iter().any(|x| x == "pool waiting_queue"), "{t:?}");
    // the wait for the queue's tick is a station inside it
    assert!(t.iter().any(|x| x == "queue every (5)"), "{t:?}");
    assert!(t.iter().any(|x| x == "tick"), "{t:?}");
}

/// A pool's queue order is a line of its own (#571).
#[test]
fn a_queue_order_is_a_line_of_its_own() {
    let t = texts("kong_svf");
    assert!(
        t.iter()
            .any(|x| x == "queue by (s * o + floor((o * o + o) / 2))"),
        "{t:?}"
    );
}

/// A queue key longer than a label holds is cut at a space, and says so.
#[test]
fn a_long_queue_key_is_cut_short() {
    let t = texts("ascend");
    let q = t
        .iter()
        .find(|x| x.starts_with("queue by ("))
        .expect("a queue line");
    assert!(q.ends_with(" …)") && q.len() < 60, "{q}");
}

/// `examples/pd-disaggregation/llmd_nixl_pull.sq`: the prompt's KV is in the prefiller's pool
/// through the transfer (leased past its scope) and in the decoder's from
/// the transfer on, so the read's two stations (the prefiller's NIC and the
/// decoder's, held at once) hold both. The prefiller's request slot ends
/// with its scope, so it encloses the prefill station alone. The decoder's
/// slot is only reserved during the transfer (`reqs (0) reserve (1)`: the
/// request is parked, not running, `scheduler.py:1264-1268`), so it
/// encloses the decode station and not the read.
///
/// What the router picks is an instance: `P[i]` with its NIC and pools, and
/// `D[j]` with its own. The figure boxes each and draws the read between
/// the boxes, with what it moves and the link's latency written on it, and
/// so no pool box at the NICs: two boxes would cross there.
#[test]
fn a_transfer_between_instances_is_drawn_between_their_boxes() {
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "P"), ["P.reqs", "P.kv"]);
    for nic in ["P.nic", "D.nic"] {
        assert_eq!(pools_of(&p, &net, nic), ["P.kv", "D.kv"], "{nic}");
    }
    assert_eq!(pools_of(&p, &net, "D"), ["D.kv", "D.reqs"]);
    let node = |name: &str| net.node_of(stage(&p, name)).unwrap();
    let (pf, eg, ing, d) = (node("P"), node("P.nic"), node("D.nic"), node("D"));
    assert!(
        net.node_of(stage(&p, "tool")).is_none(),
        "the tool call is the workload's"
    );
    assert_eq!(net.flows, vec![vec![eg, ing]]);
    let note = &net.flow_notes[0];
    assert_eq!(note.from.as_deref(), Some("P.kv[i]"));
    assert_eq!(note.to.as_deref(), Some("D.kv[j]"));
    assert_eq!(note.latency, ["latency (2e-3)"]);
    assert_eq!(net.instances.len(), 2);
    assert_eq!(net.instances[0].nodes, [pf, eg]);
    assert_eq!(net.instances[1].nodes, [ing, d]);
    for nic in [eg, ing] {
        assert!(net.drawn_pools(nic).is_empty());
    }
    let f = deployment::layout(&p, &net);
    let boxes = f.boxes(BoxStyle::Instance);
    let st = f.stations();
    // a box per member: `P[0]`, `P[1]`, `D[0]`, `D[1]` (`NP = ND = 2`)
    assert_eq!(boxes.len(), 4);
    let inside = |b: &serq::view::figure::Rect, k: usize| b.contains(&st[k].0);
    let pbox = *boxes.iter().find(|b| inside(b, pf)).unwrap();
    let dbox = *boxes.iter().find(|b| inside(b, d)).unwrap();
    assert!(inside(&pbox, pf) && inside(&pbox, eg) && !inside(&pbox, ing));
    assert!(inside(&dbox, ing) && inside(&dbox, d) && !inside(&dbox, eg));
    assert!(!pbox.overlaps(&dbox), "the read is between the boxes");
}

fn pools_at(src: &str, stage_name: &str) -> Vec<String> {
    let src = format!(
        "pool p {{ cap 2; }} stage A : delay;
        server {{ {src}
        }} "
    );
    let p = common::compile(
        &common::main_source(&src),
        "workload { arrive poisson(1); session { turn; } }",
        &common::horizon(1.0),
    )
    .unwrap();
    let net = deployment::project(&p);
    pools_of(&p, &net, stage_name)
}

/// A hold of no units reserves and occupies nothing: it draws no boundary.
#[test]
fn a_reservation_only_hold_encloses_nothing() {
    assert!(
        pools_at(
            "hold p (cost(p, 0)) reserve (cost(p, 1)) { run A (cost(A, 1)); }",
            "A"
        )
        .is_empty()
    );
    assert_eq!(
        pools_at("hold p (cost(p, 1)) { run A (cost(A, 1)); }", "A"),
        ["p"]
    );
}

/// A `release` gives back the innermost hold of its pool (`interp.rs`), so
/// after releasing a reservation the outer hold still encloses - which a
/// view that left the reservation off the hold stack would get wrong.
#[test]
fn a_release_takes_the_innermost_hold_even_of_no_units() {
    assert_eq!(
        pools_at(
            "hold p (cost(p, 1)) { hold p (cost(p, 0)) { release p; run A (cost(A, 1)); } }",
            "A"
        ),
        ["p"]
    );
}

/// `examples/pd-disaggregation/llmd_nixl_pull.sq`'s router sends a request either
/// to a prefiller and over the two NICs (remote), after the read's fixed
/// wait (the relation's `latency`, the read's own, written on it), or
/// straight to the decoder (local); a request whose KV is already there
/// skips both. The router decides before any station, so it is the
/// decision a request starts from. The figure is what a request runs: the
/// server's `max_model_len` check contributes a rejection exit. The client's
/// tool call and next turn are not drawn.
#[test]
fn the_router_branches_to_a_remote_or_a_local_prefill() {
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    let at = |name: &str| End::Node(net.node_of(stage(&p, name)).unwrap());
    let (pf, eg, ing, d) = (at("P"), at("P.nic"), at("D.nic"), at("D"));
    assert!(net.node_of(stage(&p, "tool")).is_none());
    let route = decision(&net).expect("the router");
    assert!(net.has_edge(End::Arrival, route));
    assert!(net.has_edge(route, pf), "remote");
    assert!(net.has_edge(route, d), "local");
    assert!(
        net.has_edge(route, End::Exit),
        "the server rejects a prompt beyond max_model_len before routing"
    );
    assert!(net.has_edge(pf, eg));
    assert!(net.has_edge(ing, d));
    assert!(!net.has_edge(eg, ing), "held at once, not passed in turn");
    // `if (!transferred)` right after `set transferred = 0`: the read is
    // skipped only when a preempted request runs its hold again, at D
    assert!(!net.has_edge(pf, d), "a remote request always reads");
    assert!(
        net.edges
            .iter()
            .find(|e| e.from == pf && e.to == eg)
            .is_some_and(|e| e.label.is_none()),
        "a decided guard labels nothing"
    );
    let out: Vec<_> = net.edges.iter().filter(|e| e.to == End::Exit).collect();
    assert_eq!(out.len(), 2);
    assert!(out.iter().any(|e| e.from == d && e.label.is_none()));
    assert!(out.iter().any(|e| e.from == route));
    assert!(net.arrival.contains("Poisson"));
}

/// `vllm.sq` splits its session into a workload and a server, and the
/// figure is the server's: a request arrives at the engine and goes out.
/// The tool call, the next turn and the session's `end` are the workload's.
/// `vllm_replay.sq`'s server reads the workload's `prev`, which the figure
/// declares and does not draw.
#[test]
fn a_split_program_is_drawn_as_its_server() {
    let p = program("vllm");
    let net = deployment::project(&p);
    let engine = End::Node(net.node_of(stage(&p, "vllm")).unwrap());
    assert_eq!(net.nodes.len(), 1);
    assert!(net.node_of(stage(&p, "tool")).is_none());
    assert!(net.has_edge(End::Arrival, engine));
    let out: Vec<_> = net.edges.iter().filter(|e| e.to == End::Exit).collect();
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].from, out[0].label.as_deref()), (engine, None));
    let p = program("vllm_replay");
    let net = deployment::project(&p);
    for st in ["front", "vllm"] {
        assert!(net.node_of(stage(&p, st)).is_some(), "{st}");
    }
    for st in ["gate", "tool"] {
        assert!(net.node_of(stage(&p, st)).is_none(), "{st}");
    }
}

/// The stations a figure has, by stage name.
fn drawn_stages(p: &Program, net: &deployment::Net) -> Vec<String> {
    let mut v: Vec<String> = net
        .nodes
        .iter()
        .filter_map(|n| n.stage.map(|s| p.stages[s].name.clone()))
        .collect();
    v.sort();
    v
}

/// A session whose requests ran two different things, one inside a pool
/// the workload held and one outside it, had no one request to draw. The
/// workload is a file of its own now and holds none of the deployment's
/// pools, so every request runs the server: the figure is the server's,
/// without the workload's station. (Requests that differ in the IR are
/// the next test's.)
#[test]
fn requests_that_run_different_things_draw_the_session() {
    let deployment = "pool live { cap 2; }
        stage A : fifo;
        server { run A (cost(A, 1)); }
        ";
    let e = common::compile(
        &common::main_source(deployment),
        "stage tool : delay;
        workload { arrive poisson(1);
          session { turn; run tool (cost(tool, 1)); hold live (cost(live, 1)) { turn; } }
        }",
        &common::horizon(10.0),
    )
    .unwrap_err();
    assert!(
        e.contains("pool `live` is the deployment file's: the workload runs on and holds its own"),
        "{e}"
    );
    let p = compile(
        deployment,
        "stage tool : delay;
        workload { arrive poisson(1);
          session { turn; run tool (cost(tool, 1)); turn; }
        }",
    );
    assert!(deployment::request(&p).is_some());
    let net = deployment::project(&p);
    assert_eq!(drawn_stages(&p, &net), ["A"]);
}

/// The view reads what each request runs from the IR, not from the parser's
/// splicing one server at every `turn`: an IR whose second request runs B
/// where the first runs A has no one request, and B is drawn.
#[test]
fn requests_that_differ_in_the_ir_draw_the_session() {
    let mut p = compile(
        "stage A : fifo;
        server { run A (cost(A, 1)); }
        ",
        "stage C : delay;
        workload { arrive poisson(1); session { turn; run C (cost(C, 1)); turn; } }",
    );
    // `B`, which no statement of the text runs (it would not link), is
    // added to the IR with the run that reaches it
    let mut stage_b = p.stages[stage(&p, "A")].clone();
    stage_b.name = "B".into();
    p.stages.push(stage_b);
    let b = stage(&p, "B");
    let server_runs: Vec<(usize, usize)> = (0..p.blocks.len())
        .flat_map(|k| (0..p.blocks[k].len()).map(move |i| (k, i)))
        .filter(|&(k, i)| {
            p.sides[k][i] == serq::ir::Side::Server
                && matches!(p.blocks[k][i], serq::ir::CStmt::Run { .. })
        })
        .collect();
    assert_eq!(server_runs.len(), 2, "one run of the server at each turn");
    let (k, i) = server_runs[1];
    let serq::ir::CStmt::Run {
        stage: s,
        work: serq::ir::CExpr::Cost(t, _),
        ..
    } = &mut p.blocks[k][i]
    else {
        unreachable!("a run of a cost")
    };
    s.base = b;
    *t = serq::ir::CostTarget::Stage { base: b, count: 1 };
    let p = Program::from_json(&p.to_json()).unwrap();
    assert!(deployment::request(&p).is_none());
    let net = deployment::project(&p);
    assert_eq!(drawn_stages(&p, &net), ["A", "B", "C"]);
}

/// A server's guard on a workload attribute is not decided by the figure:
/// the server does not know what the workload set (`routing.sq`'s
/// `if (first)`).
#[test]
fn a_server_guard_on_the_workload_draws_both_arms() {
    let src = "stage big : fifo; stage small : fifo;
        server (first, n) { if (first) { run big (cost(big, n)); } else { run small (cost(small, n)); } }
        ";
    let workload = "stage tool : delay;
        workload {
          arrive poisson(1);
          init { set first = 1; }
          turn { set n = ~exp(10); }
          send (first, n);
          session { loop {  turn; set first = 0;
              if (~bernoulli(0.5)) { run tool (cost(tool, 1)); } else { end; } } }
        }";
    let p = common::compile(&common::main_source(src), workload, &common::horizon(100.0)).unwrap();
    let net = deployment::project(&p);
    assert_eq!(drawn_stages(&p, &net), ["big", "small"]);
}

/// A guard is decided by what this path set, and only until a `turn`
/// draws the attributes again.
#[test]
fn a_turn_forgets_what_the_path_set() {
    let p = compile(
        "stage A : fifo; stage B : fifo;
        server (n) { if (n > 0) { run A (cost(A, n)); } else { run B (cost(B, 1)); }
        }
        ",
        "workload { arrive poisson(1); turn { set n = ~exp(10); } send (n);
          session { set n = 0;  turn;
          }
        }",
    );
    let net = deployment::project(&p);
    assert_eq!(drawn_stages(&p, &net), ["A", "B"]);
}

/// A decided guard draws only its arm when the other has no station: it
/// would only skip stations, joining two the session never travels
/// between (`llmd_nixl_pull.sq`'s read). An other arm with a station of its
/// own runs when the hold is executed again after a preemption, and is
/// drawn.
#[test]
fn a_decided_guard_drops_only_an_arm_with_no_station() {
    let program = |arms: &str| {
        compile(
            &format!(
                "stage A : fifo; stage B : fifo; stage C : fifo; pool kv {{ cap 100; }}
        server {{ set x = 0; hold kv (cost(kv, 1)) {{ run A (cost(A, 1)); {arms} set x = 1; run C (cost(C, 1)); }}
        }}
        "
            ),
            POISSON,
        )
    };
    let p = program("if (!x) { run B (cost(B, 1)); }");
    let net = deployment::project(&p);
    assert_eq!(drawn_stages(&p, &net), ["A", "B", "C"]);
    assert!(edge(&p, &net, "A", "C").is_none(), "the skip is not drawn");
    assert!(edge(&p, &net, "A", "B").is_some_and(|e| e.label.is_none()));
    let p = program("if (!x) { run B (cost(B, 1)); } else { run C (cost(C, 1)); }");
    let net = deployment::project(&p);
    assert!(edge(&p, &net, "A", "C").is_some(), "an arm with a station");
}

/// A guard on constants alone is the program's setting, which `--set`
/// changes: both arms are drawn.
#[test]
fn a_guard_on_constants_draws_both_arms() {
    let p = compile(
        "let mode = 0; stage A : fifo; stage B : fifo;
        server { if (mode == 0) { run A (cost(A, 1)); } else { run B (cost(B, 1)); }
        }
        ",
        POISSON,
    );
    let net = deployment::project(&p);
    assert_eq!(drawn_stages(&p, &net), ["A", "B"]);
}

// --- loops ------------------------------------------------------------------

/// A session program over stages `A`, `B`, `C`, projected. The stages are
/// the client's own, so they are declared in the workload's file.
fn shape(session: &str) -> (Program, deployment::Net) {
    // a stage no statement runs does not link: declare those the session names
    let stages: String = ["A", "B", "C"]
        .iter()
        .filter(|s| session.contains(&format!("run {s} ")))
        .map(|s| format!("stage {s} : delay; "))
        .collect();
    let workload = format!(
        "{stages}
        workload {{ arrive poisson(1); session {{ turn; {session} }} }}
        "
    );
    let p = common::compile(
        &common::main_source("server {}"),
        &workload,
        &common::horizon(1.0),
    )
    .unwrap();
    let net = deployment::project(&p);
    (p, net)
}

fn edge<'a>(
    p: &Program,
    net: &'a deployment::Net,
    from: &str,
    to: &str,
) -> Option<&'a deployment::Edge> {
    let at = |name: &str| match name {
        "arrival" => End::Arrival,
        "exit" => End::Exit,
        _ => End::Node(net.node_of(stage(p, name)).unwrap()),
    };
    let (from, to) = (at(from), at(to));
    net.edges.iter().find(|e| e.from == from && e.to == to)
}

/// The decision node a loop's body starts from, if it has one.
fn decision(net: &deployment::Net) -> Option<End> {
    net.nodes
        .iter()
        .position(|n| n.kind == StationKind::Decision)
        .map(End::Node)
}

fn has(net: &deployment::Net, p: &Program, from: End, to: &str) -> Option<deployment::Edge> {
    let to = match to {
        "exit" => End::Exit,
        _ => End::Node(net.node_of(stage(p, to)).unwrap()),
    };
    net.edges
        .iter()
        .find(|e| e.from == from && e.to == to)
        .cloned()
}

/// A loop body that starts with a branch decides before its first station:
/// one decision node, which every turn comes back to and which sends it down
/// each arm with the arm's guard.
#[test]
fn a_loop_that_decides_first_returns_to_its_decision() {
    let (p, net) = shape(
        "loop { set a = ~bernoulli(0.5); if (a) { run A (cost(A, 1)); } else { run B (cost(B, 1)); } run C (cost(C, 1)); }",
    );
    let d = decision(&net).expect("a decision");
    assert!(net.has_edge(End::Arrival, d));
    for (to, label) in [("A", "a"), ("B", "else")] {
        let e = has(&net, &p, d, to).unwrap_or_else(|| panic!("decision -> {to}"));
        assert_eq!(e.label.as_deref(), Some(label));
    }
    let c = End::Node(net.node_of(stage(&p, "C")).unwrap());
    assert!(net.edges.iter().any(|e| e.from == c && e.to == d && e.back));
    assert!(edge(&p, &net, "C", "A").is_none(), "not back to each arm");
}

/// A loop whose body is a loop: the inner one decides, the outer one comes
/// in through it.
#[test]
fn a_nested_loop_returns_to_the_inner_decision() {
    let (p, net) = shape(
        "run C (cost(C, 1)); loop { loop { set a = ~bernoulli(0.5); if (a) { run A (cost(A, 1)); } else { run B (cost(B, 1)); } } }",
    );
    let d = decision(&net).expect("a decision");
    assert_eq!(
        net.nodes
            .iter()
            .filter(|n| n.kind == StationKind::Decision)
            .count(),
        1
    );
    for st in ["A", "B"] {
        assert!(has(&net, &p, d, st).is_some());
        let n = End::Node(net.node_of(stage(&p, st)).unwrap());
        assert!(net.has_edge(n, d), "{st} returns");
    }
    let c = End::Node(net.node_of(stage(&p, "C")).unwrap());
    assert!(net.has_edge(c, d));
}

/// An arm with no station of its own goes on to the station after the
/// branch: the decision sends a turn to `A` or straight to `B`, and `B`
/// comes back to the decision, not to itself.
#[test]
fn a_loop_through_an_empty_arm() {
    let (p, net) = shape(
        "loop { set a = ~bernoulli(0.5); if (a) { } else { run A (cost(A, 1)); } run B (cost(B, 1)); }",
    );
    let d = decision(&net).expect("a decision");
    assert!(has(&net, &p, d, "A").is_some());
    assert!(has(&net, &p, d, "B").is_some());
    let b = End::Node(net.node_of(stage(&p, "B")).unwrap());
    assert!(net.has_edge(b, d));
    assert!(edge(&p, &net, "B", "B").is_none());
}

/// A body that can end before its first station ends at its decision, on
/// the first turn and every one after.
#[test]
fn a_loop_that_can_end_before_a_station() {
    let (p, net) = shape(
        "loop { set c = ~bernoulli(0.5); if (c) { end; } run A (cost(A, 1)); run B (cost(B, 1)); }",
    );
    let d = decision(&net).expect("a decision");
    assert!(has(&net, &p, d, "exit").is_some_and(|e| e.label.as_deref() == Some("c")));
    assert!(has(&net, &p, d, "A").is_some());
    let b = End::Node(net.node_of(stage(&p, "B")).unwrap());
    assert!(net.has_edge(b, d));
    assert!(edge(&p, &net, "arrival", "exit").is_none());
}

/// Coming back to the station the loop was entered from is a way in too:
/// a body that ends or goes back to `A` decides (#199 review).
#[test]
fn a_return_to_the_same_station_is_a_way_in() {
    let (p, net) = shape(
        "run A (cost(A, 1)); loop { set c = ~bernoulli(0.5); if (c) { end; } else { run A (cost(A, 1)); } }",
    );
    let d = decision(&net).expect("a decision");
    assert!(has(&net, &p, d, "exit").is_some());
    assert!(has(&net, &p, d, "A").is_some());
    let a = End::Node(net.node_of(stage(&p, "A")).unwrap());
    assert!(net.has_edge(a, d));
}

/// The decision is named by the `choose`s before the first station, down
/// every path: one made before the loop, or after a station, is not its.
#[test]
fn a_decision_is_named_by_its_leading_chooses() {
    let (_, net) = shape(
        "set j = 0; loop { choose j in 2 by (0); set c = ~bernoulli(0.5);
           if (c) { choose k in 2 by (0); run A (cost(A, 1)); } else { run B (cost(B, 1)); }
           choose m in 2 by (0); run C (cost(C, 1)); }",
    );
    let d = net
        .nodes
        .iter()
        .find(|n| n.kind == StationKind::Decision)
        .expect("a decision");
    assert_eq!(d.label, "choose j, k");
}

/// The pass that looks for the ways in leaves nothing behind: a body with a
/// flow and a decision has the flow once and the decision once. The runs
/// are the session's, so the view draws the session and its loop.
#[test]
fn the_looking_pass_leaves_nothing() {
    // the session's stations are the workload's own
    let workload = "stage A : delay; stage x : ps(1); stage y : ps(1);
        use \"std/share\";
        workload { arrive batch(1);
          session {
            loop {
              set c = ~bernoulli(0.5);
              if (c) { end; } turn;
              run x, y (cost(x, y, 1), share = share.maxmin);
              run A (cost(A, 1));
            }
          }
        }
        ";
    let p = compile("server {}", workload);
    let net = deployment::project(&p);
    assert_eq!(net.flows.len(), 1);
    assert_eq!(net.flow_notes.len(), 1);
    assert_eq!(
        net.nodes
            .iter()
            .filter(|n| n.kind == StationKind::Decision)
            .count(),
        1
    );
}

/// A decision stands before the stations it sends a turn to, after the
/// stations of an instance are put side by side too (#199 review).
#[test]
fn a_decision_stays_before_its_stations() {
    let src = "pool kv[2] { cap 9; } stage A[2] : delay; stage B[2] : delay; stage C[2] : delay;
        server {
          choose i in 2 by (0);
          hold kv[i] (cost(kv, 1)) { run A[i] (cost(A, 1)); }
          loop {
            set c = ~bernoulli(0.5);
            if (c) { hold kv[i] (cost(kv, 1)) { run B[i] (cost(B, 1)); } } else { hold kv[i] (cost(kv, 1)) { run C[i] (cost(C, 1)); } }
          }
        }
        ";
    let p = compile(src, "workload { arrive batch(1); session { turn; } }");
    let net = deployment::project(&p);
    let End::Node(d) = decision(&net).expect("a decision") else {
        unreachable!()
    };
    // before the whole instance, so its box does not take the diamond in
    let a = net.node_of(stage(&p, "A")).unwrap();
    assert!(d < a, "the decision before the instance's first station");
    for st in ["B", "C"] {
        let k = net.node_of(stage(&p, st)).unwrap();
        assert!(d < k, "the decision before {st}");
        let e = net
            .edges
            .iter()
            .find(|e| e.from == End::Node(d) && e.to == End::Node(k))
            .unwrap();
        assert!(!e.back, "decision -> {st} goes forward");
    }
}

/// A body that starts at one station needs no decision: it comes back to it.
#[test]
fn a_loop_with_one_way_in_has_no_decision() {
    let (p, net) = shape(
        "loop { run A (cost(A, 1)); set c = ~bernoulli(0.5); if (c) { end; } run B (cost(B, 1)); }",
    );
    assert!(decision(&net).is_none());
    assert!(edge(&p, &net, "B", "A").is_some_and(|e| e.back));
}

/// Every enclosure holds the stations it encloses, and enclosures either nest
/// or stay apart - the property that makes the picture readable.
#[test]
fn enclosures_contain_their_stations_and_nest() {
    for name in PROGRAMS {
        let p = program(name);
        let net = deployment::project(&p);
        let f = deployment::layout(&p, &net);
        no_overlapping_enclosures(&f, name);
        // `layout` pushes one station per node, in node order, then those
        // of a stacked instance's members past the first
        let boxes = f.boxes(BoxStyle::Enclosure);
        let stations = f.stations();
        let members = f
            .items
            .iter()
            .filter(|i| {
                matches!(i, serq::view::figure::Item::Station { node, .. }
                    if node.ends_with(']') && !node.ends_with("[0]"))
            })
            .count();
        assert_eq!(
            stations.len() - members,
            net.nodes.len(),
            "{name}: a station per node"
        );
        let frames = f.boxes(BoxStyle::Frame);
        for (i, (node, (rect, _))) in net.nodes.iter().zip(stations).enumerate() {
            // a pool held here alone is in the station's frame, not a box
            let inside = boxes.iter().filter(|b| b.contains(&rect)).count();
            let drawn = net.drawn_pools(i).len() - net.resident_pools(i).len();
            assert_eq!(
                inside, drawn,
                "{name}: {} is in {inside} boxes, wants {drawn}",
                node.label
            );
            let framed = frames.iter().filter(|b| b.contains(&rect)).count();
            let want = usize::from(!net.resident_pools(i).is_empty());
            assert_eq!(framed, want, "{name}: {} has {framed} frames", node.label);
        }
    }
}

/// The figure is big enough for what it contains.
#[test]
fn figures_are_fitted() {
    for name in PROGRAMS {
        let p = program(name);
        let f = deployment::figure(&p);
        assert!(f.width > 0.0 && f.height > 0.0, "{name}: empty figure");
        for r in f
            .boxes(BoxStyle::Enclosure)
            .iter()
            .chain(f.boxes(BoxStyle::Frame).iter())
            .chain(f.boxes(BoxStyle::Solid).iter())
        {
            assert!(
                r.right() <= f.width + 0.01,
                "{name}: box past the right edge"
            );
            assert!(r.bottom() <= f.height + 0.01, "{name}: box past the bottom");
        }
    }
}

// --- the writers ------------------------------------------------------------

/// Every program renders in both formats without panicking, and the output
/// is not empty. This is what `make check` runs.
#[test]
fn every_program_renders() {
    for name in PROGRAMS {
        let p = program(name);
        let f = deployment::figure(&p);
        let svg = serq::view::svg::render(&f);
        assert!(svg.starts_with("<svg"), "{name}: not an svg");
        assert!(svg.ends_with("</svg>\n"));
        let tikz = serq::view::tikz::render(&f);
        assert!(
            tikz.contains("\\begin{tikzpicture}"),
            "{name}: not a tikzpicture"
        );
        assert!(tikz.ends_with("\\end{tikzpicture}\n"));
    }
}

/// `view::render` is the one place a format's name meets its writer: the CLI
/// and pyserq both go through it.
#[test]
fn a_format_is_named_once() {
    let f = deployment::figure(&program("vllm"));
    assert_eq!(
        serq::view::render(&f, "tikz").unwrap(),
        serq::view::tikz::render(&f)
    );
    assert_eq!(
        serq::view::render(&f, "svg").unwrap(),
        serq::view::svg::render(&f)
    );
    // every name the CLI accepts has a writer
    for name in serq::view::format_names() {
        assert!(serq::view::render(&f, name).is_ok(), "{name}");
    }
    assert_eq!(
        serq::view::render(&f, "png").unwrap_err(),
        "unknown format `png` (tikz, svg)"
    );
}

/// TikZ output goes into a LaTeX document, so every special character in a
/// label has to survive the trip.
#[test]
fn tikz_escapes_labels() {
    let p = program("vllm");
    let tikz = serq::view::tikz::render(&deployment::figure(&p));
    for (i, line) in tikz.lines().enumerate() {
        if !line.trim_start().starts_with("\\node") {
            continue;
        }
        let body = line.split_once('{').map(|x| x.1).unwrap_or("");
        for bad in ['_', '#', '%'] {
            let unescaped = body
                .char_indices()
                .any(|(j, c)| c == bad && !body[..j].ends_with('\\'));
            assert!(!unescaped, "line {}: unescaped `{bad}`: {line}", i + 1);
        }
    }
}

/// The figures the site shows are the program's figure, not a copy that
/// once was: every `docs/assets/NAME.deployment.svg` is what `serq draw`
/// makes of `examples/*/NAME.sq` or `docs/tutorial/programs/NAME.sq`
/// now, and `make draw-golden` rewrites them. A figure without its program
/// is an error, not a keepsake.
#[test]
fn docs_assets_are_current() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut seen = 0;
    for entry in std::fs::read_dir(root.join("docs/assets"))
        .unwrap()
        .flatten()
    {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".deployment.svg") else {
            continue;
        };
        let tutorial = root
            .join("docs/tutorial/programs")
            .join(format!("{stem}.sq"));
        // a workload's file may share its program's name: it is not one
        let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(root.join("examples"))
            .unwrap()
            .flatten()
            .filter(|g| g.file_name() != "workloads")
            .map(|g| g.path().join(format!("{stem}.sq")))
            .filter(|p| p.is_file())
            .collect();
        if tutorial.is_file() {
            found.push(tutorial);
        }
        // one name, one program: two would leave the figure's source to
        // the order the directories are read in
        assert!(
            found.len() <= 1,
            "docs/assets/{name}: several programs are {stem}.sq: {found:?}"
        );
        let src_path = found
            .pop()
            .unwrap_or_else(|| panic!("docs/assets/{name} has no program: {stem}.sq"));
        let p = example(&src_path);
        let got = serq::view::svg::render(&deployment::figure(&p));
        // the view reads the IR alone: the program read back from its JSON
        // draws the same figure (#265)
        let ir = Program::from_json(&p.to_json()).unwrap();
        assert_eq!(
            serq::view::svg::render(&deployment::figure(&ir)),
            got,
            "{}: its IR draws another figure",
            src_path.display()
        );
        if std::env::var("SERQ_BLESS").is_ok() {
            std::fs::write(&path, &got).unwrap();
        } else {
            let want = std::fs::read_to_string(&path).unwrap();
            assert!(
                want == got,
                "docs/assets/{name} is not {}'s figure; run `make draw-golden`",
                src_path.strip_prefix(root).unwrap_or(&src_path).display()
            );
        }
        seen += 1;
    }
    assert!(seen > 0, "no figures under docs/assets");
}

/// `routing.sq`'s request migrates or stays: to the link, or straight to a
/// replica; its tool call and next turn are the workload's, and not drawn.
/// `pd_tandem.sq`, one session with no server apart, is drawn whole: its
/// job re-enters down both arms of its `mode` branch, drawn although `mode`
/// is one constant in a run, since a guard on constants is the program's
/// setting.
#[test]
fn the_examples_start_at_their_decision() {
    let p = program("routing");
    let net = deployment::project(&p);
    let d = decision(&net).expect("routing decides first");
    assert!(net.node_of(stage(&p, "tool")).is_none(), "the workload's");
    assert!(has(&net, &p, d, "link").is_some());
    assert!(has(&net, &p, d, "rep").is_some());
    let p = program("pd_tandem");
    let net = deployment::project(&p);
    let d = decision(&net).expect("pd_tandem decides first");
    for st in ["agg", "prefill"] {
        assert!(has(&net, &p, d, st).is_some(), "{st}");
    }
    for st in ["agg", "decode"] {
        let n = End::Node(net.node_of(stage(&p, st)).unwrap());
        assert!(
            net.edges.iter().any(|e| e.from == n && e.to == d && e.back),
            "{st}"
        );
    }
}

/// A run over several stages is one job at several stations: they are
/// bracketed together, no arrow runs between them, and the session comes in
/// at the first and leaves from the last.
#[test]
fn a_run_over_several_stages_is_one_bracketed_job() {
    let src = "pool kvP { cap 100; } pool kvD[2] { cap 100; }
        stage P : delay; stage egress : ps(1); stage ingress[2] : ps(1); stage D[2] : delay;
        use \"std/share\";
        server {
          set j = serial;
          hold kvP (cost(kvP, 10)) { run P (cost(P, 1)); } lease kvP (inf);
          hold kvD[j] (cost(kvD, 10)) {
            transfer on egress, ingress[j] (1, share = share.maxmin) from kvP to kvD[j] (10);
            run D[j] (cost(D, 1));
          }
        }
        ";
    let p = compile(src, "workload { arrive batch(2); session { turn; end; } }");
    let net = deployment::project(&p);
    let at = |name: &str| net.node_of(stage(&p, name)).unwrap();
    let (pf, eg, ing, d) = (at("P"), at("egress"), at("ingress"), at("D"));
    assert_eq!(net.flows, vec![vec![eg, ing]]);
    assert!(net.has_edge(End::Node(pf), End::Node(eg)));
    assert!(net.has_edge(End::Node(ing), End::Node(d)));
    assert!(!net.has_edge(End::Node(eg), End::Node(ing)));
    assert!(!net.has_edge(End::Node(pf), End::Node(ing)));
    let f = deployment::layout(&p, &net);
    let brackets = f.boxes(BoxStyle::Flow);
    assert_eq!(brackets.len(), 1);
    for i in [eg, ing] {
        assert!(brackets[0].contains(&f.stations()[i].0));
    }
}

/// A delay before a transfer is a station: only a run's own `latency` (a
/// link's `serve … latency`, a relation's) is written on the transfer.
#[test]
fn a_delay_before_a_transfer_is_a_station() {
    let src = "pool kvP { cap 100; } pool kvD { cap 100; }
        stage P : delay; stage wait : delay; stage egress : ps(1); stage ingress : ps(1); stage D : delay;
        use \"std/share\";
        server {
          hold kvP (cost(kvP, 10)) { run P (cost(P, 1)); } lease kvP (inf);
          hold kvD (cost(kvD, 10)) { run wait (cost(wait, 1)); transfer on egress, ingress (1, share = share.maxmin) from kvP to kvD (10); run D (cost(D, 1)); }
        }
        ";
    let p = compile(src, ONE);
    let net = deployment::project(&p);
    assert!(net.node_of(stage(&p, "wait")).is_some());
    assert!(net.flow_notes[0].latency.is_empty());
}

/// A transfer crosses between instances only when two of its stations are
/// in two of them: a choice of one station is no instance, and a run from
/// it into a boxed instance keeps that instance's pools drawn (#197 review).
#[test]
fn a_run_from_an_unboxed_choice_does_not_span() {
    let src =
        "pool kv[2] { cap 100; } stage nic[2] : ps(1); stage ing[2] : ps(1); stage D[2] : delay;
        use \"std/share\";
        server {
          choose i in 2 by (0);
          choose j in 2 by (0);
          hold kv[j] (cost(kv, 10)) { run nic[i], ing[j] (cost(nic, ing, 1), share = share.maxmin); run D[j] (cost(D, 1)); }
        }
        ";
    let p = compile(src, ONE);
    let net = deployment::project(&p);
    let ing = net.node_of(stage(&p, "ing")).unwrap();
    assert_eq!(
        net.instances.len(),
        1,
        "j's: ing, D and kv; i's is one station"
    );
    assert!(!net.spans(&net.flows[0]));
    assert_eq!(net.drawn_pools(ing), net.nodes[ing].pools);
}

/// One link's latency waited before transfers with two different partners,
/// down two arms, is written on each transfer: the wait is each run's own,
/// not a station the two arms would share (#197 review drew it as one
/// station, folding it into neither).
#[test]
fn a_links_latency_is_on_each_transfer_over_it() {
    let src = "pool kvP { cap 100; } pool kvD { cap 100; }
        stage P : delay; stage a : ps(1); stage b : ps(1);
        queue L : link { serve ps(1) latency 0.5; }
        use \"std/share\";
        server {
          hold kvP (cost(kvP, 10)) { run P (cost(P, 1)); } lease kvP (inf);
          set c = ~bernoulli(0.5);
          hold kvD (cost(kvD, 10)) {
            if (c) { transfer on a, L (1, share = share.maxmin) from kvP to kvD (10); }
            else { transfer on b, L (1, share = share.maxmin) from kvP to kvD (10); }
          }
        }
        ";
    let p = compile(src, ONE);
    let net = deployment::project(&p);
    assert!(p.stages.iter().all(|s| !s.name.ends_with(".latency")));
    assert_eq!(net.flows.len(), 2);
    for n in &net.flow_notes {
        assert_eq!(n.latency, ["latency (0.5)"]);
    }
}

/// The second review of #197: a station group that moves two pairs of pools
/// says neither, two latencies read in the order named, and a run over
/// `a[i], b[j], c[i]` is no transfer between two boxes.
#[test]
fn a_transfers_note_is_what_every_run_over_it_moves() {
    let src = "pool p1 { cap 9; } pool p2 { cap 9; } pool q1 { cap 9; } pool q2 { cap 9; }
        stage s : delay; stage a : ps(1); stage b : ps(1);
        use \"std/share\";
        server {
          set c = ~bernoulli(0.5);
          if (c) {
            hold p1 (cost(p1, 1)) { run s (cost(s, 1)); } lease p1 (inf);
            hold q1 (cost(q1, 1)) { transfer on a, b (1, share = share.maxmin) from p1 to q1 (1); }
          } else {
            hold p2 (cost(p2, 1)) { run s (cost(s, 1)); } lease p2 (inf);
            hold q2 (cost(q2, 1)) { transfer on a, b (1, share = share.maxmin) from p2 to q2 (1); }
          }
        }
        ";
    let p = compile(src, ONE);
    let net = deployment::project(&p);
    assert_eq!(net.flows.len(), 1);
    let n = &net.flow_notes[0];
    assert!(n.several && n.from.is_none() && n.to.is_none(), "{n:?}");

    let src = "pool kP { cap 9; } pool kD { cap 9; } stage P : delay;
        queue A : link { serve ps(1) latency 0.25; }
        queue B : link { serve ps(1) latency 0.5; }
        use \"std/share\";
        server {
          hold kP (cost(kP, 1)) { run P (cost(P, 1)); } lease kP (inf);
          hold kD (cost(kD, 1)) { transfer on A, B (1, share = share.maxmin) from kP to kD (1); }
        }
        ";
    let p = compile(src, ONE);
    let net = deployment::project(&p);
    // the two links' waits are the one run's, summed in the order named
    assert_eq!(net.flow_notes[0].latency, ["latency (0.25 + 0.5)"]);

    let src = "pool kv[2] { cap 9; } stage a[2] : ps(1); stage b[2] : ps(1); stage c[2] : ps(1);
        stage A[2] : delay; stage B[2] : delay;
        use \"std/share\";
        server {
          choose i in 2 by (0);
          choose j in 2 by (0);
          hold kv[i] (cost(kv, 1)) { run A[i] (cost(A, 1)); }
          hold kv[j] (cost(kv, 1)) { run B[j] (cost(B, 1)); }
          run a[i], b[j], c[i] (cost(a, b, c, 1), share = share.maxmin);
        }
        ";
    let p = compile(src, ONE);
    let net = deployment::project(&p);
    assert!(!net.spans(&net.flows[0]));
}

/// A flow's stations stand side by side even when one of them was reached
/// alone before, so the bracket takes in no other station: `ingress` is
/// used alone first, then with `egress`, with `D` between.
#[test]
fn a_flows_stations_are_neighbours_in_the_row() {
    let src = "stage ingress : ps(1); stage D : delay; stage egress : ps(1);
        use \"std/share\";
        server { run ingress (cost(ingress, 1)); run D (cost(D, 1)); run egress, ingress (cost(egress, ingress, 1), share = share.maxmin);
        }
        ";
    let p = compile(src, ONE);
    let net = deployment::project(&p);
    let at = |name: &str| net.node_of(stage(&p, name)).unwrap();
    let (eg, ing, d) = (at("egress"), at("ingress"), at("D"));
    assert_eq!(net.flows, vec![vec![eg, ing]]);
    assert_eq!(ing, eg + 1, "side by side, in the run's order");
    let f = deployment::layout(&p, &net);
    let bracket = f.boxes(BoxStyle::Flow)[0];
    assert!(
        !bracket.contains(&f.stations()[d].0),
        "D is outside the bracket"
    );
    // `D -> egress` now points left: it is drawn as a return
    let e = net
        .edges
        .iter()
        .find(|e| e.from == End::Node(d) && e.to == End::Node(eg))
        .unwrap();
    assert!(e.back);
}

/// An arrow forward past other stations goes below the row, not through
/// them: `llmd_nixl_pull.sq`'s router sends a local request straight to the
/// decoder, past the prefiller and the two NICs.
#[test]
fn an_arrow_past_stations_goes_below_the_row() {
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    let f = deployment::layout(&p, &net);
    let End::Node(r) = decision(&net).expect("the router") else {
        unreachable!()
    };
    let (pf, d) = (
        net.node_of(stage(&p, "P")).unwrap(),
        net.node_of(stage(&p, "D")).unwrap(),
    );
    let st = f.stations();
    let (rr, rp, rd) = (st[r].0, st[pf].0, st[d].0);
    let row_bottom = rp.bottom();
    let through = f.items.iter().any(|it| match it {
        serq::view::figure::Item::Edge { pts, .. } => {
            pts.len() == 2 && (pts[0].x - rr.right()).abs() < 1e-9 && (pts[1].x - rd.x).abs() < 1e-9
        }
        _ => false,
    });
    assert!(
        !through,
        "no straight arrow from the router to D along the row"
    );
    // leaving the router's bottom and entering D's, in a solid line below
    // the row
    let below = f.items.iter().any(|it| match it {
        serq::view::figure::Item::Edge { pts, style, .. } => {
            *style == serq::view::figure::EdgeStyle::Flow
                && pts.len() == 4
                && pts[0].x > rr.x
                && pts[0].x < rr.right()
                && pts[1].y > row_bottom
                && (pts[3].x - (rd.x + rd.w * 0.5)).abs() < 1e-9
        }
        _ => false,
    });
    assert!(below, "router -> D in a lane below");
}

/// The order a flow imposes decides which way an arrow points: `v -> u`
/// is forward in session order, `u -> v` a return; with `u` pulled next to
/// `a` (the flow `a, u`), `u` stands before `v`, and so `u -> v` is drawn
/// forward and `v -> u` as the return.
#[test]
fn a_reordered_arrow_is_drawn_the_way_it_points() {
    let src = "stage a : ps(1); stage v : delay; stage u : ps(1);
        use \"std/share\";
        server { run a (cost(a, 1)); run v (cost(v, 1)); run u (cost(u, 1)); run v (cost(v, 1)); run a, u (cost(a, u, 1), share = share.maxmin);
        }
        ";
    let p = compile(src, ONE);
    let net = deployment::project(&p);
    let at = |name: &str| End::Node(net.node_of(stage(&p, name)).unwrap());
    let (u, v) = (at("u"), at("v"));
    let find = |from, to| {
        net.edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap()
    };
    assert!(!find(u, v).back, "u stands before v now");
    assert!(find(v, u).back);
}

/// No two labels of a figure share ink: a station's name or note wider
/// than the station, or a pool's options wider than its glyphs, gets the
/// room it needs (`replica.sq`'s budget ran into its `kv` options).
#[test]
fn labels_do_not_overlap() {
    use serq::view::figure::{Anchor, Item, TextSize};
    // every program the repository ships, not only the ones drawn above
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files: Vec<std::path::PathBuf> = vec![];
    for dir in std::fs::read_dir(root.join("examples")).unwrap().flatten() {
        // a workload's file is drawn with the program it runs
        if dir.file_name() == "workloads" {
            continue;
        }
        for f in std::fs::read_dir(dir.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            files.push(f.path());
        }
    }
    for f in std::fs::read_dir(root.join("docs/tutorial/programs"))
        .unwrap()
        .flatten()
    {
        files.push(f.path());
    }
    files.retain(|f| f.extension().is_some_and(|e| e == "sq"));
    files.sort();
    assert!(files.len() > PROGRAMS.len());
    for path in &files {
        let name = path.strip_prefix(root).unwrap().display().to_string();
        let p = example(path);
        let f = deployment::figure(&p);
        let boxes: Vec<(f64, f64, f64, &str)> = f
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Text {
                    at,
                    text,
                    anchor,
                    size,
                    ..
                } => {
                    let w = size.width_of(text);
                    let x0 = match anchor {
                        Anchor::Start => at.x,
                        Anchor::Middle => at.x - w / 2.0,
                        Anchor::End => at.x - w,
                    };
                    let h = match size {
                        TextSize::Small => 9.0,
                        _ => 11.0,
                    };
                    Some((x0, x0 + w, at.y - h / 2.0, text.as_str()))
                }
                _ => None,
            })
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let same_line = (a.2 - b.2).abs() < 8.0;
                let apart = a.1 <= b.0 || b.1 <= a.0;
                assert!(
                    !same_line || apart,
                    "{name}: `{}` and `{}` overlap",
                    a.3,
                    b.3
                );
            }
        }
    }
}

// --- instances whose stages and pools two of them address (#206) ------------

fn instance_net(decls: &str, session: &str) -> (Program, deployment::Net) {
    let src = format!(
        "{decls}
         server {{ choose i in 2 by (0); choose j in 2 by (0); {session} }}"
    );
    let p = common::compile(&common::main_source(&src), ONE, &common::horizon(10.0))
        .unwrap_or_else(|e| panic!("{e}"));
    let net = deployment::project(&p);
    (p, net)
}

/// A stage family two instances index (`E[i]`, then `E[j]`) is one station
/// of the projection: it is in neither box, not in the first.
#[test]
fn a_stage_two_instances_index_is_in_neither_box() {
    let (p, net) = instance_net(
        "pool kv[2] { cap 9; } stage E[2] : delay; stage X[2] : delay;",
        "run E[i] (cost(E, 1)); hold kv[i] (cost(kv, 1)) { run X[i] (cost(X, 1)); } run E[j] (cost(E, 1));",
    );
    let at = |name: &str| net.node_of(stage(&p, name)).unwrap();
    assert_eq!(net.instance_of(at("E")), None);
    assert!(net.instance_of(at("X")).is_some(), "i's box is X and kv");
}

/// A pool family two instances hold (`q[i]`, `q[j]`) is neither's: each
/// station keeps the enclosure it is held in.
#[test]
fn a_pool_two_instances_hold_is_neither_instances() {
    let (p, net) = instance_net(
        "pool q[2] { cap 9; } stage A[2] : delay; stage C[2] : delay; stage B[2] : delay; stage D[2] : delay;",
        "hold q[i] (cost(q, 1)) { run A[i] (cost(A, 1)); run C[i] (cost(C, 1)); } hold q[j] (cost(q, 1)) { run B[j] (cost(B, 1)); run D[j] (cost(D, 1)); }",
    );
    let q = pool(&p, "q");
    assert_eq!(net.instances.len(), 2);
    assert!(net.instances.iter().all(|g| !g.pools.contains(&q)));
    for s in ["A", "B"] {
        let k = net.node_of(stage(&p, s)).unwrap();
        assert_eq!(net.drawn_pools(k), [q], "{s}");
    }
}

/// A `load` and `release` that open the else arm follow no transfer of the
/// then arm: the transfer's note says nothing it did not move.
#[test]
fn a_load_in_the_else_arm_is_not_the_then_arms_transfer() {
    let (_, net) = instance_net(
        "pool p[2] { cap 9; } pool q[2] { cap 9; } stage A[2] : delay; stage a[2] : ps(1); stage b[2] : ps(1); use \"std/share\";",
        "set c = ~bernoulli(0.5);
         hold p[i] (cost(p, 1)) { run A[i] (cost(A, 1)); } lease p[i] (inf);
         hold q[j] (cost(q, 1)) {
           if (c) { run a[i], b[j] (cost(a, b, 1), share = share.maxmin); } else { load q[j] (cost(q, 1)); release p[i]; }
         }",
    );
    let n = &net.flow_notes[0];
    assert!(n.from.is_none() && n.to.is_none(), "{n:?}");
}

/// Boxing `a[i], c[i]` and `b[j], d[j]` keeps the transfer `a[i], b[j]`
/// side by side: `a` ends its box and `b` begins the next.
#[test]
fn boxing_instances_keeps_a_transfers_stations_together() {
    let (p, net) = instance_net(
        "stage a[2] : ps(1); stage b[2] : ps(1); stage c[2] : delay; stage d[2] : delay; use \"std/share\";",
        "run a[i], b[j] (cost(a, b, 1), share = share.maxmin); run c[i] (cost(c, 1)); run d[j] (cost(d, 1));",
    );
    let at = |name: &str| net.node_of(stage(&p, name)).unwrap();
    assert_eq!(net.flows, vec![vec![at("a"), at("b")]]);
    assert_eq!(at("b"), at("a") + 1, "side by side across the boundary");
    assert_eq!(net.instances.len(), 2);
    assert!(net.spans(&net.flows[0]));
    for g in &net.instances {
        let mut k = g.nodes.clone();
        k.sort_unstable();
        assert!(k.windows(2).all(|w| w[1] == w[0] + 1), "{k:?}");
    }
}

/// An enclosure that starts in an instance and runs on past it is not
/// taken into the box: the box would cover the station outside.
#[test]
fn a_box_takes_in_only_the_enclosures_wholly_in_it() {
    let (p, net) = instance_net(
        "pool g { cap 9; } stage A[2] : delay; stage B[2] : delay; stage S : delay;",
        "hold g (cost(g, 1)) { run A[i] (cost(A, 1)); run B[i] (cost(B, 1)); run S (cost(S, 1)); }",
    );
    let s = net.node_of(stage(&p, "S")).unwrap();
    assert_eq!(net.instances.len(), 1);
    assert_eq!(net.instance_of(s), None);
    let f = deployment::layout(&p, &net);
    let b = f.boxes(BoxStyle::Instance)[0];
    assert!(!b.contains(&f.stations()[s].0));
}

/// Every instance box and every pool enclosure are apart or one holds the
/// other: an enclosure that crosses a box draws a hold the box denies.
fn boxes_do_not_cross(p: &Program, net: &deployment::Net) {
    let f = deployment::layout(p, net);
    for b in f.boxes(BoxStyle::Instance) {
        for e in f.boxes(BoxStyle::Enclosure) {
            assert!(
                !b.overlaps(&e) || b.contains(&e) || e.contains(&b),
                "instance {b:?} crosses enclosure {e:?}"
            );
        }
    }
}

/// A reply from `j` to `i` after `i` sent to `j` cannot stand side by side
/// with both boxes: it stays an arrow between them, and keeps its stations
/// in their boxes (#216 review).
#[test]
fn a_transfer_back_stays_between_the_boxes() {
    let (p, net) = instance_net(
        "pool kp[2] { cap 9; } pool kd[2] { cap 9; }
         stage P[2] : delay; stage D[2] : delay;
         stage a[2] : ps(1); stage b[2] : ps(1); stage a2[2] : ps(1); stage b2[2] : ps(1);
         use \"std/share\";",
        "hold kp[i] (cost(kp, 1)) { run P[i] (cost(P, 1)); run a[i], b[j] (cost(a, b, 1), share = share.maxmin); }
         hold kd[j] (cost(kd, 1)) { run D[j] (cost(D, 1)); run b2[j], a2[i] (cost(b2, a2, 1), share = share.maxmin); }",
    );
    let at = |name: &str| net.node_of(stage(&p, name)).unwrap();
    assert_eq!(net.flows.len(), 2);
    assert!(net.flows.iter().all(|g| net.spans(g)));
    assert_eq!(net.instance_of(at("a2")), net.instance_of(at("P")));
    assert_eq!(net.instance_of(at("b2")), net.instance_of(at("D")));
    boxes_do_not_cross(&p, &net);
}

/// A stage the session indexes by `i` and by a constant is in no box, and
/// `i`'s pool is not drawn there: its enclosure would cross `i`'s box
/// (#216 review).
#[test]
fn an_instances_pool_is_drawn_only_in_its_box() {
    let (p, net) = instance_net(
        "pool kp[2] { cap 9; } stage P[2] : delay; stage a[2] : delay;",
        "hold kp[i] (cost(kp, 1)) { run P[i] (cost(P, 1)); run a[i] (cost(a, 1)); run P[0] (cost(P, 1)); }",
    );
    let at = |name: &str| net.node_of(stage(&p, name)).unwrap();
    assert_eq!(net.instance_of(at("P")), None);
    assert!(net.drawn_pools(at("P")).is_empty());
    assert_eq!(net.drawn_pools(at("a")), [pool(&p, "kp")]);
    boxes_do_not_cross(&p, &net);
}

/// After a branch whose arms end on different flows, a `load` is noted on
/// neither.
#[test]
fn after_a_branch_a_load_follows_no_arms_transfer() {
    let (_, net) = instance_net(
        "pool q[2] { cap 9; } stage P[2] : delay; stage a[2] : ps(1); stage b[2] : ps(1);
         use \"std/share\";",
        "set c = ~bernoulli(0.5);
         hold q[j] (cost(q, 1)) {
           run a[i], b[j] (cost(a, b, 1), share = share.maxmin);
           if (c) { run P[i] (cost(P, 1)); } else { }
           load q[j] (cost(q, 1));
         }",
    );
    let n = &net.flow_notes[0];
    assert!(n.from.is_none() && n.to.is_none(), "{n:?}");
}

/// A loop body can run zero times: an assignment inside it cannot decide
/// the branch after it or remove that branch's other station from the view.
#[test]
fn a_while_body_does_not_determine_its_continuation() {
    let src = common::main_source(
        "stage a : delay; stage b : delay; stage c : delay;
      server {
        set flag = 0;
        while (flag) { run a (cost(a, 1)); set flag = 1; }
        if (flag) { run b (cost(b, 1)); } else { run c (cost(c, 1)); }
      }
      ",
    );
    let p = common::compile(
        &src,
        "workload { arrive batch(1); }",
        &common::horizon(10.0),
    )
    .unwrap();
    let net = deployment::project(&p);
    assert!(net.node_of(stage(&p, "c")).is_some(), "zero passes reach c");
}

/// The figure is addressable as well as a picture: the SVG names each
/// station's stage (`data-stage`, an array's members by their shared
/// name), each queue glyph's and drum's pool (`data-pool`, with which of
/// the two it is), the arrival and exit arrows (an exit with the stage it
/// leaves) and an engine's budget bar (`data-role`), so that a page
/// animating a run over the figure finds them by what the program calls
/// them, not by where the layout put them. A decision diamond and a pool's
/// tick are no stage and get no name. The TikZ writer ignores the ids.
#[test]
fn the_svg_names_stages_pools_and_roles() {
    use serq::view::figure::Item;
    for name in ["llmd_nixl_pull", "thunderagent"] {
        let p = program(name);
        let net = deployment::project(&p);
        let f = deployment::layout(&p, &net);
        // a station's stage is its node's: the stages the session reaches
        let want: std::collections::BTreeSet<String> = net
            .nodes
            .iter()
            .filter_map(|n| n.stage.map(|st| p.stages[st].name.clone()))
            .collect();
        let mut got = std::collections::BTreeSet::new();
        for i in &f.items {
            if let Item::Station { kind, stage, .. } = i {
                match stage {
                    Some(st) => {
                        got.insert(st.clone());
                    }
                    None => assert!(
                        *kind == StationKind::Decision || *kind == StationKind::Delay,
                        "{name}: a station without a stage is a decision or a tick: {i:?}"
                    ),
                }
            }
        }
        // a stacked instance's stations are named by their members'
        let families: std::collections::BTreeSet<String> = got.iter().map(|s| family(s)).collect();
        assert_eq!(families, want, "{name}");
        let svg = serq::view::render(&f, "svg").unwrap();
        for st in &got {
            assert!(
                svg.contains(&format!(r#"<g data-node="{st}" data-stage="{st}">"#)),
                "{name}: {st}"
            );
        }
        // one group per station, every group closed
        assert_eq!(svg.matches(" data-stage=").count(), got.len(), "{name}");
        assert_eq!(svg.matches("<g data-").count(), svg.matches("</g>").count());
        // a pool's groups are told apart by their role
        for m in svg.match_indices("<g data-pool=") {
            let line = &svg[m.0..svg[m.0..].find('>').unwrap() + m.0];
            assert!(
                line.contains(r#"data-role="queue""#) || line.contains(r#"data-role="drum""#),
                "{name}: {line}"
            );
        }
        assert!(svg.contains(r#"data-role="budget""#), "{name}");
        assert!(svg.contains(r#"data-role="arrival""#), "{name}");
        let tikz = serq::view::render(&f, "tikz").unwrap();
        assert!(!tikz.contains("data-"), "{name}");
    }
    // llmd_nixl_pull: the last station is `D`, and the session leaves from
    // it (`docs/use-cases/pd.md`: the decoder answers), from each of the
    // `ND = 2` decoders drawn; the program also ends a session at the
    // router when the context is full (`end` under `K + n >=
    // max_model_len`), an exit from the decision, with no stage
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    let svg = serq::view::render(&deployment::layout(&p, &net), "svg").unwrap();
    assert!(svg.contains(r#"data-role="exit" data-from="D[0]""#));
    assert!(svg.contains(r#"data-role="exit" data-from="D[1]""#));
    assert!(svg.contains(r##"data-role="exit" data-from="#0""##));
    assert_eq!(svg.matches(r#"data-role="exit""#).count(), 3);
    // sessions arrive at the router's decision, the first node
    assert!(svg.contains(r##"data-role="arrival" data-to="#0""##));
    // every arrow between two nodes says which: the net's four edges
    // between nodes (the decision to `P` and to `D`, `P` to its NIC, the
    // decoder's NIC to `D`) for each member, two each, and the run that
    // spans the two instances (`P.nic` to `D.nic`, the flow's stages in the
    // order they are named) from each prefiller to each decoder, four
    let between = net
        .edges
        .iter()
        .filter(|e| matches!((e.from, e.to), (End::Node(_), End::Node(_))))
        .count();
    assert_eq!(between, 4);
    let spanning = net.flows.iter().filter(|g| net.spans(g)).count();
    assert_eq!(spanning, 1);
    assert_eq!(svg.matches(r#"data-role="flow""#).count(), 2 * between + 4);
    // every name an arrow uses is a node in the figure, and the bracket
    // names the two NICs the transfer holds at once
    for m in svg
        .match_indices("data-from=\"")
        .chain(svg.match_indices("data-to=\""))
    {
        let rest = &svg[m.0 + m.1.len()..];
        let name = &rest[..rest.find('"').unwrap()];
        assert!(
            svg.contains(&format!(r#"<g data-node="{name}""#)),
            "{name} is not a node"
        );
    }
    // and a copy between two stacked instances has no bracket
    assert!(!svg.contains(r#"data-role="bracket""#));
    for (i, j) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        assert!(svg.contains(&format!(
            r#"data-role="flow" data-from="P.nic[{i}]" data-to="D.nic[{j}]""#
        )));
    }
    // the decision to each prefiller (`remote_prefill`), and to each decoder
    // alone (`else`); the next turn goes through the workload, which the
    // figure omits, so no arrow returns from `D`
    for m in 0..2 {
        assert!(svg.contains(&format!(
            r##"data-role="flow" data-from="#0" data-to="P[{m}]""##
        )));
        assert!(svg.contains(&format!(
            r##"data-role="flow" data-from="#0" data-to="D[{m}]""##
        )));
        assert!(!svg.contains(&format!(r#"data-from="D[{m}]" data-to="#)));
    }
    // a transfer whose NICs sit side by side has a bracket and an arrow
    // inside it, in the order the run names them: the push mode's NICs are
    // in no instance, and each still has an arrow in and one out
    let p = program("vllm_nixl_push");
    let svg = serq::view::render(&deployment::figure(&p), "svg").unwrap();
    assert!(svg.contains(r#"data-role="bracket" data-nodes="P.nic D.nic""#));
    assert!(svg.contains(r#"data-role="flow" data-from="P.nic" data-to="D.nic""#));
    // the push's `join`: the prefill leg reaches the copy at the source's
    // NIC, the session (the decode request) at the reader's, where its
    // blocks wait for the bytes; the router sends nothing into `P.nic`
    assert!(svg.contains(r#"data-role="flow" data-from="P" data-to="P.nic""#));
    assert!(svg.contains(r#"data-role="flow" data-from="front" data-to="D.nic""#));
    assert!(!svg.contains(r#"data-from="front" data-to="P.nic""#));
    assert!(!svg.contains(r#"data-from="P" data-to="D.nic""#));
    // thunderagent: its queue `waiting_queue` is drawn as a queue glyph and a
    // drum, one pool under two roles, and its tick is a station of no stage
    let p = program("thunderagent");
    let svg = serq::view::render(&deployment::figure(&p), "svg").unwrap();
    assert!(svg.contains(r#"<g data-pool="waiting_queue" data-role="queue">"#));
    assert!(svg.contains(r#"<g data-pool="waiting_queue" data-role="drum">"#));
    // its decision and its tick are nodes without a stage
    assert!(svg.contains(r##"<g data-node="#0">"##));
    assert!(svg.contains(r##"<g data-node="#1">"##));
    assert!(svg.contains(r##"data-role="flow" data-from="#0" data-to="#1""##));
}

/// The figure draws a pull and a push apart (`CStmt::Run::poster`): the
/// word leads the copy's note, a dot on its bracket stands over the
/// poster's NIC, and the bracket and the arrow name both. `llmd_nixl_pull.sq`'s decoder reads; `vllm_nixl_push.sq`'s
/// prefiller writes. A `transfer on` written by hand names no poster.
#[test]
fn a_copy_shows_who_posts_it() {
    use serq::view::figure::{CopyMark, Item};
    let marks = |name: &str| -> (Vec<Option<CopyMark>>, Vec<Option<CopyMark>>, usize) {
        let f = deployment::figure(&program(name));
        let mut brackets = vec![];
        let mut arrows = vec![];
        let mut dots = 0;
        for i in f.items {
            match i {
                Item::Box {
                    style: BoxStyle::Flow,
                    copy,
                    ..
                } => brackets.push(copy),
                Item::Edge { copy: Some(c), .. } => arrows.push(Some(c)),
                Item::Dot { .. } => dots += 1,
                _ => {}
            }
        }
        (brackets, arrows, dots)
    };
    // between two stacked instances: no bracket, an arrow from each
    // prefiller to each decoder posted by its decoder, a dot over each
    let pull = |j: usize| {
        Some(CopyMark {
            pull: true,
            poster: format!("D.nic[{j}]"),
        })
    };
    assert_eq!(
        marks("llmd_nixl_pull"),
        (vec![], vec![pull(0), pull(1), pull(0), pull(1)], 2)
    );
    assert!(texts("llmd_nixl_pull").contains(&"pull · P.kv[i] → D.kv[j]".to_string()));
    // the push's two NICs are in no instance: the arrow under the bracket
    // carries the mark as the arrow across a gap does
    let push = CopyMark {
        pull: false,
        poster: "P.nic".into(),
    };
    assert_eq!(
        marks("vllm_nixl_push"),
        (vec![Some(push.clone())], vec![Some(push)], 1)
    );
    assert!(texts("vllm_nixl_push").contains(&"push · latency (0.01)".to_string()));
    let f = deployment::figure(&program("pd_two_fabrics"));
    assert!(
        !f.items.iter().any(|i| matches!(
            i,
            Item::Dot { .. } | Item::Box { copy: Some(_), .. } | Item::Edge { copy: Some(_), .. }
        )),
        "a transfer written by hand names no poster"
    );
}

// --- an instance drawn once per member --------------------------------------

/// `llmd_nixl_pull.sq` with `NP` and `ND` as given.
fn pd_with(np: usize, nd: usize) -> serq::view::figure::Figure {
    let src = std::fs::read_to_string(program_path("llmd_nixl_pull"))
        .unwrap()
        .replace("let NP = 2;", &format!("let NP = {np};"))
        .replace("let ND = 2;", &format!("let ND = {nd};"));
    let w = serq::paired_workload(&program_path("llmd_nixl_pull")).unwrap();
    let workload = std::fs::read_to_string(w).unwrap();
    deployment::figure(&common::compile(&src, &workload, &common::horizon(10.0)).unwrap())
}

/// `choose i in NP` picks one of `NP` pods, and the figure draws the `NP`:
/// a box per member, stacked, each station named by its member (`P[2]`),
/// an arrow from the router to each, and the copy from each prefiller to
/// each decoder. Past eight the figure says how many it left out rather
/// than run on down the page.
#[test]
fn an_instance_is_drawn_once_per_member() {
    use serq::view::figure::{EdgeRole, Item};
    let f = pd_with(3, 1);
    assert_eq!(
        f.boxes(BoxStyle::Instance).len(),
        4,
        "three prefillers, one decoder"
    );
    let flows: Vec<(String, String)> = f
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Edge {
                role: Some(EdgeRole::Flow { from, to }),
                ..
            } => Some((from.clone(), to.clone())),
            _ => None,
        })
        .collect();
    for m in 0..3 {
        let has = |a: String, b: String| flows.contains(&(a, b));
        assert!(has("#0".into(), format!("P[{m}]")), "{flows:?}");
        assert!(has(format!("P[{m}]"), format!("P.nic[{m}]")), "{flows:?}");
        assert!(has(format!("P.nic[{m}]"), "D.nic[0]".into()), "{flows:?}");
    }
    // the members stand one under another, apart
    let station = |name: String| {
        f.items
            .iter()
            .find_map(|i| match i {
                Item::Station { node, rect, .. } if *node == name => Some(*rect),
                _ => None,
            })
            .unwrap()
    };
    let p: Vec<_> = (0..3).map(|m| station(format!("P[{m}]"))).collect();
    assert_eq!(p[0].x, p[1].x);
    assert!(p[0].bottom() < p[1].y && p[1].bottom() < p[2].y);
    let boxes = f.boxes(BoxStyle::Instance);
    for (k, a) in boxes.iter().enumerate() {
        for b in &boxes[k + 1..] {
            assert!(!a.overlaps(b), "{a:?} {b:?}");
        }
    }

    let f = pd_with(10, 2);
    assert_eq!(f.boxes(BoxStyle::Instance).len(), 8 + 2);
    assert!(
        f.items
            .iter()
            .any(|i| matches!(i, Item::Text { text, .. } if text == "8 of 10 members drawn"))
    );
}

/// A KV copy between a family of one and a single queue (#645 A2): the
/// linker indexed the source's NIC only past one member, so `P[1]` with one
/// `D` did not link ("stage `P.nic` is a family of 1; index it"), and the
/// view drew a copy out of a stacked instance into none as a bracket named
/// `P.nic`, leaving `P.nic[0]` with no arrow out. Each way it links, draws,
/// and runs as the program with two single queues does.
#[test]
fn a_copy_between_a_family_of_one_and_one_queue_links_draws_and_runs_alike() {
    let src = std::fs::read_to_string(program_path("vllm_nixl_push")).unwrap();
    let edit = |pairs: &[(&str, &str)]| {
        pairs.iter().fold(src.clone(), |s, (a, b)| {
            assert!(s.contains(a), "{a}");
            s.replace(a, b)
        })
    };
    let source_family = edit(&[
        ("  queue P : prefill {", "  queue P[1] : prefill {"),
        (
            "fork { P.prefill (prompt); }",
            "fork { P[i].prefill (prompt); }",
        ),
        (
            "        P.prefill (prompt);  ",
            "        P[i].prefill (prompt);  ",
        ),
        (
            "      D.decode (prompt) from P;",
            "      D.decode (prompt) from P[i];",
        ),
        (
            "      set prompt = new;\n",
            "      set prompt = new;\n      choose i in 1 by (work(P[i]));\n",
        ),
    ]);
    let reader_family = edit(&[
        ("  queue D : decode {", "  queue D[1] : decode {"),
        (
            "      D.decode (prompt) from P;",
            "      D[j].decode (prompt) from P;",
        ),
        (
            "      set prompt = new;\n",
            "      set prompt = new;\n      choose j in 1 by (work(D[j]));\n",
        ),
    ]);
    // under the workload `examples/pairs.json` pairs the example with, its
    // trace read beside it
    let wpath = serq::paired_workload(&program_path("vllm_nixl_push")).expect("a pair");
    let workload = std::fs::read_to_string(&wpath).unwrap();
    let ov = common::horizon(200.0);
    let parked = |s: &str| {
        let p = common::compile(s, &workload, &ov).unwrap_or_else(|e| panic!("{e}"));
        deployment::checked_figure(&p).unwrap_or_else(|e| panic!("{e}"));
        serq::run_ir(&p, wpath.parent())
            .unwrap()
            .observe("parked")
            .unwrap()
            .samples
            .clone()
    };
    let single = parked(&src);
    assert!(!single.is_empty());
    assert_eq!(parked(&source_family), single);
    assert_eq!(parked(&reader_family), single);
    // the view's half is no matter of size: two prefillers and one decoder
    // drew `P.nic[0]` with no arrow out as well
    let two = source_family
        .replace("queue P[1] : prefill {", "queue P[2] : prefill {")
        .replace("choose i in 1 by", "choose i in 2 by");
    let p = common::compile(&two, &workload, &ov).unwrap_or_else(|e| panic!("{e}"));
    deployment::checked_figure(&p).unwrap_or_else(|e| panic!("{e}"));
}

/// An instance's panel names the stations in it (#645 B1, serq-lar U15):
/// what an editor reads of an instance is the figure's, the one definition,
/// not a second one from the text. Its names are the stations drawn in it,
/// no more and no fewer. `pd_two_fabrics.sq` indexes stages and pools
/// directly, with no entry call: still its two instances, eight panels.
#[test]
fn an_instances_panel_names_its_stations() {
    use serq::view::figure::Item;
    let panels = |path: &str| -> Vec<Vec<String>> {
        let p = common::example(path, &common::horizon(10.0)).unwrap();
        let f = deployment::checked_figure(&p).unwrap();
        let stations: Vec<(&String, _)> = f
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Station { node, rect, .. } => Some((node, rect.centre())),
                _ => None,
            })
            .collect();
        let mut panels = vec![];
        for i in &f.items {
            let Item::Box {
                style: BoxStyle::Instance,
                nodes,
                rect,
                ..
            } = i
            else {
                continue;
            };
            let mut inside: Vec<&String> = stations
                .iter()
                .filter(|(_, c)| {
                    rect.x <= c.x && c.x <= rect.right() && rect.y <= c.y && c.y <= rect.bottom()
                })
                .map(|(n, _)| *n)
                .collect();
            let mut named: Vec<&String> = nodes.iter().collect();
            inside.sort();
            named.sort();
            assert_eq!(named, inside, "{path}");
            panels.push(nodes.clone());
        }
        panels
    };
    for path in [
        "examples/pd-disaggregation/llmd_nixl_pull.sq",
        "examples/pd-disaggregation/pd_batching.sq",
    ] {
        assert!(panels(path).iter().all(|p| !p.is_empty()), "{path}");
    }
    // two instances, each drawn once per member of its families (4), each
    // member's panel naming its own (`P[1] nvl_out[1] nic_out[1]`)
    let two = panels("examples/pd-disaggregation/pd_two_fabrics.sq");
    assert_eq!(two.len(), 8, "{two:?}");
    let mut families: Vec<Vec<String>> = two
        .iter()
        .map(|p| {
            p.iter()
                .map(|n| n[..n.find('[').unwrap()].to_string())
                .collect()
        })
        .collect();
    families.sort();
    families.dedup();
    assert_eq!(
        families,
        [
            vec!["P", "nvl_out", "nic_out"],
            vec!["nvl_in", "nic_in", "D"]
        ]
    );
}

/// A choose is an instance of its own (#685): `pd_batching.sq` chooses `j`
/// once in each arm of an `if`, an `E[j]` in one and a `D[j]` in the other,
/// and no instance holds both, as no request is at both. (`E[j]` alone is
/// one station, which is drawn in no box: the decoder's is the one left.)
#[test]
fn a_variable_chosen_twice_is_two_instances() {
    let p = common::example(
        "examples/pd-disaggregation/pd_batching.sq",
        &common::horizon(10.0),
    )
    .unwrap();
    let net = deployment::project(&p);
    let names = |g: &deployment::Instance| -> Vec<String> {
        g.nodes
            .iter()
            .filter_map(|&i| net.nodes[i].stage.map(|s| p.stages[s].name.clone()))
            .collect()
    };
    let js: Vec<Vec<String>> = net
        .instances
        .iter()
        .filter(|g| g.label.starts_with("choose j"))
        .map(names)
        .collect();
    assert_eq!(js, [vec!["D.nic".to_string(), "D".to_string()]]);
}

/// But two chooses of one variable that pick for the same stations are one
/// instance (#692 review): a choose in a `while`'s body, which the view
/// walks twice, and a `j` chosen in each arm of an `if` and read after it.
#[test]
fn chooses_of_one_variable_over_the_same_stations_are_one_instance() {
    for src in [
        "stage A[2] : delay; stage B[2] : delay; server { set k = 0; while (k < 2) { \
         choose j in 2 by (0); run A[j] (cost(A, 1)); run B[j] (cost(B, 1)); set k = k + 1; } }",
        "stage A[2] : delay; stage B[2] : delay; server { set c = ~bernoulli(0.5); if (c) { \
         choose j in 2 by (0); run A[j] (cost(A, 1)); } else { choose j in 2 by (1); \
         run A[j] (cost(A, 1)); } run B[j] (cost(B, 1)); }",
    ] {
        let p = compile(src, ONE);
        let net = deployment::project(&p);
        let boxes: Vec<Vec<String>> = net
            .instances
            .iter()
            .map(|g| {
                g.nodes
                    .iter()
                    .filter_map(|&i| net.nodes[i].stage.map(|s| p.stages[s].name.clone()))
                    .collect()
            })
            .collect();
        assert_eq!(boxes, [vec!["A".to_string(), "B".to_string()]], "{src}");
    }
}
