//! `serq::outline`: where each declaration of program text is written.

mod common;

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
    assert!(text(&src, p.range).ends_with("P (prompt, 1);\n    }\n  }"));
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
    // the workload is a file of its own: the deployment has none
    assert!(o.workload.is_none());
}

#[test]
fn a_routers_chooses_their_counts_and_keys_are_found_in_the_text() {
    let src = read("examples/pd-disaggregation/llmd_nixl_pull.sq");
    let o = outline(&src);
    let chooses: Vec<_> = o
        .chooses
        .iter()
        .map(|c| {
            (
                c.var.as_str(),
                text(&src, c.count),
                c.keys.iter().map(|&k| text(&src, k)).collect::<Vec<_>>(),
                c.block.as_str(),
                c.owner.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        chooses,
        [
            (
                "j",
                "ND",
                vec!["holders(D[j].kv) + queued(D[j].kv)", "~uniform(0, 1)"],
                "route",
                Some("gw")
            ),
            (
                "i",
                "NP",
                vec![
                    "cachedin(P[i].kv) > 0 ? 0 : 1",
                    "work(P[i]) + queued(P[i].reqs)",
                    "~uniform(0, 1)"
                ],
                "route",
                Some("gw")
            ),
        ]
    );
    let j = &o.chooses[0];
    assert_eq!(
        text(&src, j.range),
        "choose j in ND by (holders(D[j].kv) + queued(D[j].kv), ~uniform(0, 1));"
    );
    assert_eq!(
        text(&src, j.by),
        "(holders(D[j].kv) + queued(D[j].kv), ~uniform(0, 1))"
    );
}

#[test]
fn a_workloads_statements_are_found_in_the_text() {
    // the workload's statements are its file's, outlined beside the
    // deployment it runs against
    let deployment = read("examples/pd-disaggregation/llmd_nixl_pull.sq");
    let src = read("examples/workloads/chat_reply.sq");
    let (_, o) = serq::outline_paired(&deployment, &src).unwrap();
    let items: Vec<_> = o
        .workload_items
        .iter()
        .map(|w| {
            (
                w.kind.as_str(),
                w.name.as_deref(),
                w.block.as_str(),
                w.value.map(|v| text(&src, v)),
            )
        })
        .collect();
    assert_eq!(
        items,
        [
            ("arrive", None, "workload", Some("poisson(Lambda)")),
            ("set", Some("K"), "init", Some("0")),
            (
                "set",
                Some("n"),
                "turn",
                Some("ceil(K == 0 ? ~uniform(1000, 3000) : ~exp(500))")
            ),
            ("set", Some("o"), "turn", Some("floor(~exp(200)) + 1")),
            ("set", Some("more"), "turn", Some("~bernoulli(p)")),
            ("send", None, "workload", Some("input: K + n, output: o")),
            ("turn", None, "session", None),
            ("set", Some("K"), "session", Some("K + n + reply.produced")),
            ("tool", None, "session", Some("~exp(Z)")),
            ("turn", None, "session", None),
        ]
    );
    let arrive = &o.workload_items[0];
    assert_eq!(text(&src, arrive.range), "arrive poisson(Lambda);");
}

#[test]
fn a_choose_in_the_server_is_the_servers() {
    let src = read("examples/pd-disaggregation/pd_two_fabrics.sq");
    let o = outline(&src);
    let chooses: Vec<_> = o
        .chooses
        .iter()
        .map(|c| (text(&src, c.range), c.block.as_str(), c.owner.as_deref()))
        .collect();
    assert_eq!(
        chooses,
        [
            ("choose i in NP by (work(P[i]));", "server", None),
            (
                "choose j in ND by (holders(kvD[j]) + queued(kvD[j]));",
                "server",
                None
            ),
        ]
    );
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
    assert_eq!(stages, ["stage front : delay;"]);
    // the stage its sessions wait at is the workload file's, in its outline
    let workload = read("examples/workloads/short_base.sq");
    let (_, w) = serq::outline_paired(&src, &workload).unwrap();
    let stages: Vec<_> = w
        .decls
        .iter()
        .filter(|d| d.kind == "stage")
        .map(|d| text(&workload, d.range))
        .collect();
    assert_eq!(stages, ["stage gate : delay;"]);
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
    let src = "// é\nfn main() {\n  let é_ = 1; stage s : delay;\n  server { run s (1); }\n}\n";
    let o = outline(src);
    let s = o.decls.iter().find(|d| d.kind == "stage").unwrap();
    assert_eq!(text(src, s.range), "stage s : delay;");
}

#[test]
fn a_program_that_does_not_link_has_an_outline() {
    let src = read("examples/multi-turn/vllm.sq")
        .replace("vllm (input, output);", "vllm (input, nowhere);");
    let err = common::inspected(
        &src,
        &read("examples/workloads/chat.sq"),
        &serq::Overrides::default(),
    )
    .unwrap_err();
    assert!(err.contains("nowhere"), "{err}");
    let o = outline(&src);
    assert_eq!(
        text(&src, o.server.unwrap().body).trim(),
        "vllm (input, nowhere);"
    );
}

/// The outline reads the deployment alone: a server with no workload's file
/// beside it, which a run refuses, still has its outline.
#[test]
fn a_deployment_alone_has_an_outline() {
    let src =
        "fn main() {\n  stage s : delay;\n  server (n) reply (m) { run s (n); set m = 1; }\n}\n";
    assert!(
        serq::compile_inspected(src, &serq::Overrides::default())
            .unwrap_err()
            .contains("the server needs a workload to run")
    );
    let o = outline(src);
    assert!(o.workload.is_none());
    let server = o.server.unwrap();
    assert!(text(src, server.range).starts_with("server (n) reply (m) {"));
    let sig = o.signature.unwrap();
    assert_eq!(sig.params.len(), 1);
    assert_eq!(sig.reply, ["m"]);
    // a workload written in `fn main` is still an error to report
    assert!(
        serq::outline("fn main() { workload { session { turn; end; } } }")
            .unwrap_err()
            .contains("a workload is a file of its own")
    );
}

#[test]
fn a_parse_error_is_the_diagnostic() {
    let e = serq::outline("fn main() { queue }").unwrap_err();
    assert!(e.contains("1:"), "{e}");
}

/// Each device and engine, the capacities it declares and the pool on
/// each, and a device's KV (#645 A3, A4): what an editor may put where,
/// read from the program, not known beside it.
#[test]
fn devices_and_engines_say_their_capacities_and_pools() {
    let src = read("examples/pd-disaggregation/vllm_nixl_push.sq");
    let o = outline(&src);
    let d = o.devices.iter().find(|d| d.name == "D.gpu").unwrap();
    assert_eq!(d.engine.as_deref(), Some("D"));
    assert_eq!(d.memory.as_deref(), Some("D.kv"));
    let kv: Vec<_> = d
        .caps
        .iter()
        .map(|c| (c.name.as_str(), c.pool.as_deref()))
        .collect();
    assert_eq!(kv, [("kv", Some("D.kv"))]);
    assert_eq!(text(&src, d.caps[0].range), "kv");
    let e = o.engines.iter().find(|e| e.name == "P").unwrap();
    assert_eq!(e.device, "P.gpu");
    let caps: Vec<_> = e
        .caps
        .iter()
        .map(|c| (c.name.as_str(), c.pool.as_deref()))
        .collect();
    assert_eq!(caps, [("reqs", Some("P.reqs"))]);
    // who admits each pool's queue (#645 B9): the engine it is on, or the
    // engine `on D.gpu` names; the linker's own answer
    assert_eq!(e.caps[0].admitted_by.as_deref(), Some("P"));
    assert_eq!(d.caps[0].admitted_by.as_deref(), Some("D"));

    // a device no engine runs on has no KV, whatever is on it
    let o = outline(
        "fn main() { device ram { buf cap 10; } pool buf on ram { } stage s : delay; \
         server { run s (cost(s, 1)); } }",
    );
    let ram = &o.devices[0];
    assert_eq!((ram.engine.as_deref(), ram.memory.as_deref()), (None, None));

    // two pools on an engine's device, which does not link: both are
    // listed, and the memory is the first `pool … on` it, as the linker
    // takes it, not the first capacity
    let o = outline(
        "fn main() { device gpu { a cap 10; b cap 10; } \
         engine E on gpu { tokens cap 8; scheduler { advance running; admit waiting; } \
         worker { forward (1); } } \
         pool b on gpu { } pool a on gpu { } server { } }",
    );
    let gpu = &o.devices[0];
    assert_eq!(gpu.memory.as_deref(), Some("b"));
    let pools: Vec<_> = gpu.caps.iter().map(|c| c.pool.as_deref()).collect();
    assert_eq!(pools, [Some("a"), Some("b")]);
    // `on gpu`, no `E.`: no engine admits them, the engine's KV as well
    assert!(gpu.caps.iter().all(|c| c.admitted_by.is_none()));
    assert_eq!(ram.caps[0].pool.as_deref(), Some("buf"));
    assert_eq!(ram.caps[0].admitted_by, None);
}

/// Where a queue may still take a pool (#645 B6): each capacity no pool is
/// on, as `pool … on` spells it there, and who would admit it. A device's
/// capacity has two spellings that differ in that: `on gpu` (none admits)
/// and `on X.gpu` (`X`'s scheduler). Once the device an engine runs on
/// holds a pool, it offers none (`E-device-two-pools`).
#[test]
fn each_queue_says_where_a_pool_may_go() {
    let queue = |pools: &str| {
        format!(
            "fn main() {{ queue X : agg {{ device gpu {{ kv cap 1000; }} engine on gpu {{ \
             reqs cap 4; tokens cap 64; scheduler {{ advance running; admit waiting; }} \
             worker {{ forward (1); }} }} {pools} decode (p) {{ X (p, 1); }} }} \
             server (n) {{ X.decode (n); }} }}"
        )
    };
    let spots = |src: &str| -> Vec<(String, String, Option<String>)> {
        let o = outline(src);
        o.queues[0]
            .accepts
            .pools
            .iter()
            .map(|s| (s.on.clone(), s.cap.clone(), s.admitted_by.clone()))
            .collect()
    };
    let x = || Some("X".to_string());
    assert_eq!(
        spots(&queue("")),
        [
            ("X".to_string(), "reqs".to_string(), x()),
            ("gpu".to_string(), "kv".to_string(), None),
            ("X.gpu".to_string(), "kv".to_string(), x()),
        ]
    );
    // the KV taken, the engine's slots are left
    assert_eq!(
        spots(&queue("pool kv on gpu { block 16; }")),
        [("X".to_string(), "reqs".to_string(), x())]
    );
    // every capacity of the push example has its pool
    let src = read("examples/pd-disaggregation/vllm_nixl_push.sq");
    assert!(
        outline(&src)
            .queues
            .iter()
            .all(|q| q.accepts.pools.is_empty())
    );
}
