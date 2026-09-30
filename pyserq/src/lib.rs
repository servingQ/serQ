//! pyserq: serQ in Python. It shows what the IR is the definition of — a
//! program and a run of it — and nothing of the text frontend: a program
//! is compiled from a file or from text to its IR, and the IR is run.
//!
//! ```python
//! import pyserq
//! p = pyserq.compile("examples/single-turn/mg1.sq", sets={"lam": 0.8}, seed=10)
//! r = pyserq.run(p)            # the GIL is released while it runs
//! r.json()                     # what `serq run --json` prints
//! values, times, sessions, turns = r.observe("sojourn")   # what `--dump` writes
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// A compiled program: its IR, and the directory a relative trace is read
/// against (the program file's, as `serq run` does; none for text).
#[pyclass(frozen)]
struct Program {
    ir: serq::Program,
    base: Option<PathBuf>,
}

#[pymethods]
impl Program {
    /// The IR as JSON (`serq ir`).
    fn to_json(&self) -> String {
        self.ir.to_json()
    }

    /// An IR read from JSON; it is validated, and a relative trace in it is
    /// read against the current directory.
    #[staticmethod]
    fn from_json(s: &str) -> PyResult<Self> {
        let ir = serq::Program::from_json(s).map_err(PyValueError::new_err)?;
        Ok(Program { ir, base: None })
    }
}

/// What a run reports.
#[pyclass(frozen)]
struct Report(serq::Report);

#[pymethods]
impl Report {
    /// The summary `serq run --json` prints.
    fn json(&self) -> String {
        self.0.json()
    }

    /// One observation's samples: `(values, times, sessions, turns)`, what
    /// `serq run --dump` writes for it.
    #[allow(clippy::type_complexity)]
    fn observe(&self, name: &str) -> PyResult<(Vec<f64>, Vec<f64>, Vec<u64>, Vec<u32>)> {
        let o = self
            .0
            .observe(name)
            .ok_or_else(|| PyValueError::new_err(format!("no observation `{name}`")))?;
        Ok((
            o.samples.clone(),
            o.records.iter().map(|r| r.0).collect(),
            o.records.iter().map(|r| r.1).collect(),
            o.records.iter().map(|r| r.2).collect(),
        ))
    }
}

/// A `--set` value: a number stands for itself, a string is an expression.
#[derive(FromPyObject)]
enum SetValue {
    Num(f64),
    Expr(String),
}

/// `compile(path=None, *, source=None, sets={}, seed=None, horizon=None,
/// warmup=None, arrivals=None, trace=None)`: a program file or program text
/// to its IR, with the overrides of `serq run`.
#[pyfunction]
#[pyo3(signature = (path=None, *, source=None, sets=HashMap::new(), seed=None, horizon=None, warmup=None, arrivals=None, trace=None))]
#[allow(clippy::too_many_arguments)]
fn compile(
    path: Option<PathBuf>,
    source: Option<&str>,
    sets: HashMap<String, SetValue>,
    seed: Option<u64>,
    horizon: Option<f64>,
    warmup: Option<f64>,
    arrivals: Option<usize>,
    trace: Option<PathBuf>,
) -> PyResult<Program> {
    let mut ov = serq::Overrides {
        seed,
        horizon,
        warmup,
        arrivals,
        trace: trace.map(|t| t.to_string_lossy().into_owned()),
        ..Default::default()
    };
    for (name, v) in sets {
        match v {
            SetValue::Num(x) => ov.set_num(&name, x).map_err(PyValueError::new_err)?,
            SetValue::Expr(e) => ov.set(&name, &e).map_err(PyValueError::new_err)?,
        }
    }
    let (ir, base) = match (&path, source) {
        (Some(p), None) => (serq::load(p, &ov), p.parent().map(Path::to_path_buf)),
        (None, Some(src)) => (serq::compile_source(src, &ov), None),
        _ => {
            return Err(PyValueError::new_err(
                "compile takes exactly one of a path and a source",
            ));
        }
    };
    let base = if ov.trace.is_some() { None } else { base };
    Ok(Program {
        ir: ir.map_err(PyValueError::new_err)?,
        base,
    })
}

/// `run(program)`: run it. The run does not hold the GIL, so runs in
/// threads proceed in parallel.
#[pyfunction]
fn run(py: Python<'_>, program: &Program) -> PyResult<Report> {
    let (ir, base) = (&program.ir, program.base.as_deref());
    py.allow_threads(|| serq::run_ir(ir, base))
        .map(Report)
        .map_err(PyValueError::new_err)
}

#[pymodule]
fn pyserq(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Program>()?;
    m.add_class::<Report>()?;
    m.add_function(wrap_pyfunction!(compile, m)?)?;
    m.add_function(wrap_pyfunction!(run, m)?)?;
    m.add("IR_VERSION", serq::ir::IR_VERSION)?;
    m.add("REPORT_VERSION", serq::engine::report::REPORT_VERSION)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
