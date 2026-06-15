# 설계: 추세필터 변동성돌파 전략 + 정직한 OOS 검증 인프라

> 작성일: 2026-06-15 · 대상 크레이트: `crates/strategy`, `crates/api` · 엔진/라이브 런타임 불변

## 0. 배경 — 왜 이 작업인가 (지난 세션의 정직한 실패)

직전 세션은 순수 롤링윈도우 변동성돌파(`VolatilityBreakoutStrategy`, lookback/k만 튜닝)를
**워크포워드 + OOS로 정직하게** 검증했고 결과는 **OOS 평균 +0.00%**(수수료 반영 전).
IS(과거)에서 최고였던 파라미터가 OOS(미래)에서 거의 매번 무너졌고, 최적 파라미터가
윈도우마다 계속 바뀌었다 = 노이즈, 강건한 엣지 없음. 단순 6개월 백테스트는 +1.78%로
좋아 보였지만 추세국면 1회가 캐리한 과적합/행운이었다.

**교훈(재발 금지)**: "백테스트 수익 좋아질 때까지 파라미터 돌리기" = 과최적화 정의 그대로.
**IS 숫자 믿지 말 것. OOS 숫자로만 판정.**

## 1. 목표 / 성공 기준 (goal-driven)

사용자 목표: **"실거래 할 수 있는 전략"**. 단 "완벽"=무한튜닝이 아님.
검증 가능한 합격 기준 = **워크포워드 OOS에서 수수료/슬리피지 반영 후 평균 양(+) ·
여러 윈도우에서 일관**.

1. `cargo test --workspace` → 기존 109 + 신규 전략테스트(~6) = **115+ 통과 / 0 실패** (병렬 OK).
2. `cargo fmt --all -- --check` clean, `cargo clippy --workspace --all-targets` 신규 warning 0.
3. 워크포워드 하니스 실행 → **수수료 반영 OOS 결과** 산출, PROGRESS.md 기록.
4. **합격 시**: 라이브 런타임 2곳 전략 1줄 교체 후 사용자에게 testnet 드라이런 여부 질문.
   **불합격 시**: 정직하게 보고, 다음 가설 논의. **"OOS 좋아질 때까지 그리드 확장" 금지.**

## 2. 가설 (이 설계가 추세필터인 이유)

순수 돌파의 실패 원인 = whipsaw(터졌다 되돌아옴, 돌파 적중률 <50%). 가설:
**돌파 방향과 장기추세 방향이 같을 때만 진입하면 whipsaw 대부분이 제거되어 OOS 엣지가
생긴다.** 보장은 없다 — OOS+수수료로만 판정한다(아래 §5).

## 3. Component A — 추세필터 전략 (`crates/strategy/src/lib.rs`)

새 전략 `TrendFilteredBreakoutStrategy`. 기존 `VolatilityBreakoutStrategy`와
`TechnicalStrategy`는 **그대로 보존**(복귀 가능).

```rust
pub struct TrendFilteredBreakoutStrategy {
    lookback: usize,   // 레인지 참조 봉 수.   기본 20
    k: f64,            // 변동성 계수.         기본 0.5
    ma_period: usize,  // 장기추세 MA 봉 수.   기본 50
}

impl Default for TrendFilteredBreakoutStrategy {
    fn default() -> Self { Self { lookback: 20, k: 0.5, ma_period: 50 } }
}
```

`evaluate(candles)`:

```
1. need = max(lookback + 1, ma_period)
   candles.len() < need → vec![]                  (워밍업)
2. 기존 돌파 로직(window_high_low + open±range*k vs close)으로 후보 방향 산출.
   range <= 0 → vec![] (기존과 동일)
3. ma = SMA(close, ma_period) over 직전 ma_period 봉 (latest 포함)
4. 추세 게이트:
   - Buy 후보  AND  latest.close > ma  → Buy 통과
   - Sell 후보 AND  latest.close < ma  → Sell 통과
   - 그 외 → vec![]   (추세 역행 돌파 = whipsaw로 간주, 차단)
5. score = score_from_distance(돌파 초과폭)  (기존 헬퍼 재사용)
```

- **재사용(중복 금지)**: `window_high_low`, `build_signal`, `score_from_distance`.
  신규 헬퍼는 `simple_moving_average(closes: &[f64], period) -> Option<f64>` 하나만.
- **파라미터 생성자**: `new(lookback, k, ma_period)` + `.lookback()`/`.k()`/`.ma_period()`
  게터(워크포워드 그리드 주입용, 기존 `VolatilityBreakoutStrategy` 패턴 동일).
- **MA 기준선 = close vs SMA(close)**. 종가가 SMA 위 = 상승추세로 간주. 단순·재현가능.

### 버퍼 제약 (엔진 불변 유지)
라이브/테스트넷 버퍼 = 최대 100 1m 캔들(`PAPER_MAX_CANDLES_PER_KEY=100`). 백테스트
러너 = `BACKTEST_HISTORY_LIMIT=64`. 따라서 **ma_period ≤ 50 권장**(lookback 20 + ma 50
모두 64/100 안에 들어감). 그리드의 ma_period 후보는 {20, 50}으로 제한.

## 4. Component B — 수수료/슬리피지 모델 (`crates/api/src/backtest_runner.rs`)

백테스트 평가 전용. **라이브 PnL 계산은 건드리지 않는다.**

```rust
const BACKTEST_TAKER_FEE: f64 = 0.0004;     // 0.04% 회당 (Binance 선물 taker 근접)
const BACKTEST_SLIPPAGE_PCT: f64 = 0.0001;  // 0.01% 회당 (체결가 불리 방향, 보수적)
```

거래 1건(진입+청산)당 비용:

```
cost = (entry_notional + exit_notional) * BACKTEST_TAKER_FEE
     + (entry_notional + exit_notional) * BACKTEST_SLIPPAGE_PCT
  where  entry_notional = entry_price * quantity
         exit_notional  = exit_price  * quantity
```

- `close_positions`에서 `pnl` 집계 직후 `pnl -= cost`로 차감(거래당 1회). 미청산 포지션을
  마지막 캔들 mark로 닫는 tail 루프(`run_backtest` 내)도 동일하게 비용 차감(현실 정합).
- 비용은 `Decimal`로 계산(가격/수량이 Decimal). 상수는 `Decimal::from_f64_retain` 또는
  스케일 정수(`Decimal::new(4, 4)`=0.0004)로 정확히 표현.
- **승/패 판정은 비용 차감 후 PnL 기준**(실거래 정합 — 수수료 떼고 나서 이익이어야 win).

## 5. Component C — 워크포워드 하니스 (영구 통합테스트, `crates/api/tests/walk_forward.rs`)

지난 세션은 임시 `#[ignore]` 테스트로 돌리고 **삭제**했음 → 이번엔 **영구 박제**.
`#[ignore]` 부착 → 일반 `cargo test`엔 안 걸리고, 명시 실행(`cargo test --test
walk_forward -- --ignored --nocapture`)으로만 돈다. DB(`DATABASE_URL`/`TEST_DATABASE_URL`)
필요.

```
윈도우: 4개 (2024-06 ~ 2026-06), 각 = IS 4개월(그리드) → OOS 2개월(검증), rolling.
그리드: lookback ∈ {10,20,30,50} × k ∈ {0.3,0.5,0.7} × ma_period ∈ {20,50}  = 24조합
심볼: BTCUSDT + ETHUSDT (합산)
각 조합: 비용 반영 run_backtest → realized_pnl(비용후).
윈도우별: IS에서 최고 조합 1개 선택 → 그 파라미터로 OOS 측정.
출력(--nocapture):
  - 윈도우별: IS-best 파라미터, IS PnL, OOS PnL(비용후)
  - 전체 IS 그리드(plateau vs spike 구분용 — 단일 spike면 과적합 의심)
  - OOS 평균 PnL(비용후)
```

**판정 규칙(코드가 강제하지 않고 사람이 읽고 결정)**:
- OOS 평균 + (비용후) AND 4윈도우 중 다수(≥3) + → **합격 후보** → 라이브 교체 검토.
- OOS 평균 ≤ 0 또는 윈도우 절반 이상 − → **불합격** → 정직하게 보고, 그리드 확장 금지.

⚠️ **금지**: OOS 숫자 보고 그리드(파라미터 후보)를 늘려 다시 돌리는 것 = 과최적화 재발.
그리드는 이 spec에 고정. 불합격이면 새 가설(별도 brainstorming)로.

## 6. 런타임 통합 (검증 합격 시에만, 엔진 불변 2줄)

| 위치 | 변경 |
|---|---|
| `crates/api/src/testnet_runtime.rs:71` | `VolatilityBreakoutStrategy::default()` → `TrendFilteredBreakoutStrategy::default()` |
| `crates/api/src/strategy_runtime.rs:65` | 동일 교체 |
| `crates/api/src/backtest_runner.rs:129` | 백테스트도 새 전략 인스턴스화(워크포워드는 `new(...)` 직접) |

- 시그널→주문 흐름 100% 불변: 방향만 전략이 결정, 리스크게이트·시장주문·SL/TP 보호주문은
  엔진이 그대로 처리 → 돈 실수 여지 없음.
- `strategy_version()` → `"trend_filtered_breakout_v1"` 갱신(백테스트 기록 메타 정합).
- **불합격 시 교체 안 함** — 현 `VolatilityBreakoutStrategy::default()` 유지.

## 7. 테스트 (TDD, lib.rs 하단 `#[cfg(test)]`, RED 먼저)

기존 `candle_hlc`/`breakout_window` 헬퍼 재사용 + 필요 시 MA를 흔들 헬퍼 추가.

1. `trend_filter_does_not_signal_with_insufficient_history` — ma_period 미만 → vec![].
2. `trend_filter_allows_long_above_ma` — 상승 돌파 + close > MA → Buy 1개.
3. `trend_filter_blocks_long_below_ma` — 상승 돌파지만 close < MA → vec![] (whipsaw 차단).
4. `trend_filter_allows_short_below_ma` — 하락 돌파 + close < MA → Sell 1개.
5. `trend_filter_blocks_short_above_ma` — 하락 돌파지만 close > MA → vec![].
6. `trend_filter_no_signal_inside_band` — 돌파 자체가 없으면 → vec![].
- 백테스트 비용 모델: `close_positions` 비용 차감 유닛테스트(거래 1건 비용 정확도).

## 8. 알려진 한계 / 후속

- 파라미터 하드코딩(설정화는 후속). `evaluate()`는 캔들만 받음(오더북/펀딩비 불가).
- 최신 캔들만 판단(포지션 상태 모름, SL/TP가 청산). 단일 심볼 독립 평가.
- 백테스트 비용 모델은 **고정 bps 근사** — 실제 펀딩비/체결깊이별 슬리피지는 미반영(보수적
  상수로 갈음). 라이브 검증(testnet)으로 보완.
- 워크포워드 한계: `BACKTEST_HISTORY_LIMIT=64` 롤링윈도우(라이브 동일 제약). 정통 일봉 불가.

## 9. 범위 밖 (이번에 안 함)

- 엔진/버퍼/리스크/보호주문/라이브 PnL 계산 변경.
- 파라미터 설정화, AI 필터(룰 기반 유지 — 사용자 합의).
- 그리드 확장(과최적화 방지 — §5 고정).
- 평균회귀/페어트레이딩 등 다른 가설(불합격 시 별도 brainstorming).
