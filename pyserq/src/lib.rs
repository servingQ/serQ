//! pyserq: serQ in Python. It shows what the IR is the definition of — a
//! program and a run of it — and nothing of the text frontend: a program
//! is compiled from a file or from text to its IR, and the IR is run.
//!
//! ```python
//! import pyserq
//! p = pyserq.compile("examples/single-turn/mg1.sq", sets={"lam": 0.8}, seed=10)
//! r = pyserq.run(p)            # the GIL is released while it runs
//! r.json()                     # what `serq run --json` prints
//! o = r.observes["sojourn"]   # o.mean, o.ci, ...; o.values, o.times: what `--dump` writes
//! r.stages[0].utilization, r.pools[0].preemptions
//! pyserq.read_trace("examples/replay/data/short_base.csv")  # the sessions a replay draws from
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

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

/// What a run reports: the fields of `serq run --json`, by the same names
/// (`REPORT_VERSION`), with each observation's samples.
#[pyclass(frozen)]
struct Report(Arc<serq::Report>);

#[pymethods]
impl Report {
    /// The summary `serq run --json` prints.
    fn json(&self) -> String {
        self.0.json()
    }

    #[getter]
    fn horizon(&self) -> f64 {
        self.0.horizon
    }
    #[getter]
    fn end(&self) -> f64 {
        self.0.end
    }
    #[getter]
    fn warmup(&self) -> f64 {
        self.0.warmup
    }
    #[getter]
    fn seed(&self) -> u64 {
        self.0.seed
    }
    #[getter]
    fn events(&self) -> u64 {
        self.0.events
    }
    #[getter]
    fn arrivals(&self) -> u64 {
        self.0.arrivals
    }
    #[getter]
    fn ended(&self) -> u64 {
        self.0.ended
    }
    #[getter]
    fn turns(&self) -> u64 {
        self.0.turns
    }
    #[getter]
    fn mean_live(&self) -> f64 {
        self.0.mean_live
    }

    /// The observations by name, in the program's order.
    #[getter]
    fn observes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for (i, o) in self.0.observes.iter().enumerate() {
            d.set_item(&o.name, Observe(self.0.clone(), i))?;
        }
        Ok(d)
    }

    /// One row per stage (a replicated stage has a row per replica, under
    /// one name).
    #[getter]
    fn stages(&self) -> Vec<serq::engine::report::StageReport> {
        self.0.stages.clone()
    }

    #[getter]
    fn pools(&self) -> Vec<serq::engine::report::PoolReport> {
        self.0.pools.clone()
    }
}

/// One observation: its statistics as `serq run --json` prints them, and
/// its samples as `serq run --dump` writes them.
#[pyclass(frozen)]
struct Observe(Arc<serq::Report>, usize);

impl Observe {
    fn get(&self) -> &serq::engine::report::ObserveReport {
        &self.0.observes[self.1]
    }
}

#[pymethods]
impl Observe {
    #[getter]
    fn name(&self) -> &str {
        &self.get().name
    }
    #[getter]
    fn count(&self) -> u64 {
        self.get().count
    }
    #[getter]
    fn mean(&self) -> f64 {
        self.get().mean
    }
    /// Batch-means 95 % half-width (NaN below 40 samples).
    #[getter]
    fn ci(&self) -> f64 {
        self.get().ci.half_width
    }
    #[getter]
    fn cv2(&self) -> f64 {
        self.get().cv2
    }
    #[getter]
    fn p99(&self) -> f64 {
        self.get().p99
    }
    #[getter]
    fn values(&self) -> Vec<f64> {
        self.get().samples.clone()
    }
    #[getter]
    fn times(&self) -> Vec<f64> {
        self.get().records.iter().map(|r| r.0).collect()
    }
    #[getter]
    fn sessions(&self) -> Vec<u64> {
        self.get().records.iter().map(|r| r.1).collect()
    }
    #[getter]
    fn turns(&self) -> Vec<u32> {
        self.get().records.iter().map(|r| r.2).collect()
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
        .map(|r| Report(Arc::new(r)))
        .map_err(PyValueError::new_err)
}

/// `read_trace(path)`: a trace's sessions, each a list of its turns
/// `(new, out, think, forced)`: the corpus a replay draws its sessions
/// from, read as a replay reads it. A relative path is read from the
/// current directory.
#[pyfunction]
#[allow(clippy::type_complexity)]
fn read_trace(path: PathBuf) -> PyResult<Vec<Vec<(f64, f64, f64, f64)>>> {
    let corpus = serq::read_trace(&path).map_err(PyValueError::new_err)?;
    Ok(corpus
        .sessions
        .iter()
        .map(|s| {
            s.turns
                .iter()
                .map(|t| (t.new, t.out, t.think, t.forced))
                .collect()
        })
        .collect())
}

#[pymodule]
fn pyserq(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Program>()?;
    m.add_class::<Report>()?;
    m.add_class::<Observe>()?;
    m.add_class::<serq::engine::report::StageReport>()?;
    m.add_class::<serq::engine::report::PoolReport>()?;
    m.add_function(wrap_pyfunction!(compile, m)?)?;
    m.add_function(wrap_pyfunction!(run, m)?)?;
    m.add_function(wrap_pyfunction!(read_trace, m)?)?;
    m.add("IR_VERSION", serq::ir::IR_VERSION)?;
    m.add("REPORT_VERSION", serq::engine::report::REPORT_VERSION)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
