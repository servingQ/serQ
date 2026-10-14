//! `serq run FILE [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--instance F] [--def k=expr]... [--trace F] [--json] [--dump DIR] [--log FILE] [--name value]...`
//! `serq check FILE [--instance F] [--def k=expr]... [--name value]...`
//! `serq ir FILE [--instance F] [--def k=expr]... [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--trace F] [--inline-trace] [--name value]...`
//! `serq draw FILE [--format tikz|svg] [--out PATH] [--name value]...` (experimental)
//! `serq target FILE [--instance F] [--def k=expr]... [--name value]...`
//!
//! `--name value` is an input the program declares with `args.number`.
//! `serq fmt [--check] FILE...`
//! `serq --version`
//!
//! FILE is program text (`.sq`) or IR (`.json`, as written by `serq ir`).

use std::path::Path;
use std::process::exit;

use serq::Overrides;

fn usage(cmd: &str) -> &'static str {
    match cmd {
        "run" => {
            "serq run FILE [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--instance F] [--def name=expr]... [--trace F] [--json] [--dump DIR] [--log FILE] [--name value]..."
        }
        "check" => "serq check FILE [--instance F] [--def name=expr]... [--name value]...",
        "ir" => {
            "serq ir FILE [--instance F] [--def name=expr]... [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--trace F] [--inline-trace] [--name value]..."
        }
        "draw" => {
            "serq draw FILE [--instance F] [--def name=expr]... [--format tikz|svg] [--out PATH] [--name value]..."
        }
        "target" => "serq target FILE [--instance F] [--def name=expr]... [--name value]...",
        "fmt" => "serq fmt [--check] FILE...",
        _ => "serq <run|check|ir|draw|target|fmt> FILE [OPTIONS] | serq --version",
    }
}

fn argument_error(cmd: &str, message: impl std::fmt::Display) -> ! {
    eprintln!("error: {message}\nusage: {}", usage(cmd));
    exit(2)
}

fn time_arg(cmd: &str, flag: &str, value: &str, positive: bool) -> f64 {
    match value.parse::<f64>() {
        Ok(x) if x.is_finite() && (if positive { x > 0.0 } else { x >= 0.0 }) => x,
        _ => argument_error(
            cmd,
            format!(
                "invalid value `{value}` for {flag}; expected a finite {} number\nhelp: use {flag} {}",
                if positive { "positive" } else { "nonnegative" },
                if positive { "10" } else { "0" }
            ),
        ),
    }
}

fn fail(file: &Path, e: impl std::fmt::Display) -> ! {
    eprintln!("{}: {e}", file.display());
    exit(1)
}

fn format_files(args: &[String]) {
    let mut check = false;
    let mut files = Vec::new();
    for arg in args.iter().skip(1) {
        if arg == "--check" {
            check = true;
        } else if arg.starts_with('-') {
            argument_error("fmt", format!("unknown option `{arg}`"));
        } else {
            files.push(Path::new(arg));
        }
    }
    if files.is_empty() {
        argument_error("fmt", "missing FILE\nhelp: supply one or more .sq files");
    }
    // Read and validate the whole batch before writing any file.
    let mut changes = Vec::new();
    for file in files {
        if file.extension().is_none_or(|ext| ext != "sq") {
            fail(file, "fmt expects a .sq file");
        }
        let source = std::fs::read_to_string(file).unwrap_or_else(|e| fail(file, e));
        let formatted = serq::frontend::fmt::format(&source).unwrap_or_else(|e| fail(file, e));
        if source != formatted {
            changes.push((file, formatted));
        }
    }
    if check {
        for (file, _) in &changes {
            eprintln!("would reformat {}", file.display());
        }
        if !changes.is_empty() {
            exit(1);
        }
    } else {
        for (file, formatted) in changes {
            std::fs::write(file, formatted).unwrap_or_else(|e| fail(file, e));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    if matches!(cmd, "--version" | "-V") {
        // what ran, for a record of a run next to its IR (`run --json` has
        // it as `serq_version`)
        if let Some(extra) = args.get(1) {
            argument_error(cmd, format!("`{cmd}` takes no arguments, found `{extra}`"));
        }
        println!("serq {}", serq::VERSION);
        return;
    }
    if !matches!(cmd, "run" | "check" | "ir" | "draw" | "target" | "fmt") {
        argument_error(
            cmd,
            format!(
                "unknown command `{cmd}`\nhelp: choose run, check, ir, draw, target, or fmt, or --version"
            ),
        );
    }
    if cmd == "fmt" {
        format_files(&args);
        return;
    }
    if args.get(1).is_none_or(|s| s.starts_with("--")) {
        argument_error(
            cmd,
            "missing FILE\nhelp: supply a .sq program or .json IR file",
        );
    }
    let file = Path::new(&args[1]);
    let mut ov = Overrides::default();
    let mut json = false;
    let mut dump: Option<String> = None;
    let mut log: Option<String> = None;
    let mut inline = false;
    let mut format = String::from("tikz");
    let mut out: Option<String> = None;
    let mut i = 2;
    while i < args.len() {
        let flag = args[i].as_str();
        let option = flag.strip_prefix("--").unwrap_or_else(|| {
            argument_error(
                cmd,
                format!("unexpected argument `{flag}`\nhelp: options and program inputs are written --name value"),
            )
        });
        if option.is_empty() {
            argument_error(
                cmd,
                "unexpected `--`\nhelp: program inputs stand among the options, for example --seed 2 --Lambda 0.3",
            );
        }
        // A word that is not the interpreter's is the program's: an input
        // declared with `args.number`, which the linker checks by name.
        if !serq::frontend::args::OPTIONS.contains(&option) {
            let (name, value, consumed) =
                serq::frontend::args::number(option, args.get(i + 1).map(String::as_str))
                    .unwrap_or_else(|e| argument_error(cmd, e));
            ov.lets.push((name, serq::frontend::ast::Expr::Num(value)));
            i += if consumed { 2 } else { 1 };
            continue;
        }
        let allowed: &[&str] = match flag {
            "--def" | "--instance" => &["run", "check", "ir", "draw", "target"],
            "--seed" | "--horizon" | "--warmup" | "--arrivals" | "--trace" => &["run", "ir"],
            "--json" | "--dump" | "--log" => &["run"],
            "--inline-trace" => &["ir"],
            "--format" | "--out" => &["draw"],
            _ => unreachable!("`{flag}` is in args::OPTIONS but has no arm"),
        };
        if !allowed.contains(&cmd) {
            argument_error(
                cmd,
                format!(
                    "{flag} is not supported by `{cmd}`\nhelp: {flag} applies to {}",
                    allowed.join(", ")
                ),
            );
        }
        let next = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).filter(|v| !v.starts_with("--")).cloned().unwrap_or_else(|| {
                argument_error(cmd, format!("missing value for {flag}\nhelp: supply a value immediately after {flag}"))
            })
        };
        match flag {
            "--seed" => {
                let value = next(&mut i);
                ov.seed = Some(value.parse().unwrap_or_else(|_| argument_error(cmd,
                    format!("invalid value `{value}` for --seed; expected an unsigned integer\nhelp: use --seed 1"))));
            }
            "--arrivals" => {
                let value = next(&mut i);
                ov.arrivals = Some(value.parse::<usize>().ok().filter(|n| *n > 0).unwrap_or_else(|| {
                    argument_error(cmd, format!("invalid value `{value}` for --arrivals; expected a positive integer"))
                }));
            }
            "--horizon" => ov.horizon = Some(time_arg(cmd, flag, &next(&mut i), true)),
            "--warmup" => ov.warmup = Some(time_arg(cmd, flag, &next(&mut i), false)),
            "--trace" => ov.trace = Some(next(&mut i)),
            "--instance" => {
                let path = next(&mut i);
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| fail(Path::new(&path), format!("cannot read: {e}")));
                ov.instance(&text)
                    .unwrap_or_else(|e| fail(Path::new(&path), e));
            }
            "--def" => {
                let kv = next(&mut i);
                let (k, v) = kv.split_once('=').unwrap_or_else(|| {
                    argument_error(
                        cmd,
                        format!("invalid --def `{kv}`; expected name=expr\nhelp: use --def service=~exp(1)"),
                    )
                });
                ov.define(k.trim(), v).unwrap_or_else(|e| {
                    argument_error(
                        cmd,
                        format!(
                            "{e}\nhelp: use --def name=expr with a declared `def name(...) {{ expr }}`"
                        ),
                    )
                });
            }
            "--json" => json = true,
            "--dump" => dump = Some(next(&mut i)),
            "--log" => log = Some(next(&mut i)),
            "--inline-trace" => inline = true,
            "--format" => {
                format = next(&mut i);
                if !serq::view::format_names().contains(&format.as_str()) {
                    argument_error(
                        cmd,
                        format!(
                            "invalid --format `{format}`\nhelp: choose {}",
                            serq::view::format_names().join(" or ")
                        ),
                    );
                }
            }
            "--out" => out = Some(next(&mut i)),
            _ => unreachable!("validated option"),
        }
        i += 1;
    }
    // Checking, target extraction and drawing do not execute or export this IR.
    let load = if matches!(cmd, "check" | "target" | "draw") {
        serq::load_inspected
    } else {
        serq::load
    };
    let base = if ov.trace.is_some() {
        None
    } else {
        file.parent()
    };
    if cmd == "target" {
        let prog = load(file, &ov).unwrap_or_else(|e| fail(file, e));
        let config = serq::target::vllm(&prog).unwrap_or_else(|e| fail(file, e));
        println!(
            "{}",
            serde_json::to_string_pretty(&config).expect("a JSON value serialises")
        );
        return;
    }
    let mut prog = load(file, &ov).unwrap_or_else(|e| fail(file, e));
    if inline {
        prog = serq::inline_trace(prog, base).unwrap_or_else(|e| fail(file, e));
    }
    match cmd {
        "check" => {
            // every station the view draws has an arrow in and one out (#624)
            serq::view::deployment::checked_figure(&prog).unwrap_or_else(|e| fail(file, e));
            println!(
                "OK: {} pool(s), {} stage(s), {} attribute(s), {} block(s)",
                prog.pools.len(),
                prog.stages.len(),
                prog.attrs.len(),
                prog.blocks.len()
            )
        }
        "ir" => println!("{}", prog.to_json()),
        "draw" => {
            let figure =
                serq::view::deployment::checked_figure(&prog).unwrap_or_else(|e| fail(file, e));
            let text = serq::view::render(&figure, &format).unwrap_or_else(|e| fail(file, e));
            match &out {
                None => print!("{text}"),
                Some(path) => {
                    if let Err(e) = std::fs::write(path, &text) {
                        eprintln!("cannot write {path}: {e}");
                        exit(1)
                    }
                }
            }
        }
        "run" => {
            let r = match &log {
                None => serq::run_ir(&prog, base),
                Some(_) => serq::run_ir_logged(&prog, base, usize::MAX),
            }
            .unwrap_or_else(|e| fail(file, e));
            if let Some(path) = &log
                && let Err(e) = std::fs::write(path, r.log.as_ref().expect("a logged run").json())
            {
                eprintln!("cannot write {path}: {e}");
                exit(1)
            }
            if let Some(d) = &dump
                && let Err(e) = r.dump(Path::new(d))
            {
                eprintln!("cannot write {d}: {e}");
                exit(1)
            }
            if json {
                println!("{}", r.json());
            } else {
                print!("{}", r.text());
            }
        }
        _ => unreachable!("validated command"),
    }
}
