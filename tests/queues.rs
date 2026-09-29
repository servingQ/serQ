//! Queues are parse-time sugar (`docs/design/queue.md`): a program written
//! with queues compiles to the IR the same program written with pools,
//! stages and a server compiles to, up to the names the queue gives its
//! pools (`Q.p`), its stage (`Q`) and its entries' attributes (`Q.x`).

use seq::{Overrides, compile_source, parser::parse};

/// Compile both; the queue program's IR with `renames` applied to its JSON
/// is the flat program's IR.
fn same_ir(queues: &str, flat: &str, renames: &[(&str, &str)]) {
    let q = compile_source(queues, &Overrides::default())
        .unwrap_or_else(|e| panic!("queues: {e}\n{queues}"))
        .to_json();
    let f = compile_source(flat, &Overrides::default())
        .unwrap_or_else(|e| panic!("flat: {e}\n{flat}"))
        .to_json();
    let mut q = q;
    for (from, to) in renames {
        q = q.replace(&format!("\"{from}\""), &format!("\"{to}\""));
    }
    assert_eq!(q, f);
}

fn refused(src: &str, needle: &str) {
    let e = match parse(src) {
        Err(e) => e.to_string(),
        Ok(p) => match seq::link::link(&p, &Overrides::default()) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("accepted:\n{src}"),
        },
    };
    assert!(e.contains(needle), "{src}\n  {e}");
}

const WORKLOAD: &str = "workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set j = 0; } session { request; end; } } run { horizon 100; }";

/// The engine's admission, allocation and service inside the queue; the
/// gateway's `route` is the server; the family's size is a constant.
#[test]
fn a_queue_is_its_pools_its_stage_and_the_server_statements() {
    same_ir(
        &format!(
            "let N = 2;
             queue gw : gateway {{ route {{ E[j].decode (prompt); observe done = now; }} }}
             queue E[N] : decode {{
               pool kv {{ cap 100; block 16; admit via E; }}
               serve step {{ cost 1; memory kv; }}
               decode (prompt) {{
                 admit if kv (min(prompt, budget_left(E))) reserve (prompt) fit {{
                   prefill (prompt) growing kv; decode (o - 1) growing kv;
                 }} keep (prompt + o);
               }}
             }}
             {WORKLOAD}"
        ),
        &format!(
            "let N = 2;
             pool kv[2] {{ cap 100; block 16; admit via E; }}
             stage E[2] : step {{ cost 1; memory kv; }}
             server {{
               admit if kv[j] (min(prompt, budget_left(E[j]))) reserve (prompt) fit {{
                 prefill on E[j] (prompt) growing kv[j]; decode on E[j] (o - 1) growing kv[j];
               }} keep (prompt + o);
               observe done = now;
             }}
             {WORKLOAD}"
        ),
        &[("E.kv", "kv")],
    );
}

/// `from P[i]` is the pool `P`'s entry leases; a link's `transfer … from … to
/// (m)` is `run; load; release`, the time the link's own; an entry's `set` is
/// the queue's attribute; `mark x` is `set Q.x = now`, read as `Q[i].x`.
#[test]
fn a_transfer_between_queues_is_the_flat_transfer() {
    let queues = "
      let NP = 1; let ND = 2; let Bw = 1000;
      queue gw : gateway { route {
        set t0 = now;
        P.prefill (prompt);
        D[j].decode (prompt) from P;
        observe ttft = D[j].first - t0;
      } }
      queue P[NP] : prefill {
        pool kv { cap 1000; }
        serve fifo;
        prefill (prompt) { admit if kv (prompt) fit { run (prompt); } keep (prompt) lease kv (inf); }
      }
      queue D[ND] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        decode (prompt) { admit if kv (prompt) fit { prefill (prompt) growing kv; mark first; } }
        decode (prompt) from src {
          set c = 0;
          admit if kv (prompt) fit {
            nic[self].transfer (prompt - c) from src to kv (prompt - 1);
            prefill (1) growing kv; mark first;
            decode (o - 1) growing kv;
          }
        }
      }
      queue nic[ND] : link { serve ps(1); transfer (n) { run (n / Bw); } }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set j = 0; } session { request; end; } }
      run { horizon 100; }";
    let flat = "
      let NP = 1; let ND = 2; let Bw = 1000;
      pool kvP { cap 1000; }
      pool kvD[2] { cap 1000; block 16; }
      stage P : fifo;
      stage D[2] : step { cost 1; memory kvD; }
      stage nic[2] : ps(1);
      server {
        set t0 = now;
        admit if kvP (prompt) fit { run P (prompt); } keep (prompt) lease kvP (inf);
        set c = 0;
        admit if kvD[j] (prompt) fit {
          transfer on nic[j] ((prompt - c) / Bw) from kvP to kvD[j] (prompt - 1);
          prefill on D[j] (1) growing kvD[j]; set first = now;
          decode on D[j] (o - 1) growing kvD[j];
        }
        observe ttft = first - t0;
      }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set j = 0; } session { request; end; } }
      run { horizon 100; }";
    same_ir(
        queues,
        flat,
        &[
            ("P.kv", "kvP"),
            ("D.kv", "kvD"),
            ("D.c", "c"),
            ("D.first", "first"),
        ],
    );
}

#[test]
fn an_entry_sees_its_parameters_and_its_queue() {
    let program = |entry: &str, gw: &str| {
        format!(
            "queue gw : gateway {{ route {{ set t0 = now; {gw} E.decode (prompt); }} }}
             queue E : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }} decode (prompt) {{ {entry} }} }}
             {WORKLOAD}"
        )
    };
    // the header: parameters, own pools and stage, constants
    refused(
        &program(
            "admit if kv (prompt + t0) fit { prefill (1) growing kv; }",
            "",
        ),
        "the header reads `t0`",
    );
    // the body: also the context and the request's hidden attributes, not
    // another attribute of the session
    refused(
        &program(
            "admit if kv (prompt) fit { prefill (1) growing kv; observe w = now - t0; }",
            "",
        ),
        "reads `t0`, a session attribute set outside the queue",
    );
    assert!(
        parse(&program(
            "admit if kv (prompt) fit { prefill (1) growing kv; decode (o - 1) growing kv; }",
            ""
        ))
        .is_ok(),
        "`o` is hidden, so the body may read it"
    );
    // another queue's pool
    refused(
        &program(
            "admit if kv (prompt) fit { prefill (1) growing kv; observe q = holders(gw.kv); }",
            "",
        ),
        "not a pool or stage of `E`",
    );
    // its own pool is nobody else's
    refused(
        &program(
            "admit if kv (prompt) fit { prefill (1) growing kv; }",
            "grow E.kv (1);",
        ),
        "only `E`'s entries hold it",
    );
    refused(
        &program(
            "admit if kv (prompt) fit { prefill (1) growing kv; }",
            "observe x = E.late;",
        ),
        "no entry of `E` marks `late`",
    );
}

#[test]
fn roles_and_entries_agree() {
    let rest = "workload { arrive batch(1); init { set prompt = 1; } session { request; end; } } run { horizon 1; }";
    refused(
        &format!("queue gw : gateway {{ route {{ }} }} queue P : prefill {{ serve fifo; }} {rest}"),
        "plays `prefill` and has no `prefill` entry",
    );
    refused(
        &format!(
            "queue gw : gateway {{ route {{ }} }} queue P {{ serve fifo; prefill (p) {{ run (p); }} }} {rest}"
        ),
        "declare `queue P : prefill`",
    );
    refused(
        &format!(
            "queue gw : gateway {{ route {{ }} }} queue P : prefill {{ serve fifo; prefill (p) {{ run (p); }} warm (p) {{ run (p); }} }} {rest}"
        ),
        "not an entry of any role",
    );
    refused(
        &format!("queue gw : gateway {{ route {{ }} }} server {{ }} {rest}"),
        "the gateway's `route` is the server",
    );
    refused(
        &format!("queue gw[2] : gateway {{ route {{ }} }} {rest}"),
        "a gateway is one queue",
    );
    refused(
        "queue prefill : link { serve fifo; transfer (n) { run (n); } } stage s : fifo; session { run s (1); end; }",
        "serving word",
    );
}

#[test]
fn a_call_is_checked_against_the_entry() {
    let decls = "
      queue P : prefill { pool kv { cap 10; } serve fifo; prefill (p) { admit if kv (p) fit { run (p); } keep (p); } }
      queue D[2] : decode { pool kv { cap 10; } serve step { cost 1; memory kv; }
        decode (p) { admit if kv (p) fit { prefill (p) growing kv; } }
        decode (p) from src { admit if kv (p) fit { prefill (p) growing kv; } } }";
    let program = |route: &str| {
        format!(
            "queue gw : gateway {{ route {{ {route} }} }} {decls}
             workload {{ arrive batch(1); init {{ set prompt = 1; set j = 0; }} session {{ request; end; }} }} run {{ horizon 1; }}"
        )
    };
    refused(&program("D.decode (prompt);"), "is a family of 2; index it");
    refused(
        &program("P[j].prefill (prompt);"),
        "is one queue, not a family",
    );
    refused(
        &program("D[j].decode (prompt, 1);"),
        "takes 1 argument(s), got 2",
    );
    refused(&program("D[j].decode (~exp(1));"), "draws a sample");
    refused(&program("D[j].warm (prompt);"), "has no such entry");
    // `from P`: P leases nothing, so there is nothing to take
    refused(
        &program("P.prefill (prompt); D[j].decode (prompt) from P;"),
        "no entry of `P` leases a pool",
    );
    refused(&program("X.decode (prompt);"), "no queue `X` is declared");
    // `self` is a member's word
    refused(
        "stage s : fifo; session { observe s = self; end; } run { horizon 1; }",
        "`self` is a queue entry's word",
    );
    refused(
        "queue gw : gateway { route { observe s = self; } } stage s : fifo;
         workload { arrive batch(1); session { request; end; } } run { horizon 1; }",
        "not a family",
    );
}

#[test]
fn a_family_size_is_a_constant() {
    let rest = "queue gw : gateway { route { E[j].decode (1); } } workload { arrive batch(1); init { set j = 0; } session { request; end; } } run { horizon 1; }";
    let decl = |n: &str| {
        format!(
            "let N = 2; queue E[{n}] : decode {{ pool kv {{ cap 10; }} serve step {{ cost 1; memory kv; }} decode (p) {{ admit if kv (p) fit {{ prefill (p) growing kv; }} }} }} {rest}"
        )
    };
    assert!(parse(&decl("N")).is_ok());
    assert!(parse(&decl("N + 1")).is_ok());
    refused(&decl("N / 4"), "array size must be a positive integer");
    refused(&decl("j"), "positive integer or a `let` constant");
}
