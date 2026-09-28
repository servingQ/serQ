# IR v4: 암묵적인 것을 정적으로

RFC [#41](https://github.com/vrvrv/seQ/issues/41)의 본문입니다. 이슈가 논의의 자리이고 이 문서는 그 결과를 따라갑니다. After는 전부 스케치이고 컴파일하지 않았습니다. Before는 저장소에서 복사했습니다. 개념마다 5살용 그림책이 한 장씩 있습니다(맨 아래, 비공개 아티팩트라 공유 설정이 필요합니다).

## 한 줄

IR에서 암묵적으로 정해지는 것 넷(식이 평가되는 순간, 캐시 단위의 소유자, 동점을 깨는 순서, 같은 시각의 사건 순서)을 명시적이고 정적으로 만들고, 그 정적 성질이 허가하는 최적화 하나(step coalescing)를 얻습니다.

## 왜

Rust의 소유권은 기능이 아니라, 동적이고 암묵적이던 것(누가 언제 해제하는가)을 정적으로 만든 것입니다. 그래서 컴파일러가 버그를 거부하고 동시에 GC를 뺐습니다. 같은 수를 두려면 우리의 "C 버그 목록"이 필요하고, 그것이 `docs/review.md` §3에 있습니다.

두 번의 검증에서 잡힌 열다섯 개를 분류하면 다섯 부류이고, 넷이 암묵적인 것입니다.

| 부류 | `docs/review.md` §3의 항목 | 암묵적이었던 것 |
|---|---|---|
| 평가 시점 | 첫 chunk 과다 예약 → `budget_left`; 예산 0에 admission → `admit via`; #23 | 식이 언제 평가되는가 |
| 캐시 수명 | 끝난 세션의 접두사가 살아남음; `end`가 버림 → keep; `drop`이 vLLM이 두는 블록을 지움; 생성 토큰 블록 | 캐시 단위를 누가 소유하는가 |
| 순서 | prefill 동시 완료 자리 바꿈; LRU 동점을 세션 번호로 | 동점을 무엇이 깨는가 |
| 같은 순간 | 두 도착 사이에 iteration 시작; 일 0인 run이 영원히 상주 | 같은 시각의 사건 순서 |
| 이름 | `let`이 속성에 가려짐; `cached`가 어느 풀의 것인가 | 이름 공간 (이 RFC 밖) |
| **복구** | decode 중 preemption 뒤 본문이 처음부터 다시 돕니다 (이 RFC를 쓰는 중 발견, 아래 5) | preemption이 무엇을 보존하는가 |

PS 가상 시계 버그와 철회된 이중안정 주장은 인터프리터 버그와 통계 문제라 IR이 못 고칩니다. 빼둡니다.

## 1. 모든 식은 평가 순간 하나에 속한다

그림책: [볼 때는 그 자리에서](https://claude.ai/artifact/WMxGr1RvWeiWPzSVLybE7w)

지금 `CtxVar`는 "의미론이 공급하는 곳에서만 의미 있다"고 문서에 적혀 있고, 그것이 seQ의 undefined behavior입니다. `budget_left`를 `@session` 문장에서 읽으면 조용히 0이 나옵니다.

순간은 넷입니다. `@session`(세션이 문장에 도달), `@admit(P)`(P의 스케줄러가 결정), `@step(S)`(S의 iteration 시작), `@evict(P)`. 문맥 변수는 자기 순간 아래에서만 합법이고, `@session` 값이 `@admit` 식에 들어가면 스냅샷이라는 것이 IR에 보입니다.

**자기 리뷰에서 바꾼 것.** 채팅에서는 "모든 식에 태그"라고 했는데, 순간은 사실 IR의 위치가 정합니다. hold 헤더는 `@admit`, `evict` 키는 `@evict`, `budget`과 `cost`는 `@step`, 문장은 `@session`. 그러니 태그를 식에 붙일 필요가 없고, `Program::validate`가 위치별 허용표를 검사하면 됩니다. 새 노드는 admission 블록 하나입니다. 비용이 크게 줄었습니다.

**Before** (`src/ir.rs:56-57`, `159-170`)

```rust
/// Context variables: meaningful only where the semantics supplies them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CtxVar {
```

```rust
    Hold {
        pools: Vec<(CRef, CExpr, Option<CExpr>)>,
        reuse: Option<CExpr>,
        body: BlockId,
        cache: Option<CExpr>,
    },
```

**After** (스케치)

```rust
/// Where an expression is evaluated. `validate` rejects a context variable
/// outside its moment: `BudgetLeft` only at `Admit`/`Step`, `Age`/`Size`/`Last` only at `Evict`.
pub enum Moment { Session, Admit(usize), Step(usize), Evict(usize) }

    Hold {
        /// Evaluated once, at `Moment::Admit`, in the session's environment; `Let`s bind once.
        at_admission: Vec<AdmitStmt>,          // Let(slot, CExpr) | Need(CRef, CExpr) | Take(CRef, CExpr) | Reuse(CRef, CExpr)
        body: BlockId,
        keep: Vec<(CRef, CExpr)>,               // Moment::Session, at scope exit
    },
```

잡히는 것: 위 표의 첫 행 전부, 링커 린트 하나, `at admission` 바인딩의 `~` 금지 규칙(블록 안 `Let`은 한 번 묶이므로 필요 없음). 6번의 허가 조건도 이 표에서 검사됩니다.

**같은 표로 검사되는 것 하나 더: 스케줄러가 답을 훔쳐보지 않는다.** 지금 `o`는 `turn`에서 요청 전에 뽑히고, hold 헤더·evict 키·budget 식이 `o`를 읽는 것을 막는 것이 없습니다. `reserve (prompt + o)`가 합법입니다. vLLM은 `max_tokens`라는 상한만 알고 실제 길이는 EOS까지 모릅니다. 프로그램들은 `reserve (prompt)`로 그 규율을 손으로 지키고 있습니다. 속성에 "스케줄러 순간에서 읽기 금지" 표시를 두면 위치별 허용표가 그대로 검사합니다.

```rust
pub struct Program { …, pub hidden: Vec<usize> /* attr slots illegal at Admit/Step/Evict */, … }
```

상한이 필요하면 프로그램이 별도 속성으로 선언합니다(`max_out`). 뽑힌 `o`와 선언된 상한이 다른 것이라는 사실이 7의 정리에 필요합니다.

## 2. 단위는 항상 소유자가 하나다

그림책: [블록은 언제나 한 상자에](https://claude.ai/artifact/GvY7xFGJ7PgaCjpkA9h7pp)

풀의 모든 단위는 세 상태 중 하나입니다. hold가 소유(allocated), 캐시가 소유(kept, 되돌려 빌릴 자격 키 하나를 기억), 자유. 전이는 move만 있습니다. acquire는 자유·캐시→hold(`reuse`가 되돌려 빌리기), release는 `keep`만큼 캐시로 나머지는 자유로, evict와 drop은 캐시→자유.

그러면 `allocated + cached + free = cap`이 등식이 되어 매 사건마다 단언할 수 있고, Lean에서는 보존 법칙입니다. 죽은 엔트리는 "캐시가 소유하되 자격자가 없는 단위"로 정의가 생깁니다. `end`가 접두사를 두는가 버리는가는 인터프리터 기본값이 아니라 IR의 명시적 move가 됩니다.

**자기 리뷰에서 바꾼 것.** `cap`이 무한(기본값)인 풀에서는 자유가 무한이라 등식이 공허합니다. 유한 풀에만 단언합니다. 자격자를 세션 번호가 아니라 키로 두면 나중의 내용 주소 캐시(공통 시스템 프롬프트, §9 한계 2)가 같은 상태 기계 위에 놓입니다. 등식이 부등식보다 Lean 증명을 실제로 쉽게 하는지는 `SeqLang.Step.invariant` 쪽에서 먼저 확인할 것. 확인 전에는 채택하지 않습니다.

**Before** (`docs/language.md` §3)

```
The invariant `allocated + cached ≤ cap` holds in every reachable configuration
(`SeqLang.Step.invariant`). `end` releases every hold but *keeps* the
session's cached prefixes
```

**After** (스케치)

```rust
pub struct CPool { …, pub on_end: EndMove /* Keep | Free */, … }
// interp, debug build: after every event, for every finite pool
debug_assert_eq!(p.allocated + p.cached + p.free, p.cap);
```

## 3. 순서는 선언된 키이고 동점은 사건 번호가 깬다

그림책: [번호표가 정해요](https://claude.ai/artifact/E4eZ7WnErxSYcJMnh5UfJ1)

IR의 모든 순서 있는 집합(풀 큐, 상주 목록, 축출 순서, preemption 희생자, `choose` 동점)은 키 튜플로 정의되고, 마지막 원소는 이름 붙은 사건의 일련번호여야 합니다. `arrived`, `admitted`, `released`. 전순서가 되고 구현 순서가 존재하지 않습니다.

**Before** (`src/interp.rs:141-142`, 주석으로만 존재)

```rust
    /// Release order (breaks ties in `last`: entries released at the same
    /// instant age in the order they were released).
```

**After** (스케치)

```rust
pub enum Seq { Arrived, Admitted, Released }
pub struct COrder { pub keys: Vec<CExpr>, pub tie: Seq }   // validate: `tie` required
pub struct CPool { …, pub evict: COrder, pub queue: COrder, pub victim: COrder, … }
pub struct CStep { …, pub serve: COrder, … }               // replaces decode_first + exclusive_prefill
```

`serve`가 식이 되면 #19의 B안과 #8의 기준 2 위반 둘(`exclusive prefill`, `decode first`)이 같이 닫힙니다.

## 4. 시각은 정수 틱 위의 초조밀 시간이다

그림책: [3시에도 첫째 둘째가 있어요](https://claude.ai/artifact/Ay3Eqd9PXbExfTp2ovXjnK)

시각은 `(t, n)`. `t`는 정수 틱, `n`은 같은 `t` 안의 순서. settle 규칙은 "`t`가 오르기 전에 모든 `n`을 끝낸다"가 되고, "도착은 iteration 시작보다 먼저"는 선언된 우선순위입니다. 일 0인 `run`은 no-op이라고 IR이 정의합니다.

**자기 리뷰에서 추가한 것.** 채팅에서는 초조밀 시간만 말했는데, `time: f64` 위에서는 "같은 순간"이 반올림에 달려 있습니다. 트레이스 간격 `i·spacing`이 정확히 같은지가 그렇고, macOS와 Linux의 staleness diff가 1 ulp 다른 일이 이미 있었습니다. 정수 틱(ns)이면 Lean 조각의 ℕ 시계와 같은 종류이고 플랫폼 차이가 사라집니다. 비용 모델의 f64 적합값은 평가할 때 틱으로 양자화합니다. 1 ns는 A100 스텝 14 ms 앞에서 무시됩니다.

**Before** (`src/interp.rs:33`, `src/ir.rs` `Program`)

```rust
    time: f64,
```

```rust
    pub horizon: f64,
    pub warmup: f64,
```

**After** (스케치)

```rust
pub struct Instant { pub tick: u64, pub sub: u32 }   // ns; `sub` orders same-tick events
pub struct Program { …, pub tick_ns: u64, pub horizon: u64, pub warmup: u64, … }
```

## 5. 세션은 스택이 아니라 자동자다

그림책: [말은 언제나 한 칸에](https://claude.ai/artifact/MoULVkKwvVqvypSS11oAHq)

지금 세션 상태는 "블록 프레임의 스택"이고 Lean은 세션 수준 의미론을 형식화하지 못했습니다(§9 한계 4). 프로그램에 프로시저 호출과 재귀가 없으므로 그 스택은 프로그램 카운터 하나로 결정됩니다. 막힐 수 있는 지점(acquire, run, turn)을 상태로 하는 자동자로 컴파일하면 `loop`와 `branch`는 점프, preemption은 "acquire 상태로 점프", 세션 상태는 `pc`와 속성 벡터뿐입니다.

**5가 첫 번째로 고쳐야 하는 것: preemption은 재시작이 아니라 재개다.** 지금 `preempt()`는 희생자의 프레임을 hold 문장까지 되감고 hold를 큐의 머리에 다시 넣습니다. 본문이 처음부터 다시 돕니다. `prefill (prompt - c)` 그리고 `decode (o - 1)` 전부. decode run의 진행도 g는 되감긴 프레임에 있었으니 사라집니다. vLLM은 `num_computed_tokens = 0`만 하고 생성된 토큰은 요청에 남습니다. 재스케줄 때 prompt + g를 prefill로 다시 계산하고 o − g − 1개만 decode합니다.

| | seQ 지금 | vLLM |
|---|---|---|
| 재admission 뒤 decode | o − 1 번 다시 | o − g − 1 번 |
| 재계산 prefill | prompt − c | prompt + g − c |
| 히트 상한 | `hitmax = floor((prompt−1)/bs)·bs`, g의 블록은 죽은 엔트리 | prompt + g − 1 |

`docs/review.md` §2가 "preemption은 스코프를 중단하고 문장을 재실행하는 것, 정확히 `_preempt_request`"라고 적은 것은 prefill 중에는 맞고 decode 중에는 틀립니다. Lean 모델도 같은 규칙입니다.

**어떤 오라클도 이 경로를 지나지 않았습니다.** `tools/oracle/*.out.json`의 preemptions: `chunked` 0, `hol` 0, `longchunk` 0, `mixed` 0, `seqcap` 0, `preempt` 1. 그 1은 가용 블록 10개에서 prefill 도중의 self-preemption이라 토큰을 만들기 전입니다. `programs/vllm_replay.seq`를 spacing 3.5 s와 2.5 s로 돌린 보고서의 `kv` 행은 둘 다 `preempt 0`이고, KV 사용은 128 160 토큰 중 793과 1 865입니다. 2.5 s의 붕괴는 `reqs` 큐 대기 35.8 s이고 메모리 압박이 아닙니다. 3 321건 일치는 preemption 복구를 한 번도 시험하지 않았습니다.

**Before** (`src/interp.rs:1463-1477`, `scheduler.py:1560-1561`)

```rust
    fn preempt(&mut self, victim: usize, pl: usize) {
        …
        self.detach(victim);
        // unwind holds inner to `hi` (nested holds), then `hi` itself
        while self.sessions[victim].holds.len() > hi {
            let h = self.sessions[victim].holds.pop().unwrap();
            self.release_hold(victim, &h);
            // pop frames down to and including that hold's frame
            while let Some(f) = self.sessions[victim].frames.pop() {
                if f.kind == FrameKind::Hold && f.block == h.body {
                    break;
                }
            }
        }
```

```python
        request.status = RequestStatus.PREEMPTED
        request.num_computed_tokens = 0
```

**After** (스케치, 자동자 위에서)

```rust
// preemption: jump to `Scope.reentry` with the attribute vector intact.
// A run's position counter lives in the vector (slot `done` of the innermost run),
// so the program can say what vLLM does:
//   prefill (prompt + done - c) growing kv;   // recompute the KV of every token it has
//   decode  (o - 1 - done)      growing kv;   // generate only what is left
```

지금 IR에서도 run의 위치를 세션 속성으로 노출하면 같은 프로그램을 쓸 수 있습니다. 다만 프레임 되감기 규칙 옆에 "이 속성은 되감지 않는다"는 예외가 하나 더 붙습니다. 자동자에서는 예외가 아니라 정의입니다.

오라클에 이 경로를 넣는 시나리오 하나가 필요합니다. `preempt` 시나리오에서 `out`을 키워 decode 도중에 자리가 모자라게 하면 됩니다. 이 RFC와 별개로 지금 인터프리터에 대한 버그 이슈 대상입니다.

**자기 리뷰에서 확인한 것.** 건전성의 근거는 "프로시저가 없다"입니다. 열린 hold의 목록은 `pc`를 감싸는 스코프에서 정적으로 읽히므로 상태 표 옆에 스코프 트리가 필요합니다. `End`는 어느 깊이에서든 모든 스코프를 닫는 전이입니다.

**Before** (`src/ir.rs:269`)

```rust
    pub blocks: Vec<Vec<CStmt>>,
```

**After** (스케치)

```rust
pub struct Program { …, pub states: Vec<State>, pub scopes: Vec<Scope>, pub entry: StateId, … }
pub struct State { pub scope: ScopeId, pub kind: StateKind, pub next: StateId }
pub enum StateKind { Turn, Acquire(HoldSpec), Run(RunSpec), Set(..), Observe(..), Branch(CExpr, StateId), Choose(..), Exit(ScopeId), End }
pub struct Scope { pub parent: Option<ScopeId>, pub hold: Option<HoldSpec>, pub reentry: StateId /* preemption target */ }
```

## 6. Step coalescing

그림책: [똑같은 장은 한 번에 넘겨요](https://claude.ai/artifact/McVWxCUB9LZzq41EQuHTwj)

상주 구성이 바뀌지 않는 동안 iteration은 결정적으로 반복됩니다. 다음 사건까지의 iteration 수 `k`는 폐형식으로 계산되고, `k`번을 한 번에 적용합니다. §9가 병목이라고 적은 유체 예산의 5000 iteration/초가 여기서 줄어듭니다.

허가 조건은 정적이고 1번의 허용표에서 검사됩니다. `cost`와 `budget`이 step 모양에만 의존하고 `now`나 `est_*`, `price`를 읽지 않으며, iteration마다 선형으로 변하는 변수(`kvb`는 `ndec`씩)에 대해 조각별 아핀이어야 합니다. 그러면 `k`번의 비용 합이 산술급수로 닫힙니다. 조건을 만족하지 않는 스테이지는 지금처럼 한 번씩 돕니다.

**자기 리뷰에서 낮춘 것.** 대기 큐가 비지 않고 예산이 남는 동안은 매 iteration에 admission이 가능하므로 `k = 1`입니다. vLLM이 무너진 구간에서는 이득이 없고, 큐가 대체로 비는 논문의 유체 replica에서 이득이 큽니다. 성장의 블록 경계는 자유 블록이 `k`번 분량 이상이면 건너뛸 수 있고, 아니면 다음 경계가 사건입니다. 그리고 5번 위에서만 정확합니다. 상주의 상태가 `pc`와 위치 카운터뿐이어야 `k`번 뒤의 상태를 닫힌 식으로 쓸 수 있습니다.

## 7. 진행 검사와 deadlock 정리

시뮬레이션은 지금 인스턴스 하나의 deadlock을 드러내지만 감지하지 않습니다. `docs/language.md` §7이 "1 000 블록에서 두 엔진이 같은 스텝에 deadlock"이라고 적은 것이 그 예입니다. self-preemption은 grow 실패 → 자기 자신 희생 → 재큐 → 재admission → grow 실패의 livelock이고, iteration은 계속 돌아 시간이 가므로 horizon까지 조용히 갑니다. 보고서에 줄이 없습니다.

**싼 쪽 (IR 변경 없음).** 진행 없이 두 번 preempt된 세션을 보고서에 `stuck`으로 적고 `seq-lang check`가 실패합니다. "진행 없이"는 5의 위치 카운터가 같다는 뜻이라 5 위에서 정확하고, 지금 IR에서는 재admission 시각 사이에 토큰이 0이라는 근사로 씁니다.

**비싼 쪽 (5 위에서만 쓸 수 있는 문장).** 목표 정리:

> `preempt lifo`와 `reserve` 아래에서, 모든 세션에 `cap ≥ prompt + max_out`이면 admission된 모든 세션은 완료된다.

논증은 admission 순서 귀납입니다. 가장 먼저 admission된 holder는 다른 holder가 있는 동안 희생자가 아니고, 혼자일 때는 가설로 들어맞습니다. vLLM이 시작 시 `max_model_len ≤ blocks·bs`를 단언하는 것과 같은 논증입니다. 세션 수준 의미론이 Lean 관계가 아니면(§9 한계 4) 이 문장을 쓸 수 없고, 가설의 `max_out`은 1의 "선언된 상한"입니다. 뽑힌 `o ~ exp(200)`은 무계라 상한 없이는 정리가 거짓입니다.

유한 인스턴스에서 도착 순서를 비결정으로 두고 상태 공간을 다 도는 검사(P, TLA+ 식)도 자동자 위에서 가능하지만, 이 RFC의 범위 밖입니다.

**서브에이전트가 가설을 하나 더 요구합니다.** 부모 세션이 유한 풀의 단위를 든 채 자식 세션을 기다리면(`Join`), 가장 먼저 admission된 holder가 진행한다는 귀납이 깨집니다. 정리에 "유한 풀을 든 채 join하지 않는다"가 들어가고, 링커가 `Hold` 안의 `Join`을 거부하면 그 가설은 구조가 됩니다. `Spawn`/`Join` 자체는 `docs/design/subagents.md`에 있고 v4b의 항목입니다.

## 값과 비용

| 개념 | 없애는 부류 | Lean | 시뮬레이터 | 비용 |
|---|---|---|---|---|
| 1 순간 (위치 + 허용표 + admission 블록) | 평가 시점 | 문맥 변수의 UB가 사라짐 | 순수 식 캐시, 6의 허가 | 노드 하나, 생성기 수정 |
| 2 단위 소유 | 캐시 수명 | 등식 보존 법칙 (확인 필요) | 매 사건 단언 | 풀 코드 재작성 |
| 3 선언된 순서 | 순서 | 순서가 트레이스의 함수 | 없음 | 작음 |
| 4 정수 틱 + 초조밀 | 같은 순간 | ℕ 시계와 같은 종류 | 플랫폼 차이 소멸 | 시간 타입 교체 |
| 5 자동자 | 복구 (preemption 재개) | 세션 의미론 형식화 가능, 7의 정리를 쓸 수 있음 | 프레임 없음, 상태 탐색 | IR 전면 개편 |
| 6 coalescing | 없음 | 없음 | 구간에 따라 한두 자릿수 | 5 위에서만 |

## 단계

**v4a** (싼 것, 의미 변경이지만 모양 변경은 작음): 3, 4, 1의 허용표와 admission 블록과 `hidden`, 2의 `on_end` move, 7의 `stuck` 보고. 5의 preemption 복구 버그는 v4a 전에 지금 인터프리터에서 run 위치를 속성으로 노출해 고치고 오라클 시나리오를 하나 추가합니다. #21의 "같은 모양 다른 의미 → 범프 + 릴리스 노트"가 여기 적용됩니다.

**v4b** (재설계): 5, 그 위에 6, 그리고 7의 정리. 2의 등식은 Lean 쪽 확인 뒤 v4a나 v4b 어느 쪽에든.

어느 단계든 `gen_seq_oracle.py`는 마이그레이션이 아니라 재작성이고 `tools/oracle/*.ir.json` 일곱 개는 재생성입니다. v4b는 `SeqExec.lean`이 자동자 형태를 읽어야 합니다. 그 대가로 사는 것은 위 표의 열둘이 다음 프로그램에서 부류째 불가능해지는 것, 그리고 오라클이 한 번도 지나지 않은 preemption 복구가 정의에 의해 맞게 되는 것입니다.

프론트엔드(모델/인스턴스 분리, trait 어휘, 단위 라벨)는 이 RFC와 독립이고 별도 이슈로 냅니다. 이 RFC는 IR만 다룹니다.

## 그림책

1. [볼 때는 그 자리에서](https://claude.ai/artifact/WMxGr1RvWeiWPzSVLybE7w) — 순간
2. [블록은 언제나 한 상자에](https://claude.ai/artifact/GvY7xFGJ7PgaCjpkA9h7pp) — 소유
3. [번호표가 정해요](https://claude.ai/artifact/E4eZ7WnErxSYcJMnh5UfJ1) — 순서
4. [3시에도 첫째 둘째가 있어요](https://claude.ai/artifact/Ay3Eqd9PXbExfTp2ovXjnK) — 시각
5. [말은 언제나 한 칸에](https://claude.ai/artifact/MoULVkKwvVqvypSS11oAHq) — 자동자
6. [똑같은 장은 한 번에 넘겨요](https://claude.ai/artifact/McVWxCUB9LZzq41EQuHTwj) — 합치기

