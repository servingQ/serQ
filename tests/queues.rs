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
/// the queue's attribute; `mark x` is `set Q.x = now`, read as `Q.x`.
#[test]
fn a_transfer_between_queues_is_the_flat_transfer() {
    let queues = "
      let ND = 2; let Bw = 1000;
      queue gw : gateway { route {
        set t0 = now;
        P.prefill (prompt);
        D[j].decode (prompt) from P;
        observe ttft = D.first - t0;
      } }
      queue P : prefill {
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
      let ND = 2; let Bw = 1000;
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

/// A link's `latency` is a wait before every transfer over it: a delay
/// stage of its own, run first, one per link named, in the order named.
#[test]
fn a_link_latency_is_a_wait_before_the_read() {
    let queues = "
      let x0 = 0.5; let x1 = 0.25;
      queue gw : gateway { route {
        P[i].prefill (prompt);
        D[j].decode (prompt) from P[i];
      } }
      queue egress[2] : link { serve ps(100) latency x1; }
      queue ingress[2] : link { serve ps(200) latency x0; }
      queue P[2] : prefill {
        pool kv { cap 1000; }
        serve fifo;
        prefill (prompt) { hold kv (prompt) { run (prompt); } cache (prompt) lease kv (inf); }
      }
      queue D[2] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        decode (prompt) { hold kv (prompt) { prefill (prompt) growing kv; } }
        decode (prompt) from src {
          hold kv (prompt) {
            transfer on egress[src], ingress[self] (prompt) from src to kv (prompt - 1);
            decode (o - 1) growing kv;
          }
        }
      }
      share maxmin;
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { request gw; end; } }
      run { horizon 100; }";
    let flat = "
      let x0 = 0.5; let x1 = 0.25;
      stage egress[2] : ps(100);
      stage egressL[2] : delay;
      stage ingress[2] : ps(200);
      stage ingressL[2] : delay;
      pool kvP[2] { cap 1000; }
      stage P[2] : fifo;
      pool kvD[2] { cap 1000; block 16; }
      stage D[2] : step { cost 1; memory kvD; }
      share maxmin;
      server {
        hold kvP[i] (prompt) { run P[i] (prompt); } cache (prompt) lease kvP[i] (inf);
        hold kvD[j] (prompt) {
          run egressL[i] (x1);
          run ingressL[j] (x0);
          transfer on egress[i], ingress[j] (prompt) from kvP[i] to kvD[j] (prompt - 1);
          decode on D[j] (o - 1) growing kvD[j];
        }
      }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { request; end; } }
      run { horizon 100; }";
    same_ir(
        queues,
        flat,
        &[
            ("P.kv", "kvP"),
            ("D.kv", "kvD"),
            ("egress.latency", "egressL"),
            ("ingress.latency", "ingressL"),
        ],
    );
}

/// The latency is the link's constant: an entry the transfer is written in
/// does not rename it, a draw or a reading is refused, and `--set` reaches
/// it (#196 review).
#[test]
fn a_latency_is_the_links_constant() {
    let program = |latency: &str, param: &str| {
        format!(
            "let x = 0.5;
      queue gw : gateway {{ route {{ P.prefill (prompt); D.decode (32) from P; }} }}
      queue egress : link {{ serve ps(100); }}
      queue ingress : link {{ serve ps(200) latency {latency}; }}
      queue P : prefill {{
        pool kv {{ cap 1000; }}
        serve fifo;
        prefill (prompt) {{ hold kv (prompt) {{ run (prompt); }} cache (prompt) lease kv (inf); }}
      }}
      queue D : decode {{
        pool kv {{ cap 1000; }}
        serve step {{ cost 1; memory kv; }}
        decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }}
        decode ({param}) from src {{
          hold kv ({param}) {{ transfer on egress, ingress ({param}) from src to kv ({param} - 1); }}
        }}
      }}
      share maxmin;
      workload {{ arrive batch(1); init {{ set prompt = 32; }} session {{ request gw; end; }} }}
      run {{ horizon 100; }}"
        )
    };
    let wait = |src: &str, ov: &Overrides| -> String {
        let p = compile_source(src, ov).unwrap_or_else(|e| panic!("{e}\n{src}"));
        let ir = p.to_json();
        let at = ir.find("\"Delay\"").map(|_| ()).is_some();
        assert!(at, "a delay stage");
        ir
    };
    // the entry's parameter `x` is not the constant `x`
    let named = wait(&program("x", "x"), &Overrides::default());
    let other = wait(&program("x", "y"), &Overrides::default());
    assert_eq!(named, other.replace("\"y\"", "\"x\""));
    assert!(named.contains("0.5"), "waits 0.5, the constant");
    // --set reaches the latency
    let ov = Overrides {
        lets: vec![("x".into(), parse_expr("0.125").unwrap())],
        ..Default::default()
    };
    assert!(wait(&program("x", "y"), &ov).contains("0.125"));
    for bad in ["~exp(1)", "work(egress)"] {
        refused(
            &program(bad, "y"),
            "`latency` is a number or a constant over `let`s",
        );
    }
}

/// In an expression a word that is also a keyword (`latency`, `cap`) can
/// only be a name: a `def` argument that reads it is checked against what
/// the body assigns like any other name (#196 review).
#[test]
fn a_keyword_named_attribute_is_a_read() {
    for word in ["latency", "cap"] {
        let src = format!(
            "def get() = {word};
             def f(x) {{ set {word} = 2; observe o = x; }}
             stage s : delay;
             session {{ set {word} = 1; f(get()); run s (1); end; }}
             run {{ horizon 1; }}"
        );
        let e = compile_source(&src, &Overrides::default()).unwrap_err();
        assert!(
            e.contains(&format!("reads `{word}`, which `f` assigns")),
            "{e}"
        );
    }
}

/// `latency` is a link's, a number or a constant, and not a called link's.
#[test]
fn a_latency_belongs_to_a_link() {
    refused(
        &format!(
            "queue gw : gateway {{ route {{ }} }} queue P : prefill {{ serve fifo latency 1; prefill (p) {{ run (p); }} }} {WORKLOAD}"
        ),
        "`latency` belongs to the `serve` of a queue that plays `link`",
    );
    refused(
        "stage s : ps(1) latency 1; session { run s (1); end; } run { horizon 1; }",
        "`latency` belongs to the `serve` of a queue that plays `link`",
    );
    refused(
        &format!(
            "queue gw : gateway {{ route {{ }} }} queue nic : link {{ serve ps(1) latency 1; transfer (n) {{ run (n); }} }} {WORKLOAD}"
        ),
        "write the latency in the entry body",
    );
    refused(
        &format!(
            "queue gw : gateway {{ route {{ }} }} queue nic : link {{ serve ps(1) latency prompt; }} {WORKLOAD}"
        ),
        "`latency` is a number or a constant over `let`s",
    );
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
        "no entry of `E` marks or sets `late`",
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
        &format!("queue gw[1] : gateway {{ route {{ }} }} {rest}"),
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
    // a family of one is still called by index: a program reads the same at N = 1
    let one = parse(&decl("N - 1")).unwrap();
    assert!(serq::frontend::link::link(&one, &Overrides::default()).is_ok());
    // and only by index: one call, one spelling, whatever N is
    refused(
        &decl("N - 1").replace("E[j].decode", "E.decode"),
        "is a family of 1; index it",
    );
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
    for (call, needle) in [
        (
            "F[j].prefill (p);",
            "reads `j`, a session attribute set outside the queue",
        ),
        (
            "F[0].prefill (p) from F[j];",
            "reads `j`, a session attribute set outside the queue",
        ),
        (
            "F[0].prefill (p) from F[0] to F[j].kv (1);",
            "holds `F.kv`, which is not a pool of `E`",
        ),
        // a stage's index is read as the body reads (#87 review)
        (
            "run F[j] (1);",
            "reads `j`, a session attribute set outside the queue",
        ),
    ] {
        refused(&program(call), needle);
    }
}

/// A chain of entries that each call the next twice has no cycle and
/// doubles at every link: the expansion is bounded, not the memory.
#[test]
fn an_expansion_is_bounded() {
    let mut decls = String::new();
    let n = 30;
    for k in 0..n {
        let body = if k + 1 < n {
            format!("Q{}.prefill (p); Q{}.prefill (p);", k + 1, k + 1)
        } else {
            "run (p);".to_string()
        };
        decls.push_str(&format!(
            "queue Q{k} : prefill {{ serve fifo; prefill (p) {{ {body} }} }}\n"
        ));
    }
    refused(
        &format!(
            "queue gw : gateway {{ route {{ Q0.prefill (prompt); }} }} {decls}
             workload {{ arrive batch(1); init {{ set prompt = 1; }} session {{ request gw; end; }} }} run {{ horizon 1; }}"
        ),
        "the queue calls expand to more than",
    );
}

/// A session's own attributes (`out`, `more`, …) are an entry's only if
/// the workload hides them: the scheduler knows the cache hit and what it
/// has computed, not the length the session wants.
#[test]
fn a_session_attribute_reaches_an_entry_only_if_hidden() {
    let program = |hidden: &str| {
        format!(
            "queue gw : gateway {{ route {{ E.decode (prompt); }} }}
             queue E : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; decode (out) growing kv; }} }} }}
             workload {{ arrive batch(1); {hidden} init {{ set prompt = 3; set out = 2; }} session {{ request gw; end; }} }}
             run {{ horizon 10; }}"
        )
    };
    refused(
        &program(""),
        "reads `out`, a session attribute set outside the queue",
    );
    compile_source(&program("hidden out;"), &Overrides::default()).unwrap();
}

/// #203: a `def` that says `request gw;` captures what `gw`'s `route`
/// assigns, not what every gateway's does.
#[test]
fn a_def_captures_what_the_gateway_it_requests_assigns() {
    let program = |defs: &str, session: &str| {
        format!(
            "{defs}
             queue clean : gateway {{ route {{ E.decode (prompt); }} }}
             queue dirty : gateway {{ route {{ set x = now; E.decode (prompt); }} }}
             queue E : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }} }}
             workload {{ arrive batch(1); hidden o; init {{ set prompt = 3; set o = 2; }} session {{ set x = 1; {session} end; }} }} run {{ horizon 10; }}"
        )
    };
    let go = "def go(x) { request clean; observe b = x; }";
    compile_source(&program(go, "go(x);"), &Overrides::default()).unwrap();
    let go = "def go(x) { request dirty; observe b = x; }";
    refused(&program(go, "go(x);"), "an argument of `go` reads `x`");
    // a gateway a parameter names is the argument's
    let send = "def send(g, x) { request g; observe b = x; }";
    compile_source(&program(send, "send(clean, x);"), &Overrides::default()).unwrap();
    refused(
        &program(send, "send(dirty, x);"),
        "an argument of `send` reads `x`",
    );
    refused(
        &program(send, "send(dirty, x);"),
        "which its `request dirty;` assigns",
    );
    // through a definition the body passes a gateway to, that gateway
    let ask = "def ask(g) { request g; } def go(x) { ask(clean); observe b = x; }";
    compile_source(&program(ask, "go(x);"), &Overrides::default()).unwrap();
    let ask = "def ask(g) { request g; } def go(x) { ask(dirty); observe b = x; }";
    refused(&program(ask, "go(x);"), "an argument of `go` reads `x`");
    // and one it passes its own parameter to, the argument's
    let ask = "def ask(g) { request g; } def go(g, x) { ask(g); observe b = x; }";
    compile_source(&program(ask, "go(clean, x);"), &Overrides::default()).unwrap();
    refused(
        &program(ask, "go(dirty, x);"),
        "an argument of `go` reads `x`",
    );
    // and through one that names its gateway, that one
    let ask = "def ask() { request clean; } def go(x) { ask(); observe b = x; }";
    compile_source(&program(ask, "go(x);"), &Overrides::default()).unwrap();
}

/// The third review of #87: six ways a program still got past the queue's
/// contract, each refused or, for a constant, accepted as the linker does.
#[test]
fn the_contract_holds_at_every_edge() {
    let wl = |session: &str| {
        format!(
            "workload {{ arrive batch(1); hidden o; init {{ set prompt = 3; set o = 2; }} session {{ {session} }} }} run {{ horizon 10; }}"
        )
    };
    let engine = "queue E : decode { pool kv { cap 100; } serve step { cost 1; memory kv; }
                    decode (p) { hold kv (p) { prefill (p) growing kv; } } }";
    // 1. what a named gateway assigns is what a request assigns: an argument
    //    that reads it would read the new value
    refused(
        &format!(
            "def go(x) {{ request gw; observe b = x; }}
             queue gw : gateway {{ route {{ set t0 = now; E.decode (prompt); }} }} {engine}
             {}",
            wl("set t0 = 1; go(t0); end;")
        ),
        "an argument of `go` reads `t0`",
    );
    // 2. a family's size folds as the linker folds a constant
    compile_source(
        &format!(
            "let N = min(2, 3);
             queue gw : gateway {{ route {{ E[N - 1].decode (prompt); }} }}
             queue E[N] : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p + N) {{ prefill (p) growing kv; }} }} }}
             {}",
            wl("request gw; end;")
        ),
        &Overrides::default(),
    )
    .unwrap();
    // 3. a role's entry has the role's parameters
    refused(
        &format!(
            "queue gw : gateway {{ route {{ P.prefill (1, 2); }} }}
             queue P : prefill {{ serve fifo; prefill (a, b) {{ run (a + b); }} }}
             {}",
            wl("request gw; end;")
        ),
        "takes 1 parameter(s), as the role says",
    );
    // 4. a queue that is also a gateway: only its `route` is the deployment's
    refused(
        &format!(
            "queue gw : gateway, prefill {{ serve fifo; route {{ gw.prefill (prompt); }} prefill (p) {{ run (p + o + prompt); }} }}
             {}",
            wl("request gw; end;")
        ),
        "reads `prompt`, a session attribute set outside the queue",
    );
    // 5. the serving forms do not take another queue's pool either
    refused(
        &format!(
            "pool shared {{ cap 100; }} stage nic : ps(1);
             queue gw : gateway {{ route {{ P.prefill (prompt); hold shared (1) {{ transfer on nic (1) from P.kv to shared (1); }} }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo; prefill (p) {{ hold kv (p) {{ run (p); }} cache (p) lease kv (inf); }} }}
             {}",
            wl("request gw; end;")
        ),
        "`kv` is a pool of queue `P`",
    );
    // 6. a `from` needs the lease on every way through the entry
    refused(
        &format!(
            "stage nic : delay;
             queue gw : gateway {{ route {{ P.prefill (prompt); D.decode (prompt) from P; }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo;
               prefill (p) {{ branch (p > 1) {{ hold kv (p) {{ run (p); }} cache (p) lease kv (inf); }} else {{ run (p); }} }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }}
               decode (p) from src {{ hold kv (p) {{ transfer on nic (p) from src to kv (p); }} }} }}
             {}",
            wl("request gw; end;")
        ),
        "`P.prefill` leases nothing",
    );
}

/// The fourth review of #87.
#[test]
fn the_contract_holds_at_four_more_edges() {
    let wl = "workload { arrive batch(1); hidden o; init { set prompt = 3; set o = 2; } session { request gw; end; } } run { horizon 10; }";
    let engine = |body: &str| {
        format!(
            "queue E[2] : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ {body} }} }}"
        )
    };
    // an entry's `set` is read from outside as `Q.x`
    compile_source(
        &format!(
            "queue gw : gateway {{ route {{ E[0].decode (prompt); observe r = E.result; }} }} {} {wl}",
            engine("hold kv (p) { prefill (p) growing kv; } set result = 7;")
        ),
        &Overrides::default(),
    )
    .unwrap();
    // `Q[i].x` is refused in a function's argument too
    refused(
        &format!(
            "queue gw : gateway {{ route {{ E[0].decode (prompt); observe r = min(E[missing].done, 0); }} }} {} {wl}",
            engine("hold kv (p) { prefill (p) growing kv; mark done; }")
        ),
        "write `E.done`",
    );
    // an entry does not set a `let` constant: its header reads the constant
    refused(
        &format!(
            "let n = 5; queue gw : gateway {{ route {{ E[0].decode (prompt); }} }} {} {wl}",
            engine("hold kv (n) { prefill (p) growing kv; } set n = 1;")
        ),
        "sets `n`, a `let` constant",
    );
    // a call takes `from` a queue, so the queue's lease check applies
    refused(
        &format!(
            "stage nic : delay;
             queue gw : gateway {{ route {{ P.prefill (prompt); D.decode (prompt) from P.kv; }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo; prefill (p) {{ hold kv (p) {{ run (p); }} cache (p) lease kv (inf); }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }}
               decode (p) from src {{ hold kv (p) {{ transfer on nic (p) from src to kv (p); }} }} }}
             {wl}"
        ),
        "a call takes from a queue",
    );
    // a queue's pool is one per member, not a family of its own
    refused(
        &format!(
            "queue gw : gateway {{ route {{ E[0].decode (prompt); }} }}
             queue E[3] : decode {{ pool kv[2] {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }} }} {wl}"
        ),
        "a queue's pool is the member's",
    );
}

/// `D pull P latency x share maxmin;`: the queues own their NICs, and a
/// `transfer` without `on` in `D`'s entry is the read over `P`'s NIC and
/// `D`'s at once, after `D`'s wait - the flat program with the stages and
/// the policy written out (#200).
#[test]
fn a_pull_relation_is_the_flat_read() {
    let queues = "
      let x0 = 0.5;
      queue gw : gateway { route {
        P[i].prefill (prompt);
        D[j].decode (prompt) from P[i];
      } }
      queue P[2] : prefill {
        pool kv { cap 1000; }
        serve fifo;
        nic ps(100);
        prefill (prompt) { hold kv (prompt) { run (prompt); } cache (prompt) lease kv (inf); }
      }
      queue D[2] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        nic ps(200);
        decode (prompt) { hold kv (prompt) { prefill (prompt) growing kv; } }
        decode (prompt) from src {
          hold kv (prompt) {
            transfer (prompt) from src to kv (prompt - 1);
            decode (o - 1) growing kv;
          }
        }
      }
      D pull P latency x0 share maxmin;
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { request gw; end; } }
      run { horizon 100; }";
    let flat = "
      let x0 = 0.5;
      pool kvP[2] { cap 1000; }
      stage P[2] : fifo;
      stage nicP[2] : ps(100);
      pool kvD[2] { cap 1000; block 16; }
      stage D[2] : step { cost 1; memory kvD; }
      stage nicD[2] : ps(200);
      stage wait[2] : delay;
      share maxmin;
      server {
        hold kvP[i] (prompt) { run P[i] (prompt); } cache (prompt) lease kvP[i] (inf);
        hold kvD[j] (prompt) {
          run wait[j] (x0);
          transfer on nicP[i], nicD[j] (prompt) from kvP[i] to kvD[j] (prompt - 1);
          decode on D[j] (o - 1) growing kvD[j];
        }
      }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { request; end; } }
      run { horizon 100; }";
    same_ir(
        queues,
        flat,
        &[
            ("P.kv", "kvP"),
            ("D.kv", "kvD"),
            ("P.nic", "nicP"),
            ("D.nic", "nicD"),
            ("D.nic.latency", "wait"),
        ],
    );
}

/// A pull relation names what it couples and how: both queues declared
/// above with a NIC, one source per reader, a policy, and the source the
/// entry is called from.
#[test]
fn a_pull_relation_says_what_it_couples() {
    let program = |p_nic: &str, rel: &str, call_from: &str| {
        format!(
            "queue gw : gateway {{ route {{ P.prefill (prompt); Q.prefill (prompt); D.decode (prompt) from {call_from}; }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo; {p_nic}
               prefill (p) {{ hold kv (p) {{ run (p); }} cache (p) lease kv (inf); }} }}
             queue Q : prefill {{ pool kv {{ cap 100; }} serve fifo; nic ps(1);
               prefill (p) {{ hold kv (p) {{ run (p); }} cache (p) lease kv (inf); }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }} nic ps(1);
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }}
               decode (p) from src {{ hold kv (p) {{ transfer (p) from src to kv (p); }} }} }}
             {rel}
             workload {{ arrive batch(1); init {{ set prompt = 3; }} session {{ request gw; end; }} }} run {{ horizon 10; }}"
        )
    };
    compile_source(
        &program("nic ps(1);", "D pull P share maxmin;", "P"),
        &Overrides::default(),
    )
    .unwrap();
    refused(
        &program("", "D pull P share maxmin;", "P"),
        "queue `P` has no `nic`",
    );
    refused(
        &program("nic ps(1);", "D pull P;", "P"),
        "names how concurrent reads divide the NICs",
    );
    refused(
        &program(
            "nic ps(1);",
            "D pull P share maxmin; share bottleneck;",
            "P",
        ),
        "`share` is given twice: the pull relation",
    );
    refused(&program("nic ps(1);", "", "P"), "pulls from no queue");
    // the read takes from the entry's source, not any pool (#207 review)
    refused(
        &program("nic ps(1);", "D pull P share maxmin;", "P").replace(
            "transfer (p) from src to kv (p)",
            "transfer (p) from kv to kv (p)",
        ),
        "a read takes from the entry's source, `from src`",
    );
    refused(
        &program("nic ps(1);", "D pull P share maxmin;", "Q"),
        "`D` pulls from `P`, and this entry was called `from Q.kv`",
    );
    refused(
        &program("nic ps(1);", "D pull P latency ~exp(1) share maxmin;", "P"),
        "`latency` is a number or a constant over `let`s",
    );
    refused(
        &format!("D pull P share maxmin; {}", program("nic ps(1);", "", "P")),
        "no queue `D` is declared above",
    );
}

/// What the review of the rebased #87 found an entry could still reach.
#[test]
fn an_entry_reaches_only_its_own() {
    let wl = "workload { arrive batch(1); hidden o, src; init { set prompt = 3; set o = 2; set src = 7; } session { request gw; end; } } run { horizon 10; }";
    // the `from` name is a number only in an index, whatever `hidden` says
    refused(
        &format!(
            "stage nic : delay;
             queue gw : gateway {{ route {{ P[0].prefill (prompt); D.decode (prompt) from P[0]; }} }}
             queue P[2] : prefill {{ pool kv {{ cap 100; }} serve fifo; prefill (p) {{ hold kv (p) {{ run (p); }} cache (p) lease kv (inf); }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }}
               decode (p) from src {{ observe x = src; hold kv (p) {{ transfer on nic (p) from src to kv (p); }} }} }}
             {wl}"
        ),
        "reads `src` as a number",
    );
    // another queue's mark is the gateway's to read
    refused(
        &format!(
            "queue gw : gateway {{ route {{ A.prefill (prompt); B.decode (prompt); }} }}
             queue A : prefill {{ serve fifo; prefill (p) {{ run (p); mark secret; }} }}
             queue B : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ observe leaked = A.secret; hold kv (p) {{ prefill (p) growing kv; }} }} }}
             {wl}"
        ),
        "reads `A.secret`, another queue's",
    );
    // a top-level pool is not the entry's to allocate
    refused(
        &format!(
            "pool shared {{ cap 100; }}
             queue gw : gateway {{ route {{ P.prefill (prompt); }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo; prefill (p) {{ hold shared (p) {{ run (p); }} }} }}
             {wl}"
        ),
        "holds `shared`, which is not a pool of `P`",
    );
    // a mark is the request's: an index would say a member it does not read
    refused(
        &format!(
            "queue gw : gateway {{ route {{ set t0 = now; D[0].decode (prompt); observe t = D[missing].first_token - t0; }} }}
             queue D[2] : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; mark first_token; }} }} }}
             {wl}"
        ),
        "write `Q.x`",
    );
    // every entry of the source leaves the lease a `from` takes
    refused(
        &format!(
            "stage nic : delay;
             queue gw : gateway {{ route {{ branch (prompt > 5) {{ P.prefill (prompt); }} else {{ P.decode (prompt); }} D.decode (prompt) from P; }} }}
             queue P : prefill, decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               prefill (p) {{ hold kv (p) {{ prefill (p) growing kv; }} cache (p) lease kv (inf); }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (p) {{ prefill (p) growing kv; }} }}
               decode (p) from src {{ hold kv (p) {{ transfer on nic (p) from src to kv (p); }} }} }}
             {wl}"
        ),
        "`P.decode` leases nothing",
    );
    // a queue's pools are above its stage, whose `memory` names them
    refused(
        &format!(
            "queue gw : gateway {{ route {{ E.prefill (prompt); }} }}
             queue E : prefill {{ serve step {{ cost 1; memory kv; }} pool kv {{ cap 10; }}
               prefill (p) {{ hold kv (1) {{ prefill (p) growing kv; }} }} }}
             {wl}"
        ),
        "declares its pools first",
    );
    // a family is bounded: every member is a pool or stage of its own
    refused(
        &format!(
            "queue gw : gateway {{ route {{ E[0].prefill (prompt); }} }}
             queue E[1e20] : prefill {{ serve fifo; prefill (p) {{ run (p); }} }}
             {wl}"
        ),
        "more than a family holds",
    );
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
