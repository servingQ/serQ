# 프론트엔드 설계: 모델, 인스턴스, 주장

2026-09-28의 스케치입니다. 목적은 "서빙 시스템을 의미론적으로 표현하는 것"과 "표현된 시스템에 config를 넣어 돌리는 것"을 분리하는 것이고, [IR v4](ir-v4.md)와 독립입니다. 커널 프로세스 언어는 오늘의 IR과 같아서 `link(model, instance)`가 오늘의 닫힌 IR을 냅니다. 뒤에 붙은 자기 비판 두 번이 이 문서의 절반이고, 그 판정이 이슈로 낼 순서를 정합니다.

## 핵심 결정 넷

**1. 프로그램은 모델, 인스턴스, 주장의 세 문서.** 모델은 구조만 씁니다. 숫자는 구조적 상수(요청 1개) 말고는 전부 `param`이고 타입과 단위를 가집니다. 인스턴스는 한 모델의 파라미터 서명에 대한 값 배정이고, 트레이스, 비용 모델, 시드, 지평도 여기 속합니다. 주장은 모델에 대한 것(모든 인스턴스에서 성립, Lean)과 인스턴스에 대한 것(측정치나 오라클과 일치, `make check`)으로 나뉩니다. 모델은 자유변수를 가진 항, 인스턴스는 값매김, 의미는 `⟦M⟧ : Instance(M) → Process`입니다. ML의 functor와 signature가 이 관계입니다.

**2. 세션은 효과를 일으키는 코루틴, 배치는 핸들러의 집합.** 세션이 하는 일은 여덟 효과입니다. `acquire`, `grow`, `release`, `run`, `sample`, `observe`, `turn`, `now`. 풀이 acquire/grow/release를, 스테이지가 run을, 워크로드가 turn을, 인스턴스가 sample과 observe를 처리합니다. 정책은 핸들러의 파라미터이므로 프로그램에 놓입니다.

**3. 평가 시점은 치환이 아니라 블록.** admission 시점에 핸들러가 한 번 실행하는 블록을 둡니다. 바깥에서 읽은 값은 블록 안에 스냅샷으로만 들어갑니다. vLLM의 "스케줄러가 집을 때 조회"와 H-pin의 "도착 때 조회"가 둘 다 표현되고 차이가 문법에서 보입니다.

**4. 자원은 스코프이자 affine 값.** `acquire … as h { … }`는 스코프, `h : Held<kv>`는 스코프 안에서만 쓰는 affine 값. `grow`는 `h`에만.

그 위에 둘. 단위는 타입(`tokens`, `s`, `count`), 블록 반올림은 `kv.blocks(p - 1)`. 서빙 어휘는 trait(`impl Prefill, Decode`).

## vLLM을 세 문서로

```
model vllm {
  param block_size : tokens;
  param blocks     : count;                 // num_gpu_blocks
  param max_seqs   : count;                 // max_num_seqs
  param budget     : tokens;                // max_num_batched_tokens
  param chunk_cap  : tokens = unbounded;    // long_prefill_token_threshold
  param step_cost  : fn(Step) -> s;         // measured on the device
  param Prompt0, Prompt, Out : dist<tokens>;
  param Think : dist<s>;
  param Continue : dist<bool>;
  param Arrival : process;

  resource kv : Pool<tokens> {
    cap     = blocks * block_size;
    grain   = block_size;
    evict   = order by (released);          // LRU, tail first per block
    on_full = preempt (latest admitted);    // vLLM running[-1]; or `wait`
    admit   = via engine, head only;        // or `first fit`
  }
  resource reqs : Pool<count> { cap = max_seqs; }

  stage engine : Step {
    budget = budget;
    chunk  = chunk_cap;
    cost   = step_cost;
    serve  = residents by (admitted);       // or by (mode == decode ? 0 : 1, admitted)
    memory = kv;
  } impl Prefill, Decode;
  stage tool : Delay impl Tool;

  workload {
    arrive Arrival;
    state K : tokens = 0;
    turn { n ~ (K == 0 ? Prompt0 : Prompt); o ~ Out; more ~ Continue; }
  }

  process session {
    turn;
    loop {
      let t0 = now;
      let prompt = K + n;
      acquire reqs (1), kv at admission {
        let hit = min(kv.cached, kv.blocks(prompt - 1));   // read when the scheduler takes me
        need  prompt;                                       // scheduler_reserve_full_isl
        take  min(prompt, hit + engine.budget_left);
        reuse hit;
      } as h {
        observe hit = kv.cached > 0;
        prefill (prompt - kv.cached) growing h;
        observe ttft = now - t0;
        decode (o - 1) growing h;
      } keep (prompt + o);
      observe response = now - t0;
      K := prompt + o;
      if more { tool ~Think; turn; } else { end; }
    }
  }
}
```

H-pin은 `let hit = …` 한 줄을 `at admission` 블록 밖으로 옮기는 것입니다. 지금은 같은 변경이 `admit via` 삭제라서 의도가 보이지 않았습니다.

```
instance a100_short of vllm {
  block_size = 16 tokens;  blocks = 8010;  max_seqs = 64;  budget = 512 tokens;
  step_cost = |st| 13.9 ms + 41 us * st.ndec + 0.138 us * st.kvb
                 + 51.5 us * st.npre + 4.02 ns * st.attn;
  trace "short.csv" ordered binds { n, o, think, more };
  Arrival = spaced 3.0 s;
  run { horizon 2000 s; warmup 200 s; seeds { arrival 1, workload 2, session 3, evict 4 } }
}

claims vllm {
  invariant kv: allocated + cached <= cap;                    // ∀ instance; Lean
  theorem   serve_is_decode_first when chunk_cap = unbounded; // Lean
}
claims a100_short {
  oracle "tools/oracle/*.json" on { ttft_step, last_step, preemptions };
  expect ttft.mean within 15% of "data/exp/gpu_seq/3.0s.csv";
}
```

## 의미론

전이 두 종류의 LTS입니다. 순간 전이는 효과의 처리, 시간 전이는 스테이지가 일을 진행하는 지연. 순간 전이가 하나라도 가능하면 시간은 흐르지 않습니다(seQ의 settle, timed automata의 maximal progress). 난수는 스트림별 효과라 프로그램은 시드의 결정적 함수입니다.

| 효과 | 핸들러 | 핸들러의 정책 식 |
|---|---|---|
| `acquire`, `grow`, `release` | Pool | `evict`, `on_full`, `admit`, `grain` |
| `run` | Stage | `serve`, `budget`, `chunk`, `cost` |
| `turn` | Workload | 분포 또는 트레이스 |
| `sample`, `observe`, `now` | Instance | 시드, 기록 대상, 시계 |

## 자기 비판 1: "새 정리가 생기는가"로 재면

- **효과와 핸들러는 메커니즘이 아니라 비유.** "vLLM 오라클은 다른 핸들러"라고 했지만 `vllm_replay_oracle.py`는 `schedule()`을 통째로 돌립니다. admission, 서빙 순서, preemption을 한 스텝에서 같이 결정하므로 효과별 핸들러로 쪼개 꽂을 수 없습니다. Lean에서는 핸들러 합성 기계가 더 붙어 더 많은 일입니다.
- **모델/인스턴스 분리는 Lean에 주장만큼 주지 않는다.** 기호 파라미터를 넣는 순간 `decide`가 사라지고 사람이 증명합니다. 예로 든 두 주장은 이미 의미론의 정리로 있습니다. 새로 생기는 것은 "모든 인스턴스에서 성립하는 이 프로그램의 안전 성질"뿐이고, 그것은 [IR v4](ir-v4.md) §7의 deadlock 정리처럼 자동자 위에서만 쓸 수 있습니다. 시뮬레이터 쪽 진짜 이득은 스윕과 보정이 1급이 되는 것입니다.
- **`at admission` 블록은 읽힘을 IR 노드로 산다.** `~`를 허용하는 프로그램이 없으니 새 정리도 없습니다.
- **`Held<P>`는 뺀다.** 스코프가 이미 정리를 주고, affine 타입은 Lean에 타이핑 판단 하나를 더 형식화하게 만듭니다.
- **단위 타입은 마찰이 있다.** 비용 식은 `s/token²` 계수를 섞고 정책 식은 토큰 수에 `ln`을 씁니다. #14의 라벨이 먼저입니다.

## 자기 비판 2: "규칙이 줄고 표현이 직접적인가"로 재면

이 잣대가 기준 1과 맞고, 이 잣대에서는 대부분이 살아남습니다. 단순함은 프로그램 줄 수가 아니라 독자가 알아야 하는 규칙의 수로 잽니다.

| 조각 | 사라지는 것 | 새로 알아야 하는 것 | 판정 |
|---|---|---|---|
| admission 블록 | `at admission (x = e)` 절, `~` 금지 규칙, 린트 하나, 헤더 절의 시점 불일치(#31) | 블록 하나 = 시점 하나, 동사 셋 (`need`, `take`, `reuse`) | 채택. `~`를 계속 금지하면 치환으로 정의할 수 있어 IR 그대로 |
| trait 어휘 | "Which stage" 문단의 해소 규칙, `prefill on P2` 특수형 | `impl Prefill` 한 줄 | 채택 |
| 단위 라벨과 `kv.blocks(e)` | 상수 옆 주석 관행, 블록 반올림 식 | 단위 이름 셋 | 채택, 검사는 나중 |
| 정책을 식으로 | `decode first`와 `exclusive prefill`, 암묵 HOL blocking | `serve = by (…)` 식과 이름 붙은 기본값 | 채택 ([IR v4](ir-v4.md) §3) |
| 모델/인스턴스 | `let` 상수 블록, 실험마다 프로그램 복사 | 서명 타입 언어, 문서가 셋 | 조건부. 모델 안의 숫자를 링커가 거부해야 함. 옛 형태가 남으면 두 표기가 생겨 기준 0 위반 |
| claims | §5의 "무엇에 검사됐는가" 표를 밖에서 찾는 일 | 주장 문법 | 조건부 |
| 효과와 핸들러 | 없음 (프로그램에는) | 없음 | 문서의 구조로만. 스펙 §3을 표 하나와 시간 규칙 한 문단으로 다시 쓸 수 있음 |
| `Held<P>` | 없음 | 타입 하나 | 제거 |

커널은 줄지 않습니다. 열한 개 문장은 이미 작고 그게 Lean이 읽는 것이라 그대로가 맞습니다. 이 설계가 단순하게 만드는 층은 표면 문법과 스펙입니다.

## 재는 방법

프로그램 열두 개를 새 문법으로 옮겨 세 숫자를 셉니다. 프로그램마다 줄 수와 주석 줄 수, 스펙 §2와 §3에서 사라지는 문단 수, 그리고 `vllm.seq`와 `vllm_replay.seq`가 한 모델의 두 인스턴스로 합쳐지는가. 마지막은 반반입니다. replay의 `front` 스테이지는 구조적 차이라 모델에 두고 한 인스턴스에서 비용을 0으로 놓아야 하는데, 그것이 어색하면 분리가 덜 된 것입니다.

## 이슈로 낼 순서

1. admission 블록 (sugar, IR 변경 없음). #23과 #31의 절반을 닫습니다.
2. trait 어휘 (파서, IR 변경 없음). #5의 해소 규칙을 선언으로 대체합니다.
3. 단위 라벨 (#14의 검사 없는 쪽).
4. 모델/인스턴스 분리. 옛 형태를 금지하는 조건을 명시한 RFC로.
5. claims. 4 다음에.
