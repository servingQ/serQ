//! `serq::outline`: where each declaration of program text is written.

use serq::frontend::outline::{Outline, Range};

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn text(src: &str, r: Range) -> &str {
    &src[r.start()..r.end()]
}

fn outline(src: &str) -> Outline {
    serq::outline(src).unwrap()
}

#[test]
fn a_pull_relation_its_queues_and_its_calls_are_found_in_the_text() {
    let src = read("examples/pd-disaggregation/llmd_nixl_pull.sq");
    let o = outline(&src);

    let main = o.main.as_ref().unwrap();
    assert!(text(&src, main.range).starts_with("fn main() {"));
    assert!(text(&src, main.range).ends_with('}'));
    assert_eq!(&src[main.range.end()..], "\n");

    let uses: Vec<_> = o.uses.iter().map(|u| u.path.as_str()).collect();
    assert_eq!(uses, ["std/args", "std/kvt", "std/share"]);
    assert_eq!(text(&src, o.uses[1].range), "use \"std/kvt\";");

    let lets: Vec<_> = o.decls.iter().filter(|d| d.kind == "let").collect();
    assert_eq!(text(&src, lets[0].range), "let NP = 2;");
    let thr = lets.iter().find(|d| d.name == "thr").unwrap();
    assert_eq!(
        text(&src, thr.range),
        "let thr = args.number(\"thr\", 512);"
    );

    let r = &o.relations[0];
    assert_eq!(
        (r.mode.as_str(), r.source.as_str(), r.reader.as_str()),
        ("pull", "P", "D")
    );
    assert_eq!(
        text(&src, r.range),
        "kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);"
    );
    assert_eq!(text(&src, r.function), "nixl_pull");

    let names: Vec<_> = o.queues.iter().map(|q| q.name.as_str()).collect();
    assert_eq!(names, ["gw", "P", "D"]);
    let p = &o.queues[1];
    assert_eq!(p.roles, ["prefill"]);
    assert_eq!(p.family.as_deref(), Some("NP"));
    assert!(text(&src, p.range).starts_with("queue P[NP] : prefill {"));
    assert!(text(&src, p.range).ends_with("lease kv (inf);\n    }\n  }"));
    let kinds: Vec<_> = p.members.iter().map(|m| m.kind.as_str()).collect();
    assert_eq!(kinds, ["device", "engine", "pool", "pool", "nic", "entry"]);
    assert_eq!(text(&src, p.members[4].range), "nic ps(BwP);");
    assert_eq!(p.members[3].name.as_deref(), Some("kv"));
    assert_eq!(
        text(&src, p.members[3].range),
        "pool kv on gpu { block bs; evict lru; preempt lifo; }"
    );

    let d = &o.queues[2];
    let entries: Vec<_> = d.members.iter().filter(|m| m.kind == "entry").collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].from, None);
    assert_eq!(entries[1].from.as_deref(), Some("P"));
    assert!(text(&src, entries[1].range).starts_with("decode (prompt) from P {"));

    let route = &o.routes[0];
    assert_eq!(route.gateway, "gw");
    assert!(text(&src, route.range).starts_with("route {"));
    assert!(
        text(&src, route.body)
            .trim()
            .starts_with("choose j in ND by")
    );

    let calls: Vec<_> = o
        .calls
        .iter()
        .map(|c| (text(&src, c.range), c.block.as_str(), c.owner.as_deref()))
        .collect();
    assert_eq!(
        calls,
        [
            ("P[i].prefill (input);", "route", Some("gw")),
            ("D[j].decode (input) from P[i];", "route", Some("gw")),
            ("D[j].decode (input);", "route", Some("gw")),
            ("gw.route();", "server", None),
        ]
    );
    let c = &o.calls[1];
    assert_eq!(
        (
            c.queue.as_str(),
            c.index.as_deref(),
            c.verb.as_str(),
            c.from.as_deref()
        ),
        ("D", Some("j"), "decode", Some("P"))
    );

    let server = o.server.as_ref().unwrap();
    assert!(
        text(&src, server.range).starts_with("server (input, hidden output) reply (produced) {")
    );
    // what crosses, for an editor that writes calls and entries
    let sig = o.signature.as_ref().unwrap();
    let params: Vec<_> = sig
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.hidden))
        .collect();
    assert_eq!(params, [("input", false), ("output", true)]);
    assert_eq!(sig.reply, ["produced"]);
    assert!(text(&src, o.workload.unwrap()).starts_with("workload {"));
}

#[test]
fn a_push_relation_says_push() {
    let src = read("examples/pd-disaggregation/vllm_nixl_push.sq");
    let o = outline(&src);
    let r = &o.relations[0];
    assert_eq!(
        (r.mode.as_str(), r.source.as_str(), r.reader.as_str()),
        ("push", "P", "D")
    );
    assert_eq!(text(&src, r.function), "nixl_push");
    let stages: Vec<_> = o
        .decls
        .iter()
        .filter(|d| d.kind == "stage")
        .map(|d| text(&src, d.range))
        .collect();
    assert_eq!(stages, ["stage front : delay;", "stage gate : delay;"]);
    // a call inside `fork { … }` is a call of the route
    assert!(
        o.calls
            .iter()
            .any(|c| text(&src, c.range) == "P.prefill (prompt);" && c.block == "route")
    );
}

#[test]
fn an_engine_outside_a_queue_is_a_declaration() {
    let src = read("examples/multi-turn/vllm.sq");
    let o = outline(&src);
    let kinds: Vec<_> = o
        .decls
        .iter()
        .filter(|d| d.kind != "let")
        .map(|d| (d.kind.as_str(), d.name.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("device", "gpu"),
            ("pool", "kv"),
            ("engine", "vllm"),
            ("pool", "reqs")
        ]
    );
    let engine = o.decls.iter().find(|d| d.kind == "engine").unwrap();
    assert!(text(&src, engine.range).starts_with("engine vllm on gpu {"));
    assert!(text(&src, engine.range).ends_with('}'));
    assert!(o.queues.is_empty() && o.calls.is_empty());
    assert_eq!(
        text(&src, o.server.unwrap().body).trim(),
        "vllm (input, output);"
    );
}

#[test]
fn ranges_are_bytes_after_a_wide_character() {
    // `é` is two bytes and one column
    let src = "// é\nfn main() {\n  let é_ = 1; stage s : delay;\n  workload { arrive poisson(1); turn { set n = 1; } }\n  server { run s (1); }\n}\n";
    let o = outline(src);
    let s = o.decls.iter().find(|d| d.kind == "stage").unwrap();
    assert_eq!(text(src, s.range), "stage s : delay;");
}

#[test]
fn a_program_that_does_not_link_has_an_outline() {
    let src = read("examples/multi-turn/vllm.sq")
        .replace("vllm (input, output);", "vllm (input, nowhere);");
    let ov = serq::Overrides::default();
    assert!(serq::compile_inspected(&src, &ov).is_err());
    let o = outline(&src);
    assert_eq!(
        text(&src, o.server.unwrap().body).trim(),
        "vllm (input, nowhere);"
    );
}

#[test]
fn a_parse_error_is_the_diagnostic() {
    let e = serq::outline("fn main() { queue }").unwrap_err();
    assert!(e.contains("1:"), "{e}");
}
