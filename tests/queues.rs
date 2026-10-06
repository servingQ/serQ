//! Queues are parse-time sugar (`docs/design/queue.md`): a program written
//! with queues compiles to the IR the same program written with pools,
//! stages and a server compiles to, up to the names the queue gives its
//! pools (`Q.p`), its stage (`Q`) and its entries' attributes (`Q.x`).

mod common;

use serq::{
    Overrides, compile_source,
    frontend::parser::{parse, parse_expr},
};

/// Compile both; the queue program's IR with `renames` applied to its JSON
/// is the flat program's IR.
fn same_ir(queues: &str, flat: &str, renames: &[(&str, &str)]) {
    let q = compile_source(&common::main_source(queues), &common::horizon(100.0))
        .unwrap_or_else(|e| panic!("queues: {e}\n{queues}"))
        .to_json();
    let f = compile_source(&common::main_source(flat), &common::horizon(100.0))
        .unwrap_or_else(|e| panic!("flat: {e}\n{flat}"))
        .to_json();
    let mut q = q;
    for (from, to) in renames {
        q = q.replace(&format!("\"{from}\""), &format!("\"{to}\""));
    }
    assert_eq!(q, f);
}

fn refused(src: &str, needle: &str) {
    let e = match parse(&common::main_source(src)) {
        Err(e) => e.to_string(),
        Ok(p) => match serq::frontend::link::link(&p, &common::horizon(100.0)) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("accepted:\n{src}"),
        },
    };
    assert!(e.contains(needle), "{src}\n  {e}");
}

const WORKLOAD: &str = "workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set j = 0; } session { turn; end; } } server { gw.route(); } ";

/// The engine's admission, allocation and service inside the queue; the
/// named request selects the gateway's `route`; the family's size is a constant.
#[test]
fn a_queue_is_its_pools_its_stage_and_the_server_statements() {
    same_ir(
        &format!(
            "use \"std/args\"; let N = args.number(\"N\", 2);
             queue gw : gateway {{ route {{ E[j].decode (prompt); observe done = now; }} }}
             queue E[N] : decode {{
               pool kv {{ cap 100; block 16; admit via E; }}
               serve step {{ cost 1; memory kv; }}
               decode (prompt) {{
                 hold kv (cost(kv, min(prompt, budget_left(E)))) reserve (cost(kv, prompt)) {{
                   run E prefill (cost(E, prompt)) growing kv; run E decode (cost(E, o - 1)) growing kv;
                 }} cache (cost(kv, prompt + o));
               }}
             }}
             {WORKLOAD}"
        ),
        &format!(
            "use \"std/args\"; let N = args.number(\"N\", 2);
             pool kv[2] {{ cap 100; block 16; admit via E; }}
             stage E[2] : step {{ cost 1; memory kv; }}
             server {{
               hold kv[j] (cost(kv, min(prompt, budget_left(E[j])))) reserve (cost(kv, prompt)) {{
                 run E[j] prefill (cost(E, prompt)) growing kv[j]; run E[j] decode (cost(E, o - 1)) growing kv[j];
               }} cache (cost(kv, prompt + o));
               observe done = now;
             }}
             {}",
            WORKLOAD.replace("server { gw.route(); }", "")
        ),
        &[("E.kv", "kv")],
    );
}

/// A family sized by an aggregate `let` is the family of its value: the
/// parser folds it as the linker writes it out (#274).
#[test]
fn a_queue_family_sized_by_an_aggregate() {
    let queue = |n: &str| {
        format!(
            "use \"std/args\"; let N = args.number(\"N\", {n});
             queue gw : gateway {{ route {{ E[j].decode (prompt); observe done = now; }} }}
             queue E[N] : decode {{
               pool kv {{ cap 100; block 16; admit via E; }}
               serve step {{ cost 1; memory kv; }}
               decode (prompt) {{ hold kv (cost(kv, prompt)) {{ run E prefill (cost(E, prompt)) growing kv; }} }}
             }}
             {WORKLOAD}"
        )
    };
    let ir = |src: &str| {
        serq::compile_source(&common::main_source(src), &common::horizon(100.0))
            .unwrap()
            .to_json()
    };
    assert_eq!(ir(&queue("max i in 2 (i + 1)")), ir(&queue("2")));
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
        prefill (prompt) { hold kv (cost(kv, prompt)) { run (cost(P, prompt)); } cache (cost(kv, prompt)) lease kv (inf); }
      }
      queue D[ND] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        decode (prompt) { hold kv (cost(kv, prompt)) { run D prefill (cost(D, prompt)) growing kv; mark first; } }
        decode (prompt) from src {
          set c = 0;
          hold kv (cost(kv, prompt)) {
            nic[self].transfer (prompt - c) from src to kv (prompt - 1);
            run D prefill (cost(D, 1)) growing kv; mark first;
            run D decode (cost(D, o - 1)) growing kv;
          }
        }
      }
      queue nic[ND] : link { serve ps(1); transfer (n) { run (cost(nic, n / Bw)); } }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set j = 0; } session { turn; end; } } server { gw.route(); }
      ";
    let flat = "
        let ND = 2; let Bw = 1000;
        pool kvP { cap 1000; }
        pool kvD[2] { cap 1000; block 16; }
        stage P : fifo;
        stage D[2] : step { cost 1; memory kvD; }
        stage nic[2] : ps(1);
        server {
          set t0 = now;
          hold kvP (cost(kvP, prompt)) { run P (cost(P, prompt)); } cache (cost(kvP, prompt)) lease kvP (inf);
          set c = 0;
          hold kvD[j] (cost(kvD, prompt)) {
            transfer on nic[j] ((prompt - c) / Bw) from kvP to kvD[j] (prompt - 1);
            run D[j] prefill (cost(D, 1)) growing kvD[j]; set first = now;
            run D[j] decode (cost(D, o - 1)) growing kvD[j];
          }
          observe ttft = first - t0;
        }
        workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set j = 0; } session { turn; end; } }
        ";
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
        prefill (prompt) { hold kv (cost(kv, prompt)) { run (cost(P, prompt)); } cache (cost(kv, prompt)) lease kv (inf); }
      }
      queue D[ND] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        decode (prompt) { hold kv (cost(kv, prompt)) { run D prefill (cost(D, prompt)) growing kv; } }
        decode (prompt) from src {
          hold kv (cost(kv, prompt)) {
            run setup (cost(setup, 1));
            transfer on egress[src], ingress[self] (prompt) from src to kv (prompt - 1);
            run D decode (cost(D, o - 1)) growing kv;
          }
        }
      }
      share maxmin;
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { turn; end; } } server { gw.route(); }
      ";
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
          hold kvP[i] (cost(kvP, prompt)) { run P[i] (cost(P, prompt)); } cache (cost(kvP, prompt)) lease kvP[i] (inf);
          hold kvD[j] (cost(kvD, prompt)) {
            run setup (cost(setup, 1));
            transfer on egress[i], ingress[j] (prompt) from kvP[i] to kvD[j] (prompt - 1);
            run D[j] decode (cost(D, o - 1)) growing kvD[j];
          }
        }
        workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { turn; end; } }
        ";
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
        prefill (prompt) { hold kv (cost(kv, prompt)) { run (cost(P, prompt)); } cache (cost(kv, prompt)) lease kv (inf); }
      }
      queue D[2] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        decode (prompt) { hold kv (cost(kv, prompt)) { run D prefill (cost(D, prompt)) growing kv; } }
        decode (prompt) from src {
          hold kv (cost(kv, prompt)) {
            transfer on egress[src], ingress[self] (prompt) from src to kv (prompt - 1);
            run D decode (cost(D, o - 1)) growing kv;
          }
        }
      }
      share maxmin;
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { turn; end; } } server { gw.route(); }
      ";
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
          hold kvP[i] (cost(kvP, prompt)) { run P[i] (cost(P, prompt)); } cache (cost(kvP, prompt)) lease kvP[i] (inf);
          hold kvD[j] (cost(kvD, prompt)) {
            run egressL[i] (cost(egressL, x1));
            run ingressL[j] (cost(ingressL, x0));
            transfer on egress[i], ingress[j] (prompt) from kvP[i] to kvD[j] (prompt - 1);
            run D[j] decode (cost(D, o - 1)) growing kvD[j];
          }
        }
        workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { turn; end; } }
        ";
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
            "use \"std/args\"; let x = args.number(\"x\", 0.5);
      queue gw : gateway {{ route {{ P.prefill (prompt); D.decode (32) from P; }} }}
      queue egress : link {{ serve ps(100); }}
      queue ingress : link {{ serve ps(200) latency {latency}; }}
      queue P : prefill {{
        pool kv {{ cap 1000; }}
        serve fifo;
        prefill (prompt) {{ hold kv (cost(kv, prompt)) {{ run (cost(P, prompt)); }} cache (cost(kv, prompt)) lease kv (inf); }}
      }}
      queue D : decode {{
        pool kv {{ cap 1000; }}
        serve step {{ cost 1; memory kv; }}
        decode (p) {{ hold kv (cost(kv, p)) {{ run D prefill (cost(D, p)) growing kv; }} }}
        decode ({param}) from src {{
          hold kv (cost(kv, {param})) {{ transfer on egress, ingress ({param}) from src to kv ({param} - 1); }}
        }}
      }}
      share maxmin;
      workload {{ arrive batch(1); init {{ set prompt = 32; }} session {{ turn; end; }} }} server {{ gw.route(); }}
      "
        )
    };
    let wait = |src: &str, ov: &Overrides| -> String {
        let p =
            compile_source(&common::main_source(src), ov).unwrap_or_else(|e| panic!("{e}\n{src}"));
        let ir = p.to_json();
        let at = ir.find("\"Delay\"").map(|_| ()).is_some();
        assert!(at, "a delay stage");
        ir
    };
    // the entry's parameter `x` is not the constant `x`
    let named = wait(&program("x", "x"), &common::horizon(100.0));
    let other = wait(&program("x", "y"), &common::horizon(100.0));
    assert_eq!(named, other.replace("\"y\"", "\"x\""));
    assert!(named.contains("0.5"), "waits 0.5, the constant");
    // --set reaches the latency
    let ov = Overrides {
        lets: vec![("x".into(), parse_expr("0.125").unwrap())],
        ..common::horizon(100.0)
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
            "def get() {{ {word} }}
        def f(x) {{ set {word} = 2; observe o = x; }}
        stage s : delay;
        workload {{ session {{ turn; end;
        }} }}
        server {{ set {word} = 1; f(get()); run s (cost(s, 1));
        }}
        "
        );
        let e = compile_source(&common::main_source(&src), &common::horizon(1.0)).unwrap_err();
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
            "queue gw : gateway {{ route {{ }} }} queue P : prefill {{ serve fifo latency 1; prefill (p) {{ run (cost(P, p)); }} }} {WORKLOAD}"
        ),
        "`latency` belongs to the `serve` of a queue that plays `link`",
    );
    refused(
        "stage s : ps(1) latency 1; workload { session { turn; end; \n} }\nserver { run s (cost(s, 1));\n} ",
        "`latency` belongs to the `serve` of a queue that plays `link`",
    );
    refused(
        &format!(
            "queue gw : gateway {{ route {{ }} }} queue nic : link {{ serve ps(1) latency 1; transfer (n) {{ run (cost(nic, n)); }} }} {WORKLOAD}"
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
      queue P : prefill { pool kv { cap 100; } serve fifo; prefill (p) { hold kv (cost(kv, p)) { run (cost(P, p)); } cache (cost(kv, p)) lease kv (inf); } }
      queue D : decode { pool kv { cap 100; } serve step { cost 1; memory kv; }
        decode (p) { hold kv (cost(kv, p)) { run D prefill (cost(D, p)) growing kv; } }
        decode (p) from src { hold kv (cost(kv, p)) { BODY } } }
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; } session { turn; end; } } server { gw.route(); }
      ";
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
           decode (prompt) { hold kv (cost(kv, x)) at admission (x = prompt + t0) { run E prefill (cost(E, 1)) growing kv; } } }
         workload { arrive batch(1); init { set prompt = 3; } session { turn; end; } } server { gw.route(); }
         ",
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
        &program(
            "hold kv (cost(kv, prompt + t0)) { run E prefill (cost(E, 1)) growing kv; }",
            "",
        ),
        "the header reads `t0`",
    );
    // the body: also the context and the request's hidden attributes, not
    // another attribute of the session
    refused(
        &program(
            "hold kv (cost(kv, prompt)) { run E prefill (cost(E, 1)) growing kv; observe w = now - t0; }",
            "",
        ),
        "reads `t0`, a session attribute set outside the queue",
    );
    assert!(
        parse(&common::main_source(&program(
            "hold kv (cost(kv, prompt)) { run E prefill (cost(E, 1)) growing kv; run E decode (cost(E, o - 1)) growing kv; }",
            ""
        )))
        .is_ok(),
        "`o` is hidden, so the body may read it"
    );
    // another queue's pool
    refused(
        &program(
            "hold kv (cost(kv, prompt)) { run E prefill (cost(E, 1)) growing kv; observe q = holders(gw.kv); }",
            "",
        ),
        "not a pool or stage of `E`",
    );
    // its own pool is nobody else's
    refused(
        &program(
            "hold kv (cost(kv, prompt)) { run E prefill (cost(E, 1)) growing kv; }",
            "grow E.kv (cost(E.kv, 1));",
        ),
        "only `E`'s entries hold it",
    );
    refused(
        &program(
            "hold kv (cost(kv, prompt)) { run E prefill (cost(E, 1)) growing kv; }",
            "observe x = E.late;",
        ),
        "no entry of `E` marks or sets `late`",
    );
}

#[test]
fn roles_and_entries_agree() {
    let rest = "workload { arrive batch(1); init { set prompt = 1; } session { turn; end; } } server { gw.route(); } ";
    refused(
        &format!("queue gw : gateway {{ route {{ }} }} queue P : prefill {{ serve fifo; }} {rest}"),
        "plays `prefill` and has no `prefill` entry",
    );
    refused(
        &format!(
            "queue gw : gateway {{ route {{ }} }} queue P {{ serve fifo; prefill (p) {{ run (cost(P, p)); }} }} {rest}"
        ),
        "declare `queue P : prefill`",
    );
    refused(
        &format!(
            "queue gw : gateway {{ route {{ }} }} queue P : prefill {{ serve fifo; prefill (p) {{ run (cost(P, p)); }} warm (p) {{ run (cost(P, p)); }} }} {rest}"
        ),
        "not an entry of any role",
    );
    refused(
        &format!("queue gw : gateway {{ route {{ }} }} server {{ }} {rest}"),
        "duplicate server",
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
        "queue prefill : link { serve fifo; transfer (n) { run (cost(prefill, n)); } } stage s : fifo; workload { session { turn; end; \n} }\nserver { run s (cost(s, 1));\n}",
        "serving word",
    );
}

#[test]
fn a_call_is_checked_against_the_entry() {
    let decls = "
      queue P : prefill { pool kv { cap 10; } serve fifo; prefill (p) { hold kv (cost(kv, p)) { run (cost(P, p)); } cache (cost(kv, p)); } }
      queue D[2] : decode { pool kv { cap 10; } serve step { cost 1; memory kv; }
        decode (p) { hold kv (cost(kv, p)) { run D prefill (cost(D, p)) growing kv; } }
        decode (p) from src { hold kv (cost(kv, p)) { run D prefill (cost(D, p)) growing kv; } } }";
    let program = |route: &str| {
        format!(
            "queue gw : gateway {{ route {{ {route} }} }} {decls}
             workload {{ arrive batch(1); init {{ set prompt = 1; set j = 0; }} session {{ turn; end; }} }} server {{ gw.route(); }} "
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
        "stage s : fifo; workload { session { turn; end; \n} }\nserver { observe s = self;\n} ",
        "`self` is a queue entry's word",
    );
    refused(
        "queue gw : gateway { route { observe s = self; } } stage s : fifo;
         workload { arrive batch(1); session { turn; end; } } server { gw.route(); } ",
        "not a family",
    );
}

#[test]
fn a_family_size_is_a_constant() {
    let rest = "queue gw : gateway { route { E[j].decode (1); } } workload { arrive batch(1); init { set j = 0; } session { turn; end; } } server { gw.route(); } ";
    let decl = |n: &str| {
        format!(
            "use \"std/args\"; let N = args.number(\"N\", 2); queue E[{n}] : decode {{ pool kv {{ cap 10; }} serve step {{ cost 1; memory kv; }} decode (p) {{ hold kv (cost(kv, p)) {{ run E prefill (cost(E, p)) growing kv; }} }} }} {rest}"
        )
    };
    assert!(parse(&common::main_source(&decl("N"))).is_ok());
    assert!(parse(&common::main_source(&decl("N + 1"))).is_ok());
    // a family of one is still called by index: a program reads the same at N = 1
    let one = parse(&common::main_source(&decl("N - 1"))).unwrap();
    assert!(serq::frontend::link::link(&one, &common::horizon(1.0)).is_ok());
    // and only by index: one call, one spelling, whatever N is
    refused(
        &decl("N - 1").replace("E[j].decode", "E.decode"),
        "is a family of 1; index it",
    );
    refused(&decl("N / 4"), "array size must be a positive integer");
    refused(&decl("j"), "positive integer or a `let` constant");
}

#[test]
fn server_routes_turns_to_gateways_before_their_declarations() {
    let queues = "
      workload { arrive batch(1); }
      server { branch (1) { second.route(); } else { first.route(); } }
      queue unused : gateway { route { observe unused = 99; } }
      queue first : gateway { route { observe selected = 1; } }
      queue second : gateway { route { observe selected = 2; } }
      ";
    let flat = "workload { arrive batch(1); }
      server { branch (1) { observe selected = 2; } else { observe selected = 1; } }
      ";
    same_ir(queues, flat, &[]);
    same_ir(&queues.replace("second", "router"), flat, &[]);
}

#[test]
fn gateway_routing_belongs_to_the_server() {
    refused(
        "queue gw : gateway { route { } } workload { session { turn; } }",
        "written against a `server` block",
    );
    refused(
        "queue gw : gateway { route { } } workload { session { gw.route(); } } server {}",
        "queue entries belong in `server`",
    );
    refused("workload {} server { missing.route(); }", "no queue");
    refused(
        "queue gw : gateway { route { turn; } }",
        "`turn` is the session's",
    );
    refused(
        "queue gw : gateway { route {} } workload { session { request gw; } }",
        "`request` is replaced by `turn;`",
    );
}

#[test]
fn call_indices_obey_entry_read_boundaries() {
    let program = |call: &str| {
        format!(
            "queue gw : gateway {{ route {{ E.prefill (1); }} }}
         queue E : prefill {{ serve fifo; prefill (p) {{ {call} }} }}
         queue F[2] : prefill {{ serve fifo; prefill (p) {{ }} }}
         workload {{ arrive batch(1); init {{ set j = 0; }} session {{ turn; end; }} }} server {{ gw.route(); }}
         "
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
            "run F[j] (cost(F, 1));",
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
            "run (cost(F, p));".to_string()
        };
        decls.push_str(&format!(
            "queue Q{k} : prefill {{ serve fifo; prefill (p) {{ {body} }} }}\n"
        ));
    }
    refused(
        &format!(
            "queue gw : gateway {{ route {{ Q0.prefill (prompt); }} }} {decls}
             workload {{ arrive batch(1); init {{ set prompt = 1; }} session {{ turn; end; }} }} server {{ gw.route(); }} "
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
               decode (p) {{ hold kv (cost(kv, p)) {{ run E prefill (cost(E, p)) growing kv; run E decode (cost(E, out)) growing kv; }} }} }}
             workload {{ arrive batch(1); {hidden} init {{ set prompt = 3; set out = 2; }} session {{ turn; end; }} }} server {{ gw.route(); }}
             "
        )
    };
    refused(
        &program(""),
        "reads `out`, a session attribute set outside the queue",
    );
    compile_source(
        &common::main_source(&program("hidden out;")),
        &common::horizon(10.0),
    )
    .unwrap();
}

/// A turn in a definition changes only the actual server's attributes;
/// an unused gateway cannot capture a caller's argument.
#[test]
fn a_def_captures_what_its_turns_server_assigns() {
    let program = |target: &str, defs: &str| {
        format!(
            "
      {defs}
      queue clean : gateway {{ route {{ observe done = now; }} }}
      queue dirty : gateway {{ route {{ set x = now; }} }}
      workload {{ arrive batch(1); init {{ set x = 1; }} session {{ go(x); }} }}
      server {{ {target}.route(); }}
      "
        )
    };
    for defs in [
        "def go(x) { turn; observe b = x; }",
        "def next() { turn; } def go(x) { next(); observe b = x; }",
    ] {
        compile_source(
            &common::main_source(&program("clean", defs)),
            &common::horizon(10.0),
        )
        .unwrap();
        refused(&program("dirty", defs), "which its `turn;` assigns");
    }
}

/// The third review of #87: six ways a program still got past the queue's
/// contract, each refused or, for a constant, accepted as the linker does.
#[test]
fn the_contract_holds_at_every_edge() {
    let wl = |session: &str| {
        format!(
            "workload {{ arrive batch(1); hidden o; init {{ set prompt = 3; set o = 2; }} session {{ {session} }} }} server {{ gw.route(); }} "
        )
    };
    let engine = "queue E : decode { pool kv { cap 100; } serve step { cost 1; memory kv; }
                    decode (p) { hold kv (cost(kv, p)) { run E prefill (cost(E, p)) growing kv; } } }";
    // 1. what a named gateway assigns is what a request assigns: an argument
    //    that reads it would read the new value
    refused(
        &format!(
            "def go(x) {{ turn; observe b = x; }}
             queue gw : gateway {{ route {{ set t0 = now; E.decode (prompt); }} }} {engine}
             {}",
            wl("set t0 = 1; go(t0); end;")
        ),
        "an argument of `go` reads `t0`",
    );
    // 2. a family's size folds as the linker folds a constant
    compile_source(
        &common::main_source(&format!(
            "use \"std/args\"; let N = args.number(\"N\", min(2, 3));
             queue gw : gateway {{ route {{ E[N - 1].decode (prompt); }} }}
             queue E[N] : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (cost(kv, p + N)) {{ run E prefill (cost(E, p)) growing kv; }} }} }}
             {}",
            wl("turn; end;")
        )),
        &common::horizon(10.0),
    )
    .unwrap();
    // 3. a role's entry has the role's parameters
    refused(
        &format!(
            "queue gw : gateway {{ route {{ P.prefill (1, 2); }} }}
             queue P : prefill {{ serve fifo; prefill (a, b) {{ run (cost(P, a + b)); }} }}
             {}",
            wl("turn; end;")
        ),
        "takes 1 parameter(s), as the role says",
    );
    // 4. a queue that is also a gateway: only its `route` is the deployment's
    refused(
        &format!(
            "queue gw : gateway, prefill {{ serve fifo; route {{ gw.prefill (prompt); }} prefill (p) {{ run (cost(gw, p + o + prompt)); }} }}
             {}",
            wl("turn; end;")
        ),
        "reads `prompt`, a session attribute set outside the queue",
    );
    // 5. the serving forms do not take another queue's pool either
    refused(
        &format!(
            "pool shared {{ cap 100; }} stage nic : ps(1);
             queue gw : gateway {{ route {{ P.prefill (prompt); hold shared (cost(shared, 1)) {{ transfer on nic (1) from P.kv to shared (1); }} }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo; prefill (p) {{ hold kv (cost(kv, p)) {{ run (cost(P, p)); }} cache (cost(kv, p)) lease kv (inf); }} }}
             {}",
            wl("turn; end;")
        ),
        "`kv` is a pool of queue `P`",
    );
    // 6. a `from` needs the lease on every way through the entry
    refused(
        &format!(
            "stage nic : delay;
             queue gw : gateway {{ route {{ P.prefill (prompt); D.decode (prompt) from P; }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo;
               prefill (p) {{ branch (p > 1) {{ hold kv (cost(kv, p)) {{ run (cost(P, p)); }} cache (cost(kv, p)) lease kv (inf); }} else {{ run (cost(P, p)); }} }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (cost(kv, p)) {{ run D prefill (cost(D, p)) growing kv; }} }}
               decode (p) from src {{ hold kv (cost(kv, p)) {{ transfer on nic (p) from src to kv (p); }} }} }}
             {}",
            wl("turn; end;")
        ),
        "`P.prefill` leases nothing",
    );
}

/// The fourth review of #87.
#[test]
fn the_contract_holds_at_four_more_edges() {
    let wl = "workload { arrive batch(1); hidden o; init { set prompt = 3; set o = 2; } session { turn; end; } } server { gw.route(); } ";
    let engine = |body: &str| {
        format!(
            "queue E[2] : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ {body} }} }}"
        )
    };
    // an entry's `set` is read from outside as `Q.x`
    compile_source(&common::main_source(
        &format!(
            "queue gw : gateway {{ route {{ E[0].decode (prompt); observe r = E.result; }} }} {} {wl}",
            engine("hold kv (cost(kv, p)) { run E prefill (cost(E, p)) growing kv; } set result = 7;")
        )),
        &common::horizon(10.0),
    )
    .unwrap();
    // `Q[i].x` is refused in a function's argument too
    refused(
        &format!(
            "queue gw : gateway {{ route {{ E[0].decode (prompt); observe r = min(E[missing].done, 0); }} }} {} {wl}",
            engine("hold kv (cost(kv, p)) { run E prefill (cost(E, p)) growing kv; mark done; }")
        ),
        "write `E.done`",
    );
    // an entry does not set a `let` constant: its header reads the constant
    refused(
        &format!(
            "let n = 5; queue gw : gateway {{ route {{ E[0].decode (prompt); }} }} {} {wl}",
            engine("hold kv (cost(kv, n)) { run E prefill (cost(E, p)) growing kv; } set n = 1;")
        ),
        "sets `n`, a `let` constant",
    );
    // a call takes `from` a queue, so the queue's lease check applies
    refused(
        &format!(
            "stage nic : delay;
             queue gw : gateway {{ route {{ P.prefill (prompt); D.decode (prompt) from P.kv; }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo; prefill (p) {{ hold kv (cost(kv, p)) {{ run (cost(P, p)); }} cache (cost(kv, p)) lease kv (inf); }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (cost(kv, p)) {{ run D prefill (cost(D, p)) growing kv; }} }}
               decode (p) from src {{ hold kv (cost(kv, p)) {{ transfer on nic (p) from src to kv (p); }} }} }}
             {wl}"
        ),
        "a call takes from a queue",
    );
    // a queue's pool is one per member, not a family of its own
    refused(
        &format!(
            "queue gw : gateway {{ route {{ E[0].decode (prompt); }} }}
             queue E[3] : decode {{ pool kv[2] {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (cost(kv, p)) {{ run E prefill (cost(E, p)) growing kv; }} }} }} {wl}"
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
        prefill (prompt) { hold kv (cost(kv, prompt)) { run (cost(P, prompt)); } cache (cost(kv, prompt)) lease kv (inf); }
      }
      queue D[2] : decode {
        pool kv { cap 1000; block 16; }
        serve step { cost 1; memory kv; }
        nic ps(200);
        decode (prompt) { hold kv (cost(kv, prompt)) { run D prefill (cost(D, prompt)) growing kv; } }
        decode (prompt) from src {
          hold kv (cost(kv, prompt)) {
            transfer (prompt) from src to kv (prompt - 1);
            run D decode (cost(D, o - 1)) growing kv;
          }
        }
      }
      D pull P latency x0 share maxmin;
      workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { turn; end; } } server { gw.route(); }
      ";
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
          hold kvP[i] (cost(kvP, prompt)) { run P[i] (cost(P, prompt)); } cache (cost(kvP, prompt)) lease kvP[i] (inf);
          hold kvD[j] (cost(kvD, prompt)) {
            run wait[j] (cost(wait, x0));
            transfer on nicP[i], nicD[j] (prompt) from kvP[i] to kvD[j] (prompt - 1);
            run D[j] decode (cost(D, o - 1)) growing kvD[j];
          }
        }
        workload { arrive batch(1); hidden o; init { set prompt = 32; set o = 4; set i = 1; set j = 0; } session { turn; end; } }
        ";
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
               prefill (p) {{ hold kv (cost(kv, p)) {{ run (cost(P, p)); }} cache (cost(kv, p)) lease kv (inf); }} }}
             queue Q : prefill {{ pool kv {{ cap 100; }} serve fifo; nic ps(1);
               prefill (p) {{ hold kv (cost(kv, p)) {{ run (cost(Q, p)); }} cache (cost(kv, p)) lease kv (inf); }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }} nic ps(1);
               decode (p) {{ hold kv (cost(kv, p)) {{ run D prefill (cost(D, p)) growing kv; }} }}
               decode (p) from src {{ hold kv (cost(kv, p)) {{ transfer (p) from src to kv (p); }} }} }}
             {rel}
             workload {{ arrive batch(1); init {{ set prompt = 3; }} session {{ turn; end; }} }} server {{ gw.route(); }} "
        )
    };
    compile_source(
        &common::main_source(&program("nic ps(1);", "D pull P share maxmin;", "P")),
        &common::horizon(10.0),
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
    let wl = "workload { arrive batch(1); hidden o, src; init { set prompt = 3; set o = 2; set src = 7; } session { turn; end; } } server { gw.route(); } ";
    // the `from` name is a number only in an index, whatever `hidden` says
    refused(
        &format!(
            "stage nic : delay;
             queue gw : gateway {{ route {{ P[0].prefill (prompt); D.decode (prompt) from P[0]; }} }}
             queue P[2] : prefill {{ pool kv {{ cap 100; }} serve fifo; prefill (p) {{ hold kv (cost(kv, p)) {{ run (cost(P, p)); }} cache (cost(kv, p)) lease kv (inf); }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (cost(kv, p)) {{ run D prefill (cost(D, p)) growing kv; }} }}
               decode (p) from src {{ observe x = src; hold kv (cost(kv, p)) {{ transfer on nic (p) from src to kv (p); }} }} }}
             {wl}"
        ),
        "reads `src` as a number",
    );
    // another queue's mark is the gateway's to read
    refused(
        &format!(
            "queue gw : gateway {{ route {{ A.prefill (prompt); B.decode (prompt); }} }}
             queue A : prefill {{ serve fifo; prefill (p) {{ run (cost(A, p)); mark secret; }} }}
             queue B : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ observe leaked = A.secret; hold kv (cost(kv, p)) {{ run B prefill (cost(B, p)) growing kv; }} }} }}
             {wl}"
        ),
        "reads `A.secret`, another queue's",
    );
    // a top-level pool is not the entry's to allocate
    refused(
        &format!(
            "pool shared {{ cap 100; }}
             queue gw : gateway {{ route {{ P.prefill (prompt); }} }}
             queue P : prefill {{ pool kv {{ cap 100; }} serve fifo; prefill (p) {{ hold shared (cost(shared, p)) {{ run (cost(P, p)); }} }} }}
             {wl}"
        ),
        "holds `shared`, which is not a pool of `P`",
    );
    // a mark is the request's: an index would say a member it does not read
    refused(
        &format!(
            "queue gw : gateway {{ route {{ set t0 = now; D[0].decode (prompt); observe t = D[missing].first_token - t0; }} }}
             queue D[2] : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (cost(kv, p)) {{ run D prefill (cost(D, p)) growing kv; mark first_token; }} }} }}
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
               prefill (p) {{ hold kv (cost(kv, p)) {{ run P prefill (cost(P, p)) growing kv; }} cache (cost(kv, p)) lease kv (inf); }}
               decode (p) {{ hold kv (cost(kv, p)) {{ run P prefill (cost(P, p)) growing kv; }} }} }}
             queue D : decode {{ pool kv {{ cap 100; }} serve step {{ cost 1; memory kv; }}
               decode (p) {{ hold kv (cost(kv, p)) {{ run D prefill (cost(D, p)) growing kv; }} }}
               decode (p) from src {{ hold kv (cost(kv, p)) {{ transfer on nic (p) from src to kv (p); }} }} }}
             {wl}"
        ),
        "`P.decode` leases nothing",
    );
    // a queue's pools are above its stage, whose `memory` names them
    refused(
        &format!(
            "queue gw : gateway {{ route {{ E.prefill (prompt); }} }}
             queue E : prefill {{ serve step {{ cost 1; memory kv; }} pool kv {{ cap 10; }}
               prefill (p) {{ hold kv (cost(kv, 1)) {{ run E prefill (cost(E, p)) growing kv; }} }} }}
             {wl}"
        ),
        "declares its pools first",
    );
    // a family is bounded: every member is a pool or stage of its own
    refused(
        &format!(
            "queue gw : gateway {{ route {{ E[0].prefill (prompt); }} }}
             queue E[1e20] : prefill {{ serve fifo; prefill (p) {{ run (cost(E, p)); }} }}
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
           session { turn; end; } } server { gw.route(); }
         ",
        "stage D : fifo;
        workload { arrive batch(1); hidden x; init { set x = 3; }
          session { turn; end;
          }
        }
        server { observe seen = x;
        } ",
        &[],
    );
}

#[test]
fn array_sizes_reject_direct_and_indirect_overrides() {
    let src = "use \"std/args\"; let N = args.number(\"N\", 2); let M = N + 1;
      queue D[M] : decode { serve fifo; decode (p) { } }
      queue gw : gateway { route { D[2].decode (1); } }
      workload { arrive batch(1); session { turn; end; } } server { gw.route(); }
      ";
    // N directly sizes D and M indirectly sizes it; both reads are checked.
    for src in [src.to_string(), src.replace("D[M]", "D[N]")] {
        let name = "N";
        let ov = Overrides {
            lets: vec![(name.into(), parse_expr("4").unwrap())],
            ..common::horizon(1.0)
        };
        let err = compile_source(&common::main_source(&src), &ov).unwrap_err();
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
    src.push_str("workload { arrive batch(1); session { turn; end; } } server { gw.route(); } ");
    compile_source(&common::main_source(&src), &common::horizon(1.0)).unwrap();
    let cycle = src.replace("observe done = p;", "Q0.prefill (p);");
    refused(&cycle, "entries call each other in a cycle");
}

#[test]
fn expansion_errors_point_to_the_call() {
    let src = "queue gw : gateway { route {\n  D.decode ();\n} }
queue D : decode { serve fifo; decode (p) { } }
workload { arrive batch(1); session { turn; end; } } server { gw.route(); }
";
    let err = parse(&common::main_source(src)).unwrap_err();
    assert_eq!((err.line, err.col), (2, 3));
    assert!(err.msg.contains("takes 1 argument(s), got 0"), "{err}");
}
