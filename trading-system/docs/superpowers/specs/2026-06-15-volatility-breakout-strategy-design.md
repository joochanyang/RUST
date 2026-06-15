# 설계: 양방향 롤링윈도우 변동성 돌파 전략 (`VolatilityBreakoutStrategy`)

> 작성일: 2026-06-15 · 대상 크레이트: `crates/strategy` · 엔진 불변

## 1. 목표 / 배경

`crates/strategy/src/lib.rs`에 두 번째 전략 `VolatilityBreakoutStrategy`를 추가한다.
현재 유일한 전략은 `TechnicalStrategy`(RSI+볼린저 역추세) 하나뿐이다. 트레이딩
엔진(리스크 게이트 / 시장주문 / SL·TP 보호주문)은 **전혀 손대지 않는다** —
이번 작업의 산출물은 `Strategy::evaluate()` 한 메서드의 새 구현과, 두 런타임의
전략 인스턴스 교체(2줄)뿐이다.

### 성공 기준 (goal-driven)
1. `cargo test --workspace` → 기존 103 + 신규 6 = **109 통과 / 0 실패** (병렬 OK).
2. `cargo fmt --all -- --check` clean, `cargo clippy --workspace --all-targets` 신규 warning 0.
3. 런타임 교체 후 `cargo build -p trading-api` 성공.
4. 6개월 백테스트(BTC·ETH, 이미 DB에 적재된 binance 1m 캔들)에서 trade 수 > 0, 수익률/MDD 산출.

## 2. 데이터 분석 근거 (이 설계가 양방향인 이유)

구현 전 DB의 binance 1m 캔들(2023-06-13 ~ 2026-06-15, 각 ~158만 개)로 최근 시장
성격을 측정했다(UTC 일봉 집계):

| 구간 | 심볼 | 수익률 | 일평균 레인지 | 자기상관(lag-1) |
|---|---|---|---|---|
| 1개월 | BTC / ETH | -19.1% / -24.6% | 3.24% / 4.45% | +0.28 / +0.09 |
| 3개월 | BTC / ETH | -7.5% / -17.7% | 3.21% / 4.34% | +0.11 / +0.04 |
| 6개월 | BTC / ETH | -28.7% / -48.2% | 3.64% / 4.95% | -0.07 / -0.00 |

변동성 돌파(k=0.5) 사후 적중률(돌파 후 종가가 타겟 위 마감): BTC 47.2%, ETH 42.9%.

**해석**: 최근 6개월은 강한 하락장 + 모멘텀 약함(자기상관≈0) + 돌파 적중률<50%
(whipsaw). 롱 전용 변동성 돌파는 이 환경에서 엣지가 없다. 따라서 **양방향(롱+숏)**
으로 구현해 하락 돌파도 포착한다. (정통 일봉 래리 윌리엄스는 라이브 버퍼 100캔들
제약상 불가 — 아래 §5.)

## 3. 핵심 로직

`evaluate(&self, candles: &[Candle]) -> Vec<Signal>`:

```
입력: candles (최대 100 × 1m, 시간 오름차순)
파라미터: lookback (기본 20), k (기본 0.5)

1. candles.len() < lookback + 1  → return vec![]   (버퍼 워밍, ~21분)
2. latest  = candles.last()                          (방금 닫힌 돌파 후보 봉)
3. window  = candles[len-1-lookback .. len-1]        (latest 제외한 직전 N봉)
4. range   = max(window.high) - min(window.low)      (Decimal→f64)
5. range == 0  → return vec![]                       (평탄/무의미 돌파 방지)
6. offset       = range * k
   long_target  = latest.open + offset
   short_target = latest.open - offset
7. latest.close >= long_target  → Buy  신호
   latest.close <= short_target → Sell 신호
   둘 다 아니면 → vec![]
   (offset >= 0 이므로 long_target >= short_target, 동시 충족 불가)
8. score = score_from_distance(돌파 초과폭)            (기존 헬퍼 재사용)
```

- **돌파 판정 = 종가(close)**. 평가는 1m 롤오버(새 open_time)마다 1회 → `latest`는
  방금 닫힌 봉. 종가 돌파 = 확정 돌파 → 노이즈↓·백테스트 재현성↑. (고가 기준은 봉
  중간 일시 터치까지 잡아 whipsaw↑ → 채택 안 함.)
- **window는 latest 제외**. latest를 range에 넣으면 자기 자신 돌파라 논리가 깨진다.

## 4. 파라미터 / 구조

```rust
pub struct VolatilityBreakoutStrategy {
    lookback: usize,   // 레인지 참조 봉 수.  기본 20
    k: f64,            // 변동성 계수.        기본 0.5 (래리 윌리엄스 정석)
}

impl Default for VolatilityBreakoutStrategy {
    fn default() -> Self { Self { lookback: 20, k: 0.5 } }
}

impl Strategy for VolatilityBreakoutStrategy {
    fn name(&self) -> &'static str { "volatility_breakout" }
    fn evaluate(&self, candles: &[Candle]) -> Vec<Signal> { /* §3 */ }
}
```

- 파라미터는 `Default`에 하드코딩(현 `TechnicalStrategy` 패턴 동일). 설정화(env/config)는
  후속 과제 — PROGRESS.md §3 "파라미터 하드코딩" 한계에 이미 기록됨.
- 기존 헬퍼 **재사용**(중복 구현 금지): `build_signal`, `score_from_distance`.
  high/low f64 변환은 `closes_as_f64`와 동일하게 `ToPrimitive::to_f64`, 실패 시 무신호.

## 5. 라이브 버퍼 제약 (정통 일봉 방식이 불가한 이유 — 박제)

라이브/테스트넷 런타임의 `CandleBuffers`는 per-symbol `VecDeque`로 **최대 100개
1m 캔들**만 보관한다(`PAPER_MAX_CANDLES_PER_KEY=100`, `testnet_runtime.rs` /
`strategy_runtime.rs`). 시작 시 DB 백필 없음 → 빈 상태에서 1캔들/분 누적.
따라서 "전일 전체(1440개)" 레인지는 **접근 불가**. 정통 래리 윌리엄스(전일 레인지 ×
당일 시가)는 엔진 수정(버퍼 캡 ↑ + 시작 백필) 없이는 불가능하므로, "엔진 불변"
제약 하에서 **직전 N봉 롤링 윈도우** 변형을 채택한다. lookback=20이면 21분 워밍 후
즉시 동작하고 100캔들 버퍼 안에 충분히 들어간다.

## 6. 런타임 통합 (엔진 불변, 2줄 교체)

| 위치 | 변경 |
|---|---|
| `crates/api/src/testnet_runtime.rs:71` | `TechnicalStrategy::default()` → `VolatilityBreakoutStrategy::default()` |
| `crates/api/src/strategy_runtime.rs:65` | 동일 교체 |

- `evaluate()` 호출부(`testnet_runtime.rs:239`, `strategy_runtime.rs:144`)와
  `backtest_runner.rs:158`은 trait 경유 → 코드 변경 없이 새 전략 사용.
- 시그널→주문 흐름 **100% 불변**: 방향만 전략이 결정, 리스크게이트·시장주문·
  SL/TP 보호주문(Algo API)은 엔진이 그대로 처리 → 돈 실수 여지 없음.
- ⚠️ `backtest_runner.rs`의 `BACKTEST_HISTORY_LIMIT = 64` → lookback 20이면
  21 ≤ 64 OK. **lookback은 ≤ 50 권장**(백테스트가 굶지 않도록).

## 7. 테스트 (TDD, lib.rs 하단 `#[cfg(test)]`)

기존 RSI 테스트 패턴(`candle(index, close)` 헬퍼) 재사용. high/low를 따로 줄 수
있도록 `candle_hlc` 헬퍼를 추가한다. RED 먼저:

1. `does_not_signal_with_insufficient_history` — lookback 미만 → vec![].
2. `signals_buy_on_upside_breakout` — 평탄 N봉(레인지 고정) 후 시가+range·k 초과
   종가 → Buy 1개.
3. `signals_sell_on_downside_breakout` — 대칭 → Sell 1개.
4. `no_signal_inside_band` — 종가가 밴드 안 → vec![].
5. `no_signal_on_zero_range` — N봉 동일가(range=0) → vec![].
6. `score_grows_with_breakout_distance` — 더 강한 돌파 → score 더 큼.

## 8. 백테스트 검증 (구현 후)

이미 적재된 binance 1m 캔들 사용(import 불필요). `POST /api/backtest-runs/run`
또는 `backtest_runner` 직접 경유로 최근 6개월(BTC·ETH) 리플레이 → trade 수,
수익률, `max_drawdown_pct` 산출. 결과를 PROGRESS.md에 기록. (양/음 무관 —
데이터상 엣지 검증이 목적.)

## 9. 알려진 한계 / 후속 (전략과 별개)

- 파라미터 하드코딩 → 튜닝하려면 설정화 필요.
- `evaluate()`는 캔들만 받음 → 오더북/펀딩비 사용하려면 trait 확장 필요.
- 최신 캔들만 보고 판단 → 포지션 보유상태/진입가 모름(SL/TP가 청산 담당).
- 단일 심볼 독립 평가 → 페어트레이딩 등 멀티심볼은 구조 확장 필요.

## 10. 범위 밖 (이번에 안 함)

- 엔진/버퍼/리스크/보호주문 변경.
- 파라미터 설정화, AI 필터(룰 기반 유지 — 사용자 합의).
- 정통 일봉 래리 윌리엄스(버퍼 제약상 불가).
- 추세필터(MA 등) — 별도 옵션으로 논의됐으나 이번 범위 아님.
