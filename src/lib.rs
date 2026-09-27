//! # ROUTE
//!
//! A small language in which an LLM serving deployment is a program: a
//! *deployment* of memory pools and stages, a *workload* of sessions, and a
//! *route* every session follows. This crate parses, links and runs ROUTE
//! programs as discrete-event simulations. The language is specified in
//! `docs/language.md`; `programs/` holds the deployments of the
//! paper and of vLLM v1.
//!
//! ```no_run
//! let src = std::fs::read_to_string("programs/mg1.route").unwrap();
//! let report = seq::run_source(&src, &seq::Overrides::default(), None).unwrap();
//! println!("{}", report.text());
//! ```

pub mod ast;
pub mod lexer;
pub mod link;
pub mod parser;
pub mod report;
pub mod sim;
pub mod stats;
pub mod trace;

use std::path::Path;

pub use link::{Linked, Overrides};
pub use report::Report;
pub use stats::Estimate;

/// Parse, link and run a program. `base` resolves a relative trace path.
pub fn run_source(src: &str, ov: &Overrides, base: Option<&Path>) -> Result<Report, String> {
    let prog = parser::parse(src).map_err(|e| e.to_string())?;
    let linked = link::link(&prog, ov).map_err(|e| e.to_string())?;
    let corpus = match &linked.trace {
        None => None,
        Some(path) => {
            let p = Path::new(path);
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                base.map_or_else(|| p.to_path_buf(), |b| b.join(p))
            };
            let text = std::fs::read_to_string(&full)
                .map_err(|e| format!("cannot read trace {}: {e}", full.display()))?;
            Some(trace::Corpus::from_csv(&text)?)
        }
    };
    Ok(sim::Sim::new(&linked, corpus).run())
}

/// Parse and link only (static checks).
pub fn check_source(src: &str, ov: &Overrides) -> Result<Linked, String> {
    let prog = parser::parse(src).map_err(|e| e.to_string())?;
    link::link(&prog, ov).map_err(|e| e.to_string())
}

/// Read a program file and run it; `--set` style overrides apply.
pub fn run_file(path: &Path, ov: &Overrides) -> Result<Report, String> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    run_source(&src, ov, path.parent())
}

/// Convenience for tests: run `programs/<name>.route` with overrides given
/// as `name=expr` strings.
pub fn run_program(name: &str, sets: &[&str], seed: Option<u64>, horizon: Option<f64>) -> Report {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("programs")
        .join(format!("{name}.route"));
    let mut ov = Overrides {
        seed,
        horizon,
        ..Default::default()
    };
    for s in sets {
        let (k, v) = s.split_once('=').expect("name=expr");
        let e = parser::parse_expr(v).expect("override expression");
        ov.lets.push((k.trim().to_string(), e));
    }
    run_file(&path, &ov).unwrap_or_else(|e| panic!("{name}: {e}"))
}
