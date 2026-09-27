//! Differential test against the real vLLM v1 scheduler (upstream
//! `ref/vllm` at 0c87a197, `tools/vllm_oracle.py` driven with a fake model
//! runner, rbln platform plugin disabled with `VLLM_PLUGINS=`). For each
//! scenario in `tools/oracle/*.json` the oracle's output (`*.out.json`)
//! gives the step of every request's first token and last token and the
//! number of preemptions; the same scenario as a seQ program with an
//! iteration cost of 1 must give the same steps.

use seq::{Overrides, run_source};

#[derive(Clone)]
struct Req {
    prompt: usize,
    out: usize,
    arrive: usize,
}

struct Scenario {
    budget: usize,
    max_seqs: usize,
    bs: usize,
    blocks: usize,
    chunk: usize,
    reqs: Vec<Req>,
    /// (first-token step, done step) per request, from the oracle
    want: Vec<(f64, f64)>,
    preemptions: u64,
}

fn r(prompt: usize, out: usize, arrive: usize) -> Req {
    Req {
        prompt,
        out,
        arrive,
    }
}

fn program(sc: &Scenario) -> String {
    let n = sc.reqs.len();
    let sel = |f: &dyn Fn(&Req) -> usize| -> String {
        // nested conditionals on `serial`
        let mut e = format!("{}", f(&sc.reqs[n - 1]));
        for i in (0..n - 1).rev() {
            e = format!("(serial == {i} ? {} : {e})", f(&sc.reqs[i]));
        }
        e
    };
    format!(
        r#"
        let bs = {bs};
        let B = {budget};
        pool kv {{ cap {blocks} * bs; block bs; evict lru; preempt lifo; }}
        pool slots {{ cap {max_seqs}; admit via engine; }}
        stage engine : step {{ budget B; chunk {chunk}; cost 1; memory kv; }}
        stage gate : delay;
        workload {{ arrive batch({n}); init {{ set prompt = {prompt}; set o = {out}; set arrive = {arrive}; }} }}
        route {{
          run gate (arrive);
          // the scheduler admits with the blocks of the chunk the budget the
          // running requests leave allows, if the whole prompt fits
          // (scheduler_reserve_full_isl), and grows from there
          hold slots (1), kv (min(prompt, budget_left(engine))) fits (prompt) {{
            run engine prefill (prompt) growing kv;
            observe first = now;
            observe who_first = serial;
            run engine decode (o - 1) growing kv;
          }}
          observe done = now;
          observe who = serial;
          end;
        }}
        run {{ horizon 100000; }}
        "#,
        bs = sc.bs,
        budget = sc.budget,
        blocks = sc.blocks - 1, // the null block
        max_seqs = sc.max_seqs,
        chunk = sc.chunk,
        prompt = sel(&|q| q.prompt),
        out = sel(&|q| q.out),
        arrive = sel(&|q| q.arrive),
    )
}

fn check(name: &str, sc: &Scenario) {
    let rep = run_source(&program(sc), &Overrides::default(), None).unwrap();
    // observations are stored in the order they were made, so pair each
    // value with the serial observed next to it
    let who = &rep.observe("who").unwrap().samples;
    let who_first = &rep.observe("who_first").unwrap().samples;
    let first = &rep.observe("first").unwrap().samples;
    let done = &rep.observe("done").unwrap().samples;
    let mut got = vec![(0.0, 0.0); sc.reqs.len()];
    for (k, &w) in who_first.iter().enumerate() {
        got[w as usize].0 = first[k];
    }
    for (k, &w) in who.iter().enumerate() {
        got[w as usize].1 = done[k];
    }
    assert_eq!(
        got,
        sc.want,
        "{name}: (first, done) per request\n{}",
        rep.text()
    );
    assert_eq!(
        rep.pool("kv").unwrap().preemptions,
        sc.preemptions,
        "{name}: preemptions"
    );
}

/// tools/oracle/preempt.json: two 80-token prompts on 10 blocks of 16 with
/// a budget of 100; the second admits with the 20 tokens the budget
/// leaves, cannot grow its own next chunk and preempts itself (vLLM
/// `running[-1]`), then waits for the first to finish.
#[test]
fn preempt() {
    check(
        "preempt",
        &Scenario {
            budget: 100,
            max_seqs: 16,
            bs: 16,
            blocks: 11,
            chunk: 0,
            reqs: vec![r(80, 30, 0), r(80, 30, 0)],
            want: vec![(1.0, 30.0), (31.0, 60.0)],
            preemptions: 1,
        },
    );
}

/// tools/oracle/chunked.json: a 3000-token prompt takes three 1024 chunks;
/// two later requests share the budget its last chunk leaves.
#[test]
fn chunked() {
    check(
        "chunked",
        &Scenario {
            budget: 1024,
            max_seqs: 16,
            bs: 16,
            blocks: 1000,
            chunk: 0,
            reqs: vec![r(3000, 2, 0), r(700, 5, 1), r(100, 3, 1)],
            want: vec![(3.0, 4.0), (4.0, 8.0), (4.0, 6.0)],
            preemptions: 0,
        },
    );
}

/// tools/oracle/seqcap.json: `max_num_seqs = 2` admits two of four.
#[test]
fn seqcap() {
    check(
        "seqcap",
        &Scenario {
            budget: 1024,
            max_seqs: 2,
            bs: 16,
            blocks: 1000,
            chunk: 0,
            reqs: vec![r(1024, 5, 0); 4],
            want: vec![(1.0, 5.0), (3.0, 7.0), (7.0, 11.0), (9.0, 13.0)],
            preemptions: 0,
        },
    );
}

/// tools/oracle/hol.json: FCFS with head-of-line blocking: the 16-token
/// third request waits behind a 96-token one that does not fit.
#[test]
fn head_of_line() {
    check(
        "hol",
        &Scenario {
            budget: 1024,
            max_seqs: 16,
            bs: 16,
            blocks: 11,
            chunk: 0,
            reqs: vec![r(96, 10, 0), r(96, 10, 0), r(16, 3, 0)],
            want: vec![(1.0, 10.0), (11.0, 20.0), (11.0, 13.0)],
            preemptions: 0,
        },
    );
}

/// tools/oracle/mixed.json: six requests of mixed lengths and arrivals on
/// 39 blocks with `max_num_seqs = 4` and a 512 budget.
#[test]
fn mixed() {
    check(
        "mixed",
        &Scenario {
            budget: 512,
            max_seqs: 4,
            bs: 16,
            blocks: 40,
            chunk: 0,
            reqs: vec![
                r(300, 40, 0),
                r(200, 30, 2),
                r(250, 20, 3),
                r(150, 60, 3),
                r(400, 10, 5),
                r(100, 25, 9),
            ],
            want: vec![
                (1.0, 40.0),
                (3.0, 32.0),
                (33.0, 52.0),
                (41.0, 100.0),
                (53.0, 62.0),
                (63.0, 87.0),
            ],
            preemptions: 0,
        },
    );
}

/// tools/oracle/longchunk.json: `long_prefill_token_threshold = 1000`
/// caps each request's chunk; two 3000-token prompts finish together.
#[test]
fn long_prefill_threshold() {
    check(
        "longchunk",
        &Scenario {
            budget: 4096,
            max_seqs: 16,
            bs: 16,
            blocks: 1000,
            chunk: 1000,
            reqs: vec![r(3000, 2, 0), r(3000, 2, 0)],
            want: vec![(3.0, 4.0), (3.0, 4.0)],
            preemptions: 0,
        },
    );
}
