//! The multi-turn prefix-cache scenario (`tools/oracle/cache_trace.*`): the
//! vLLM replay program on a unit step clock gives, for every turn, the
//! first-token step, the last-token step and the cached tokens that the
//! real scheduler gives (`cache_trace.out.csv`, from
//! `tools/vllm_replay_oracle.py`). Lean proves the same for its executable
//! semantics (`RouteOracle.lean`, `vllm_cache_trace`).

use std::path::Path;

use seq::{Overrides, parser, run_file};

#[test]
fn cache_trace_matches_the_real_scheduler() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(dir.join("programs/vllm_replay.route")).unwrap();
    let csv = dir.join("tools/oracle/cache_trace.csv");
    let src = src.replace(
        "trace \"data/short_base.csv\" ordered;",
        &format!("trace \"{}\" ordered;", csv.display()),
    );
    let tmp = std::env::temp_dir().join(format!("cache_trace_{}.route", std::process::id()));
    std::fs::write(&tmp, src).unwrap();
    let mut ov = Overrides::default();
    for (k, v) in [
        ("N", "3"),
        ("spacing", "3"),
        ("B", "64"),
        ("max_seqs", "4"),
        ("blocks", "20"),
        ("c_it", "1"),
        ("d", "0"),
        ("e", "0"),
        ("a", "0"),
        ("b", "0"),
        ("c0", "0"),
        ("f", "0"),
    ] {
        ov.lets
            .push((k.to_string(), parser::parse_expr(v).unwrap()));
    }
    let r = run_file(&tmp, &ov).unwrap();
    let _ = std::fs::remove_file(&tmp);
    let get = |name: &str| {
        let o = r.observe(name).unwrap();
        let mut m = std::collections::HashMap::new();
        for (rec, v) in o.records.iter().zip(&o.samples) {
            m.insert((rec.1, rec.2), *v);
        }
        m
    };
    let (sent, ttft, lat, cached) = (
        get("sent"),
        get("ttft"),
        get("latency"),
        get("cached_tokens"),
    );
    let want = std::fs::read_to_string(dir.join("tools/oracle/cache_trace.out.csv")).unwrap();
    let mut n = 0;
    for line in want.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        let key = (f[0].parse::<u64>().unwrap(), f[1].parse::<u32>().unwrap());
        let (first, done, c): (f64, f64, f64) = (
            f[3].parse().unwrap(),
            f[4].parse().unwrap(),
            f[6].parse().unwrap(),
        );
        assert_eq!(sent[&key] + ttft[&key], first, "{key:?} first token");
        assert_eq!(sent[&key] + lat[&key], done, "{key:?} last token");
        assert_eq!(cached[&key], c, "{key:?} cached tokens");
        n += 1;
    }
    assert_eq!(n, 9);
}
