//! # serQ
//!
//! A serQ program describes an LLM serving deployment: memory pools and
//! stages, and the server every request runs; the workload of sessions is a
//! file of its own, paired with it when the program runs.
//! Its definition is the IR (`ir::Program`, `docs/ir.md`): `engine::interp` runs it,
//! the Lean model is generated from it, and tools build or edit it as data.
//! The text syntax (`frontend`; `docs/language.md`) is one frontend
//! that compiles to it. `examples/` holds example deployments, among them
//! vLLM v1.
//!
//! ```
//! let deployment = std::fs::read_to_string("examples/single-turn/mg1.sq").unwrap();
//! let workload = std::fs::read_to_string("examples/workloads/mg1.sq").unwrap();
//! let options = serq::Overrides {
//!     horizon: Some(100.0), warmup: Some(10.0), ..Default::default()
//! };
//! let names = ("mg1.sq", "workloads/mg1.sq");
//! let report = serq::run_paired(&deployment, &workload, names, &options, None).unwrap();
//! println!("{}", report.text());
//! ```

pub mod engine;
pub mod frontend;
pub mod ir;
pub mod target;
pub mod view;

use std::path::Path;

pub use engine::dist::Dist;
pub use engine::report::Report;
pub use engine::stats::Estimate;
pub use frontend::link::{Linked, Overrides};
pub use ir::Program;

/// Compile program text to IR (parse and link; declared inputs are bound).
/// The version of this serq, as `serq --version` prints it and `Report::json`
/// records it (`serq_version`): the one version of `Cargo.toml`
/// (`scripts/version.py`), which pyserq inherits.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn compile_source(src: &str, ov: &Overrides) -> Result<ir::Program, String> {
    let prog =
        frontend::parser::parse_with(src, &ov.defs, &overridden(ov)).map_err(|e| e.render(src))?;
    finish(
        prog,
        &|span, msg| match span {
            Some(span) => span.render(src, msg),
            None => msg.to_string(),
        },
        ov,
    )
}

/// Compile a deployment file paired with a workload file
/// (`frontend::paired`): the tool's pairing, `serq run D --workload W`.
/// `names` are how a message names the files (their paths).
pub fn compile_paired(
    deployment: &str,
    workload: &str,
    names: (&str, &str),
    ov: &Overrides,
) -> Result<ir::Program, String> {
    let p = frontend::paired::Paired::new(deployment, workload, names);
    let prog = frontend::parser::parse_paired(&p, &ov.defs, &overridden(ov))?;
    finish(prog, &|span, msg| p.render(span, msg), ov)
}

/// `compile_paired`, for inspection (`load_inspected`).
pub fn compile_paired_inspected(
    deployment: &str,
    workload: &str,
    names: (&str, &str),
    ov: &Overrides,
) -> Result<ir::Program, String> {
    compile_paired(deployment, workload, names, &inspected(ov))
}

/// The outlines of a pair: the deployment file's (what an editor of the
/// deployment changes) and the workload file's, each with byte ranges into
/// its own text (`Outline::split`).
pub fn outline_paired(
    deployment: &str,
    workload: &str,
) -> Result<(frontend::outline::Outline, frontend::outline::Outline), String> {
    let p = frontend::paired::Paired::new(deployment, workload, ("deployment", "workload"));
    let (mut d, mut w) = frontend::parser::outline_paired(&p)?.split(p.at);
    // the names as each file writes them (`frontend::paired`)
    for decl in d.decls.iter_mut().chain(w.decls.iter_mut()) {
        decl.name = frontend::paired::written(&decl.name);
    }
    Ok((d, w))
}

/// Run a pair (`compile_paired`); a trace the workload reads is read next
/// to the workload file (`base`).
pub fn run_paired(
    deployment: &str,
    workload: &str,
    names: (&str, &str),
    ov: &Overrides,
    base: Option<&Path>,
) -> Result<Report, String> {
    let p = compile_paired(deployment, workload, names, ov)?;
    run_ir(&p, base)
}

/// The supplied inputs, which the parser checks cannot change an array size.
fn overridden(ov: &Overrides) -> Vec<String> {
    ov.lets.iter().map(|(name, _)| name.clone()).collect()
}

/// How a message at a place of the text is shown: the program's own
/// text, or the file of a pair it is in.
type Render<'a> = dyn Fn(Option<frontend::diagnostic::Span>, &str) -> String + 'a;

/// Link a parsed program and check its IR.
fn finish(
    prog: frontend::ast::Program,
    render: &Render,
    ov: &Overrides,
) -> Result<ir::Program, String> {
    let (mut p, spans) = frontend::link::link_located(&prog, ov)
        .map_err(|e| render(e.span, &format!("link error: {}", e.message)))?;
    if let Some(t) = &ov.trace {
        p.trace = Some(t.clone());
    }
    // The linker resolves names; the IR's own check is what knows which
    // moment supplies which context variable (`age` in a session statement,
    // `tokens` in a queue key), so a text program meets it too. An error
    // about a statement points at it in the text (#279).
    p.validate_located().map_err(|e| {
        // a stage no statement runs is about its declaration, not a statement
        let declared = p
            .unreached_stage()
            .filter(|&k| e.message == p.unreached_message(k))
            .and_then(|k| {
                prog.stages
                    .iter()
                    .find(|s| s.name == p.stages[k].name)?
                    .span
            });
        let span =
            declared.or_else(|| e.at.and_then(|(b, k)| spans.get(b)?.get(k)?.of(&e.message)));
        render(span, &e.message)
    })?;
    Ok(p)
}

/// Where each declaration, relation and entry call of program text is
/// written, as byte ranges (`frontend::outline`): for an editor that changes
/// the text in place. It parses and does not link, so a program with a link
/// error has an outline; a parse error is the diagnostic `check` prints.
pub fn outline(src: &str) -> Result<frontend::outline::Outline, String> {
    frontend::parser::outline(src).map_err(|e| e.render(src))
}

/// Parse and link only (static checks).
pub fn check_source(src: &str, ov: &Overrides) -> Result<Linked, String> {
    compile_source(src, ov)
}

/// `load`, for a command that inspects the program and neither runs nor
/// exports it (`check`, `target`, `draw`): program text without a horizon
/// is given a placeholder one, since nothing here chooses an experiment's
/// duration. IR keeps its own.
pub fn load_inspected(path: &Path, ov: &Overrides) -> Result<ir::Program, String> {
    if path.extension().is_some_and(|e| e == "json") {
        return load(path, ov);
    }
    load(path, &inspected(ov))
}

/// `compile_source`, for inspection (`load_inspected`).
pub fn compile_inspected(src: &str, ov: &Overrides) -> Result<ir::Program, String> {
    compile_source(src, &inspected(ov))
}

fn inspected(ov: &Overrides) -> Overrides {
    Overrides {
        horizon: ov.horizon.or(Some(f64::MAX)),
        ..ov.clone()
    }
}

/// Refuse a program argument or a `def` override on a program already in
/// IR: its constants are folded and its definitions expanded.
pub fn refuse_text_overrides(ov: &Overrides) -> Result<(), String> {
    if !ov.lets.is_empty() {
        return Err("a program argument applies to program text, not to IR \
             (an IR's constants are already folded)\n\
             help: override the program text (.sq) this IR was compiled from"
            .into());
    }
    if !ov.defs.is_empty() {
        return Err("a `def` override applies to program text, not to IR \
             (an IR's definitions are already expanded)\n\
             help: override the program text (.sq) this IR was compiled from"
            .into());
    }
    Ok(())
}

/// Load a program file as IR: `.json` is read as IR (only the run
/// parameters and the trace can be overridden, since constants are already
/// folded); anything else is compiled from text.
pub fn load(path: &Path, ov: &Overrides) -> Result<ir::Program, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if path.extension().is_some_and(|e| e == "json") {
        refuse_text_overrides(ov)?;
        let mut p = ir::Program::from_json(&text)?;
        if let Some(h) = ov.horizon {
            p.horizon = h;
        }
        if let Some(w) = ov.warmup {
            p.warmup = w;
        }
        if let Some(s) = ov.seed {
            p.seed = s;
        }
        if let Some(n) = ov.arrivals {
            p.arrivals = Some(n);
        }
        if let Some(t) = &ov.trace {
            p.trace = Some(t.clone());
        }
        p.validate()?;
        Ok(p)
    } else {
        compile_source(&text, ov)
    }
}

/// `load` for a deployment file paired with a workload file
/// (`compile_paired`), read from their paths.
pub fn load_paired(
    deployment: &Path,
    workload: &Path,
    ov: &Overrides,
) -> Result<ir::Program, String> {
    if deployment.extension().is_some_and(|e| e == "json") {
        return Err(IR_HAS_ITS_WORKLOAD.into());
    }
    let read = |path: &Path| {
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    };
    let (d, w) = (read(deployment)?, read(workload)?);
    let names = (
        deployment.display().to_string(),
        workload.display().to_string(),
    );
    compile_paired(&d, &w, (&names.0, &names.1), ov)
}

/// Why a workload is not paired with IR (`.json`): the IR is the whole
/// program, workload included.
pub const IR_HAS_ITS_WORKLOAD: &str =
    "--workload pairs a .sq deployment with a workload file; an IR is a whole program already";

/// `load_paired`, for inspection (`load_inspected`).
pub fn load_paired_inspected(
    deployment: &Path,
    workload: &Path,
    ov: &Overrides,
) -> Result<ir::Program, String> {
    load_paired(deployment, workload, &inspected(ov))
}

/// Run an IR program. A relative trace path is resolved against `base`
/// (the program file's directory), unless it was overridden.
pub fn run_ir(p: &ir::Program, base: Option<&Path>) -> Result<Report, String> {
    run_with(p, base, None)
}

/// `run_ir`, keeping the log of the run (`engine::log`): what each session
/// did, when, at most `cap` events, in `Report::log`. The run is the same.
pub fn run_ir_logged(p: &ir::Program, base: Option<&Path>, cap: usize) -> Result<Report, String> {
    run_with(p, base, Some(cap))
}

fn run_with(p: &ir::Program, base: Option<&Path>, log: Option<usize>) -> Result<Report, String> {
    p.validate()?;
    let corpus = load_trace(p, base)?;
    let interp = engine::interp::Interp::new(p, corpus);
    match log {
        Some(cap) => interp.logged(cap).run(),
        None => interp.run(),
    }
}

/// The program's trace corpus, if it names one; a relative path is resolved
/// against `base`.
pub fn load_trace(
    p: &ir::Program,
    base: Option<&Path>,
) -> Result<Option<ir::trace::Corpus>, String> {
    let Some(path) = &p.trace else {
        return Ok(None);
    };
    let f = Path::new(path);
    let full = if f.is_absolute() {
        f.to_path_buf()
    } else {
        base.map_or_else(|| f.to_path_buf(), |b| b.join(f))
    };
    read_trace(&full).map(Some)
}

/// The trace file at `path`: the corpus a replay draws its sessions from.
pub fn read_trace(path: &Path) -> Result<ir::trace::Corpus, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read trace {}: {e}", path.display()))?;
    ir::trace::Corpus::from_csv(&text).map_err(|e| format!("trace {}: {e}", path.display()))
}

/// Replace the program's trace file by its sessions, as explicit sessions
/// with turns in the IR (`ir::Program::inline_trace`).
pub fn inline_trace(p: ir::Program, base: Option<&Path>) -> Result<ir::Program, String> {
    match load_trace(&p, base)? {
        None => Err("the program has no trace to inline".into()),
        Some(c) => p.inline_trace(&c),
    }
}

/// Parse, link and run program text. `base` resolves a relative trace path.
pub fn run_source(src: &str, ov: &Overrides, base: Option<&Path>) -> Result<Report, String> {
    let p = compile_source(src, ov)?;
    run_ir(&p, if ov.trace.is_some() { None } else { base })
}

/// Read a program file (text or IR) and run it.
pub fn run_file(path: &Path, ov: &Overrides) -> Result<Report, String> {
    let p = load(path, ov)?;
    run_ir(
        &p,
        if ov.trace.is_some() {
            None
        } else {
            path.parent()
        },
    )
}

/// The path of `examples/<group>/<name>.sq` in this crate. Example names
/// are unique across groups, so the group is not part of the name; the
/// workloads (`examples/workloads/`) are not programs.
pub fn program_path(name: &str) -> std::path::PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let file = format!("{name}.sq");
    std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .flatten()
        .filter(|g| g.file_name() != "workloads")
        .map(|g| g.path().join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("no examples/*/{file}"))
}

/// The workload file `examples/pairs.json` pairs the program at `path`
/// with: which workload each example and tutorial program runs, for tests
/// and tools (`docs/design/paired-workload.md`). `path` is this crate's,
/// absolute or relative to it.
pub fn paired_workload(path: &Path) -> Option<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let list = root.join("examples/pairs.json");
    let text = std::fs::read_to_string(&list).unwrap_or_else(|e| panic!("{}: {e}", list.display()));
    let pairs: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", list.display()));
    let path = path.strip_prefix(root).unwrap_or(path);
    pairs.get(path.to_str()?).map(|w| root.join(w))
}

/// Convenience for tests: run `examples/*/<name>.sq` with overrides given
/// as `name=expr` strings. Supply `Some(horizon)` for a source model; use
/// `run_file` when the experiment also needs warm-up or an arrival limit.
pub fn run_program(name: &str, sets: &[&str], seed: Option<u64>, horizon: Option<f64>) -> Report {
    let path = program_path(name);
    let mut ov = Overrides {
        seed,
        horizon,
        ..Default::default()
    };
    for s in sets {
        let (k, v) = s.split_once('=').expect("name=expr");
        let e = frontend::parser::parse_expr(v).expect("override expression");
        ov.lets.push((k.trim().to_string(), e));
    }
    let workload = paired_workload(&path).unwrap_or_else(|| panic!("{name}: no pair"));
    // a trace is the workload's, read beside its file
    load_paired(&path, &workload, &ov)
        .and_then(|p| run_ir(&p, workload.parent()))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}
