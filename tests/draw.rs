//! Tests for `serq draw`: the projection, the geometry and the writers.
//!
//! The assertions are on the `Figure` and on the projected `Net`, not on the
//! rendered bytes, for the same reason `tsncd` unit-tests its geometry rather
//! than its images: a coordinate that moved is a diagnosable failure, and a
//! diff of two SVGs is not. The golden files at the bottom guard the writers.

use serq::ir::Program;
use serq::view::deployment::{self, End};
use serq::view::figure::{BoxStyle, Figure, StationKind};
use serq::{Overrides, compile_source, compile_source_at, program_path};

const PROGRAMS: [&str; 8] = [
    "mg1",
    "ps",
    "closed",
    "replica",
    "routing",
    "vllm",
    "vllm_request",
    "llmd_nixl_pull",
];

fn program(name: &str) -> Program {
    let path = program_path(name);
    let src = std::fs::read_to_string(&path).unwrap();
    compile_source_at(&src, path.parent(), &Overrides::default())
        .unwrap_or_else(|e| panic!("{name}: {e}"))
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

// --- the station kinds -----------------------------------------------------

/// Each stage kind has its glyph: `mg1.sq` is one FIFO server, `ps.sq` one
/// processor-sharing server, and `llmd_nixl_pull.sq` has a step engine on each
/// side, a processor-sharing NIC on each and a delay for the tool call.
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
        ("egress", StationKind::Ps),
        ("ingress", StationKind::Ps),
        ("tool", StationKind::Delay),
    ] {
        let i = net
            .node_of(stage(&p, name))
            .expect("stage is on the session");
        assert_eq!(net.nodes[i].kind, kind, "{name}");
    }
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

/// A chain of guards that moves nobody must not multiply the paths out.
/// `routing.sq` has five sibling `branch (policy == k)` blocks.
#[test]
fn guard_chains_do_not_multiply_edges() {
    let p = program("routing");
    let net = deployment::project(&p);
    assert_eq!(net.nodes.len(), 3, "link, rep[j], tool");
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

fn compile(src: &str) -> Program {
    compile_source(src, &Overrides::default()).expect("the fixture compiles")
}

/// `grow` advances the *innermost* hold holding the pool, so a `growing` run
/// nested inside another hold still belongs to the outer one. Walking only
/// `Run`/`Branch`/`Loop` misses it and the figure claims a prefix cache on
/// pools the interpreter never caches in.
#[test]
fn growing_is_found_through_nested_holds() {
    let p = compile(
        r#"
        pool kv { cap 100000; } pool reqs { cap 8; } pool gate { cap 4; }
        stage engine : step { budget 512; cost 1e-3; memory kv; }
        workload { arrive poisson(0.2); turn { set n = 100; set o = 2; } }
        session { turn;
          hold kv (32), reqs (1) {
            hold gate (1) { run engine prefill (n) growing kv; }
          } cache (n + o);
          end; }
        run { horizon 200; }
        "#,
    );
    let net = deployment::project(&p);
    assert_eq!(net.cached, vec![pool(&p, "kv")], "reqs is never cached in");
}

/// A pool held around two stations with an unheld one between them gets one
/// enclosure per run, not one box swallowing the station in the middle.
#[test]
fn disjoint_holds_of_one_pool_get_separate_enclosures() {
    let p = compile(
        r#"
        pool kv { cap 100; }
        stage s1 : fifo; stage s2 : delay; stage s3 : fifo;
        workload { arrive poisson(0.2); }
        session { hold kv (1) { run s1 (1); } run s2 (1); hold kv (1) { run s3 (1); } end; }
        run { horizon 100; }
        "#,
    );
    let net = deployment::project(&p);
    let f = deployment::layout(&p, &net);
    assert_eq!(f.boxes(BoxStyle::Enclosure).len(), 2);
    // and the station in the middle is in neither
    let middle = f.stations()[1].0;
    assert!(
        f.boxes(BoxStyle::Enclosure)
            .iter()
            .all(|b| !b.contains(&middle))
    );
}

/// Feedback edges, early exits and second entry points all need a lane below
/// the station row, and must not be given the same one.
#[test]
fn lanes_below_the_row_do_not_collide() {
    for name in PROGRAMS {
        let p = program(name);
        let f = deployment::figure(&p);
        let mut ys: Vec<f64> = f
            .items
            .iter()
            .filter_map(|i| match i {
                serq::view::figure::Item::Edge { pts, .. } if pts.len() >= 3 => Some(pts[1].y),
                _ => None,
            })
            .collect();
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

/// A session that opens with a branch has more than one entry station. A second
/// arrow along the row would run through the first one, and a second copy of
/// the arrival label would be drawn on top of the first.
#[test]
fn a_second_entry_point_does_not_overdraw_the_first() {
    let p = compile(
        r#"
        stage s1 : fifo; stage s2 : fifo;
        workload { arrive poisson(1); init { set a = 1; } }
        session { branch (a) { run s1 (1); } else { run s2 (1); } end; }
        run { horizon 100; }
        "#,
    );
    let net = deployment::project(&p);
    assert_eq!(
        net.edges.iter().filter(|e| e.from == End::Arrival).count(),
        2
    );
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
        workload { arrive batch(1); turn { set a = 2; } }
        session { turn; observe o = k ^ a; run s (1); end; }
        run { horizon 10; }
        "#,
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
/// `replica.sq` has no `growing` and really does keep a unit of `batch`.
#[test]
fn cache_targets_follow_the_release_rule() {
    let p = program("vllm");
    let net = deployment::project(&p);
    assert_eq!(net.cached, vec![pool(&p, "kv")], "growing kv, so not reqs");

    let p = program("replica");
    let net = deployment::project(&p);
    let mut cached = net.cached.clone();
    cached.sort_unstable();
    let mut want = vec![pool(&p, "batch"), pool(&p, "kv")];
    want.sort_unstable();
    assert_eq!(cached, want);
}

/// Pools held around every visit to a stage, and only those.
#[test]
fn nested_holds_nest() {
    let p = program("replica");
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "engine"), ["live", "batch", "kv"]);
    assert_eq!(
        pools_of(&p, &net, "tool"),
        ["live"],
        "the tool call keeps its slot"
    );
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
         workload { arrive batch(1); init { set c = 1; } }
         session { hold p (1) { branch (c) { release p; run s1 (1); } else { run s2 (1); } run s3 (1); } end; }
         run { horizon 10; }",
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
         workload { arrive batch(1); }
         session { hold p (1) { run s1 (1); } lease p (inf); run s2 (1); release p; run s3 (1); end; }
         run { horizon 10; }",
    );
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "s1"), ["p"]);
    assert_eq!(pools_of(&p, &net, "s2"), ["p"]);
    assert!(pools_of(&p, &net, "s3").is_empty());
}

/// `examples/pd-disaggregation/llmd_nixl_pull.sq`: the prompt's KV is in the prefiller's pool
/// through the transfer (leased past its scope) and in the decoder's from
/// the transfer on, so the read's two stations (the prefiller's NIC and the
/// decoder's, held at once) are inside both enclosures, the prefill station
/// in the prefiller's only and the decode station in the decoder's only.
/// The prefiller's request slot ends with its scope, so it encloses the
/// prefill station alone. The decoder's slot is only reserved during the
/// transfer (`reqsD[j] (0) reserve (1)`: the request is parked, not
/// running, `scheduler.py:1264-1268`), so it encloses the decode station
/// and not the read.
#[test]
fn a_transfer_puts_the_read_in_both_enclosures() {
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "P"), ["reqsP", "kvP"]);
    for nic in ["egress", "ingress"] {
        assert_eq!(pools_of(&p, &net, nic), ["kvP", "kvD"], "{nic}");
    }
    assert_eq!(pools_of(&p, &net, "D"), ["kvD", "reqsD"]);
    assert!(pools_of(&p, &net, "tool").is_empty());
    let f = deployment::layout(&p, &net);
    let boxes = f.boxes(BoxStyle::Enclosure);
    for nic in ["egress", "ingress"] {
        let (rect, _) = f.stations()[net.node_of(stage(&p, nic)).unwrap()];
        assert_eq!(
            boxes.iter().filter(|b| b.contains(&rect)).count(),
            2,
            "{nic}"
        );
    }
    let (eg, ing) = (
        net.node_of(stage(&p, "egress")).unwrap(),
        net.node_of(stage(&p, "ingress")).unwrap(),
    );
    assert_eq!(net.flows, vec![vec![eg, ing]]);
}

fn pools_at(src: &str, stage_name: &str) -> Vec<String> {
    let src = format!(
        "pool p {{ cap 2; }} stage A : delay; workload {{ arrive poisson(1); }}
         session {{ {src} }} run {{ horizon 1; }}"
    );
    let p = compile_source(&src, &Overrides::default()).unwrap();
    let net = deployment::project(&p);
    pools_of(&p, &net, stage_name)
}

/// A hold of no units reserves and occupies nothing: it draws no boundary.
#[test]
fn a_reservation_only_hold_encloses_nothing() {
    assert!(pools_at("hold p (0) reserve (1) { run A (1); }", "A").is_empty());
    assert_eq!(pools_at("hold p (1) { run A (1); }", "A"), ["p"]);
}

/// A `release` gives back the innermost hold of its pool (`interp.rs`), so
/// after releasing a reservation the outer hold still encloses - which a
/// view that left the reservation off the hold stack would get wrong.
#[test]
fn a_release_takes_the_innermost_hold_even_of_no_units() {
    assert_eq!(
        pools_at("hold p (1) { hold p (0) { release p; run A (1); } }", "A"),
        ["p"]
    );
}

/// `examples/pd-disaggregation/llmd_nixl_pull.sq`'s router sends a request either
/// to a prefiller and over the two NICs (remote), after the read's fixed
/// wait (`setup`), or straight to the decoder (local); a request whose KV
/// is already there skips both. Every turn
/// ends at the decoder, which the session leaves or resumes after a tool call.
#[test]
fn the_router_branches_to_a_remote_or_a_local_prefill() {
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    let at = |name: &str| End::Node(net.node_of(stage(&p, name)).unwrap());
    let (pf, setup, eg, ing, d, tool) = (
        at("P"),
        at("setup"),
        at("egress"),
        at("ingress"),
        at("D"),
        at("tool"),
    );
    assert!(net.has_edge(End::Arrival, pf), "remote");
    assert!(net.has_edge(End::Arrival, d), "local");
    assert!(net.has_edge(pf, setup));
    assert!(net.has_edge(setup, eg));
    assert!(net.has_edge(ing, d));
    assert!(!net.has_edge(eg, ing), "held at once, not passed in turn");
    assert!(net.has_edge(pf, d), "the KV is already on the decoder");
    assert!(net.has_edge(d, tool), "more");
    assert!(net.has_edge(d, End::Exit));
    assert!(net.has_edge(tool, pf), "next turn, remote");
    assert!(net.has_edge(tool, d), "next turn, local");
    assert!(net.arrival.contains("Poisson"));
}

// --- loops ------------------------------------------------------------------

/// A session program over stages `A`, `B`, `C`, projected.
fn shape(session: &str) -> (Program, deployment::Net) {
    let src = format!(
        "stage A : delay; stage B : delay; stage C : delay;
         workload {{ arrive poisson(1); }}
         session {{ {session} }}
         run {{ horizon 1; }}"
    );
    let p = compile_source(&src, &Overrides::default()).unwrap();
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

/// A loop body that starts with a branch is re-entered down every arm, and
/// the arrow back says which.
#[test]
fn a_loop_returns_to_every_arm() {
    let (p, net) = shape(
        "loop { set a = ~bernoulli(0.5); branch (a) { run A (1); } else { run B (1); } run C (1); }",
    );
    for (to, label) in [("A", "a"), ("B", "else")] {
        let e = edge(&p, &net, "C", to).unwrap_or_else(|| panic!("C -> {to}"));
        assert!(e.back, "C -> {to} returns");
        assert_eq!(e.label.as_deref(), Some(label));
    }
}

/// A loop whose body is a loop: each arm follows the other.
#[test]
fn a_nested_loop_returns_to_every_arm() {
    let (p, net) = shape(
        "run C (1); loop { loop { set a = ~bernoulli(0.5); branch (a) { run A (1); } else { run B (1); } } }",
    );
    assert!(edge(&p, &net, "A", "B").is_some());
    assert!(edge(&p, &net, "B", "A").is_some());
    assert!(edge(&p, &net, "C", "A").is_some() && edge(&p, &net, "C", "B").is_some());
}

/// An arm with no station of its own reaches the station after the branch:
/// the loop comes back to `A` down the other arm, and down the empty one it
/// would come back to `B` itself, which is a visit, not an edge.
#[test]
fn a_loop_through_an_empty_arm() {
    let (p, net) =
        shape("loop { set a = ~bernoulli(0.5); branch (a) { } else { run A (1); } run B (1); }");
    assert!(edge(&p, &net, "B", "A").is_some_and(|e| e.back));
    assert!(edge(&p, &net, "B", "B").is_none());
    assert!(edge(&p, &net, "arrival", "B").is_some());
}

/// A body that can end before its first station ends from wherever the
/// loop came from: the arrival the first time, the last station after.
#[test]
fn a_loop_that_can_end_before_a_station() {
    let (p, net) =
        shape("loop { set c = ~bernoulli(0.5); branch (c) { end; } run A (1); run B (1); }");
    assert!(edge(&p, &net, "arrival", "exit").is_some());
    assert!(edge(&p, &net, "B", "exit").is_some());
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
        // `layout` pushes one station per node, in node order.
        let boxes = f.boxes(BoxStyle::Enclosure);
        let stations = f.stations();
        assert_eq!(
            stations.len(),
            net.nodes.len(),
            "{name}: a station per node"
        );
        for (node, (rect, _)) in net.nodes.iter().zip(stations) {
            let inside = boxes.iter().filter(|b| b.contains(&rect)).count();
            assert_eq!(
                inside,
                node.pools.len(),
                "{name}: {} is in {inside} boxes, wants {}",
                node.label,
                node.pools.len()
            );
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

fn golden(name: &str, got: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var("SERQ_BLESS").is_ok() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}\nrun `make draw-golden`", path.display()));
    assert!(
        want == got,
        "{name} changed; check the figure and run `make draw-golden`"
    );
}

/// The writers' output, byte for byte. Deterministic: the layout takes no
/// clock and no RNG.
#[test]
fn golden_files_are_current() {
    let p = program("vllm");
    golden(
        "vllm.deployment.svg",
        &serq::view::svg::render(&deployment::figure(&p)),
    );
    let p = program("llmd_nixl_pull");
    golden(
        "llmd_nixl_pull.deployment.svg",
        &serq::view::svg::render(&deployment::figure(&p)),
    );
    docs_assets_are_current();
}

/// The figures the site shows are the program's figure, not a copy that
/// once was: every `docs/assets/NAME.deployment.svg` is what `seq-lang draw`
/// makes of `examples/*/NAME.seq` or `docs/tutorial/programs/NAME.seq`
/// now, and `make draw-golden` rewrites them with the goldens. A figure
/// without its program is an error, not a keepsake.
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
            .join(format!("{stem}.seq"));
        let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(root.join("examples"))
            .unwrap()
            .flatten()
            .map(|g| g.path().join(format!("{stem}.seq")))
            .filter(|p| p.is_file())
            .collect();
        if tutorial.is_file() {
            found.push(tutorial);
        }
        // one name, one program: two would leave the figure's source to
        // the order the directories are read in
        assert!(
            found.len() <= 1,
            "docs/assets/{name}: several programs are {stem}.seq: {found:?}"
        );
        let src_path = found
            .pop()
            .unwrap_or_else(|| panic!("docs/assets/{name} has no program: {stem}.seq"));
        let src = std::fs::read_to_string(&src_path).unwrap();
        let p = seq::compile_file(&src, &src_path, &Overrides::default())
            .unwrap_or_else(|e| panic!("{}: {e}", src_path.display()));
        let got = seq::view::svg::render(&deployment::figure(&p));
        if std::env::var("SEQ_BLESS").is_ok() {
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

/// `routing.sq`'s next turn migrates or stays: back to the link, and
/// straight back to a replica. `pd_tandem.sq`'s job re-enters down both
/// arms of its `mode` branch; the projection is structural, so it draws
/// both although `mode` is one constant in a run.
#[test]
fn the_examples_return_down_every_arm() {
    let p = program("routing");
    let net = deployment::project(&p);
    assert!(edge(&p, &net, "tool", "link").is_some_and(|e| e.back));
    assert!(edge(&p, &net, "tool", "rep").is_some_and(|e| e.back));
    let p = program("pd_tandem");
    let net = deployment::project(&p);
    assert!(edge(&p, &net, "decode", "agg").is_some_and(|e| e.back));
    assert!(edge(&p, &net, "decode", "prefill").is_some_and(|e| e.back));
    assert!(edge(&p, &net, "agg", "prefill").is_some());
}

/// A run over several stages is one job at several stations: they are
/// bracketed together, no arrow runs between them, and the session comes in
/// at the first and leaves from the last.
#[test]
fn a_run_over_several_stages_is_one_bracketed_job() {
    let src = "pool kvP { cap 100; } pool kvD[2] { cap 100; }
               stage P : delay; stage egress : ps(1); stage ingress[2] : ps(1); stage D[2] : delay;
               share maxmin;
               workload { arrive batch(2); }
               session {
                 set j = serial;
                 hold kvP (10) { run P (1); } lease kvP (inf);
                 hold kvD[j] (10) {
                   transfer on egress, ingress[j] (1) from kvP to kvD[j] (10);
                   run D[j] (1);
                 }
                 end;
               }
               run { horizon 10; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
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

/// A flow's stations stand side by side even when one of them was reached
/// alone before, so the bracket takes in no other station: `ingress` is
/// used alone first, then with `egress`, with `D` between.
#[test]
fn a_flows_stations_are_neighbours_in_the_row() {
    let src = "stage ingress : ps(1); stage D : delay; stage egress : ps(1);
               share maxmin;
               workload { arrive batch(1); }
               session { run ingress (1); run D (1); run egress, ingress (1); end; }
               run { horizon 10; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
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
/// them: `llmd_nixl_pull.sq`'s `P -> D`, for a request whose KV is already
/// on the decoder, passes `setup` and the two NICs.
#[test]
fn an_arrow_past_stations_goes_below_the_row() {
    let p = program("llmd_nixl_pull");
    let net = deployment::project(&p);
    let f = deployment::layout(&p, &net);
    let (pf, d) = (
        net.node_of(stage(&p, "P")).unwrap(),
        net.node_of(stage(&p, "D")).unwrap(),
    );
    let (rp, rd) = (f.stations()[pf].0, f.stations()[d].0);
    let row_bottom = rp.bottom();
    let through = f.items.iter().any(|it| match it {
        serq::view::figure::Item::Edge { pts, .. } => {
            pts.len() == 2 && (pts[0].x - rp.right()).abs() < 1e-9 && (pts[1].x - rd.x).abs() < 1e-9
        }
        _ => false,
    });
    assert!(!through, "no straight arrow from P to D along the row");
    // leaving P's bottom at 0.625 and entering D's at 0.5, in a solid line:
    // no other edge of the figure has those ends
    let below = f.items.iter().any(|it| match it {
        serq::view::figure::Item::Edge { pts, style, .. } => {
            *style == serq::view::figure::EdgeStyle::Flow
                && pts.len() == 4
                && (pts[0].x - (rp.x + rp.w * 0.625)).abs() < 1e-9
                && pts[1].y > row_bottom
                && (pts[3].x - (rd.x + rd.w * 0.5)).abs() < 1e-9
        }
        _ => false,
    });
    assert!(below, "P -> D in a lane below");
}

/// The order a flow imposes decides which way an arrow points: `v -> u`
/// is forward in session order, `u -> v` a return; with `u` pulled next to
/// `a` (the flow `a, u`), `u` stands before `v`, and so `u -> v` is drawn
/// forward and `v -> u` as the return.
#[test]
fn a_reordered_arrow_is_drawn_the_way_it_points() {
    let src = "stage a : ps(1); stage v : delay; stage u : ps(1);
               share maxmin;
               workload { arrive batch(1); }
               session { run a (1); run v (1); run u (1); run v (1); run a, u (1); end; }
               run { horizon 10; }";
    let p = compile_source(src, &Overrides::default()).unwrap();
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
