//! pyserq: serQ in Python. It shows what the IR is the definition of — a
//! program and a run of it — and nothing of the text frontend: a program
//! is compiled from a file or from text to its IR, and the IR is run. The
//! deployment view of a program is drawn as `serq draw` draws it.
//!
//! ```python
//! import pyserq
//! p = pyserq.compile("examples/single-turn/mg1.sq", sets={"lam": 0.8}, seed=10)
//! r = pyserq.run(p)            # the GIL is released while it runs
//! r.json()                     # what `serq run --json` prints
//! o = r.observe("sojourn")    # o.mean, o.ci, ...; o.samples, o.times: what `--dump` writes
//! g = pyserq.run(pyserq.compile("examples/pd-disaggregation/llmd_nixl_pull.sq")).gauge("load_spread")
//! g.mean, g.ci, g.min, g.max  # g.times, g.values: what `--dump` writes
//! r.stage("svc").utilization; r.observes, r.gauges, r.stages, r.pools: all of them
//! pyserq.read_trace("examples/replay/data/short_base.csv")  # the sessions a replay draws from
//! pyserq.draw("examples/multi-turn/vllm.sq")  # what `serq draw` prints
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use rand::rngs::StdRng;
use rand::{Rng as _, RngCore, SeedableRng};

/// A compiled program: its IR, and the directory a relative trace is read
/// against (the program file's, as `serq run` does; none for text).
#[pyclass(frozen, module = "pyserq")]
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
#[pyclass(frozen, module = "pyserq")]
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
    /// The serq that ran (`__version__`), as `serq run --json` records it.
    #[getter]
    fn serq_version(&self) -> &'static str {
        serq::VERSION
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

    /// The gauges by name, in the program's order.
    #[getter]
    fn gauges<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for (i, g) in self.0.gauges.iter().enumerate() {
            d.set_item(&g.name, Gauge(self.0.clone(), i))?;
        }
        Ok(d)
    }

    /// The gauge `name`, or `None` (`serq::Report::gauge`).
    fn gauge(&self, name: &str) -> Option<Gauge> {
        let i = self.0.gauges.iter().position(|g| g.name == name)?;
        Some(Gauge(self.0.clone(), i))
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

    /// The observation `name`, or `None` (`serq::Report::observe`).
    fn observe(&self, name: &str) -> Option<Observe> {
        let i = self.0.observes.iter().position(|o| o.name == name)?;
        Some(Observe(self.0.clone(), i))
    }

    /// The first stage named `name`, or `None`.
    fn stage(&self, name: &str) -> Option<serq::engine::report::StageReport> {
        self.0.stage(name).cloned()
    }

    /// Every row of the stage `name`: a replicated stage's members, in
    /// index order.
    fn stages_named(&self, name: &str) -> Vec<serq::engine::report::StageReport> {
        self.0.stages_named(name).into_iter().cloned().collect()
    }

    /// The first pool named `name`, or `None`.
    fn pool(&self, name: &str) -> Option<serq::engine::report::PoolReport> {
        self.0.pool(name).cloned()
    }

    /// Every row of the pool `name`: a pool array's members, in index
    /// order.
    fn pools_named(&self, name: &str) -> Vec<serq::engine::report::PoolReport> {
        self.0.pools_named(name).into_iter().cloned().collect()
    }
}

/// One observation: its statistics as `serq run --json` prints them, and
/// its samples as `serq run --dump` writes them. Each access to `samples`,
/// `times`, `sessions` or `turns` makes a new list: bind it once.
#[pyclass(frozen, module = "pyserq")]
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
    /// Batch-means 95 % half-width (+inf below 40 samples).
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
    fn samples(&self) -> Vec<f64> {
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

/// One gauge: its statistics as `serq run --json` prints them, and its
/// change points as `serq run --dump` writes them (`times`, `values`; each
/// access makes a new list).
#[pyclass(frozen, module = "pyserq")]
struct Gauge(Arc<serq::Report>, usize);

impl Gauge {
    fn get(&self) -> &serq::engine::report::GaugeReport {
        &self.0.gauges[self.1]
    }
}

#[pymethods]
impl Gauge {
    #[getter]
    fn name(&self) -> &str {
        &self.get().name
    }
    /// Time average over `[warmup, end]`.
    #[getter]
    fn mean(&self) -> f64 {
        self.get().mean
    }
    /// Batch-means 95 % half-width over 20 equal windows.
    #[getter]
    fn ci(&self) -> f64 {
        self.get().ci.half_width
    }
    #[getter]
    fn min(&self) -> f64 {
        self.get().min
    }
    #[getter]
    fn max(&self) -> f64 {
        self.get().max
    }
    #[getter]
    fn times(&self) -> Vec<f64> {
        self.get().points.iter().map(|p| p.0).collect()
    }
    #[getter]
    fn values(&self) -> Vec<f64> {
        self.get().points.iter().map(|p| p.1).collect()
    }
}

/// A `--set` value: a number stands for itself, a string is an expression.
#[derive(FromPyObject)]
enum SetValue {
    Num(f64),
    Expr(String),
}

/// `compile(path=None, *, source=None, sets={}, defs={}, seed=None,
/// horizon=None, warmup=None, arrivals=None, trace=None)`: a program file or
/// program text to its IR, with the overrides of `serq run` (`defs` is
/// `--def`: the body of an expression definition, by name).
#[pyfunction]
#[pyo3(signature = (path=None, *, source=None, sets=HashMap::new(), defs=HashMap::new(), seed=None, horizon=None, warmup=None, arrivals=None, trace=None))]
#[allow(clippy::too_many_arguments)]
fn compile(
    path: Option<PathBuf>,
    source: Option<&str>,
    sets: HashMap<String, SetValue>,
    defs: HashMap<String, String>,
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
    overriding(&mut ov, sets, defs)?;
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

/// `sets` and `defs` into `ov`, as `--set` and `--def`.
fn overriding(
    ov: &mut serq::Overrides,
    sets: HashMap<String, SetValue>,
    defs: HashMap<String, String>,
) -> PyResult<()> {
    for (name, v) in sets {
        match v {
            SetValue::Num(x) => ov.set_num(&name, x).map_err(PyValueError::new_err)?,
            SetValue::Expr(e) => ov.set(&name, &e).map_err(PyValueError::new_err)?,
        }
    }
    for (name, body) in defs {
        ov.define(&name, &body).map_err(PyValueError::new_err)?;
    }
    Ok(())
}

/// `draw(path=None, *, source=None, sets={}, defs={}, format="tikz")`: the
/// deployment view, as `serq draw` prints it. It takes the file or text and
/// not a compiled `Program`: the view draws what one request runs, which
/// program text compiled for the view says (`serq::load_drawn`), and a
/// `Program` compiled to run has the whole session in its place.
#[pyfunction]
#[pyo3(signature = (path=None, *, source=None, sets=HashMap::new(), defs=HashMap::new(), format="tikz"))]
fn draw(
    path: Option<PathBuf>,
    source: Option<&str>,
    sets: HashMap<String, SetValue>,
    defs: HashMap<String, String>,
    format: &str,
) -> PyResult<String> {
    let render = match format {
        "tikz" => serq::view::tikz::render,
        "svg" => serq::view::svg::render,
        f => {
            return Err(PyValueError::new_err(format!(
                "unknown format `{f}` (tikz, svg)"
            )));
        }
    };
    let mut ov = serq::Overrides::default();
    overriding(&mut ov, sets, defs)?;
    let ir = match (&path, source) {
        (Some(p), None) => serq::load_drawn(p, &ov),
        (None, Some(src)) => serq::compile_drawn_source_at(src, None, &ov),
        _ => {
            return Err(PyValueError::new_err(
                "draw takes exactly one of a path and a source",
            ));
        }
    }
    .map_err(PyValueError::new_err)?;
    Ok(render(&serq::view::deployment::figure(&ir)))
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

/// The generator a run draws from: rand 0.9's `StdRng` (ChaCha12). A run
/// seeded `s` draws its arrivals from `Rng(s)` and its workload from
/// `Rng(s ^ 0x9e3779b97f4a7c15)`; a Python check that reproduces a run's
/// draws, or needs rand's stream, uses this one rather than a port of it.
#[pyclass(module = "pyserq")]
struct Rng(StdRng);

#[pymethods]
impl Rng {
    /// `StdRng::seed_from_u64(seed)`.
    #[new]
    fn new(seed: u64) -> Self {
        Rng(StdRng::seed_from_u64(seed))
    }
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }
    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }
    /// `random::<f64>()`: in [0, 1).
    fn random_f64(&mut self) -> f64 {
        self.0.random()
    }
    /// `random_range(low..=high)` over `u64`.
    fn range_u64(&mut self, low: u64, high: u64) -> PyResult<u64> {
        check_range(low <= high)?;
        Ok(self.0.random_range(low..=high))
    }
    /// `random_range(low..=high)` over `u32` (and over `usize` below 2^32,
    /// which rand samples as `u32`).
    fn range_u32(&mut self, low: u32, high: u32) -> PyResult<u32> {
        check_range(low <= high)?;
        Ok(self.0.random_range(low..=high))
    }
    /// `random_range(low..=high)` over `f64`.
    fn range_f64(&mut self, low: f64, high: f64) -> PyResult<f64> {
        check_range(low <= high && (high - low).is_finite())?;
        Ok(self.0.random_range(low..=high))
    }
}

fn check_range(ok: bool) -> PyResult<()> {
    if ok {
        Ok(())
    } else {
        Err(PyValueError::new_err("an empty or unbounded range"))
    }
}

#[pymodule]
fn pyserq(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Program>()?;
    m.add_class::<Rng>()?;
    m.add_class::<Report>()?;
    m.add_class::<Observe>()?;
    m.add_class::<Gauge>()?;
    m.add_class::<serq::engine::report::StageReport>()?;
    m.add_class::<serq::engine::report::PoolReport>()?;
    m.add_function(wrap_pyfunction!(compile, m)?)?;
    m.add_function(wrap_pyfunction!(run, m)?)?;
    m.add_function(wrap_pyfunction!(read_trace, m)?)?;
    m.add_function(wrap_pyfunction!(draw, m)?)?;
    m.add("IR_VERSION", serq::ir::IR_VERSION)?;
    m.add("REPORT_VERSION", serq::engine::report::REPORT_VERSION)?;
    m.add("__version__", serq::VERSION)?;
    Ok(())
}
