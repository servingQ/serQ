//! Queues are parse-time sugar (`docs/design/queue.md`): a program written
//! with queues compiles to the IR the same program written with pools,
//! stages and a server compiles to, up to the names the queue gives its
//! pools (`Q.p`), its stage (`Q`) and its entries' attributes (`Q.x`).

use serq::{
    Overrides, compile_source,
    frontend::parser::{parse, parse_expr},
};

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
        Ok(p) => match serq::frontend::link::link(&p, &Overrides::default()) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("accepted:\n{src}"),
        },
    };
    assert!(e.contains(needle), "{src}\n  {e}");
}

const WORKLOAD: &str = "workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set j = 0; } session { request gw; end; } } run { horizon 100; }";

/// The engine's admission, allocation and service inside the queue; the
/// named request selects the gateway's `route`; the family's size is a constant.
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
                 hold kv (min(prompt, budget_left(E))) reserve (prompt) {{
                   prefill (prompt) growing kv; decode (o - 1) growing kv;
                 }} cache (prompt + o);
               }}
             }}
             {WORKLOAD}"
        ),
        &format!(
            "let N = 2;
             pool kv[2] {{ cap 100; block 16; admit via E; }}
             stage E[2] : step {{ cost 1; memory kv; }}
             server {{
               hold kv[j] (min(prompt, budget_left(E[j]))) reserve (prompt) {{
                 prefill on E[j] (prompt) growing kv[j]; decode on E[j] (o - 1) growing kv[j];
               }} cache (prompt + o);
               observe done = now;
             }}
             {}",
            WORKLOAD.replace("request gw;", "request;")
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
        prefill (prompt) { hold kv (prompt) { run (prompt); } cache (prompt) lease kv (inf); }
      }
      queue D[ND] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        decode (prompt) { hold kv (prompt) { prefill (prompt) growing kv; mark first; } }
        decode (prompt) from src {
          set c = 0;
          hold kv (prompt) {
            nic[self].transfer (prompt - c) from src to kv (prompt - 1);
            prefill (1) growing kv; mark first;
            decode (o - 1) growing kv;
          }
        }
      }
      queue nic[ND] : link { serve ps(1); transfer (n) { run (n / Bw); } }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set j = 0; } session { request gw; end; } }
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
        hold kvP (prompt) { run P (prompt); } cache (prompt) lease kvP (inf);
        set c = 0;
        hold kvD[j] (prompt) {
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

/// A read over the sender's link and the receiver's at once: link queues
/// with no entry are their `serve`, the decoder names its own with `self`
/// and the prefiller's with the `from` name, which as an index is the
/// source member's.
#[test]
fn a_read_over_both_links_is_the_flat_read() {
    let queues = "
      let NP = 2; let ND = 2;
      queue gw : gateway { route {
        P[i].prefill (prompt);
        D[j].decode (prompt) from P[i];
      } }
      queue egress[NP] : link { serve ps(100); }
      queue ingress[ND] : link { serve ps(200); }
      stage setup : delay;
      queue P[NP] : prefill {
        pool kv { cap 1000; }
        serve fifo;
        prefill (prompt) { hold kv (prompt) { run (prompt); } cache (prompt) lease kv (inf); }
      }
      queue D[ND] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        decode (prompt) { hold kv (prompt) { prefill (prompt) growing kv; } }
        decode (prompt) from src {
          hold kv (prompt) {
            run setup (1);
            transfer on egress[src], ingress[self] (prompt) from src to kv (prompt - 1);
            decode (o - 1) growing kv;
          }
        }
      }
      share maxmin;
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { request gw; end; } }
      run { horizon 100; }";
    let flat = "
      let NP = 2; let ND = 2;
      stage egress[2] : ps(100);
      stage ingress[2] : ps(200);
      stage setup : delay;
      pool kvP[2] { cap 1000; }
      stage P[2] : fifo;
      pool kvD[2] { cap 1000; block 16; }
      stage D[2] : step { cost 1; memory kvD; }
      share maxmin;
      server {
        hold kvP[i] (prompt) { run P[i] (prompt); } cache (prompt) lease kvP[i] (inf);
        hold kvD[j] (prompt) {
          run setup (1);
          transfer on egress[i], ingress[j] (prompt) from kvP[i] to kvD[j] (prompt - 1);
          decode on D[j] (o - 1) growing kvD[j];
        }
      }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { request; end; } }
      run { horizon 100; }";
    same_ir(queues, flat, &[("P.kv", "kvP"), ("D.kv", "kvD")]);
}

/// A link says what crossing it costs: an entry, or its `serve`.
#[test]
fn a_link_has_a_cost() {
    refused(
        &format!("queue gw : gateway {{ route {{ }} }} queue nic : link {{ }} {WORKLOAD}"),
        "has neither a `serve` nor a `transfer` entry",
    );
    let calls = "
      queue gw : gateway { route { P.prefill (prompt); D.decode (prompt) from P; } }
      queue nic : link { serve ps(1); }
      queue P : prefill { pool kv { cap 100; } serve fifo; prefill (p) { hold kv (p) { run (p); } cache (p) lease kv (inf); } }
      queue D : decode { pool kv { cap 100; } serve step { cost 1; memory kv; }
        decode (p) { hold kv (p) { prefill (p) growing kv; } }
        decode (p) from src { hold kv (p) { BODY } } }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; } session { request gw; end; } }
      run { horizon 1; }";
    refused(
        &calls.replace("BODY", "nic.transfer (p) from src to kv (p);"),
        "its `serve` is its cost: `transfer on nic[…] (n) from S to P (m);`",
    );
    // the source member's index stands in an index only
    refused(
        &calls.replace("BODY", "transfer on nic (p + src) from src to kv (p);"),
        "reads `src` as a number",
    );
    // `P` is one queue: there is no member index for `src` to be
    refused(
        &calls
            .replace("queue nic : link", "queue nic[2] : link")
            .replace("BODY", "transfer on nic[src] (p) from src to kv (p);"),
        "`src` as an index: its source `P.kv` is no family's member",
    );
}

/// The `at admission` bindings are the header's: they see the parameters,
/// not the caller's attributes.
#[test]
fn an_admission_binding_sees_the_entry_only() {
    refused(
        "queue gw : gateway { route { set t0 = now; E.decode (prompt); } }
         queue E : decode { pool kv { cap 100; } serve step { cost 1; memory kv; }
           decode (prompt) { hold kv (x) at admission (x = prompt + t0) { prefill (1) growing kv; } } }
         workload { arrive batch(1); init { set prompt = 3; } session { request gw; end; } }
         run { horizon 10; }",
        "the header reads `t0`",
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
        &program("hold kv (prompt + t0) { prefill (1) growing kv; }", ""),
        "the header reads `t0`",
    );
    // the body: also the context and the request's hidden attributes, not
    // another attribute of the session
    refused(
        &program(
            "hold kv (prompt) { prefill (1) growing kv; observe w = now - t0; }",
            "",
        ),
        "reads `t0`, a session attribute set outside the queue",
    );
    assert!(
        parse(&program(
            "hold kv (prompt) { prefill (1) growing kv; decode (o - 1) growing kv; }",
            ""
        ))
        .is_ok(),
        "`o` is hidden, so the body may read it"
    );
    // another queue's pool
    refused(
        &program(
            "hold kv (prompt) { prefill (1) growing kv; observe q = holders(gw.kv); }",
            "",
        ),
        "not a pool or stage of `E`",
    );
    // its own pool is nobody else's
    refused(
        &program(
            "hold kv (prompt) { prefill (1) growing kv; }",
            "grow E.kv (1);",
        ),
        "only `E`'s entries hold it",
    );
    refused(
        &program(
            "hold kv (prompt) { prefill (1) growing kv; }",
            "observe x = E.late;",
        ),
        "no entry of `E` marks `late`",
    );
}

#[test]
fn roles_and_entries_agree() {
    let rest = "workload { arrive batch(1); init { set prompt = 1; } session { request gw; end; } } run { horizon 1; }";
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
        "never requested",
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
      queue P : prefill { pool kv { cap 10; } serve fifo; prefill (p) { hold kv (p) { run (p); } cache (p); } }
      queue D[2] : decode { pool kv { cap 10; } serve step { cost 1; memory kv; }
        decode (p) { hold kv (p) { prefill (p) growing kv; } }
        decode (p) from src { hold kv (p) { prefill (p) growing kv; } } }";
    let program = |route: &str| {
        format!(
            "queue gw : gateway {{ route {{ {route} }} }} {decls}
             workload {{ arrive batch(1); init {{ set prompt = 1; set j = 0; }} session {{ request gw; end; }} }} run {{ horizon 1; }}"
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
         workload { arrive batch(1); session { request gw; end; } } run { horizon 1; }",
        "not a family",
    );
}

#[test]
fn a_family_size_is_a_constant() {
    let rest = "queue gw : gateway { route { E[j].decode (1); } } workload { arrive batch(1); init { set j = 0; } session { request gw; end; } } run { horizon 1; }";
    let decl = |n: &str| {
        format!(
            "let N = 2; queue E[{n}] : decode {{ pool kv {{ cap 10; }} serve step {{ cost 1; memory kv; }} decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }} }} {rest}"
        )
    };
    assert!(parse(&decl("N")).is_ok());
    assert!(parse(&decl("N + 1")).is_ok());
    refused(&decl("N / 4"), "array size must be a positive integer");
    refused(&decl("j"), "positive integer or a `let` constant");
}

#[test]
fn requests_select_named_gateways_in_nested_sessions_and_before_declarations() {
    // Declaration order does not select a default. The session chooses two
    // gateways, while the third gateway contributes no executable statements.
    let queues = "
      workload { arrive batch(1); session {
        loop { branch (1) { request second; } else { request first; } end; }
      } }
      queue unused : gateway { route { observe unused = 99; } }
      queue first : gateway { route { observe selected = 1; } }
      queue second : gateway { route { observe selected = 2; } }
      run { horizon 1; }";
    let flat = "
      workload { arrive batch(1); }
      session { loop { branch (1) { observe selected = 2; } else { observe selected = 1; } end; } }
      run { horizon 1; }";
    same_ir(queues, flat, &[]);
    same_ir(&queues.replace("second", "router"), flat, &[]);
}

#[test]
fn a_gateway_declaration_does_not_bind_an_anonymous_request() {
    refused(
        "queue gw : gateway { route { } } workload { session { request; } }",
        "name a gateway with `request NAME;`",
    );
    // Mixing named and unnamed requests must not let an unresolved request
    // survive simply because the session already has an explicit target.
    refused(
        "queue gw : gateway { route { } } workload { session { request gw; request; } }",
        "name a gateway with `request NAME;`",
    );
    refused(
        "queue gw : gateway { route { } } workload { session { request missing; } }",
        "no queue `missing` is declared",
    );
    refused(
        "queue P : prefill { serve fifo; prefill (p) { run (p); } }
         workload { session { request P; } }",
        "does not play `gateway`",
    );
    refused(
        "queue gw : gateway { route { } } session { request gw; }",
        "`session` inside `workload`",
    );
    refused(
        "queue gw : gateway { route { request gw; } }",
        "does not request itself",
    );
    refused(
        "queue gw : gateway { route { } }
         workload { session { request gw; } } session { end; }",
        "one session",
    );
}

#[test]
fn named_gateways_and_anonymous_servers_have_distinct_requests() {
    same_ir(
        "queue gw : gateway { route { observe selected = 2; } }
         server { observe selected = 1; }
         workload { arrive batch(1); session { request; request gw; end; } }
         run { horizon 1; }",
        "workload { arrive batch(1); }
         session { observe selected = 1; observe selected = 2; end; }
         run { horizon 1; }",
        &[],
    );
}

#[test]
fn call_indices_obey_entry_read_boundaries() {
    let program = |call: &str| {
        format!(
            "queue gw : gateway {{ route {{ E.prefill (1); }} }}
         queue E : prefill {{ serve fifo; prefill (p) {{ {call} }} }}
         queue F[2] : prefill {{ serve fifo; prefill (p) {{ }} }}
         workload {{ arrive batch(1); init {{ set j = 0; }} session {{ request gw; end; }} }}
         run {{ horizon 1; }}"
        )
    };
    for call in [
        "F[j].prefill (p);",
        "F[0].prefill (p) from F[j];",
        "F[0].prefill (p) from F[0] to F[j].kv (1);",
    ] {
        refused(
            &program(call),
            "reads `j`, a session attribute set outside the queue",
        );
    }
}

#[test]
fn each_overload_substitutes_only_its_own_locals() {
    same_ir(
        "queue gw : gateway { route { D.decode (1); } }
         queue D : decode { serve fifo;
           decode (p) { observe seen = x; }
           decode (p) from src { set x = 1; }
         }
         workload { arrive batch(1); hidden x; init { set x = 3; }
           session { request gw; end; } }
         run { horizon 1; }",
        "stage D : fifo;
         workload { arrive batch(1); hidden x; init { set x = 3; } }
         session { observe seen = x; end; } run { horizon 1; }",
        &[],
    );
}

#[test]
fn array_sizes_reject_direct_and_indirect_overrides() {
    let src = "let N = 2; let M = N + 1;
      queue D[M] : decode { serve fifo; decode (p) { } }
      queue gw : gateway { route { D[2].decode (1); } }
      workload { arrive batch(1); session { request gw; end; } }
      run { horizon 1; }";
    for name in ["N", "M"] {
        let ov = Overrides {
            lets: vec![(name.into(), parse_expr("4").unwrap())],
            ..Overrides::default()
        };
        let err = compile_source(src, &ov).unwrap_err();
        assert!(
            err.contains("affects an array size resolved during parsing"),
            "{err}"
        );
    }
}

#[test]
fn long_acyclic_delegation_succeeds_and_cycles_fail() {
    let mut src = String::from("queue gw : gateway { route { Q0.prefill (1); } }\n");
    for i in 0..20 {
        let body = if i == 19 {
            "observe done = p;".to_string()
        } else {
            format!("Q{}.prefill (p);", i + 1)
        };
        src.push_str(&format!(
            "queue Q{i} : prefill {{ serve fifo; prefill (p) {{ {body} }} }}\n"
        ));
    }
    src.push_str("workload { arrive batch(1); session { request gw; end; } } run { horizon 1; }");
    compile_source(&src, &Overrides::default()).unwrap();
    let cycle = src.replace("observe done = p;", "Q0.prefill (p);");
    refused(&cycle, "entries call each other in a cycle");
}

#[test]
fn expansion_errors_point_to_the_call() {
    let src = "queue gw : gateway { route {\n  D.decode ();\n} }
queue D : decode { serve fifo; decode (p) { } }
workload { arrive batch(1); session { request gw; end; } }
run { horizon 1; }";
    let err = parse(src).unwrap_err();
    assert_eq!((err.line, err.col), (2, 3));
    assert!(err.msg.contains("takes 1 argument(s), got 0"), "{err}");
}
