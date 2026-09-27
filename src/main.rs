//! `seq-lang run FILE [--seed N] [--horizon T] [--warmup T] [--set k=expr]... [--trace F] [--json] [--dump DIR]`
//! `seq-lang check FILE [--set k=expr]...`
//! `seq-lang ir FILE [--set k=expr]... [--seed N] [--horizon T] [--warmup T] [--trace F] [--inline-trace]`
//!
//! FILE is program text (`.seq`) or IR (`.json`, as written by `seq-lang ir`).

use std::path::Path;
use std::process::exit;

use seq::{Overrides, parser};

fn usage() -> ! {
    eprintln!(
        "usage:\n  seq-lang run FILE [--seed N] [--horizon T] [--warmup T] [--set name=expr]... [--trace F] [--json] [--dump DIR]\n  seq-lang check FILE [--set name=expr]...\n  seq-lang ir FILE [--set name=expr]... [--seed N] [--horizon T] [--warmup T] [--trace F] [--inline-trace]\n\nFILE is program text (.seq) or IR (.json, as written by `seq-lang ir`)."
    );
    exit(2)
}

fn fail(file: &Path, e: impl std::fmt::Display) -> ! {
    eprintln!("{}: {e}", file.display());
    exit(1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        usage();
    }
    let cmd = args[0].as_str();
    let file = Path::new(&args[1]);
    let mut ov = Overrides::default();
    let mut json = false;
    let mut dump: Option<String> = None;
    let mut inline = false;
    let mut i = 2;
    while i < args.len() {
        let next = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_else(|| usage())
        };
        match args[i].as_str() {
            "--seed" => ov.seed = Some(next(&mut i).parse().unwrap_or_else(|_| usage())),
            "--horizon" => ov.horizon = Some(next(&mut i).parse().unwrap_or_else(|_| usage())),
            "--warmup" => ov.warmup = Some(next(&mut i).parse().unwrap_or_else(|_| usage())),
            "--trace" => ov.trace = Some(next(&mut i)),
            "--set" => {
                let kv = next(&mut i);
                let (k, v) = kv.split_once('=').unwrap_or_else(|| usage());
                let e = parser::parse_expr(v).unwrap_or_else(|e| {
                    eprintln!("--set {kv}: {e}");
                    exit(2)
                });
                ov.lets.push((k.trim().to_string(), e));
            }
            "--json" => json = true,
            "--dump" => dump = Some(next(&mut i)),
            "--inline-trace" => inline = true,
            _ => usage(),
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
        _ => usage(),
    }
}
