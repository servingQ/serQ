//! `seq-lang run FILE [--seed N] [--horizon T] [--warmup T] [--set k=expr]... [--json]`
//! `seq-lang check FILE`

use std::path::Path;
use std::process::exit;

use seq::{Overrides, parser};

fn usage() -> ! {
    eprintln!(
        "usage:\n  seq-lang run FILE [--seed N] [--horizon T] [--warmup T] [--set name=expr]... [--json] [--dump DIR]\n  seq-lang check FILE"
    );
    exit(2)
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
            _ => usage(),
        }
        i += 1;
    }
    let src = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {}: {e}", file.display());
            exit(1)
        }
    };
    match cmd {
        "check" => match seq::check_source(&src, &ov) {
            Ok(l) => println!(
                "OK: {} pool(s), {} stage(s), {} attribute(s), {} block(s)",
                l.pools.len(),
                l.stages.len(),
                l.attrs.len(),
                l.blocks.len()
            ),
            Err(e) => {
                eprintln!("{}: {e}", file.display());
                exit(1)
            }
        },
        "run" => match seq::run_source(&src, &ov, file.parent()) {
            Ok(r) => {
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
            Err(e) => {
                eprintln!("{}: {e}", file.display());
                exit(1)
            }
        },
        _ => usage(),
    }
}
