//! `seq-lang run FILE [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--set k=expr]... [--trace F] [--json] [--dump DIR]`
//! `seq-lang check FILE [--set k=expr]...`
//! `seq-lang ir FILE [--set k=expr]... [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--trace F] [--inline-trace]`
//! `seq-lang draw FILE [--format tikz|svg] [--out PATH]` (experimental)
//! `seq-lang fmt [--check] FILE...`
//!
//! FILE is program text (`.seq`) or IR (`.json`, as written by `seq-lang ir`).

use std::path::Path;
use std::process::exit;

use seq::Overrides;
use seq::frontend::parser;

fn usage(cmd: &str) -> &'static str {
    match cmd {
        "run" => {
            "seq-lang run FILE [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--set name=expr]... [--trace F] [--json] [--dump DIR]"
        }
        "check" => "seq-lang check FILE [--set name=expr]...",
        "ir" => {
            "seq-lang ir FILE [--set name=expr]... [--seed N] [--horizon T] [--warmup T] [--arrivals N] [--trace F] [--inline-trace]"
        }
        "draw" => "seq-lang draw FILE [--set name=expr]... [--format tikz|svg] [--out PATH]",
        "fmt" => "seq-lang fmt [--check] FILE...",
        _ => "seq-lang <run|check|ir|draw|fmt> FILE [OPTIONS]",
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
        argument_error("fmt", "missing FILE\nhelp: supply one or more .seq files");
    }
    // Read and validate the whole batch before writing any file.
    let mut changes = Vec::new();
    for file in files {
        if file.extension().is_none_or(|ext| ext != "seq") {
            fail(file, "fmt expects a .seq file");
        }
        let source = std::fs::read_to_string(file).unwrap_or_else(|e| fail(file, e));
        let formatted =
            seq::frontend::fmt::format_file(&source, file).unwrap_or_else(|e| fail(file, e));
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
    if !matches!(cmd, "run" | "check" | "ir" | "draw" | "fmt") {
        argument_error(
            cmd,
            format!("unknown command `{cmd}`\nhelp: choose run, check, ir, draw, or fmt"),
        );
    }
    if cmd == "fmt" {
        format_files(&args);
        return;
    }
    if args.get(1).is_none_or(|s| s.starts_with("--")) {
        argument_error(
            cmd,
            "missing FILE\nhelp: supply a .seq program or .json IR file",
        );
    }
    let file = Path::new(&args[1]);
    let mut ov = Overrides::default();
    let mut json = false;
    let mut dump: Option<String> = None;
    let mut inline = false;
    let mut format = String::from("tikz");
    let mut out: Option<String> = None;
    let mut i = 2;
    while i < args.len() {
        let flag = args[i].as_str();
        let allowed: &[&str] = match flag {
            "--set" => &["run", "check", "ir", "draw"],
            "--seed" | "--horizon" | "--warmup" | "--arrivals" | "--trace" => &["run", "ir"],
            "--json" | "--dump" => &["run"],
            "--inline-trace" => &["ir"],
            "--format" | "--out" => &["draw"],
            _ => argument_error(
                cmd,
                format!("unknown option `{flag}`\nhelp: use the options in the usage below"),
            ),
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
            "--set" => {
                let kv = next(&mut i);
                let (k, v) = kv.split_once('=').unwrap_or_else(|| {
                    argument_error(
                        cmd,
                        format!("invalid --set `{kv}`; expected name=expr\nhelp: use --set rate=2"),
                    )
                });
                let name = k.trim();
                if name.is_empty()
                    || !name.chars().enumerate().all(|(i, c)| {
                        c == '_'
                            || if i == 0 {
                                c.is_alphabetic()
                            } else {
                                c.is_alphanumeric()
                            }
                    })
                {
                    argument_error(
                        cmd,
                        format!(
                            "invalid --set name `{name}`; expected an identifier\nhelp: use --set name=expr with a declared let name"
                        ),
                    );
                }
                let e = parser::parse_expr(v).unwrap_or_else(|e| argument_error(cmd,
                    format!("invalid expression in --set `{kv}`: {e}\nhelp: use --set name=expr, for example --set rate=2")));
                ov.lets.push((name.to_string(), e));
            }
            "--json" => json = true,
            "--dump" => dump = Some(next(&mut i)),
            "--inline-trace" => inline = true,
            "--format" => {
                format = next(&mut i);
                if !matches!(format.as_str(), "tikz" | "svg") {
                    argument_error(
                        cmd,
                        format!("invalid --format `{format}`\nhelp: choose tikz or svg"),
                    );
                }
            }
            "--out" => out = Some(next(&mut i)),
            _ => unreachable!("validated option"),
        }
        i += 1;
    }
    let base = if ov.trace.is_some() {
        None
    } else {
        file.parent()
    };
    let mut prog = seq::load(file, &ov).unwrap_or_else(|e| fail(file, e));
    if inline {
        prog = seq::inline_trace(prog, base).unwrap_or_else(|e| fail(file, e));
    }
    match cmd {
        "check" => println!(
            "OK: {} pool(s), {} stage(s), {} attribute(s), {} block(s)",
            prog.pools.len(),
            prog.stages.len(),
            prog.attrs.len(),
            prog.blocks.len()
        ),
        "ir" => println!("{}", prog.to_json()),
        "draw" => {
            let figure = seq::view::deployment::figure(&prog);
            let text = match format.as_str() {
                "tikz" => seq::view::tikz::render(&figure),
                "svg" => seq::view::svg::render(&figure),
                f => fail(file, format!("unknown --format `{f}` (tikz, svg)")),
            };
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
            let r = seq::run_ir(&prog, base).unwrap_or_else(|e| fail(file, e));
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
