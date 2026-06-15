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

1. `cargo test --workspace` → 기존 109 + 신규 전략테스트(8) + 비용 유닛(1) = **118 통과 /
   0 실패** (병렬 OK).
2. `cargo fmt --all -- --check` clean, `cargo clippy --workspace --all-targets` 신규 warning 0.
3. 워크포워드 하니스 실행 → **수수료 반영 OOS 결과(평균·중앙값·최악·비용민감도)** 산출,
   PROGRESS.md 기록.
4. **이건 falsification 테스트다** (사용자 선택). 합격 기준은 §5의 강한 게이트
   (전 4윈도우 양수 AND 평균 > 2× 왕복비용 AND 비용2×에도 양수). **합격 시**: 라이브 3곳
   전략 교체 후 testnet 드라이런 여부 질문. **불합격(~0% 예상) 시**: "롤링 돌파 계열 엣지
   없음" 깔끔히 결론·STOP. **그리드 확장 절대 금지**(사전등록 — §5).

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
3. ma = simple_moving_average(closes, ma_period)
   - 슬라이스 = closes[closes.len() - ma_period ..]  (가장 최근 ma_period개, latest 포함)
   - None(변환 실패/슬라이스 부족) → vec![]  (기존 None 관례: 모든 None은 무신호)
4. 추세 게이트 (경계 명시 — 엄격 부등호):
   - Buy 후보  AND  latest.close >  ma  → Buy 통과
   - Sell 후보 AND  latest.close <  ma  → Sell 통과
   - latest.close == ma (정확히 같음) → vec![]  (방향 미정 → 진입 안 함)
   - 그 외(추세 역행 돌파) → vec![]   (whipsaw로 간주, 차단)
5. score = score_from_distance(돌파 초과폭)  (기존 헬퍼 재사용)
```

> ⚠️ **적대적 리뷰 검증 결과 박제 (L1/L2/L4/L5)**: SMA None은 반드시 `vec![]`(다른 모든
> None 분기와 동일 — 라이브 런타임에 배선되므로 `unwrap()` 절대 금지). 슬라이스는 위 정확한
> 표현식으로 고정(파일 안에 이미 3가지 슬라이스 관례 존재 — 돌파:latest제외, 볼린저/RSI:포함 →
> 모호하면 off-by-one). 경계 `close==ma`는 무신호로 핀(테스트로 고정 — `>=` 오타가 모든
> 테스트를 통과하지 않도록).

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
  마지막 캔들 종가(mark)로 닫는 tail 루프(`run_backtest:186-194`)도 동일 비용 차감.
- 비용은 `Decimal`로 계산(가격/수량이 Decimal). 상수는 스케일 정수로 정확히 표현
  (`Decimal::new(4, 4)`=0.0004, `Decimal::new(1, 4)`=0.0001).
- **승/패 판정은 비용 차감 후 realized PnL 기준**. ⚠️ tail-loop 청산은 기존에도
  trades/wins/losses 카운터를 안 건드림(equity만 갱신) — 이 비대칭은 **기존 동작**이므로
  유지하고, **판정은 win-rate가 아니라 realized_pnl로** 한다(승률은 참고용).

> ⚠️ **수수료 모델의 정직한 한계 (적대적 리뷰 F2/F3 — "보수적/현실 정합" 주장 삭제)**:
> - 백테스트 청산은 SL/TP 가격에 **정확히 체결된다고 가정**(gap-through 없음). 실제로는
>   갭하락/급변 시 스탑보다 불리하게 체결 → 0.01% 슬리피지는 그 순간 비용을 **과소평가**.
> - **funding fee 미반영**, 부분체결 미반영, leverage=3 선언됐으나 sizing은 `quantity =
>   notional/price`로 5% notional만 거래(risk:141-142) → 신호가 작아 노이즈 우세.
> - 따라서 **OOS 통과 = 실거래 수익 보장 아님**. "필요조건이지 충분조건 아님". funding과
>   gap-through가 marginal-OOS를 라이브 음(−)으로 뒤집을 가장 큰 두 비용.
> - **비용 민감도 체크**: 워크포워드를 비용 1×/2×/3×로 재실행. 판정이 뒤집히면 강건하지 않음.

## 5. Component C — 워크포워드 하니스 (영구 통합테스트, `crates/api/tests/walk_forward.rs`)

지난 세션은 임시 `#[ignore]` 테스트로 돌리고 **삭제**했음 → 이번엔 **영구 박제**.
`#[ignore]` 부착 → 일반 `cargo test`엔 안 걸리고, 명시 실행(`cargo test --test
walk_forward -- --ignored --nocapture`)으로만 돈다. DB(`DATABASE_URL`/`TEST_DATABASE_URL`)
필요.

**이 하니스는 "엣지를 찾는 탐색"이 아니라 "엣지 없음을 반증하는 falsification 테스트"다.**
(적대적 리뷰 O1/O2: 사용자가 "정직한 falsification으로 진행" 선택.) 지난 가설이 +0.00%로
실패한 바로 그 데이터·하니스이므로, **기대 결과를 미리 ~0%로 등록**하고, 그 근처가 나오면
"엣지 없음 확정 → STOP"으로 처리한다.

```
윈도우: 4개 (2024-06 ~ 2026-06), 각 = IS 4개월(그리드) → OOS 2개월(검증), rolling.
그리드(고정, ma_period > lookback 제약 — L3): lookback ∈ {10,20} × k ∈ {0.3,0.5,0.7}
  × ma_period ∈ {30,50}  단 ma_period > lookback 만  = 12조합
  (lookback 30/50 제외: ma_period≤lookback이면 필터가 사실상 무력화돼 실패한 순수돌파를
   재발견할 뿐 — inert combo 제거. lookback 20 × ma 30/50, lookback 10 × ma 30/50.)
심볼: BTCUSDT + ETHUSDT (합산 — 주의: 하나의 equity 곡선 = 독립 베팅 <5개, 표본 작음).
각 조합: 비용 반영 run_backtest → realized_pnl(비용후).
윈도우별: IS에서 최고 조합 1개 선택 → 그 파라미터로 OOS 측정.
출력(--nocapture):
  - 윈도우별: IS-best 파라미터, IS PnL, OOS PnL(비용후), 거래수
  - 전체 IS 그리드(plateau vs spike 구분 — 단일 spike면 과적합)
  - OOS: 평균 · 중앙값 · 최악 윈도우 (평균만 보면 fat-tail 1개에 속는다 — O1)
  - 비용 1×/2×/3× 민감도 (판정 뒤집히면 강건하지 않음 — F2)
```

**판정 규칙 (사람이 읽고 결정 — 코드 강제 아님)**:
- ⚠️ **노이즈 기준선 박제**: "4윈도우 중 ≥3 양수"는 동전던지기로도 **31.2%** 확률
  (`P(≥3 of 4 | p=0.5)=0.3125`). 이 기준만으론 운과 엣지 구별 불가 → **더 엄격히**.
- **합격(강함)**: OOS **전 4윈도우 양수** AND OOS 평균 > **2× 왕복비용** AND 최악 윈도우도
  near-0 이상 AND 비용 2×에서도 평균 양수. → 라이브 교체 **검토**(보장 아님).
- **불합격/무엣지(예상)**: OOS 평균이 0 ±(왕복비용 노이즈밴드) 안 → **"롤링 돌파 계열은
  이 데이터에 엣지 없음" 확정**. 깔끔히 로그하고 **STOP**.

⚠️ **사전등록 STOP (가장 중요 — O2)**: 결과가 ~0%로 나오면 그건 **성공적 falsification**
(가설 기각)이지 "노브 하나 더 추가할 이유"가 아니다. **이 spec의 그리드는 고정**이고,
**OOS 보고 그리드를 늘려 재실행하는 것은 금지**(과최적화 재발 = 지난 교훈). 불합격이면
같은 데이터 위 4번째 노브가 아니라 **구조적으로 다른 베팅**(다른 타임프레임/데이터 — 별도
brainstorming)으로 간다.

### S1 — 하니스 배선 (필수, 안 하면 컴파일 불가)
현재 `BacktestConfig`는 `lookback`/`k`만 있어 `ma_period`를 주입할 수 없다. **`ma_period:
Option<usize>` 추가**하고 `run_backtest`에서 전략 생성 시 주입(기존 lookback/k와 동일 패턴).
§8 "파라미터 설정화 후속" 한계는 이 한 필드에는 적용 안 됨(워크포워드 전용 주입구).

## 6. 런타임 통합 (검증 합격 시에만, 엔진 불변)

**3개 인스턴스화 site (grep으로 검증 완료 — 누락 시 라이브·백테스트 전략 불일치):**

| 위치 | 변경 |
|---|---|
| `crates/api/src/testnet_runtime.rs:19,71` | import + `VolatilityBreakoutStrategy::default()` → `TrendFilteredBreakoutStrategy::default()` |
| `crates/api/src/strategy_runtime.rs:12,65` | 동일 교체 |
| `crates/api/src/backtest_runner.rs:8,129,132` | 백테스트 default + `new(...)` 경로 교체 |

- 교체 후 **`grep -rn VolatilityBreakoutStrategy crates/api`로 잔여 site 0 확인**(누락 방지).
- 시그널→주문 흐름 100% 불변: 방향만 전략이 결정, 리스크게이트·시장주문·SL/TP 보호주문은
  엔진이 그대로 처리 → 돈 실수 여지 없음. score 범위·`build_signal` 동일 → sizing 영향 없음.
- `strategy_version()` → `"trend_filtered_breakout_v1"` 갱신(백테스트 기록 메타 정합).
- **불합격 시 교체 안 함** — 현 `VolatilityBreakoutStrategy::default()` 유지.

## 7. 테스트 (TDD, lib.rs 하단 `#[cfg(test)]`, RED 먼저)

기존 `candle_hlc`/`breakout_window` 헬퍼 재사용. ⚠️ **워밍업 함정(L4)**: 기본
`ma_period=50`이면 테스트가 `need=50`개를 만들어야 게이트까지 도달. `breakout_window`는
`lookback+1`개만 만들므로 그대로 쓰면 테스트 2~5가 **워밍업 가드에 막혀 엉뚱한 이유로
통과/실패**. → 테스트는 작은 `ma_period`로 `new(lookback, k, ma_period)` 생성(예
`ma_period=5`)하거나 `need`개 이상 캔들 구성. MA를 게이트 위/아래로 흔들 수 있게 추세
캔들을 명시적으로 깐다.

1. `trend_filter_does_not_signal_with_insufficient_history` — `need` 미만 → vec![].
2. `trend_filter_allows_long_above_ma` — 상승 돌파 + close > MA → Buy 1개.
3. `trend_filter_blocks_long_below_ma` — 상승 돌파지만 close < MA → vec![] (whipsaw 차단).
4. `trend_filter_allows_short_below_ma` — 하락 돌파 + close < MA → Sell 1개.
5. `trend_filter_blocks_short_above_ma` — 하락 돌파지만 close > MA → vec![].
6. `trend_filter_no_signal_inside_band` — 돌파 자체가 없으면 → vec![].
7. `trend_filter_blocks_on_close_equals_ma` (L5) — 상승 돌파 + close == MA 정확히
   같음 → vec![]. (`>=` 오타가 다른 테스트를 다 통과하므로 경계 핀 필수.)
8. `simple_moving_average_computes_expected_value` — 알려진 종가들로 SMA 정확값 단언
   (슬라이스 off-by-one 잡기 — L2).
- 백테스트 비용 모델: `close_positions` 비용 차감 유닛테스트(거래 1건 비용 정확도).

## 8. 알려진 한계 / 후속

- 파라미터 하드코딩(설정화는 후속). `evaluate()`는 캔들만 받음(오더북/펀딩비 불가).
- 최신 캔들만 판단(포지션 상태 모름, SL/TP가 청산). 단일 심볼 독립 평가.
- 백테스트 비용 모델은 **고정 bps 근사** — 펀딩비·gap-through·체결깊이별 슬리피지 미반영
  (§4 한계 박스 참조). 따라서 OOS 양수는 **필요조건이지 충분조건 아님**. 라이브(testnet)로 보완.
- 워크포워드 한계: `BACKTEST_HISTORY_LIMIT=64` 롤링윈도우(라이브 동일 제약). 정통 일봉 불가.

## 9. 범위 밖 (이번에 안 함)

- 엔진/버퍼/리스크/보호주문/라이브 PnL 계산 변경.
- 파라미터 설정화, AI 필터(룰 기반 유지 — 사용자 합의).
- 그리드 확장(과최적화 방지 — §5 고정).
- 평균회귀/페어트레이딩 등 다른 가설(불합격 시 별도 brainstorming).
