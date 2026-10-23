//! A hold's header and an engine's admission are read when the request is
//! admitted, so what the server sets is read there only once it is set
//! above, on every way to the hold or the call (#504,
//! `docs/design/admission-reads-set-above.md`). Until this request sets
//! it, an attribute holds what an earlier request left there, or 0.

mod common;

/// Chapter 2's workload: one turn, `send (work_size, items)`.
const MEMORY: &str = include_str!("../docs/tutorial/programs/workloads/02-memory.sq");

/// A request carrying `prompt` and the hidden `o`, and one carrying a
/// gateway's index `j` too.
const PROMPT: &str = "workload { arrive batch(1); init { set prompt = 32; set o = 4; } \
                      send (prompt, o); session { turn; end; } }";
const PROMPT_J: &str = "workload { arrive batch(1); init { set prompt = 32; set o = 4; set j = 0; } \
                        send (prompt, o, j); session { turn; end; } }";

/// A server with pool `mem` and stage `svc` whose hold reads `m` in its
/// header: `above` before it, `inside` its body, `below` after it.
fn memory(above: &str, inside: &str, below: &str) -> String {
    format!(
        "fn main() {{
  pool mem {{ cap 10; }}
  stage svc : fifo(4);
  server (work_size, items) {{
    {above}
    hold mem (cost(mem, m)) {{
      {inside}
      run svc (cost(svc, work_size));
    }}
    {below}
  }}
}}"
    )
}

fn refused(src: &str, workload: &str, needle: &str) {
    match common::compile(src, workload, &common::horizon(100.0)) {
        Ok(_) => panic!("accepted:\n{src}"),
        Err(e) => assert!(e.contains(needle), "{src}\n  {e}"),
    }
}

fn linked(src: &str, workload: &str) {
    common::compile(src, workload, &common::horizon(100.0))
        .unwrap_or_else(|e| panic!("{e}\n{src}"));
}

const HOLD_MEM: &str = "`hold mem`: the admission header reads `m`, which is not set above this hold on every way to it";

/// The program #504 was found with: the header always read `m` before the
/// body set it, 0 on the session's one request, and the hold took 0 units
/// (`mem used 0.0`, against `2.8` with the `set` above it).
#[test]
fn the_header_reading_what_its_body_sets_is_refused_as_found() {
    let src = "fn main() {
  pool mem { cap 10; }
  stage svc : fifo(4);
  server (work_size, items) {
    hold mem (cost(mem, m)) {
      set m = items;
      run svc (cost(svc, work_size));
    }
  }
}";
    refused(src, MEMORY, HOLD_MEM);
    refused(
        src,
        MEMORY,
        "the header is read when the request is admitted, so it would read what an earlier \
         request left there, or 0",
    );
    // at the `m` the header reads, a bare argument `cost` reads as a number
    refused(src, MEMORY, "5:25:");
}

/// What one arm of an `if` sets, a `while` that may not run, or a statement
/// after the hold, is not set on every way to the header.
#[test]
fn a_header_does_not_read_what_one_arm_a_while_or_a_later_statement_sets() {
    for (above, below) in [
        ("", "set m = items;"),
        ("if (items > 2) { set m = items; }", ""),
        (
            "if (items > 2) { set m = items; } else { set other = 1; }",
            "",
        ),
        ("while (items > 100) { set m = items; }", ""),
    ] {
        refused(&memory(above, "", below), MEMORY, HOLD_MEM);
    }
}

/// What every way above the hold sets is the header's: one `set`, both
/// arms, an arm that ends the request; and an earlier request's value,
/// copied above the hold before this one sets it.
#[test]
fn a_header_reads_what_every_way_above_it_sets() {
    for above in [
        "set m = items;",
        "if (items > 2) { set m = items; } else { set m = 1; }",
        "if (items > 2) { set m = items; } else { end; }",
    ] {
        linked(&memory(above, "", ""), MEMORY);
    }
    linked(
        &memory("set earlier = m;", "", "set m = items;")
            .replace("cost(mem, m)", "cost(mem, earlier)"),
        MEMORY,
    );
}

/// An entry's header is checked where the entry is called: a parameter is
/// the caller's argument, here what the route sets only after the call.
/// Neither the entry's check nor the server's caught it before #504.
#[test]
fn an_entry_parameter_bound_to_what_the_route_sets_after_the_call_is_refused() {
    let src = "fn main() {
  queue gw : gateway { route { E.decode (m); set m = prompt; } }
  queue E : agg { device gpu { kv cap 100; } pool kv on gpu { }
    engine on gpu { tokens cap inf; scheduler { advance running; admit waiting while (running.preempted == 0); } worker { forward (1); } }
    decode (p) {
      hold kv (cost(kv, p)) { run E prefill (cost(E, 1)) growing kv; }
    } }
  server (prompt, hidden o, j) { gw.route(); }
}";
    refused(
        src,
        PROMPT_J,
        "`hold E.kv`: the admission header reads `m`, which the call passes to the entry and \
         which is not set above the call on every way to it",
    );
    // the error stands at the call's `m`, where it is set above, and its
    // note at the entry's `p`
    refused(src, PROMPT_J, "2:42:");
    refused(
        src,
        PROMPT_J,
        "note: the entry's header reads it as its parameter at 6:25",
    );
    refused(src, PROMPT_J, "help: set `m` above the call");
    linked(
        &src.replace(
            "E.decode (m); set m = prompt;",
            "set m = prompt; E.decode (m);",
        ),
        PROMPT_J,
    );
}

/// An engine's admission is read at the call, as a hold's header is: what
/// it reads of the server's is set above the call (TGI's `taking`, with
/// `max_new_tokens` set after the request, linked before #504).
#[test]
fn an_engine_admission_reads_what_the_server_sets_above_the_call() {
    let program = |server: &str| {
        format!(
            "fn main() {{
  device gpu {{ kv cap 1000; }}
  engine E on gpu {{ tokens cap inf;
    scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.input + extra); }}
    worker {{ forward (1); }} }}
  pool kv on gpu {{ block 16; }}
  server (prompt, hidden o) {{ {server} }}
}}"
        )
    };
    refused(
        &program("E (prompt, o); set extra = 1;"),
        PROMPT,
        "`E (…)`: the engine's admission (`taking`, `reuse`, `prefix`, a connector's `load`) \
         reads `extra`, which is not set above this request on every way to it",
    );
    // at the call, with a note of where the engine reads it
    refused(
        &program("E (prompt, o); set extra = 1;"),
        PROMPT,
        "note: `E` reads it at 4:106",
    );
    refused(
        &program("if (prompt > 2) { set extra = 1; } E (prompt, o);"),
        PROMPT,
        "reads `extra`, which is not set above this request",
    );
    linked(&program("set extra = 1; E (prompt, o);"), PROMPT);
}

/// What a call `E (…)` writes for its caller to read after it (`E.prefill`,
/// the tokens it computed) the call sets: a header above the call would
/// read an earlier request's record, or 0.
#[test]
fn a_header_reads_an_engine_record_below_the_call() {
    let program = |server: &str| {
        format!(
            "fn main() {{
  device gpu {{ kv cap 1000; }}
  engine E on gpu {{ tokens cap inf;
    scheduler {{ advance running; admit waiting while (running.preempted == 0) taking kv (request.input); }}
    worker {{ forward (1); }} }}
  pool kv on gpu {{ block 16; }}
  pool mem {{ cap 10; }}
  server (prompt, hidden o) {{ {server} }}
}}"
        )
    };
    refused(
        &program("hold mem (cost(mem, E.prefill)) { } E (prompt, o);"),
        PROMPT,
        "`hold mem`: the admission header reads `E.prefill`, which is not set above this hold",
    );
    linked(
        &program("E (prompt, o); hold mem (cost(mem, E.prefill)) { }"),
        PROMPT,
    );
}

/// An entry's local is the queue's attribute (`E.kept`), and the entry's
/// header is checked where the entry is called: a local an earlier call of
/// the queue's entries sets on every way to it is set, and an entry no
/// statement calls is not checked, since its header never runs.
#[test]
fn an_entry_header_reads_a_local_an_earlier_call_sets_on_every_way() {
    let program = |route: &str| {
        format!(
            "fn main() {{
  queue gw : gateway {{ route {{ {route} }} }}
  queue E : agg, prefill {{ device gpu {{ kv cap 100; }} pool kv on gpu {{ }}
    engine on gpu {{ tokens cap inf; scheduler {{ advance running; admit waiting while (running.preempted == 0); }} worker {{ forward (1); }} }}
    prefill (prompt) {{
      set kept = prompt;
      hold kv (cost(kv, prompt)) {{ run E prefill (cost(E, 1)) growing kv; }}
    }}
    decode (prompt) {{
      hold kv (cost(kv, kept)) {{ set kept = prompt; run E prefill (cost(E, 1)) growing kv; }}
    }} }}
  server (prompt, hidden o, j) {{ gw.route(); }}
}}"
        )
    };
    linked(&program("E.prefill (prompt); E.decode (prompt);"), PROMPT_J);
    linked(&program("E.prefill (prompt);"), PROMPT_J);
    refused(
        &program("if (prompt > 2) { E.prefill (prompt); } E.decode (prompt);"),
        PROMPT_J,
        "`hold E.kv`: the admission header reads `E.kept`, which is not set above this hold",
    );
}
