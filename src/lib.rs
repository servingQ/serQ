//! # seQ
//!
//! A seQ program describes an LLM serving deployment: memory pools and
//! stages, a workload of sessions, and the program every session runs.
//! Its definition is the IR (`ir::Program`, `docs/ir.md`): `sim::interp` runs it,
//! the Lean model is generated from it, and tools build or edit it as data.
//! The text syntax (`frontend`; `docs/language.md`) is one frontend
//! that compiles to it. `examples/` holds example deployments, among them
//! vLLM v1.
//!
//! ```no_run
//! let src = std::fs::read_to_string("examples/single-turn/mg1.seq").unwrap();
//! let report = seq::run_source(&src, &seq::Overrides::default(), None).unwrap();
//! println!("{}", report.text());
//! ```

pub mod frontend;
pub mod ir;
pub mod sim;
pub mod view;

use std::path::Path;

pub use frontend::link::{Linked, Overrides};
pub use ir::Program;
pub use sim::dist::Dist;
pub use sim::report::Report;
pub use sim::stats::Estimate;

/// Compile program text to IR (parse and link; `--set` overrides apply).
pub fn compile_source(src: &str, ov: &Overrides) -> Result<ir::Program, String> {
    let prog = frontend::parser::parse(src).map_err(|e| e.render(src))?;
    let mut p = frontend::link::link(&prog, ov).map_err(|e| e.render(src))?;
    if let Some(t) = &ov.trace {
        p.trace = Some(t.clone());
    }
    // The linker resolves names; the IR's own check is what knows which
    // moment supplies which context variable (`age` in a session statement,
    // `ntok` in a queue key), so a text program meets it too.
    p.validate()?;
    Ok(p)
}

/// Parse and link only (static checks).
pub fn check_source(src: &str, ov: &Overrides) -> Result<Linked, String> {
    compile_source(src, ov)
}

/// Load a program file as IR: `.json` is read as IR (only the run
/// parameters and the trace can be overridden, since constants are already
/// folded); anything else is compiled from text.
pub fn load(path: &Path, ov: &Overrides) -> Result<ir::Program, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if path.extension().is_some_and(|e| e == "json") {
        if !ov.lets.is_empty() {
            return Err("--set applies to program text, not to IR (constants are folded)".into());
        }
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
        if let Some(t) = &ov.trace {
            p.trace = Some(t.clone());
        }
        p.validate()?;
        Ok(p)
    } else {
        compile_source(&text, ov)
    }
}

/// Run an IR program. A relative trace path is resolved against `base`
/// (the program file's directory), unless it was overridden.
pub fn run_ir(p: &ir::Program, base: Option<&Path>) -> Result<Report, String> {
    p.validate()?;
    let corpus = load_trace(p, base)?;
    sim::interp::Interp::new(p, corpus).run()
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
    let text = std::fs::read_to_string(&full)
        .map_err(|e| format!("cannot read trace {}: {e}", full.display()))?;
    ir::trace::Corpus::from_csv(&text)
        .map(Some)
        .map_err(|e| format!("trace {}: {e}", full.display()))
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

/// The path of `examples/<group>/<name>.seq` in this crate. Example names
/// are unique across groups, so the group is not part of the name.
pub fn program_path(name: &str) -> std::path::PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let file = format!("{name}.seq");
    std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .flatten()
        .map(|g| g.path().join(&file))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("no examples/*/{file}"))
}

/// Convenience for tests: run `examples/*/<name>.seq` with overrides given
/// as `name=expr` strings.
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
    run_file(&path, &ov).unwrap_or_else(|e| panic!("{name}: {e}"))
}
