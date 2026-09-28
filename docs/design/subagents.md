# 서브에이전트: 세션이 세션을 만든다

2026-09-28의 검토. 질문은 "서브에이전트가 IR 설계에 고려되어 있는가"였고, 답은 **아니다**입니다. 그리고 그것은 tool의 변종이 아니라 새 종류의 것입니다.

## 지금 IR에 없는 것

세션은 환경만 만듭니다. `CArrival`은 `Poisson`, `Closed`, `Batch`, `Sessions` 넷이고, 인터프리터의 `spawn`은 그 도착 과정이 부르는 것뿐입니다(`interp.rs:591`). 세션 블록의 열한 문장 중 세션을 만들거나 다른 세션을 기다리는 것은 없습니다. `agentic.seq`의 tool 호출은 `stage tool : delay`, 즉 부하와 무관한 외생 지연입니다.

서브에이전트는 부모 에이전트가 같은 모델에 요청을 보내고 그 답을 기다리는 것입니다. 부모 입장에서는 tool 호출처럼 보입니다. 시스템 입장에서는 전혀 다릅니다.

| | tool | 서브에이전트 |
|---|---|---|
| 지연의 원인 | 바깥 세계, 부하와 무관 | **같은 엔진의 응답 시간**, 부하의 함수 |
| 도착 | 환경이 만든다 | **시스템 자신이 만든다** (내생 도착) |
| 프롬프트 | 부모 컨텍스트와 무관 | 부모 컨텍스트를 접두사로 가진다 (세션 간 캐시 공유) |
| 부모의 자원 | 없음 (`end` 뒤 캐시만) | 부모가 슬롯을 든 채 기다릴 수 있다 (hold-and-wait) |
| 통계의 단위 | 세션 | **세션 트리** |

## 왜 새로운가, 넷

**1. 부하가 내생적이다.** 도착률이 λ가 아니라 λ · (1 + E[자식 수] + E[손자 수] + …)이고, 자식 수가 부모의 출력에 달렸으니 분기 과정입니다. 총 자손 수의 기대가 유한해야 하고, 그 위에 용량 조건이 붙습니다. 응답 시간이 길어지면 부모가 더 오래 기다리고, 그 사이 다른 부모들이 자식을 더 만들어 놓습니다. Lecture 5의 절벽이 tool 지연을 통해서가 아니라 시스템 자신을 통해 닫힙니다. 큐잉 이론의 대상으로는 fork-join 큐에 분기가 붙은 것이고, 논문의 모델에는 없습니다.

**2. hold-and-wait 교착.** 부모가 유한 풀의 단위(라이브 세션 캡, `reqs` 슬롯)를 든 채 자식을 기다리고, 자식이 같은 풀을 필요로 하면 캡에서 교착입니다. [IR v4](ir-v4.md) §7의 정리는 "admission 순서 귀납"이라 가장 먼저 든 세션이 진행한다고 논증하는데, 그 세션이 자식을 기다리고 있으면 진행하지 않습니다. 정리에 가설이 하나 더 필요합니다. "유한 풀을 든 채 join하지 않는다" 또는 "join 중 든 단위는 자식이 선점할 수 있다". 링커가 검사할 수 있는 형태로는 첫째가 쉽습니다. `Hold` 안의 `Join`을 거부하거나, 그 풀이 무한이어야 합니다.

**3. 세션 간 캐시 공유가 필수가 된다.** 자식의 프롬프트는 부모의 컨텍스트로 시작합니다. 지금 캐시는 세션별이라(§9 한계 2) 자식은 항상 miss입니다. vLLM은 내용 주소라 hit입니다. 서브에이전트를 모델링하는 순간 이 한계가 결과를 결정합니다. [IR v4](ir-v4.md) §2가 자격자를 세션 번호가 아니라 키로 두는 이유가 여기서 현실이 됩니다.

**4. 정책이 트리를 읽어야 한다.** 부모가 기다리는 자식은 먼저 서빙할 가치가 있고(부모의 지연은 자식의 합), 깊은 트리는 자를 가치가 있습니다. 큐 키와 evict 키가 `depth`, `parent`, `siblings_left` 같은 트리 속성을 읽을 수 있어야 하고, 그것은 기준 2대로 프로그램의 식입니다.

## IR에 무엇이 들어가나

커널 문장 둘과 세션 속성 몇 개입니다.

```rust
    /// Create `count` sessions running `session` (default: the same block),
    /// each initialised by `init` in the parent's environment. The children
    /// carry `parent`, `depth`, and start ready at the current instant.
    Spawn { count: CExpr, init: BlockId, session: Option<BlockId>, into: usize /* attr: handle */ },
    /// Block until every child of `handle` has ended.
    Join { handle: usize },
```

- 자동자([IR v4](ir-v4.md) §5)에서 `Join`은 세 번째 막힘 지점이고, 깨우는 사건은 자식의 `End`입니다. 효과/핸들러 표에서는 `spawn`과 `join`이 워크로드의 효과입니다. 도착을 만드는 것이 워크로드니까요.
- 순간 규칙([IR v4](ir-v4.md) §1)은 그대로입니다. 자식의 `init`은 부모의 `@session`에서 평가된 스냅샷으로 시작합니다.
- 소유([IR v4](ir-v4.md) §2)에서 자식이 부모의 캐시를 빌리려면 자격 키가 내용 주소여야 합니다. 첫 단계는 "부모의 키를 물려받는다"는 규칙으로 충분합니다. 부모 프롬프트가 자식 프롬프트의 접두사이니까요.
- 통계는 트리 단위 관측이 필요합니다. `observe`에 루트 세션 번호를 함께 기록하면 밖에서 합칠 수 있습니다.
- 진행 검사([IR v4](ir-v4.md) §7)의 `stuck`은 join 대기 중인 세션의 자식이 전부 큐에 막혀 있고 큐가 진행하지 않는 상태를 포함해야 합니다.

## 프로그램은 어떻게 읽히나

```
session {
  turn;
  loop {
    enter reqs (1), kv (…) { prefill …; decode …; } keep (…);
    branch (delegate) {
      spawn (k) into kids { set n = subtask_prompt; set depth = depth + 1; }
      join kids;                     // the "tool call" whose time is the engine's own
    } else {
      branch with (p) { tool Z; } else { end; }
    }
    turn;
  }
}
```

`reqs`를 든 채 `join`하지 않도록 `enter` 스코프가 닫힌 뒤에 spawn하는 것이 위 프로그램의 요점이고, 링커가 그것을 검사할 수 있습니다.

## 비용과 순서

`Spawn`과 `Join`은 IR 노드 둘이라 handshake이고, Lean 조각은 세션 수를 고정으로 두고 있어(`Exec.run … n sessions`) 동적 생성을 받아들이려면 조각의 상태가 세션 리스트여야 합니다. 그건 자동자(v4b)와 같은 개편에 넣는 것이 맞습니다. 그 전에 할 수 있는 것은 셋입니다. 첫째, 지금 언어로 근사하기. 자식의 부하를 `arrive poisson`의 별도 세션으로 흘리고 부모의 `tool` 지연을 그 세션들의 측정 응답 시간으로 놓는 것인데, 피드백이 끊기므로 절벽을 놓칩니다. 둘째, `serving-queue-theory`에 분기 fork-join의 안정 조건을 손으로 쓴 모델로 먼저 세우기. 셋째, 트레이스에 트리 구조(부모 요청 id)를 넣어 replay가 그 순서를 재현하게 하기. 그래야 나중에 오라클이 있습니다.
