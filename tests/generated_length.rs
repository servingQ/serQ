//! A length the model generates says in the server's signature whether the
//! scheduler knows it: `hidden` or `visible`, never bare (#61,
//! `docs/design/generated-length.md`). A bare parameter is the scheduler's
//! to read, which for the output of `E (n, o);` or the work of a decode run
//! is a scheduler that knows how long each request runs; forgetting
//! `hidden` made one without a word.

mod common;

fn read(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).unwrap()
}

fn replaced(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut t = text.to_string();
    for (a, b) in pairs {
        assert_eq!(t.matches(a).count(), 1, "{a}");
        t = t.replace(a, b);
    }
    t
}

fn compile(src: &str, workload: &str) -> Result<serq::Program, String> {
    common::compile(&common::main_source(src), workload, &common::horizon(100.0))
}

fn refused(src: &str, workload: &str, fragments: &[&str]) {
    let e = compile(src, workload).expect_err("refused");
    for f in fragments {
        assert!(e.contains(f), "missing {f:?}:\n{e}\n{src}");
    }
}

/// `examples/multi-turn/vllm.sq`, the issue's program, with `pairs`
/// replaced, under its workload.
fn vllm(pairs: &[(&str, &str)]) -> Result<serq::Program, String> {
    let src = replaced(&read("examples/multi-turn/vllm.sq"), pairs);
    common::compile(
        &src,
        &read("examples/workloads/chat.sq"),
        &common::horizon(100.0),
    )
}

const BARE: (&str, &str) = ("server (input, hidden output)", "server (input, output)");
const VISIBLE: (&str, &str) = (
    "server (input, hidden output)",
    "server (input, visible output)",
);
/// The admission reserves the whole sequence, the prompt and the length
/// the model will generate, as `examples/vendors/ascend.sq`'s does.
const RESERVE_ALL: (&str, &str) = (
    "reserve (request.known)",
    "reserve (request.input + request.output)",
);

/// The bug as found: without `hidden`, `vllm.sq` linked, and with an
/// admission that reserves the output it linked as a scheduler that knows
/// each request's length, which vLLM's does not (`sched/utils.py:98-119`).
#[test]
fn forgetting_hidden_on_the_length_is_refused() {
    for pairs in [&[BARE][..], &[BARE, RESERVE_ALL][..]] {
        let e = vllm(pairs).expect_err("refused");
        for f in [
            "58:18",
            "`output` is a length the model generates (`vllm (…)`'s output at 59:5 reads it)",
            "help: `hidden output`",
            "`visible output`",
        ] {
            assert!(e.contains(f), "missing {f:?}:\n{e}");
        }
    }
    // the program as shipped
    let p = vllm(&[]).unwrap();
    assert_eq!(p.hidden.len(), 1);
}

/// `hidden` still means what it meant: the admission may not read it.
#[test]
fn hidden_still_refuses_an_admission_that_reads_the_length() {
    let e = vllm(&[RESERVE_ALL]).expect_err("refused");
    assert!(
        e.contains("`output` is hidden from the scheduler, but the server's admission reads it"),
        "{e}"
    );
}

/// A scheduler that knows the length says so, and is the program a bare
/// parameter was: nothing is hidden.
#[test]
fn visible_lets_the_admission_reserve_the_length() {
    let p = vllm(&[VISIBLE, RESERVE_ALL]).unwrap();
    assert!(p.hidden.is_empty());
    let shipped = |pairs: &[(&str, &str)]| {
        let mut p = vllm(pairs).unwrap();
        p.hidden.clear();
        p.to_json()
    };
    // `visible` is the bare parameter's IR: only `hidden` is in it
    assert_eq!(shipped(&[VISIBLE]), shipped(&[]));
}

/// An engine that admits by `taking kv (request.input)`, its server
/// `server`.
fn program(server: &str) -> String {
    format!(
        "device gpu {{ kv cap 1000; }}
        engine E on gpu {{ tokens cap inf;
          scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.input); }}
          worker {{ forward (1); }} }}
        pool kv on gpu {{ block 16; }}
        {server}
        "
    )
}

/// One request of a 32-token prompt that generates 4.
const WORKLOAD: &str = "workload { arrive batch(1);
  init { set input = 32; set output = 4; }
  send (input, output);
  session { turn; end; } }";

/// What the length reads through the server's `set`s is the parameter's,
/// in any order, and through a queue's entry, whose parameters and locals
/// the parser put in place.
#[test]
fn the_length_through_a_set_is_its_parameter_s() {
    refused(
        &program("server (input, output) { set x = output; E (input, x); }"),
        WORKLOAD,
        &["`output` is a length the model generates (`E (…)`'s output at 6:50 reads it)"],
    );
    // a set in a loop reaches the length on a later pass
    refused(
        &program(
            "server (input, output) {
               set x = 1; set y = 1;
               loop { E (input, x); set x = y; set y = output; if (x > 2) { end; } }
             }",
        ),
        WORKLOAD,
        &["`output` is a length"],
    );
    // an entry takes one parameter; the parser puts the argument and the
    // entry's local in place
    let entry = |sig: &str| {
        format!(
            "queue Q : agg {{
               device gpu {{ kv cap 1000; }}
               engine on gpu {{ tokens cap inf;
                 scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.input); }}
                 worker {{ forward (1); }} }}
               pool kv on gpu {{ block 16; }}
               decode (m) {{ set produced = min(m, 4096 - 32); Q (32, produced); }}
             }}
             server ({sig}) {{ Q.decode (output); }}"
        )
    };
    let only = "workload { arrive batch(1); init { set output = 4; } send (output); session { turn; end; } }";
    // the length's call is in the entry, where the message points
    refused(
        &entry("output"),
        only,
        &["`output` is a length the model generates (`Q (…)`'s output at 7:63 reads it)"],
    );
    compile(&entry("hidden output"), only).unwrap();
    compile(&entry("visible output"), only).unwrap();
}

/// What the request's prompt reads is not asked for: a prompt is no
/// generated length, and the engine admits by it, so `hidden` there could
/// never link (`hidden_server.rs`, `a_request_s_input_may_not_be_hidden`).
#[test]
fn a_parameter_the_prompt_reads_needs_no_word() {
    // the length bounded by the context left, as llm-d's `produced`
    compile(
        &program("server (input, hidden output) { E (input, min(output, 4096 - input)); }"),
        WORKLOAD,
    )
    .unwrap();
    let one = "workload { arrive batch(1); init { set n = 32; } send (n); session { turn; end; } }";
    compile(&program("server (n) { E (n, n); }"), one).unwrap();
    compile(&program("server (n) { E (n, 1); }"), one).unwrap();
    // the prompt through what was set before the call, on either arm
    compile(
        &program(
            "server (input, hidden output) {
               set ctx = ctx + input;
               if (ctx > 64) { set ctx = input; }
               E (ctx, min(output, 4096 - ctx));
             }",
        ),
        WORKLOAD,
    )
    .unwrap();
}

/// The prompt is what the input holds where the call is: a `set` after it
/// is the next turn's context, and does not make the length a prompt. As
/// found, the server's `set`s counted in any order, so the context the turn
/// stores after its request, `ctx + output`, excused `output`, and an
/// admission that reserved it linked with no word, the bug again.
#[test]
fn a_set_after_the_call_is_no_part_of_its_prompt() {
    let server = |sig: &str| {
        program(&format!(
            "server (input, {sig}) {{
               set ctx = ctx + input;
               E (ctx, output);
               set ctx = ctx + output;
             }}"
        ))
    };
    refused(
        &server("output"),
        WORKLOAD,
        &["`output` is a length the model generates (`E (…)`'s output at"],
    );
    // and a choice it is: `hidden` links, as `visible` does
    compile(&server("hidden output"), WORKLOAD).unwrap();
    compile(&server("visible output"), WORKLOAD).unwrap();
    // the issue's program in this shape, its admission reserving the length
    let ctx = (
        "server (input, hidden output) {\n    vllm (input, output);",
        "server (input, output) {\n    set ctx = ctx + input;\n    vllm (ctx, output);\n    set ctx = ctx + output;",
    );
    let e = vllm(&[ctx, RESERVE_ALL]).expect_err("refused");
    assert!(
        e.contains("`output` is a length the model generates"),
        "{e}"
    );
}

/// `visible` is said only of a generated length: elsewhere it is the bare
/// parameter's second spelling.
#[test]
fn visible_on_any_other_parameter_is_refused() {
    refused(
        &program("server (visible input, hidden output) { E (input, output); }"),
        WORKLOAD,
        &[
            "`visible input`: no length the model generates reads `input`",
            "help: `input`",
        ],
    );
    let two = "workload { arrive batch(1); init { set input = 32; set t0 = 0; } send (input, t0);
      session { turn; end; } }";
    refused(
        &program("server (input, visible t0) { E (input, 4); observe at = now - t0; }"),
        two,
        &["`visible t0`"],
    );
}

#[test]
fn hidden_and_visible_contradict() {
    for sig in [
        "input, hidden visible output",
        "input, visible hidden output",
    ] {
        refused(
            &program(&format!("server ({sig}) {{ E (input, output); }}")),
            WORKLOAD,
            &["`output` is `hidden` or `visible`, not both"],
        );
    }
}

/// A queue's entry in the kernel's words, `engine_request.rs`'s `KERNEL`:
/// a prefill and a decode run on the queue's engine, the entry's parameter
/// `prompt` and the length `o` the request's hidden attribute, which an
/// entry reads as it is. The decode run's work is the length the model
/// generates less what an earlier attempt produced, which reads the prompt.
fn kernel(sig: &str, body: &str) -> String {
    format!(
        "let bs = 4;
        queue llm : prefill {{
          device gpu {{ kv cap 64; }}
          engine on gpu {{
            reqs cap 2;
            tokens cap 8;
            scheduler {{ advance running; admit waiting while (running.preempted == 0); }}
            worker {{ forward (1); }}
          }}
          pool kv on gpu {{ block bs; evict lru; preempt lifo; }}
          pool reqs on llm {{ queue fifo; }}
          prefill (prompt) {{
            set kept = prompt;
            hold reqs (cost(reqs, 1)),
                 kv (cost(kv, min(max(kept, computed + 1), budget_left(llm))))
                 reserve (cost(kv, max(kept, computed + 1))) {{
              set known = max(kept, computed + 1);
              set kept = known;
              {body}
            }} cache (cost(reqs, kv, prompt + 3));
          }}
        }}
        server ({sig}) {{ llm.prefill (n); }}"
    )
}

const PREFILL_DECODE: &str = "run llm prefill (cost(llm, known - cached)) growing kv;
              run llm decode (cost(llm, o - 1 - (known - prompt))) growing kv;";

const KERNEL_WORKLOAD: &str =
    "workload { arrive batch(3); turn { set n = 6; set o = 3; } send (n, o); }";

/// A decode run's work is a length the model generates as an engine's
/// request's output is, and it has no input: what it reads asks for a word,
/// the prompt's included. As first written, what a prefill run on the
/// engine read counted as the prompt, but a prefill's work may read a
/// hidden value, so a parameter both runs read passed bare though `hidden`
/// would have linked.
#[test]
fn a_decode_run_s_work_is_a_generated_length() {
    let kernel_o = |sig: &str| kernel(&format!("{sig}, hidden o"), PREFILL_DECODE);
    // `n` the prompt, which the decode subtracts and the header reads
    refused(
        &kernel_o("n"),
        KERNEL_WORKLOAD,
        &["`n` is a length the model generates (`run llm decode (…)`'s work at"],
    );
    compile(&kernel_o("visible n"), KERNEL_WORKLOAD).unwrap();
    // the prefill and the decode both read it, and the header does not:
    // `hidden` links, so bare is no word
    let both = |sig: &str| {
        kernel(
            sig,
            "run llm prefill (cost(llm, prompt)) growing kv;
              run llm decode (cost(llm, prompt)) growing kv;",
        )
        .replace("set kept = prompt;", "set kept = 6;")
    };
    refused(&both("n, hidden o"), KERNEL_WORKLOAD, &["`n` is a length"]);
    compile(&both("hidden n, hidden o"), KERNEL_WORKLOAD).unwrap();
    // the length the entry's parameter: the prompt is 6
    let length = |sig: &str| {
        kernel(
            sig,
            "run llm prefill (cost(llm, known - cached)) growing kv;
              run llm decode (cost(llm, prompt - 1 - (known - 6))) growing kv;",
        )
        .replace("set kept = prompt;", "set kept = 6;")
        .replace("prompt + 3", "6 + prompt")
    };
    refused(
        &length("n, hidden o"),
        KERNEL_WORKLOAD,
        &[
            "`n` is a length the model generates (`run llm decode (…)`'s work at 20:19 reads it)",
            "help: `hidden n`",
        ],
    );
    compile(&length("hidden n, hidden o"), KERNEL_WORKLOAD).unwrap();
}
