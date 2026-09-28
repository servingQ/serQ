//! Tests for `seq-lang draw`: the projection, the geometry and the writers.
//!
//! The assertions are on the `Figure` and on the projected `Net`, not on the
//! rendered bytes, for the same reason `tsncd` unit-tests its geometry rather
//! than its images: a coordinate that moved is a diagnosable failure, and a
//! diff of two SVGs is not. The golden files at the bottom guard the writers.

use seq::deployment::{self, End};
use seq::draw;
use seq::figure::{BoxStyle, Figure, StationKind};
use seq::ir::Program;
use seq::{Overrides, compile_source, program_path};

const PROGRAMS: [&str; 10] = [
    "mg1",
    "ps",
    "closed",
    "agentic",
    "replica",
    "pd_tandem",
    "lecture_pd",
    "routing",
    "vllm",
    "vllm_request",
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
        .expect("stage is on the route");
    net.nodes[i]
        .pools
        .iter()
        .map(|&x| p.pools[x].name.clone())
        .collect()
}

fn node(net: &deployment::Net, p: &Program, name: &str) -> End {
    End::Node(net.node_of(stage(p, name)).unwrap())
}

// --- the acceptance test ----------------------------------------------------

/// `programs/lecture_pd.seq` is the program of `fig:deployment` in Lecture 1
/// §2 of `serving-queue-theory`. The deployment view must have that figure's
/// topology, which is the one place in this feature with an independently
/// hand-drawn answer key.
#[test]
fn lecture_pd_has_the_topology_of_fig_deployment() {
    let p = program("lecture_pd");
    let net = deployment::project(&p);

    // The four stations of the figure, in the order it reads left to right.
    let names: Vec<&str> = net
        .nodes
        .iter()
        .map(|n| p.stages[n.stage].name.as_str())
        .collect();
    assert_eq!(names, ["prefill", "link", "decode", "tool"]);
    let kinds: Vec<StationKind> = net.nodes.iter().map(|n| n.kind).collect();
    assert_eq!(
        kinds,
        [
            StationKind::Fifo,
            StationKind::Ps,
            StationKind::Ps,
            StationKind::Delay
        ]
    );

    // The instance boundaries are the holds. The figure draws `link` outside
    // both dashed boxes; the program holds `memP` across the transfer, and so
    // does the lecture's own listing, where `run link X` stands between
    // `admit mem_P c` and `free mem_P cache kT`. The generated figure is the
    // one that says so.
    assert_eq!(pools_of(&p, &net, "prefill"), ["memP"]);
    assert_eq!(pools_of(&p, &net, "link"), ["memP"]);
    assert_eq!(pools_of(&p, &net, "decode"), ["memD"]);
    assert!(pools_of(&p, &net, "tool").is_empty());

    // "prefix cache of paused sessions" sits under the prefill instance only.
    assert_eq!(net.cached, vec![pool(&p, "memP")]);

    // Arrivals, the chain, the exit, and the feedback through the tool call.
    let (prefill, link, decode, tool) = (
        node(&net, &p, "prefill"),
        node(&net, &p, "link"),
        node(&net, &p, "decode"),
        node(&net, &p, "tool"),
    );
    assert!(
        net.has_edge(End::Arrival, prefill),
        "new sessions enter at prefill"
    );
    assert!(net.has_edge(prefill, link));
    assert!(net.has_edge(link, decode));
    assert!(net.has_edge(decode, tool), "resume w.p. p");
    assert!(net.has_edge(decode, End::Exit), "ends");
    assert!(net.has_edge(tool, prefill), "next turn, hit or miss");
    assert!(net.arrival.contains("Poisson"));
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
        pool kv { cap 100000; } pool slots { cap 8; } pool gate { cap 4; }
        stage engine : step { budget 512; cost 1e-3; memory kv; }
        workload { arrive poisson(0.2); turn { set n = 100; set o = 2; } }
        route { turn;
          hold kv (32), slots (1) {
            hold gate (1) { run engine prefill (n) growing kv; }
          } cache (n + o);
          end; }
        run { horizon 200; }
        "#,
    );
    let net = deployment::project(&p);
    assert_eq!(net.cached, vec![pool(&p, "kv")], "slots is never cached in");
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
        route { hold kv (1) { run s1 (1); } run s2 (1); hold kv (1) { run s3 (1); } end; }
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
                seq::figure::Item::Edge { pts, .. } if pts.len() >= 3 => Some(pts[1].y),
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

/// A route that opens with a branch has more than one entry station. A second
/// arrow along the row would run through the first one, and a second copy of
/// the arrival label would be drawn on top of the first.
#[test]
fn a_second_entry_point_does_not_overdraw_the_first() {
    let p = compile(
        r#"
        stage s1 : fifo; stage s2 : fifo;
        workload { arrive poisson(1); init { set a = 1; } }
        route { branch (a) { run s1 (1); } else { run s2 (1); } end; }
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
        .filter(|i| matches!(i, seq::figure::Item::Text { text, .. } if text.starts_with("new sessions")))
        .count();
    assert_eq!(labels, 1, "the arrival label is drawn once");
}

/// The spine has to start clear of the control rails, however deep the
/// program's `branch`es and `loop`s go.
#[test]
fn rails_stay_clear_of_the_spine() {
    let p = compile(
        r#"
        stage s : fifo;
        workload { arrive poisson(1); init { set a = 1; } }
        route { branch (a) { branch (a) { branch (a) { branch (a) {
                  branch (a) { run s (1); } } } } } end; }
        run { horizon 100; }
        "#,
    );
    let f = draw::figure(&p, false);
    let left = f
        .boxes(BoxStyle::Body)
        .iter()
        .map(|r| r.x)
        .fold(f64::MAX, f64::min);
    for item in &f.items {
        if let seq::figure::Item::Edge { pts, .. } = item {
            for q in pts {
                assert!(
                    q.x < left,
                    "a rail at x={} reaches the spine at {left}",
                    q.x
                );
            }
        }
    }
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
        route { turn; observe o = k ^ a; run s (1); end; }
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
    let round = seq::parser::parse_expr(&printed).expect("re-parses");
    assert!(matches!(
        round,
        seq::ast::Expr::Binary(seq::ast::BinOp::Pow, ..)
    ));
}

/// `release_hold` caches the growing run's position when a hold grew, and the
/// allocation otherwise, so a hold with `growing` caches in that pool alone.
/// `replica.seq` has no `growing` and really does keep a unit of `batch`.
#[test]
fn cache_targets_follow_the_release_rule() {
    let p = program("vllm");
    let net = deployment::project(&p);
    assert_eq!(net.cached, vec![pool(&p, "kv")], "growing kv, so not slots");

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
    for (i, a) in boxes.iter().enumerate() {
        for b in boxes.iter().skip(i + 1) {
            let nested = a.contains(b) || b.contains(a);
            assert!(
                !a.overlaps(b) || nested,
                "{name}: enclosures cross: {a:?} {b:?}"
            );
        }
    }
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
        for f in [deployment::figure(&p), draw::figure(&p, false)] {
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
}

/// The route view gives a column to every pool a `hold` acquires.
#[test]
fn route_columns_are_the_held_pools() {
    let p = program("replica");
    let cols: Vec<&str> = draw::columns(&p)
        .iter()
        .map(|&i| p.pools[i].name.as_str())
        .collect();
    assert_eq!(cols, ["live", "batch", "kv"]);

    let p = program("routing");
    assert!(draw::columns(&p).is_empty(), "routing.seq holds nothing");
}

/// A width the constants fix is drawn solid; one evaluated at admission is
/// drawn dashed. `vllm.seq` has one of each in the same `hold`.
#[test]
fn band_style_says_when_the_width_is_decided() {
    let p = program("vllm");
    let f = draw::figure(&p, false);
    assert_eq!(f.boxes(BoxStyle::Solid).len(), 1, "hold slots (1)");
    assert_eq!(
        f.boxes(BoxStyle::Admission).len(),
        1,
        "hold kv (c + min(prompt - c, budget_left(engine)))"
    );
    assert!(
        !f.boxes(BoxStyle::Cached).is_empty(),
        "cache (prompt + o) leaves a tail"
    );
}

// --- the writers ------------------------------------------------------------

/// Every program renders in both views and both formats without panicking,
/// and the output is not empty. This is what `make check` runs.
#[test]
fn every_program_renders() {
    for name in PROGRAMS {
        let p = program(name);
        for f in [deployment::figure(&p), draw::figure(&p, true)] {
            let svg = seq::svg::render(&f);
            assert!(svg.starts_with("<svg"), "{name}: not an svg");
            assert!(svg.ends_with("</svg>\n"));
            let tikz = seq::tikz::render(&f);
            assert!(
                tikz.contains("\\begin{tikzpicture}"),
                "{name}: not a tikzpicture"
            );
            assert!(tikz.ends_with("\\end{tikzpicture}\n"));
        }
    }
}

/// TikZ output goes into a LaTeX document, so every special character in a
/// label has to survive the trip.
#[test]
fn tikz_escapes_labels() {
    let p = program("vllm");
    let tikz = seq::tikz::render(&deployment::figure(&p));
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
    let p = program("lecture_pd");
    golden(
        "lecture_pd.deployment.tex",
        &seq::tikz::render(&deployment::figure(&p)),
    );
    golden(
        "lecture_pd.deployment.svg",
        &seq::svg::render(&deployment::figure(&p)),
    );
    let p = program("vllm");
    golden(
        "vllm.deployment.svg",
        &seq::svg::render(&deployment::figure(&p)),
    );
    golden(
        "vllm.route.svg",
        &seq::svg::render(&draw::figure(&p, false)),
    );
}
