//! `kvt.l2_link (C, Q, share = s);` (#600): the copies of `Q`'s engine's
//! `connector C`, its loads and its saves, cross `Q`'s own NIC and `C`'s
//! `serve` at once. The L2's contents are every engine's, and each engine's
//! link is its own: a copy goes at one link's speed, and no engine slows
//! another unless the L2's `serve` is the limit.

use serq::Overrides;
use serq::frontend::diagnostic::Code;

mod common;

/// The issue's deployment: two engines share one L2. The session's first
/// turn saves 1008 tokens on one engine (`floor(1008 + 1, 16)`), its second
/// loads 1007 of them on the other (`min(floor(1008, 16), 1008 - 1) - hit
/// 0`, all but the last token) at 1e-4 s a token, 0.1007 s over a link of
/// its own, and observes how long the load took, the copy alone. `{l2}` is
/// the L2's `serve`, `{nic}` the engine queue's NIC, `{link}` the relation.
fn deployment(l2: &str, nic: &str, link: &str) -> String {
    format!(
        "use \"std/kvt\";
use \"std/share\";

fn main() {{
  queue lmc : l2 {{ device host {{ kv cap 1e6; }} pool kv on host {{ block 16; evict lru; }} serve {l2}; }}
  queue E[2] : agg {{
    device gpu {{ compute (t) = t * 1e-6; hbm (k) = 1e-3; kv cap 1e5; }}
    engine on gpu {{
      reqs cap 8;
      tokens cap 8192;
      connector lmc {{
        per_token (1e-4);
        load (request.preempted ? 0 : max(0, min(floor(cachedin(lmc.kv), 16), request.known - 1) - request.hit));
        save (floor(request.input + request.output, 16));
      }}
      scheduler {{
        advance running;
        admit waiting while (running.preempted == 0)
          taking kv (min(request.known, request.hit + (request.loaded > 0 ? request.loaded : budget_left(E)))) reserve (request.known),
                 reqs (1)
          reuse (floor(request.known - 1, 16))
          cache (request.input + request.output);
      }}
      worker {{ forward (hbm(0) + compute(batch.tokens)); }}
    }}
    {nic}
    pool reqs on E {{ queue fifo; }}
    pool kv on E.gpu {{ block 16; evict lru; preempt lifo; }}
    decode (prompt) {{ E (prompt, 1); }}
  }}
  {link}
  server (K, t0) {{
    set e = serial - 2 * floor(serial / 2);   // the session's first engine
    E[K == 0 ? e : 1 - e].decode (1008);
    if (K == 1) {{ observe load_time = E.load_done - t0; }}
  }}
}}
"
    )
}

/// `n` sessions at once, each a save on one engine and a load on the
/// other: with two, the engines load at the same time.
fn workload(n: usize) -> String {
    format!(
        "workload {{
  arrive batch({n});
  init {{ set K = 0; }}
  turn {{ }}
  send (K, t0);
  session {{ set t0 = now; turn; set K = 1; set t0 = now; turn; end; }}
}}"
    )
}

const NIC: &str = "nic ps(1);";

fn link(policy: &str) -> String {
    format!("kvt.l2_link (lmc, E, share = share.{policy});")
}

/// The mean `load_time` of `n` sessions.
fn load_time(d: &str, n: usize) -> f64 {
    let r = common::run(d, &workload(n), &common::horizon(100.0))
        .unwrap_or_else(|e| panic!("{e}\n{d}"));
    let o = r.observe("load_time").expect("load_time");
    assert_eq!(o.samples.len(), n, "{}", r.text());
    o.samples.iter().sum::<f64>() / n as f64
}

fn near(got: f64, want: f64) {
    assert!((got - want).abs() < 1e-4, "{got} for {want}");
}

/// The issue's After, run: each engine copies over its own link. With an
/// L2 of two links' bandwidth a load takes one link's time, 1007 tokens at
/// 1e-4 s, 0.1007, whether one engine loads or both do; with an L2 of one
/// link's, the two loads divide the L2, as the program says: 0.2014. The
/// two policies agree, each copy's tightest stage being one it alone holds
/// or one the two hold alike.
#[test]
fn each_engine_copies_over_its_own_link() {
    for policy in ["bottleneck", "maxmin"] {
        let wide = deployment("ps(2)", NIC, &link(policy));
        near(load_time(&wide, 1), 0.1007);
        near(load_time(&wide, 2), 0.1007);
        let narrow = deployment("ps(1)", NIC, &link(policy));
        near(load_time(&narrow, 1), 0.1007);
        near(load_time(&narrow, 2), 0.2014);
    }
}

/// The issue's Before, unchanged: without a link the copies cross the L2's
/// `serve` alone, so one `serve` is every engine's link at once.
#[test]
fn without_a_link_the_l2_alone_is_crossed() {
    let one = deployment("ps(1)", "", "");
    near(load_time(&one, 1), 0.1007);
    near(load_time(&one, 2), 0.2014);
    let two = deployment("ps(2)", "", "");
    near(load_time(&two, 1), 0.0504);
    near(load_time(&two, 2), 0.1007);
}

/// What the relation lowers to: each member's load and save hold the L2's
/// stage and the member's own NIC, divided by the relation's policy, over
/// the cost of the two, and name no poster (a load and a save go opposite
/// ways). The IR is the one it was: `CTransfer`'s `also` and `share`.
#[test]
fn a_link_puts_load_and_save_on_the_member_s_nic() {
    use serq::ir::{CExpr, CStageKind, CostTarget, Share};
    let p = common::compile(
        &deployment("ps(2)", NIC, &link("bottleneck")),
        &workload(1),
        &common::horizon(100.0),
    )
    .unwrap();
    let stage = |name: &str| p.stages.iter().position(|s| s.name == name).unwrap();
    let (lmc, nic) = (stage("lmc"), stage("E.nic"));
    let mut members = 0;
    for st in &p.stages {
        let CStageKind::Step(sp) = &st.kind else {
            continue;
        };
        let i = st.index.expect("a member") as f64;
        let c = sp.connector.as_ref().expect("a connector");
        for t in [&c.loads[0], c.save.as_ref().unwrap()] {
            assert_eq!(t.stage.base, lmc);
            assert_eq!(t.also.len(), 1);
            assert_eq!((t.also[0].base, t.also[0].count), (nic, 2));
            assert!(matches!(t.also[0].index.as_deref(), Some(CExpr::Num(k)) if *k == i));
            assert_eq!(t.share, Some(Share::Bottleneck));
            assert_eq!(t.poster, None);
            let CExpr::Cost(CostTarget::Joint(on), _) = &t.work else {
                panic!("{:?}", t.work)
            };
            assert_eq!(
                on,
                &[
                    CostTarget::Stage {
                        base: lmc,
                        count: 1
                    },
                    CostTarget::Stage {
                        base: nic,
                        count: 2
                    }
                ]
            );
        }
        members += 1;
    }
    assert_eq!(members, 2);
}

fn refused(d: &str, needles: &[&str]) {
    let e = match common::compile(d, &workload(1), &common::horizon(100.0)) {
        Err(e) => e,
        Ok(_) => panic!("accepted:\n{d}"),
    };
    for n in needles {
        assert!(e.contains(n), "missing {n:?}: {e}");
    }
}

/// The relation names an L2 declared above, then a queue declared above
/// whose engine's connector copies through it and which has a NIC; once,
/// with a policy and no NIXL argument. Each refusal says what to write.
#[test]
fn a_link_names_an_l2_then_its_engine_s_queue() {
    let d = |link: &str| deployment("ps(2)", NIC, link);
    refused(
        &d("kvt.l2_link (cache, E, share = share.maxmin);"),
        &["error[E-copy-no-queue]: `kvt.l2_link (cache, E)`: no queue `cache` is declared above"],
    );
    refused(
        &d("kvt.l2_link (E, lmc, share = share.maxmin);"),
        &[
            "error[E-copy-swapped]: `kvt.l2_link (E, lmc)`: `E` plays no `l2`, and `lmc` does: \
             the L2 comes first",
            "help: fix-it: swap the two: `kvt.l2_link (lmc, E, …)`",
        ],
    );
    refused(
        &d("kvt.l2_link (E, E, share = share.maxmin);"),
        &["`kvt.l2_link (E, E)`: `E` plays no `l2`"],
    );
    refused(
        &deployment("ps(2)", "", &link("maxmin")),
        &[
            "error[E-copy-no-nic]: `kvt.l2_link (lmc, E)`: queue `E` has no `nic`",
            "help: give `E` a NIC, `nic ps(…);`",
        ],
    );
    // the engine's connector, which says what moves, is the program's: a
    // help, no fix-it (criterion 2)
    let unconnected = d(&link("maxmin")).replace(
        "      connector lmc {
        per_token (1e-4);
        load (request.preempted ? 0 : max(0, min(floor(cachedin(lmc.kv), 16), request.known - 1) - request.hit));
        save (floor(request.input + request.output, 16));
      }
",
        "",
    );
    refused(
        &unconnected,
        &[
            "error[E-copy-no-connector]: `kvt.l2_link (lmc, E)` is the link the copies of `E`'s \
             engine's `connector lmc` cross, and `E`'s engine has no `connector lmc`",
            "help: in `E`'s engine, write `connector lmc { per_token (r); load (n); save (n); }`",
        ],
    );
    refused(
        &d(&format!("{} {}", link("maxmin"), link("maxmin"))),
        &[
            "`kvt.l2_link (lmc, E)`: `E` is linked to `lmc` already, by the relation at 31:3: one \
             link per engine and L2",
        ],
    );
    refused(
        &d("kvt.l2_link (lmc, E);"),
        &[
            "`kvt.l2_link (lmc, E)`: concurrent copies divide the L2's `serve` and the NIC by \
             the policy the relation names",
        ],
    );
    for arg in ["latency = 1", "ttl = 1"] {
        refused(
            &d(&format!(
                "kvt.l2_link (lmc, E, {arg}, share = share.maxmin);"
            )),
            &["`kvt.l2_link` takes `share = …`, found `"],
        );
    }
    refused(
        &d(&link("maxmin")).replace("use \"std/kvt\";\n", ""),
        &["`kvt.l2_link` needs `use \"std/kvt\";`"],
    );
    refused(
        &d("kvt.l2_links (lmc, E, share = share.maxmin);"),
        &[
            "std/kvt has no function `l2_links`; it declares `kvt.nixl_pull (SOURCE, READER, …);`, \
             `kvt.nixl_push (SOURCE, READER, …);` and `kvt.l2_link (L2, QUEUE, …);`",
        ],
    );
}

/// The kernel's rules about runs over several stages hold for a link: the
/// L2 a link reaches serves by processor sharing at a constant rate, and
/// the links to one L2 are one fabric, with one policy.
#[test]
fn a_link_is_a_fabric_of_the_kernel() {
    refused(
        &deployment("delay", NIC, &link("maxmin")),
        &[
            "stage `lmc` is held with another stage by one run, so its capacity is shared: it must be `ps(φ)`",
        ],
    );
    // a second engine queue, linked to the same L2 by another policy
    let two = deployment("ps(2)", NIC, &link("maxmin")).replace(
        "  server (K, t0) {",
        "  queue F : agg {
    device gpu { compute (t) = t * 1e-6; hbm (k) = 1e-3; kv cap 1e5; }
    engine on gpu {
      tokens cap 8192;
      connector lmc { per_token (1e-4); load (0); }
      scheduler { advance running; admit waiting while (running.preempted == 0) taking kv (request.known); }
      worker { forward (hbm(0) + compute(batch.tokens)); }
    }
    nic ps(1);
    pool kv on F.gpu { block 16; }
    decode (prompt) { F (prompt, 1); }
  }
  kvt.l2_link (lmc, F, share = share.bottleneck);
  server (K, t0) {
    F.decode (16);",
    );
    refused(&two, &["one fabric has one policy"]);
}

/// The fix-it of a link written queue first swaps the two back, and the
/// example is as it was (`serq::diagnose`).
#[test]
fn a_swapped_link_is_swapped_back() {
    let path = "examples/multi-turn/vllm_lmcache_replicas.sq";
    let d = std::fs::read_to_string(path).unwrap();
    let w = std::fs::read_to_string(serq::paired_workload(std::path::Path::new(path)).unwrap())
        .unwrap();
    assert!(serq::diagnose(&d, Some(&w), &Overrides::default()).is_none());
    let right = "kvt.l2_link (lmc, E,";
    assert!(d.contains(right));
    let swapped = d.replacen(right, "kvt.l2_link (E, lmc,", 1);
    let r = serq::diagnose(&swapped, Some(&w), &Overrides::default()).expect("refused");
    assert_eq!(r.code, Some(Code::CopySwapped.name()), "{}", r.message);
    assert_eq!(r.fixes.len(), 1, "{}", r.message);
    assert!(
        r.fixes[0].label.contains("the L2 first"),
        "{}",
        r.fixes[0].label
    );
    let mut fixed = swapped.clone();
    let mut edits = r.fixes[0].edits.clone();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.0));
    for e in edits {
        fixed.replace_range(e.range.0..e.range.1, &e.text);
    }
    assert_eq!(fixed, d);
}

/// A save reads its member's pools as a load does: its amount and its
/// blocks are lowered as the member's (`member_transfer`). The save's
/// amount was lowered outside the member's window, so `cachedin(E.kv)`,
/// which a load may read, did not link in a save ("pool `E.kv` is a family
/// of 2; index it").
#[test]
fn a_save_reads_its_member_s_pools_as_a_load_does() {
    use serq::ir::{CArg, CExpr, CStageKind};
    fn pools(e: &CExpr, out: &mut Vec<usize>) {
        match e {
            CExpr::Call(_, args) => {
                for a in args {
                    match a {
                        CArg::Pool(r) => out.push(r.base),
                        CArg::Expr(x) => pools(x, out),
                        CArg::Stage(_) => {}
                    }
                }
            }
            CExpr::Cost(_, x) | CExpr::Unary(_, x) => pools(x, out),
            CExpr::Binary(_, a, b) => {
                pools(a, out);
                pools(b, out);
            }
            CExpr::Cond(c, a, b) => {
                pools(c, out);
                pools(a, out);
                pools(b, out);
            }
            _ => {}
        }
    }
    let d = deployment("ps(2)", NIC, &link("maxmin")).replace(
        "save (floor(request.input + request.output, 16));",
        "save (min(floor(request.input + request.output, 16), cachedin(E.kv)));",
    );
    let p = common::compile(&d, &workload(1), &common::horizon(100.0))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut members = 0;
    for st in &p.stages {
        let CStageKind::Step(sp) = &st.kind else {
            continue;
        };
        let i = st.index.expect("a member");
        let kv = p
            .pools
            .iter()
            .position(|q| q.name == "E.kv" && q.index == Some(i))
            .expect("the member's pool");
        let save = sp.connector.as_ref().unwrap().save.as_ref().unwrap();
        let mut read = vec![];
        pools(&save.amount, &mut read);
        assert_eq!(read, [kv], "member {i}");
        members += 1;
    }
    assert_eq!(members, 2);
}

/// A queue's NIC is one stage whatever crosses it: a prefiller that saves
/// to an L2 over `P.nic` while its decoders read from it over the same
/// `P.nic` (`kvt.nixl_pull`) makes the NIXL reads and the L2 saves one
/// fabric, with one policy (docs/design/l2-link.md, self-critique). A
/// policy apart from the NIXL relation's is refused.
#[test]
fn a_nic_both_relations_cross_is_one_fabric() {
    let pd = std::fs::read_to_string("examples/pd-disaggregation/llmd_nixl_pull.sq").unwrap();
    let w = std::fs::read_to_string("examples/workloads/chat_reply.sq").unwrap();
    let with = |policy: &str| {
        let lines = [
            (
                "  queue P[NP] : prefill {",
                "  queue lmc : l2 { device host { kv cap 1e6; } pool kv on host { block 16; evict \
                 lru; } serve ps(BwP); }\n  queue P[NP] : prefill {"
                    .to_string(),
            ),
            (
                "      reqs cap max_seqsP;\n",
                "      reqs cap max_seqsP;\n      connector lmc { per_token (1e-6); save \
                 (floor(request.input, 16)); }\n"
                    .to_string(),
            ),
            (
                "  kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);",
                format!(
                    "  kvt.nixl_pull (P, D, latency = x0, share = share.maxmin);\n  kvt.l2_link \
                     (lmc, P, share = share.{policy});"
                ),
            ),
        ];
        lines.iter().fold(pd.clone(), |d, (old, new)| {
            assert!(d.contains(old), "{old}");
            d.replacen(old, new, 1)
        })
    };
    common::compile(&with("maxmin"), &w, &common::horizon(100.0)).unwrap_or_else(|e| panic!("{e}"));
    let e = match common::compile(&with("bottleneck"), &w, &common::horizon(100.0)) {
        Err(e) => e,
        Ok(_) => panic!("accepted"),
    };
    assert!(
        e.contains("runs join the stages `lmc`, `P.nic`, `D.nic`")
            && e.contains("one fabric has one policy"),
        "{e}"
    );
}
