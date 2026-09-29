//! Tests for `seq-lang draw`: the projection, the geometry and the writers.
//!
//! The assertions are on the `Figure` and on the projected `Net`, not on the
//! rendered bytes, for the same reason `tsncd` unit-tests its geometry rather
//! than its images: a coordinate that moved is a diagnosable failure, and a
//! diff of two SVGs is not. The golden files at the bottom guard the writers.

use seq::ir::Program;
use seq::view::deployment::{self, End};
use seq::view::figure::{BoxStyle, Figure, StationKind};
use seq::{Overrides, compile_source, program_path};

const PROGRAMS: [&str; 8] = [
    "mg1",
    "ps",
    "closed",
    "replica",
    "routing",
    "vllm",
    "vllm_request",
    "llmd_pd",
];

fn program(name: &str) -> Program {
    let src = std::fs::read_to_string(program_path(name)).unwrap();
    compile_source(&src, &Overrides::default()).unwrap_or_else(|e| panic!("{name}: {e}"))
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

/// Each stage kind has its glyph: `mg1.seq` is one FIFO server, `ps.seq` one
/// processor-sharing server, and `llmd_pd.seq` has a step engine on each
/// side, a processor-sharing link and a delay for the tool call.
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
    let p = program("llmd_pd");
    let net = deployment::project(&p);
    for (name, kind) in [
        ("P", StationKind::Step),
        ("D", StationKind::Step),
        ("link", StationKind::Ps),
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
/// stations: `vllm.seq` prefills and decodes at one engine.
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
/// `routing.seq` has five sibling `branch (policy == k)` blocks.
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
                seq::view::figure::Item::Edge { pts, .. } if pts.len() >= 3 => Some(pts[1].y),
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
        .filter(|i| matches!(i, seq::view::figure::Item::Text { text, .. } if text.starts_with("new sessions")))
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
            seq::ir::CStmt::Observe(_, e) => Some(p.show_expr(e)),
            _ => None,
        })
        .expect("the observe is there");
    assert_eq!(printed, "(-2) ^ a");
    // and it means what it says
    let round = seq::frontend::parser::parse_expr(&printed).expect("re-parses");
    assert!(matches!(
        round,
        seq::frontend::ast::Expr::Binary(seq::frontend::ast::BinOp::Pow, ..)
    ));
}

/// `release_hold` caches the growing run's position when a hold grew, and the
/// allocation otherwise, so a hold with `growing` caches in that pool alone.
/// `replica.seq` has no `growing` and really does keep a unit of `batch`.
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

/// `examples/pd-disaggregation/llmd_pd.seq`: the prompt's KV is in the prefiller's pool
/// through the transfer (leased past its scope) and in the decoder's from
/// the transfer on, so the link station is inside both enclosures, the
/// prefill station in the prefiller's only and the decode station in the
/// decoder's only. The prefiller's request slot ends with its scope, so it
/// encloses the prefill station alone.
#[test]
fn a_transfer_puts_the_link_in_both_enclosures() {
    let p = program("llmd_pd");
    let net = deployment::project(&p);
    assert_eq!(pools_of(&p, &net, "P"), ["reqsP", "kvP"]);
    assert_eq!(pools_of(&p, &net, "link"), ["kvP", "kvD", "reqsD"]);
    assert_eq!(pools_of(&p, &net, "D"), ["kvD", "reqsD"]);
    assert!(pools_of(&p, &net, "tool").is_empty());
    let f = deployment::layout(&p, &net);
    let boxes = f.boxes(BoxStyle::Enclosure);
    let link = net.node_of(stage(&p, "link")).unwrap();
    let (rect, _) = f.stations()[link];
    assert_eq!(boxes.iter().filter(|b| b.contains(&rect)).count(), 3);
}

/// `examples/pd-disaggregation/llmd_pd.seq`'s router sends a request either
/// to a prefiller and over the link (remote), or straight to the decoder
/// (local); a request whose KV is already there skips the link. Every turn
/// ends at the decoder, which the session leaves or resumes after a tool call.
#[test]
fn the_router_branches_to_a_remote_or_a_local_prefill() {
    let p = program("llmd_pd");
    let net = deployment::project(&p);
    let at = |name: &str| End::Node(net.node_of(stage(&p, name)).unwrap());
    let (pf, link, d, tool) = (at("P"), at("link"), at("D"), at("tool"));
    assert!(net.has_edge(End::Arrival, pf), "remote");
    assert!(net.has_edge(End::Arrival, d), "local");
    assert!(net.has_edge(pf, link));
    assert!(net.has_edge(link, d));
    assert!(net.has_edge(pf, d), "the KV is already on the decoder");
    assert!(net.has_edge(d, tool), "more");
    assert!(net.has_edge(d, End::Exit));
    assert!(net.has_edge(tool, pf), "next turn");
    assert!(net.arrival.contains("Poisson"));
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
        let svg = seq::view::svg::render(&f);
        assert!(svg.starts_with("<svg"), "{name}: not an svg");
        assert!(svg.ends_with("</svg>\n"));
        let tikz = seq::view::tikz::render(&f);
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
    let tikz = seq::view::tikz::render(&deployment::figure(&p));
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
    if std::env::var("SEQ_BLESS").is_ok() {
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
        &seq::view::svg::render(&deployment::figure(&p)),
    );
    let p = program("llmd_pd");
    golden(
        "llmd_pd.deployment.svg",
        &seq::view::svg::render(&deployment::figure(&p)),
    );
}
