# 설계: 오더북 imbalance 백테스터 (SQL-우선, Phase 1)

> 작성: 2026-06-18. 사전등록 분석(`2026-06-16-orderbook-imbalance-preregistration.md`) §8 "신호가 살아남으면 — 다음 단계"의 1단계 구현.

## 0. 배경 — 왜 지금 이 백테스터인가

오더북 top-of-book imbalance 사전등록 분석이 **신호 존재를 확인**했다(2026-06-18 실행,
게이트 18/18 ready). 핵심 결과:

- **10s horizon: 6/6 피드 전부 `signal?`** (Spearman IC 0.125~0.227, Fisher-z 16~31, 본페로니 |z|>3.20)
- **30s horizon: 6/6 피드 전부 `signal?`** (IC 0.074~0.158)
- 60s: 일부 signal?, 일부 INCONCLUSIVE(비중복표본<2000), bybit ETH 1개 no-signal
- **부호 위반(SIGN VIOLATION) 0건** — 전 피드가 가설 방향(+)과 일치
- 분위수 spread는 전 피드 양수(+0.69~+2.14 bps), 10s BTC는 monotone+
- IC가 horizon 길수록 단조 감소(10s≫30s>60s) = 마이크로구조 신호의 전형

신호 연구는 **예측력 존재 여부**까지만 판정한다(사전등록 §9). 실거래 수익(수수료·슬리피지
차감 후)은 별개다. spread가 **~1 bps 수준**이라 거래비용 장벽이 결정적 — 그걸 정직하게
재는 것이 이 백테스터의 목적이다.

## 1. Scope (확정된 결정)

브레인스토밍에서 사용자가 확정한 범위:

1. **오더북 백테스터 신규 구축** — 기존 캔들 백테스터(`run_backtest`, `Strategy::evaluate(candles)`)는
   캔들 전용이라 1초 오더북 신호에 못 쓴다. 별개 인프라를 만든다.
2. **지금은 백테스터 구축·검증만** — `order_books`가 아직 2일치뿐이라 정직한 WFO+OOS(가격방향
   패밀리가 한 IS12mo→OOS6mo 다윈도우)는 **불가능**. 2일치로 smoke/산의성 검증까지. 정직한
   WFO+OOS 판정은 **몇 주 적재 후** 같은 인프라로 실행.
3. **고정 H초 홀딩 청산** — 신호가 예측한 바로 그 horizon(10s/30s/60s)만큼 들고 청산. 사전등록
   IC/분위수와 100% 대칭(t진입 → t+H청산). TP/SL 변형은 사전등록 범위 밖 → 후속.
4. **SQL-우선** — 통계 진실의 단일 출처는 SQL. Rust는 얇은 러너.
5. **TDD + 적대적 리뷰** — 사전등록 §8.1대로 가격방향 패밀리와 동일 규율.

## 2. 아키텍처 & 컴포넌트

```
deploy/analysis/
  imbalance_backtest.sql   ← 신규: 진입 → H초 홀딩 청산 → gross PnL → 비용차감 → 3진판정
crates/api/src/.../backtest_runner.rs (테스트 모듈)
  imbalance_backtest_smoke ← 신규 #[ignore] 통합테스트: SQL 실행·결과 출력 (DATABASE_URL 필요)
```

**책임 경계:**

- **SQL = 통계 진실의 단일 출처.** 사전등록 규율을 `imbalance_fast.sql`에서 그대로 상속:
  mid-price 수익률(close 아님), 비중복표본(epoch_second % H = 0), 피드별 독립(풀링 금지),
  forward = `[t+H, t+H+2s)` 첫 행(무보간), floor 2000. **인덱스 temp 테이블 plan**
  (`_ticks` + `(exchange,symbol,ts) INCLUDE (mid)` 인덱스)로 forward-match를 인덱스 시크화 —
  순수 LATERAL-over-CTE는 1M행에서 21분+ 미완이었고, temp+인덱스는 ~0.6s.
- **Rust 러너 = 얇은 `#[ignore]` 통합테스트.** 캐처 DB에 SQL을 실행하고 결과를 파싱·요약 출력.
  **머니패스/엔진/`Strategy` trait/`run_backtest`/`BacktestConfig`/HTTP API는 byte 불변** —
  이번 scope는 신호연구 백테스트뿐, 라이브 진입 경로 변경 아님.

**방향성 정의:** `imbalance = bid_size/(bid_size+ask_size)`.
- imbalance > 0.5 → **롱** 신호, imbalance < 0.5 → **숏** 신호.
- `signed_return = sign(imbalance − 0.5) × fwd_return` = 신호 방향대로 진입했을 때 gross 수익.
- 사전등록 가설(높은 imbalance → 양의 forward return)과 정합.

## 3. 비용 모델 & 판정 기준

**비용 모델 (사전등록 §8.2 "수수료 1×/2×/3×"와 정합):**

- 1회 왕복(진입+청산 = 2 fill) 비용:
  `roundtrip_cost_bps = 2 × (taker_fee_bps + half_spread_bps)`
- **taker_fee_bps = 5 bps/편도** (보수적; 3거래소 taker 통상 ~5.5bps보다 약간 낮은 보수값).
  왕복 수수료만 10 bps.
- **half_spread_bps = 진입 시점 실측** — `(best_ask − best_bid)/2 / mid × 10000`. top-of-book에
  있으므로 **가정 대신 측정**. 피드×horizon별 평균 half_spread를 비용에 반영.
- 시나리오: **1× = (실측 spread + 5bps 수수료)**, 2× = ×2, 3× = ×3 (스트레스 테스트).
  가격방향 패밀리와 동일한 1×/2×/3× 프레임.

**순수익:** `net_return_bps = signed_return_bps − roundtrip_cost_bps`. 피드×horizon별 평균 net +
승률(net>0 비율) + 비중복표본수 + 평균 half_spread.

**3진 판정 (사전등록 §5 스타일, net 기준):**

- `PASS` = 1× 비용에서 mean net > 0 **AND** 여러 피드 일관 **AND** 비중복표본 ≥ 2000
- `MARGINAL` = 1×만 양수, 2×에서 죽음 (→ 사전등록 §8.3대로 새 홀드아웃 후에야 testnet)
- `FAIL` = 1×에서도 net ≤ 0
- **현재(2일치)는 대부분 `INCONCLUSIVE (single-regime / no-OOS)`** 로 라벨링. smoke 단계임을
  판정문에 명시 — 정직한 PASS/FAIL은 적재 후 WFO 통과해야.

## 4. 테스트 & 검증 (TDD + 적대적 리뷰)

**SQL 검증 (로컬 throwaway PG, 합성데이터 — IC SQL 검증과 동일 방식):**

1. 진입방향 부호: imbalance>0.5 & 가격↑ → net 양수 / imbalance<0.5 & 가격↓ → net 양수(숏 수익)
2. 비용차감: cost=0이면 net=gross; cost↑면 net 단조↓ (1×→2×→3× 감소)
3. spread 실측: 합성 best_bid/ask에 알려진 spread 주입 → half_spread 정확 추출
4. 무신호 데이터(imbalance~0.5 무작위) → net~0, PASS 안 나옴

**적대적 리뷰 체크리스트 (사전등록 §4 함정 + 백테스트 특유):**

- **look-ahead 0**: 청산 mid는 t+H의 미래 행이지만 **진입 결정엔 안 쓰임**(진입은 t의 imbalance만).
  미래 데이터로 진입을 고르지 않는다.
- **bid-ask bounce**: mid 사용(close 아님) — `imbalance_fast.sql`에서 상속.
- **비중복표본**: epoch_second % H = 0 — 상속. overlap은 유의성 과대.
- **부호 일관성 교차검증**: 같은 표본에서 `avg(sign(imbalance−0.5) × fwd_return)`이 IC 부호와
  같은 방향(양수)인지 확인. signed_return이 IC와 어긋나면 버그.
- **무보간**: forward 행 없으면 표본 드롭(데이터 날조 금지) — 상속.

**Rust 러너:** `#[ignore]` 통합테스트 1개. 캐처 DB에 SQL 실행 → 결과 출력. 머니패스/엔진/
`Strategy` trait **전부 byte 불변**. `cargo build`·`clippy`·`fmt` clean, 기존 테스트 회귀 0.

## 5. 성공 기준 (이번 scope)

1. SQL이 합성데이터 4브랜치 전부 통과 → **verify**: 로컬 throwaway PG 실측 출력
2. 캐처 DB(2일치)에 실행돼 18피드×3horizon 결과 출력 → **verify**: net_bps·승률·평균 half_spread·
   1×/2×/3× 테이블, INCONCLUSIVE 라벨
3. 기존 테스트 회귀 0, 머니패스 불변 → **verify**: `cargo test` 통과 + `git diff`로 변경 라인이
   전부 신규 SQL/신규 테스트에만 국한됨을 추적

## 6. 한계 (정직)

- **2일치 단일 레짐** — net이 양수여도 일반화 보장 없음. PASS/FAIL 판정은 적재 후 WFO 필수.
- **고정 H초 홀딩만** — 최적 출구가 아닐 수 있음. TP/SL·동적출구는 범위 밖(후속).
- **체결 가정**: 진입·청산 모두 mid에서 즉시 체결로 가정(슬리피지·체결지연·부분체결 미모델).
  실거래는 taker로 spread를 추가로 먹음 → 비용 모델의 half_spread가 이를 근사하나 보수적 재검토 필요.
- **L1 only** — top-of-book 1레벨 imbalance만(L2~L10 depth 아님). 사전등록 §9 상속.
- **family-wise 규율**: 백테스트 변형(출구·임계·필터)을 무한정 돌리면 거짓양성. 신호연구가 죽으면 STOP.

## 7. 비-목표 (이번 scope 명시 제외)

- 라이브/testnet 진입 (사전등록 §8.3 — WFO 통과 + 새 홀드아웃 후에야)
- `Strategy` trait 확장이나 엔진 통합 (신호가 백테스트에서 살아남은 뒤 별도 RED→GREEN)
- TP/SL 등 대안 청산 모델
- L2 depth 캐처/분석
