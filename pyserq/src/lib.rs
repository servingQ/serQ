//! pyserq: serQ in Python. It shows what the IR is the definition of — a
//! program and a run of it — and nothing of the text frontend: a program
//! is compiled from a file or from text to its IR, and the IR is run. The
//! deployment view of a program is drawn as `serq draw` draws it.
//!
//! ```python
//! import pyserq
//! p = pyserq.compile("examples/single-turn/mg1.sq", sets={"lam": 0.8}, seed=10, horizon=250000, warmup=25000)
//! r = pyserq.run(p)            # the GIL is released while it runs
//! r.json()                     # what `serq run --json` prints
//! o = r.client["response"]     # what each turn's client saw: o.mean, o.ci, ...; o.samples, o.times: what `--dump` writes
//! g = pyserq.run(pyserq.compile("examples/pd-disaggregation/llmd_nixl_pull.sq", horizon=2000, warmup=200)).gauge("load_spread")
//! g.mean, g.ci, g.min, g.max  # g.times, g.values: what `--dump` writes
//! r.stage("svc").utilization; r.observes, r.client, r.status, r.gauges, r.claims, r.stages, r.pools: all of them
//! pyserq.run(pyserq.compile("examples/papers/kong_svf.sq", horizon=100000)).claim("queueing_bound").result  # "holds", ...
//! pyserq.read_trace("examples/replay/data/short_base.csv")  # the sessions a replay draws from
//! pyserq.draw("examples/multi-turn/vllm.sq")  # what `serq draw` prints
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pyo3::exceptions::{PyTypeError, PyValueError};
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
            d.set_item(&o.name, Observe(self.0.clone(), i, Table::Program))?;
        }
        Ok(d)
    }

    /// What each turn's client saw: `ttft`, `tpot` and `response`, in
    /// that order, whatever the program observes.
    #[getter]
    fn client<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for (i, o) in self.0.client.iter().enumerate() {
            d.set_item(&o.name, Observe(self.0.clone(), i, Table::Client))?;
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

    /// The claims by name, in the program's order.
    #[getter]
    fn claims<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for (i, c) in self.0.claims.iter().enumerate() {
            d.set_item(&c.name, Claim(self.0.clone(), i))?;
        }
        Ok(d)
    }

    /// The claim `name`, or `None` (`serq::Report::claim`).
    fn claim(&self, name: &str) -> Option<Claim> {
        let i = self.0.claims.iter().position(|c| c.name == name)?;
        Some(Claim(self.0.clone(), i))
    }

    /// One row per stage (a replicated stage has a row per replica, under
    /// one name).
    #[getter]
    fn stages(&self) -> Vec<serq::engine::report::StageReport> {
        self.0.stages.clone()
    }

    /// Every engine request status by name
    /// (`docs/design/request-state.md`): its stays and the requests in it.
    #[getter]
    fn status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for x in &self.0.statuses {
            d.set_item(&x.name, x.clone())?;
        }
        Ok(d)
    }

    #[getter]
    fn pools(&self) -> Vec<serq::engine::report::PoolReport> {
        self.0.pools.clone()
    }

    /// The observation `name`, or `None` (`serq::Report::observe`).
    fn observe(&self, name: &str) -> Option<Observe> {
        let i = self.0.observes.iter().position(|o| o.name == name)?;
        Some(Observe(self.0.clone(), i, Table::Program))
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

/// One observation, the program's or the client's: its statistics as `serq
/// run --json` prints them, and its samples as `serq run --dump` writes
/// them. Each access to `samples`, `times`, `sessions` or `turns` makes a
/// new list: bind it once.
#[pyclass(frozen, module = "pyserq")]
struct Observe(Arc<serq::Report>, usize, Table);

/// The report's table an observation is in.
#[derive(Clone, Copy)]
enum Table {
    /// The program's `observe`s.
    Program,
    /// What each turn's client saw.
    Client,
}

impl Observe {
    fn get(&self) -> &serq::engine::report::ObserveReport {
        match self.2 {
            Table::Program => &self.0.observes[self.1],
            Table::Client => &self.0.client[self.1],
        }
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

/// One claim: what the run found of it, by the names of `serq run --json`.
#[pyclass(frozen, module = "pyserq")]
struct Claim(Arc<serq::Report>, usize);

impl Claim {
    fn get(&self) -> &serq::engine::report::ClaimReport {
        &self.0.claims[self.1]
    }
}

#[pymethods]
impl Claim {
    #[getter]
    fn name(&self) -> &str {
        &self.get().name
    }
    /// `every_iteration`, `some_iteration` or `at_end`.
    #[getter]
    fn kind(&self) -> &'static str {
        self.get().kind_name()
    }
    /// `holds`, `fails`, `witnessed`, `not_witnessed`, `not_evaluated` or
    /// `out_of_scope`.
    #[getter]
    fn result(&self) -> &'static str {
        self.get().result.name()
    }
    /// Iterations read (an iteration claim), or 1 when a claim `at end` was.
    #[getter]
    fn checked(&self) -> u64 {
        self.get().checked
    }
    #[getter]
    fn failures(&self) -> u64 {
        self.get().failures
    }
    /// The time of the first failure, or of the first witness, or `None`.
    #[getter]
    fn first(&self) -> Option<f64> {
        self.get().first
    }
    /// Why the claim was not read, or `None`.
    #[getter]
    fn note(&self) -> Option<&str> {
        self.get().note.as_deref()
    }
}

/// A `sets` value, a declared program input: a number stands for itself, a string
/// is an expression.
#[derive(FromPyObject)]
enum SetValue {
    Num(f64),
    Expr(String),
}

/// `compile(path=None, *, source=None, workload=None, workload_source=None,
/// sets={}, defs={}, seed=None, horizon=None, warmup=None, arrivals=None,
/// trace=None)`: a program file or program text to its IR. The deployment
/// runs the workload file `workload`, or the workload text
/// `workload_source`, as `serq run FILE --workload W`
/// (`docs/design/paired-workload.md`). Source models require `horizon`; settings are supplied
/// here, separately from the model, with the options of `serq run` (`sets` supply
/// inputs declared with `args.number`, `defs` the bodies of expression definitions, by name).
#[pyfunction]
#[pyo3(signature = (path=None, *, source=None, workload=None, workload_source=None, sets=HashMap::new(), defs=HashMap::new(), seed=None, horizon=None, warmup=None, arrivals=None, trace=None))]
#[allow(clippy::too_many_arguments)]
fn compile(
    path: Option<PathBuf>,
    source: Option<&str>,
    workload: Option<PathBuf>,
    workload_source: Option<&str>,
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
    let w = Workload::given(workload, workload_source)?;
    let program = Deployment::given(path, source, "compile")?;
    // a trace is the workload's, read beside its file
    let base = match (&w, &program) {
        _ if ov.trace.is_some() => None,
        (Some(w), _) => w.dir.clone(),
        (None, Deployment::Path(p)) => p.parent().map(Path::to_path_buf),
        (None, Deployment::Source(_)) => None,
    };
    Ok(Program {
        ir: program.compile(w.as_ref(), &ov, false)?,
        base,
    })
}

/// The workload a deployment runs: a file's text, or text given as it is.
struct Workload {
    name: String,
    text: String,
    /// Its file, read by `serq::load_paired`, when it has one.
    path: Option<PathBuf>,
    /// The file's directory, where its trace is read.
    dir: Option<PathBuf>,
}

impl Workload {
    fn given(path: Option<PathBuf>, source: Option<&str>) -> PyResult<Option<Workload>> {
        match (path, source) {
            (None, None) => Ok(None),
            (Some(p), None) => {
                let text = std::fs::read_to_string(&p).map_err(|e| {
                    PyValueError::new_err(format!("cannot read {}: {e}", p.display()))
                })?;
                Ok(Some(Workload {
                    name: p.display().to_string(),
                    text,
                    dir: p.parent().map(Path::to_path_buf),
                    path: Some(p),
                }))
            }
            (None, Some(text)) => Ok(Some(Workload {
                name: "workload".into(),
                text: text.into(),
                path: None,
                dir: None,
            })),
            _ => Err(PyValueError::new_err(
                "give the workload as a path or as a source (`workload_source`), not both",
            )),
        }
    }
}

/// The deployment: a program file (text, or IR as `.json`), or its text.
enum Deployment<'a> {
    Path(PathBuf),
    Source(&'a str),
}

impl<'a> Deployment<'a> {
    fn given(path: Option<PathBuf>, source: Option<&'a str>, f: &str) -> PyResult<Self> {
        match (path, source) {
            (Some(p), None) => Ok(Deployment::Path(p)),
            (None, Some(src)) => Ok(Deployment::Source(src)),
            _ => Err(PyValueError::new_err(format!(
                "{f} takes exactly one of a path and a source"
            ))),
        }
    }

    /// Its IR, with the workload `w` paired (`serq::compile_paired`);
    /// `inspected` gives text without a horizon a placeholder one.
    fn compile(
        &self,
        w: Option<&Workload>,
        ov: &serq::Overrides,
        inspected: bool,
    ) -> PyResult<serq::Program> {
        let ir = match (self, w) {
            (Deployment::Path(p), None) if inspected => serq::load_inspected(p, ov),
            (Deployment::Path(p), None) => serq::load(p, ov),
            (Deployment::Source(src), None) if inspected => serq::compile_inspected(src, ov),
            (Deployment::Source(src), None) => serq::compile_source(src, ov),
            (Deployment::Path(p), Some(w)) => match &w.path {
                Some(wp) if inspected => serq::load_paired_inspected(p, wp, ov),
                Some(wp) => serq::load_paired(p, wp, ov),
                None if p.extension().is_some_and(|e| e == "json") => {
                    Err(serq::IR_HAS_ITS_WORKLOAD.into())
                }
                None => std::fs::read_to_string(p)
                    .map_err(|e| format!("cannot read {}: {e}", p.display()))
                    .and_then(|text| {
                        let names = (p.display().to_string(), w.name.clone());
                        let f = if inspected {
                            serq::compile_paired_inspected
                        } else {
                            serq::compile_paired
                        };
                        f(&text, &w.text, (&names.0, &names.1), ov)
                    }),
            },
            (Deployment::Source(src), Some(w)) => {
                let names = ("deployment", w.name.as_str());
                if inspected {
                    serq::compile_paired_inspected(src, &w.text, names, ov)
                } else {
                    serq::compile_paired(src, &w.text, names, ov)
                }
            }
        };
        ir.map_err(PyValueError::new_err)
    }
}

/// `sets` and `defs` into `ov`, as `--name value` and `--def`.
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

/// What `draw` draws: a compiled program, or a program file.
enum Drawn {
    Program(Py<Program>),
    Path(PathBuf),
}

impl<'py> FromPyObject<'py> for Drawn {
    fn extract_bound(a: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok(p) = a.downcast::<Program>() {
            return Ok(Drawn::Program(p.clone().unbind()));
        }
        a.extract().map(Drawn::Path).map_err(|_| {
            let ty = a.get_type().name().map_or("?".into(), |n| n.to_string());
            PyTypeError::new_err(format!(
                "expected a str, an os.PathLike or a Program, not {ty}"
            ))
        })
    }
}

/// `draw(path=None, *, source=None, workload=None, workload_source=None,
/// sets={}, defs={}, format="tikz")`: the
/// deployment view, as `serq draw` prints it: what one request runs
/// (`serq::view::deployment::request`). `path` may be a compiled
/// `Program`, drawn from its IR as a `.json` file is.
#[pyfunction]
#[pyo3(signature = (path=None, *, source=None, workload=None, workload_source=None, sets=HashMap::new(), defs=HashMap::new(), format="tikz"))]
fn draw(
    path: Option<Drawn>,
    source: Option<&str>,
    workload: Option<PathBuf>,
    workload_source: Option<&str>,
    sets: HashMap<String, SetValue>,
    defs: HashMap<String, String>,
    format: &str,
) -> PyResult<String> {
    let mut ov = serq::Overrides::default();
    overriding(&mut ov, sets, defs)?;
    let w = Workload::given(workload, workload_source)?;
    let figure = match path {
        Some(Drawn::Program(p)) if source.is_none() => {
            if w.is_some() {
                return Err(PyValueError::new_err(
                    "a compiled Program holds its workload already",
                ));
            }
            serq::refuse_text_overrides(&ov)
                .map(|()| serq::view::deployment::figure(&p.get().ir))
                .map_err(PyValueError::new_err)?
        }
        path => {
            let path = match path {
                Some(Drawn::Path(p)) => Some(p),
                Some(Drawn::Program(_)) => {
                    return Err(PyValueError::new_err(
                        "draw takes exactly one of a path and a source",
                    ));
                }
                None => None,
            };
            let ir = Deployment::given(path, source, "draw")?.compile(w.as_ref(), &ov, true)?;
            serq::view::deployment::figure(&ir)
        }
    };
    serq::view::render(&figure, format).map_err(PyValueError::new_err)
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
/// seeded `s` draws its arrivals from `Rng(s)`; a session's draws come from
/// `substream(s, serial, turn, kind)` (`src/engine/interp.rs`), seeded by
/// splitmix64 over the seed mixed alone and then each of the three. A
/// Python check that reproduces a run's draws, or needs rand's stream, uses
/// this one rather than a port of it.
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
    m.add_class::<Claim>()?;
    m.add_class::<serq::engine::report::StageReport>()?;
    m.add_class::<serq::engine::report::StatusReport>()?;
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
