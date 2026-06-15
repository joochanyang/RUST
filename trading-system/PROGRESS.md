# PROGRESS — trading-system (Rust 코인선물 AI 트레이딩)

> 마지막 갱신: 2026-06-15 (★**평균회귀(RSI+볼린저) 워크포워드 검증 완료 → OOS 평균 −185.40, positive 0/4, expectancy 0/4, 거래수 충분(337~596) → 3번째 전략 패밀리도 엣지 없음 확정. 라이브 교체 안 함. 커밋·푸시 완료 `91ac208`**). 다음 세션 이 파일부터 읽고 아래 "🚀 다음 세션 첫 액션". ★★다음 목표=사용자 선택 **다른 타임프레임(5m/1h)** — DB는 1m만 적재됨 → 1m 롤업으로 5m/1h 생성(추가 다운로드 불필요)부터.

## 🚀 다음 세션 첫 액션 — ★구조적으로 다른 전략 베팅 (clear 후 여기부터)
**트리거 문구: "rust 트레이딩 이어서 작업" 또는 "실거래 전략 만들기 이어서"**

> **목표(사용자)**: "실거래 할 수 있는 전략". 검증 가능 목표=**OOS에서 수수료 반영 후 일관 양(+)**.
> **현재 결론**: **3개 전략 패밀리(순수돌파·추세필터돌파·평균회귀) 전부** 이 데이터(binance 1m, 100캔들 버퍼, BTC+ETH, 2024~2026)에서 OOS 엣지 없음 실증. **같은 데이터·같은 1분봉 위에서 더 튜닝·더 노브 금지**(사전등록 STOP). 진짜 다른 방향=**다른 타임프레임 / 엔진버퍼 확장(정통 일봉) / 다른 데이터(오더북·펀딩비)**. 셋 다 보장 없음, 매번 OOS+수수료로 판정. **family-wise 주의**(같은 데이터 4번째 시도 = 거짓양성 누적).

### ⚠️ 먼저 읽을 것 — 과최적화 함정 (★세 번 실증됨, 재발 절대 금지)
1. **순수 변동성돌파**(lookback/k): 워크포워드 OOS 평균 **+0.00%**(수수료 전). IS-최적이 OOS서 무너짐.
2. **추세필터 변동성돌파**(lookback/k/ma_period, +수수료): 워크포워드 OOS **평균 −136.74·1/4 윈도우만 +**, 그 1개도 2×비용서 음전. IS-best 파라미터 윈도우마다 바뀜=노이즈.
3. **평균회귀(RSI+볼린저)**(2026-06-15, `TechnicalStrategy`, 평균회귀-적합 출구로 검증): OOS **평균 −185.40·positive 0/4·expectancy 0/4·거래수 337~596(충분)**. IS 27조합 전부 음수(−298~−4,964), 음수가 거래수에 비례(rsi7→1.5만거래→−4,964)=**스냅백이 taker수수료+슬리피지 못 이김**. ⭐**핵심 교훈: 검증 전 적대적 설계리뷰 필수** — 원래 계획은 돌파용 2:1 브래킷 출구를 평균회귀에 그대로 씌워 "잘못된 출구로 테스트"할 뻔했음(3에이전트 만장일치 CRITICAL). 출구를 중간밴드복귀/RSI50/하드스톱/시간스톱으로 고쳐서야 정직한 FAIL 판정 가능.
- **교훈**: "백테스트 수익 좋아질 때까지 파라미터 돌리기"=과최적화 정의 그대로. **IS 숫자 믿지 말 것. OOS+수수료로만 판정.** **출구가 전략 논리와 맞는지 먼저 확인**(진입만 보면 안 됨). 새 전략도 반드시 같은 워크포워드+OOS+수수료+적대적리뷰로 검증.

### ✅ 이번 세션 산출물 (평균회귀 검증 — 커밋·푸시 완료 `91ac208`)
- **`TechnicalStrategy` 파라미터화**(`crates/strategy/src/lib.rs`): `new(rsi_period, bollinger_period, oversold, overbought)`+getter 추가. `Default` 불변. 라이브 미사용(라이브=`VolatilityBreakoutStrategy::default()` 유지). TDD 2테스트(비기본 임계값이 신호를 실제로 바꿈 검증).
- **평균회귀 전용 백테스트 코어**(`backtest_runner.rs` 테스트모듈): `run_mean_reversion_backtest(pool,config,&TechnicalStrategy)` — 진입은 BasicRiskGate로 사이징하되 **출구는 평균회귀-적합**(하드스톱1%→중간밴드복귀(SMA)→RSI50재크로스→시간스톱60봉, 우선순위순). 프로덕션 `run_backtest`·`BacktestConfig`·HTTP API **전부 불변**(strategy 디스크리미네이터 추가 안 함 — strategy_version 오라벨·dyn디스패치·config비대화 회피). MR 출구 4단위테스트.
- **평균회귀 워크포워드 하니스**(`#[ignore]` 영구): 27조합×4윈도우, IS최소거래(≥20)충족 조합 중 IS-best 선택, OOS 1×/2×/3×, 윈도우별 trades/wins/losses/per-trade expectancy/IS-best PnL+OOS 진단, **3진 판정(PASS/FAIL/INCONCLUSIVE)**. 실행: `cargo test -p trading-api --bin trading-api backtest_runner::tests::walk_forward_mean_reversion -- --ignored --nocapture`(DATABASE_URL 필요, ~815s).
- spec: `docs/superpowers/specs/2026-06-15-mean-reversion-validation-design.md`(적대적 리뷰 반영판 + 실행결과 §8.1). **적대적 설계리뷰가 치명결함(추세용 출구) 잡아냄 — 이게 정직한 판정의 핵심.**
- 테스트 **126 passed / 0 failed / 2 ignored**(이전 120/0/1 + MR 6테스트 + MR 하니스 1 ignored). fmt clean, clippy 신규0(기존 ai_repository 인자수 1만 잔존).

### ⏭ 다음 세션 — 구조적으로 다른 베팅 후보 (브레인스토밍부터)
롤링 돌파 + 평균회귀(1분봉) 끝. 남은 진짜 다른 방향 (사용자와 brainstorming으로 1개 선택):
1. **다른 타임프레임**(5m/15m/1h 캔들) — 1분봉 노이즈/수수료드래그 회피(평균회귀가 수수료에 죽은 게 시사점). 버퍼 100캔들이면 1h×100=4일치. ⚠️ 시장ingestion이 1m 외 구독하는지·DB에 5m/1h 적재됐는지 먼저 확인(현재 DB=1m만 적재 확인됨).
2. **엔진 버퍼 확장**(`PAPER_MAX_CANDLES_PER_KEY`↑ + 시작 DB백필) → 정통 일봉 래리윌리엄스 가능. 단 엔진 수정=범위 커짐.
3. **다른 데이터**(오더북 불균형/펀딩비) — trait 확장 필요(`evaluate`가 캔들만 받음).
- ⚠️ 어느 것도 보장 없음. 매번 OOS+수수료+적대적리뷰로 판정. **family-wise 과최적화 주의**(같은 데이터 4번째 시도 = 거짓양성 누적 — 마진 PASS면 한 번도 안 쓴 새 홀드아웃 필수).
- ❓ **사용자에게 물어볼 것**: 1분봉 3패밀리 다 무엣지 → 정말 다른 타임프레임/데이터로 갈지, 아니면 "실거래 전략 개발"을 여기서 일단 멈출지.

### 0. 그린 베이스라인 (먼저 복붙)
```bash
cd ~/Documents/Rust/trading-system
set -a; source .env; set +a
export TEST_DATABASE_URL=$(echo "$DATABASE_URL" | sed 's#/trading_system$#/trading_system_test#')
cargo test --workspace   # 기대: 126 passed / 0 failed / 2 ignored (2026-06-15 실측)
```
⚠️git루트=부모 `~/Documents/Rust`. ⚠️.env 커밋금지. 봇 안 떠있음·미결 testnet 포지션 0. ✅ 이번 세션 평균회귀 변경분 커밋·푸시 완료(`91ac208`), 워크트리 clean.

### 1. 접근 (TDD+워크포워드, 한 번에 한 가설)
1. **브레인스토밍 먼저**(`superpowers:brainstorming`) — 어떤 시장 비효율/가설을 노릴지. 후보(직전 논의): ①변동성돌파+**추세필터**(장기 MA 방향과 같을 때만 진입→whipsaw 제거), ②평균회귀, ③추세추종, ④페어트레이딩(멀티심볼=trait 확장 필요). 기존 `TechnicalStrategy`(RSI)도 같은 워크포워드로 검증 가능.
2. **구현**: `crates/strategy/src/lib.rs`에 새 `impl Strategy`(엔진 불변 패턴 유지). 캔들만으로 부족하면(오더북/펀딩비) trait 확장 검토(spec §9 한계).
3. **검증(필수·정직)**: 워크포워드+OOS. **인프라 이미 있음** → 아래 §2.
4. **머니패스·엔진 변경 시**: RED→GREEN+적대적 리뷰(함정 섹션 규칙).

### 2. 워크포워드 검증 인프라 (이미 구축됨 — 재사용)
- `BacktestConfig.lookback/k`(옵셔널) + `VolatilityBreakoutStrategy::new(lookback,k)`/`.lookback()`/`.k()` 추가됨(이번 세션). `run_backtest`가 파라미터 주입. **새 전략도 동일 패턴으로 파라미터화**하면 그리드 가능.
- 워크포워드 하니스(IS 그리드→OOS 검증, 4윈도우, ~84런/472s)는 `#[ignore]` 임시 tokio 테스트로 돌렸다 제거함. **재구현 템플릿**=직전 세션 `walk_forward_param_sweep`(git 히스토리/이 파일 워크포워드 섹션 참고). 핵심: IS에서 고르고 **OOS로만 보고**, 전체 그리드도 출력해 plateau/spike 구분.
- ⚠️`BACKTEST_HISTORY_LIMIT=64` → lookback≤50. DB에 binance BTC/ETH 1m 3년치(2023-06-13~2026-06-15, 구멍 없음) 적재됨(import 불필요).
- ⚠️**백테스트 러너는 수수료/슬리피지 미반영** → 실거래 판정 전 반드시 추가(taker~0.04%×거래수). 안 그러면 OOS가 과대평가됨.

### 3. 사용자 미결정 (다음 세션 시작 시 브레인스토밍에서 확정)
- 어떤 가설/전략으로 갈지(추세필터 vs 평균회귀 vs 기타). 직전 세션 내 추천=**추세필터 1회 시도**(변동성돌파에 장기MA 방향 게이트 추가). 보장은 없고 OOS로 재검증 필수.
- 실거래 직전 체크리스트(미정): 수수료/슬리피지 반영 백테스트 + testnet 라이브 e2e(보호주문 SL/TP) + 소액 실거래 1회 테스트.

---
### 📉 워크포워드 + OOS 튜닝 결과 (2026-06-15, 직전 세션 — 순수 변동성돌파 엣지 없음 실증)
**OOS(정직한 숫자)**: W1 lb10/k0.5→−1.85%, W2 lb30/k0.5→+3.20%, W3 lb50/k0.7→−1.34%, W4 lb20/k0.5→−0.01%. **OOS 평균 +0.00%**. (4개월 IS 그리드 20조합→2개월 OOS 검증, BTC+ETH, 4윈도우 2024-06~2026-06.) IS-최적이 OOS서 무너짐+파라미터 윈도우마다 바뀜=강건한 엣지 없음. 수수료 미반영인데도 0%=실질 마이너스. W2 +3.2%는 추세국면 1회뿐.

---

### 🛠 이번 세션 추가 인프라 (영구 유지)
`BacktestConfig.lookback/k`(옵셔널) 주입 + `VolatilityBreakoutStrategy::new(lookback,k)`/`.lookback()`/`.k()` 추가 → 워크포워드 그리드용. 라이브 런타임 불변(default 그대로), HTTP API 하위호환. **109 passed, fmt/clippy clean**. 워크포워드 하니스(`walk_forward_param_sweep`)는 `#[ignore]` 임시테스트로 돌린 뒤 제거(워크트리에 없음, git 히스토리로 복원 가능).

> ⚠️ **미수정 엔진 결함 (standing order)**: testnet 봇 첫 가동(03:52~04:31) 후 **고빈도 WS 스트림(bookTicker) 부분 스톨** — 이벤트 16k~33k/분→1.2k/분 폭락, 캔들 close_time이 실시간보다 16~23분 뒤처져 latency-gate가 모든 진입 차단(WARN 5776건). **머신 절전 아님**(이벤트 ts 연속·분 누락 0), 포지션/주문 0·돈위험 0(게이트 정상). 기존 idle-timeout 재연결(`077ee9e`)이 **부분 스톨은 못 잡음**(연결이 트리클로 살아있어 'idle' 아님). 재시작으로 복구. **★사용자 standing order**: "부분 스톨 재발하면 per-stream staleness 감지 구현" — 타깃=`binance.rs`/`bybit.rs`/`bitget.rs`의 `run_public_market_stream_once`에 per-(symbol/channel) 마지막 프레임 시각 추적→개별 스트림 idle_timeout 초과 시 reconnect. TDD+적대적 리뷰 필수. **봇 재가동 시에만 유효(현재 봇 내려감)**.

## 현재 상태 (한 줄)
머니6종 + 후속 + reconcile + **latency-gate + Bitget 8h + 보호주문 Algo API(+e2e 라이브 종결) + 출시 전 감사 P0/P1 + runbook 정합성 + WS silent-stall 재연결 완료**.
**전체 테스트 99 통과/0 실패 (병렬 `cargo test` 반복 green — flaky 해소됨), fmt clean, clippy warning 1(기존 ai_repository 인자 수).** ⚠️**DB 통합테스트는 `TEST_DATABASE_URL` 필요**(미설정 시 skip). 로컬 검증용 DB=`trading_system_test`(로컬 PG).
✅ **보호주문 e2e 라이브 종결 (2026-06-15 01:46)**: 봇 상주 중 ETHUSDT 자연 진입(RSI≈12) → 보호주문 LOCK 없이 통과·SL/TP algoOrder 거래소 등록(algoStatus:NEW) 확정. 상세는 "재개 지점" §1.
✅ **WS silent-stall 재연결 (`077ee9e`)**: 3거래소 WS read 루프에 idle 30s 타임아웃+read에러 시 reconnect. e2e 모니터링 중 Binance freeze 발견→수정. 상세는 "재개 지점" §1 하위.
✅ **기동 시 position sweep (`7104483`)**: 절전/freeze로 놓친 거래소 청산을 재시작 시 DB와 대조해 고아 포지션 자동 정리. 라이브 검증(고아 ETH 자동 closed). 상세·남은갭(주기적 sweep)은 "재개 지점" §3.
✅ **출시 전 감사 P0/P1 정식 커밋 (2026-06-15, `297a9dd`)**: 이전 세션이 작업만 하고 커밋 안 했던 ~958 LOC(WS 라우팅·보호주문 partial 보상·testnet 영속화/재시작복구·testnet 리스크상태 DB화·인증 하드닝·운영가드·URL시크릿 로그차단)을 독립 검증(single-thread green·fmt·clippy·dashboard tsc) 후 커밋. `dashboard/*.tsbuildinfo` gitignore 추가.
✅ **flaky 테스트 병렬안전화 (2026-06-15, `2e01f63`)**: `account_risk_state` 2개 테스트가 `load_account_risk_state`의 **계정 전역집계**를 단언 → 공유 DB 동시 변경으로 realized PnL 테스트가 병렬에서 flaky(-980/-990 등). **프로덕션 코드는 정상**(전역집계가 의도). 테스트를 **row-scoped**(자기 포지션의 unrealized_pnl / 자기 포지션 realized 기여를 프로덕션 SQL식으로 직접 계산)로 재작성. 병렬 `cargo test` 반복 green 확인.
✅ **runbook 정합성 수정 (2026-06-15, `57b52f2`)**: ①`paper_trading_14d` 게이트(라이브 해제용 머니게이트)가 runbook.md:96 명세("paper-mode" positions/protection)와 달리 `positions`/`protection_orders`를 **mode 필터 없이** 카운트 → **testnet 데이터만으로 라이브 게이트 통과 가능 결함**. `calculate_paper_trading_evidence`(`live_readiness.rs`)에 `paper_protection`/`paper_positions` CTE 추가(=`protection_orders.entry_order_id → orders.mode='paper'` 조인)로 스코핑. positions 테이블엔 mode 컬럼 없음→orders로만 추적. ②reconcile 해피패스 테스트(`testnet_runtime.rs`)가 `signal_id=nil`을 signals seed 없이 넘겨 FK위반→spurious LOCK → `run_reconcile`에 nil signal seed 추가(프로덕션은 signal 먼저 영속화하므로 정합). DB 통합테스트 3개 추가/수정, 실 PostgreSQL로 검증.
✅ **latency-gate 해결+라이브검증**: close_time 기준 측정 → 정상 캔들 0~128ms(수정전 7-20k). **실제 진입 주문 발생 확인**(11:58 ETHUSDT).
✅ **Bitget 8h 해결+라이브검증**: 스냅샷 배치(500개 오래된순)에서 `.next_back()` 최신행 → 8h→0ms.
✅ **보호주문(SL/TP) -4120 해결+testnet 200 검증**: Binance가 2025-12-09부로 조건부주문을 Algo Service 이전. `STOP_MARKET`/`TAKE_PROFIT_MARKET`을 `/fapi/v1/order`→**`/fapi/v1/algoOrder`**(`algoType=CONDITIONAL`, `stopPrice`→`triggerPrice`)로 변경. testnet 양쪽 HTTP 200·algoStatus:NEW. 커밋 `0fee96f`·`d389631`·`659f50e` 푸시 완료.

## 위치 / 저장소
- 작업 경로: `~/Documents/Rust/trading-system` (⚠️ git repo 루트는 **부모** `~/Documents/Rust`)
- 원격: https://github.com/joochanyang/RUST.git (`main`, 추적됨)
- 마지막 커밋: `659f50e` "Route protection orders through the Algo Order API (fixes -4120)"
- 7-crate 워크스페이스: ai / api / core / exchange / execution / risk / strategy (~8,000 LOC)

## ✅ 이번까지 완료 (검증된 수정)
- **C1** mark-to-market: 오더북 틱+캔들 종가마다 `update_open_position_marks`로 DB 영속화 (`execution_repository.rs`, `strategy_runtime.rs`)
- **C2** 멱등 청산: `persist_paper_exit`·`close_position_for_exit` 양쪽 `closed_at IS NULL` 가드 → 이중청산/PnL 이중집계 차단
- **C3** 테스트넷 보호주문 실패 시 reduceOnly 플래튼 + CRITICAL 이벤트 + lock (`testnet_runtime.rs`, `MarketOrderRequest.reduce_only`)
- **H1** lot/tick 반올림: `SymbolFilters`+`fetch_symbol_filters`(exchangeInfo). 수량 절사, 보호가격 **방향인식**(LONG↓·SHORT↑), 최소조건 미달 스킵
- **H2** `order_ack_from_value`: `executedQty` 누락 시 `origQty` 폴백 제거 → 0
- **H3** `main.rs`: live 모드 시작 시 `bail!` (라이브 런타임 미구현)
- 적대적 멀티에이전트 워크플로로 2회 검증(C2 벌크경로·H1 반올림방향 결함 추가 발견·수정)

## ✅ 2차 세션 완료 (2026-06-14, 후속 #1~#3 + 확정결함)
- **#1 HTTP 타임아웃**: `binance.rs` `http_client()` = `Client::builder().timeout(10s)`. `HTTP_REQUEST_TIMEOUT=10s` 상수. `default()`가 사용. silent-server 통합테스트로 실측(10초에 timeout)
- **#2 멱등성 키(field-only, retry 미구현 — 사용자 결정)**: `MarketOrderRequest.client_order_id: Option<String>` 추가. `market_order_params()` 추출(서명순서 보존). 진입=`client_order_id_for_signal(signal.id)`=`uuid.to_string()`(36자·Binance-safe), 플래튼=`None`
- **#3 C3 e2e**: 보호실패 인라인 블록을 `handle_protection_failure(...)`로 **동작보존 추출**. DB연동 e2e 테스트로 전체 시퀀스(lock→reduceOnly플래튼→CRITICAL risk_event→alert→키 미등록) 검증
- **★확정결함(적대적리뷰 7에이전트, 1확정/3기각)**: #1 타임아웃이 기존 갭 노출 — 진입주문 타임아웃 시 Binance는 체결했는데 봇은 'Err=실패'로 보고 skip → **무방비 포지션+재진입**. #2 멱등키는 캔들마다 UUID 새로생겨 못막음
  - **수정(보수적 락)**: `TradingError::Timeout` 변종 추가 + `map_request_error`(reqwest `is_timeout()`로 분류). 진입 Err arm에서 `Timeout`이면 `handle_entry_timeout(...)` → **런타임 LOCK + CRITICAL alert + risk_event(action=`entry_order_timeout_unknown_outcome`)**. 일반 실패는 기존대로 skip. DB e2e 테스트 추가
  - 기각된 3건: http_client fallback(rustls는 build실패 불가), connect_timeout/WS 무타임아웃(WS는 변경무관·hang=무동작), 멱등키 UUID(오늘 무해·랜덤은 항상 수용)

## ✅ 3차 세션 완료 (2026-06-14, 타임아웃 자동복구 reconcile)
- **reconcile 구현(완전복구)**: `ExchangeAdapter::query_order(symbol, coid) -> Result<Option<OrderAck>>` 추가(Binance=`GET /fapi/v1/order?origClientOrderId=`, `query_order_result` 분류: Ok→Some·code -2013→None·기타Err→Err. bybit/bitget=stub). 진입 타임아웃 시 `reconcile_entry_timeout`: **체결→보호주문(`finalize_entry_with_protection` 공유, 실패시 `handle_protection_failure`)→Registered / 미체결·없음→Skipped(락X) / 조회실패→보수적 LOCK**. `finalize_entry_with_protection`는 정상경로 tail에서 **동작보존 추출**(정상·reconcile 공유)
- **★확정결함 수정(적대적리뷰 11에이전트, 1확정/7기각)**: reconcile가 타임아웃 직후 **단일 query**로 Skipped 결론 → stale read(체결됐는데 None/NEW)면 무방비+중복. **수정=`query_order_until_settled`**(미체결/없음이면 `RECONCILE_QUERY_ATTEMPTS=3`회·`RECONCILE_QUERY_DELAY=2s` 재시도, 중간 체결시 즉시 보호, 조회Err은 즉시 surface→LOCK). attempts/delay는 함수 인자로 빼서 테스트는 `Duration::ZERO`(빠름)
  - 기각된 7건 요지: status-string 취약성(애매=LOCK=안전방향), -2013 오분류(체결+미존재 모순), 시장주문 NEW-resting(시장주문은 동기체결), 재시작 영속성(기존 한계·범위밖), Skipped audit 누락(관측성·머니무관)
- 테스트: reconcile 4분기 e2e + 재시도 3종. RecordingAdapter에 `QueryOrderBehavior`(Filled/ExistsNotFilled/NotFound/QueryFails/NotFilledForFirst) + `query_calls` 카운터

## ✅ 4차 세션 완료 (2026-06-14, latency-gate 버그 수정 — 커밋 `0fee96f`·푸시 완료)
- **근본 원인(프로덕션 DB로 확정)**: 캔들 freshness를 `received_at − open_time`(봉 시작시각)으로 측정 → 1분봉이라 경계 틱도 7,287~10,463ms(DB `candles`/`risk_events` 179건 block_entry로 실측). 오더북 latency는 ~1ms(클럭 스큐 없음 확정) → 7-10초는 **순수 open_time 아티팩트**. 적대자가 "경계 틱은 0-2초라 오진단" 가설 제기했으나 **DB 데이터로 반증**(`CandleBuffers::push`는 새 open_time 첫 틱만 gate로 보내지만 그 틱도 7-10초)
- **수정(외과적·필드 추가 없음)**: `MarketEvent::event_time()` Candle arm을 `candle.open_time` → `candle.close_time()`(=`open_time + timeframe_duration`, **메서드로 즉석 계산**)로 변경. OrderBook arm 불변 → 오더북 staleness 보호 100% 보존. 구조체 필드 안 늘려 **DB 마이그레이션·9개 생성지점·백테스트 분기·직렬화 변경 전부 회피**. 정상 캔들 latency clamp→0, 마감 후 도착 캔들은 여전히 gate 차단
- **threshold 중복 제거**: `2_000` 2곳(risk/lib.rs:123 하드코딩 + risk_event_repository.rs:6 const) → `trading_core::MARKET_DATA_LATENCY_THRESHOLD_MS` 단일 const(core가 양쪽 의존, 순환 없음)
- **timeframe_duration 헬퍼**: Binance/Bitget 접미형("1m","1h","1d")+Bybit bare형("1","60","D") **대소문자 무시**(Bitget `candle1H`→`1H` 대비)+`1w` 커버. 미인식 timeframe은 `open_time` 폴백(=오늘 동작, fail-safe 더 차단)
- **검증(TDD RED→GREEN + 적대적 리뷰 2회)**: 설계 워크플로(7에이전트: 3제안→3공격→종합, Proposal 3 채택=치명결함 0)→구현→커밋 전 적대적 리뷰(4에이전트, **ship-as-is·must-fix 0**, 유일 지적=Bitget 비-1m 대문자형 잠재갭 LOW→선반영). **85 통과/0 실패, fmt clean, clippy 13(신규 0)**. 기존 테스트 1개(`market_latency_details_include_gate_context`) 의도적 수정(기준이 open→close로 이동해 received_at +60s)
- **함정 기록**: ⚠️**latency≠open_time 기준**(캔들은 close_time 기준이어야 정상틱이 fresh). 1m 캔들 경계틱도 open_time 기준이면 7-10초. 적대적 리뷰가 머니게이트에서 결정적(초기 진단 반증). DB 증거 우선(risk_events/candles/order_books). **타임아웃 트레이드오프**: 캔들 staleness 탐지창이 ~2s→~interval+2s(1m=62s)로 넓어짐(의도된 것, 실제 freeze 탐지는 오더북 경로+새 캔들 부재로)

## 🗄️ [폐기된 옛 섹션 — 참고용] VolatilityBreakoutStrategy 단순 6mo 백테스트
> ⚠️ **이 아래는 더 이상 재개 지점 아님**(진짜 재개 지점=파일 최상단). 아래 단일 6개월 백테스트
> +1.78%는 **과적합/행운이었음이 워크포워드로 반증됨**(순수돌파 OOS +0.00%, 추세필터 OOS −136.74).
> 단일 백테스트 숫자를 믿으면 안 된다는 교훈의 박제로만 남김.

> **(옛 메모)** `VolatilityBreakoutStrategy` 구현·검증·6개월 백테스트 완료(2026-06-15).
> **다음 액션은 사용자 결정 1건뿐**: ① 백테스트까지만 멈춤(현 상태 종결) / ② testnet 라이브 드라이런으로 진행.
> ②로 가면 부팅 명령어(아래 "부팅 명령어")로 봇 띄우고 보호주문 e2e처럼 자연 진입 모니터링.

### ✅ 구현 완료 내역 (커밋 대기 — 아직 커밋 안 함, 워크트리에 변경분 있음)
- **`VolatilityBreakoutStrategy` 추가** (`crates/strategy/src/lib.rs`): 양방향 롤링윈도우 변동성돌파.
  `lookback=20`·`k=0.5`, window=`candles[len-1-lookback..len-1]`(latest 제외), 종가 돌파 판정, `range<=0` 무신호.
  헬퍼 `window_high_low`(high/low f64 추출) 추가, `build_signal`·`score_from_distance` 재사용. `name()="volatility_breakout"`.
- **신규 테스트 6개**(spec §7, lib.rs 하단): `breakout_does_not_signal_with_insufficient_history`(기존 RSI 테스트와 이름충돌 회피 위해 `breakout_` 접두) / `signals_buy_on_upside_breakout` / `signals_sell_on_downside_breakout` / `no_signal_inside_band` / `no_signal_on_zero_range` / `score_grows_with_breakout_distance`. `candle_hlc`·`breakout_window` 헬퍼 추가.
- **런타임 교체 3곳**(spec는 2곳이라 했으나 backtest_runner도 concrete 인스턴스화라 백테스트가 새 전략을 쓰려면 필수): `testnet_runtime.rs:19/71`, `strategy_runtime.rs:12/65`, `backtest_runner.rs:8/123` 의 import + `TechnicalStrategy::default()` → `VolatilityBreakoutStrategy::default()`. (`TechnicalStrategy`는 코드에 그대로 남아있음 — 향후 복귀 가능)
- **`strategy_version()` 갱신**: `"technical_rsi_bollinger_v1"` → `"volatility_breakout_v1"` (백테스트 기록 메타 정합).
- **검증 완료**: `cargo test --workspace` = **109 passed / 0 failed / 0 ignored**, fmt clean, clippy 신규 0(기존 `ai_repository` 1건만), `cargo build -p trading-api` OK.

### 📊 6개월 백테스트 결과 (2025-12-15 ~ 2026-06-15, binance 1m, 초기자본 10k USDT, 일손실한도 500)
| 심볼 | 캔들 | 시그널 | 거래 | 승 | 패 | 승률 | 최종자본 | PnL | MaxDD% |
|---|---|---|---|---|---|---|---|---|---|
| BTCUSDT | 259,735 | 4,863 | 406 | 128 | 278 | 31.5% | 9,899.18 | **−100.82** | 1.98% |
| ETHUSDT | 259,745 | 4,509 | 631 | 229 | 402 | 36.3% | 10,282.26 | **+282.26** | 0.84% |
| BTC+ETH | 519,480 | 9,372 | 1,037 | 357 | 680 | 34.4% | 10,177.86 | **+177.86** | 1.27% |

- **해석**: trade>0 충족(성공기준). 같은 6개월에 BTC −29%·ETH −48% 하락장이었음에도 **포트폴리오(BTC+ETH) 순익 +1.78%, MaxDD 1.27%** — 양방향(하락 돌파 포착) 설계 덕에 강한 하락장에서 +. 승률 낮음(~34%)이나 승>패 크기(돌파/추세추종 전형: 작은손실 다수 + 큰이익 소수). BTC 단독은 소폭 −(whipsaw), ETH 단독·합산은 +.
- **검증 방법**: `backtest_runner::run_backtest`를 `#[ignore]` 임시 tokio 테스트로 실 DB(`DATABASE_URL`) 호출(BTC/ETH/합산 3회). **결과 기록 후 임시 테스트 제거함** → 현재 워크트리엔 없음. SL/TP는 risk gate 기본값(엔진 불변), 캔들 단위 SL/TP 청산.
- ⚠️ 백테스트 한계: `BACKTEST_HISTORY_LIMIT=64`라 롤링 윈도우만(라이브와 동일 제약). 수수료/슬리피지 미반영(러너 미구현). 결과는 엣지 존재 검증용이지 실수익 보장 아님.

### 사용자 결정사항 (확정 — 재논의 불필요)
- **양방향(롱+숏)** 변동성돌파. (데이터 근거: 최근 6mo BTC -29%·ETH -48%·모멘텀약함·돌파적중률<50% → 롱전용은 엣지없음. spec §2 — **백테스트로 사후 확인됨**: 양방향이 하락장서 +)
- **AI 없이 룰 기반**. 정통 일봉 래리윌리엄스는 **버퍼 100캔들 제약상 불가**(엔진수정 필요) → 롤링 N봉 변형 채택.
- 파라미터 하드코딩(설정화는 후속). `evaluate()`는 캔들만 받음·최신캔들만 판단(구조적 한계, spec §9).

### ⏭ 미결정 1건 (★다음 세션 시작 시 사용자에게 물을 것)
백테스트 결과 확인 후 — **(A) 이대로 종결**(필요시 커밋만) vs **(B) testnet 라이브 드라이런**(봇 띄우고 보호주문 e2e처럼 자연 진입 모니터링). 사용자 답 받고 진행. 커밋은 사용자 지시 시.

### ★ spec 문서 (모든 결정·근거 = 참조용)
`docs/superpowers/specs/2026-06-15-volatility-breakout-strategy-design.md` (커밋 `8ab8f50`)

### 참고: 남은 후속(전략과 별개, 선택) — 아래 "재개 지점" 참조
주기적 position sweep(현재 기동 시 1회만), signal.id 안정화, timeframe CI 가드. **급한 버그 없음.**

## ✅ 운영 확인 완료 (2026-06-14, 사용자 키로 검증)
- **`demo-fapi.binance.com` = 진짜 testnet 확정**: 사용자 testnet 키로 signed `GET /fapi/v3/account` → HTTP 200 + **testnet 가짜잔고**(USDT 5282·USDC 5000·BTC 0.01). 같은 키를 실거래 `fapi.binance.com`에 치면 **HTTP 401 `-2015`**(거부) → testnet 전용 확정, 실거래 자금 위험 없음
- **testnet 모드 ON**: `.env`(gitignore, 커밋 안 됨) → `BINANCE_TESTNET_API_KEY/SECRET`(각 64자) 설정 + `BINANCE_TESTNET_ENABLED=true` + `TRADING_MODE=testnet` + `MARKET_DATA_ENABLED=true`(testnet 게이트 필수). `BINANCE_TESTNET_MAX_ORDER_NOTIONAL=50`(주문당 상한). ⚠️채팅으로 평문 전송된 키라 rotate 권장
- 부팅 검증: `cargo run -p trading-api --bin trading-api` → settings 게이트 통과·API listening·시장스트림 수신 OK

## ✅ 출시 전 감사 수정 진행 (2026-06-14)
- **P0 Binance WS 라우팅 수정**: 기본 WS root=`wss://fstream.binance.com`. kline=`/market/stream?streams=...@kline_1m`, bookTicker=`/public/stream?streams=...@bookTicker`. Binance 2026 문서 기준 routed endpoint 대응.
- **보호주문 partial success 보상**: `ProtectionOrderRequest`에 deterministic `clientAlgoId` 2개 추가(`entryUUID32-sl/tp`). Binance `DELETE /fapi/v1/algoOrder` 구현. TP leg 실패 시 이미 생성된 SL leg 취소, TP timeout이면 TP clientAlgoId도 best-effort 취소.
- **testnet 영속화/재시작 복구**: 성공한 testnet entry/fill/position/protection을 기존 `orders/order_fills/positions/protection_orders`에 `mode=testnet`으로 저장. 시작 시 open testnet position key 복구해 재시작 중복 진입 방지.
- **panic/manual close testnet 거래소 반영**: `TRADING_MODE=testnet`에서는 DB-only paper close 대신 Binance reduce-only market flatten + deterministic SL/TP algo cancel 수행. paper/testnet open protected order 로딩은 mode별로 분리.
- **인증 기본값 강화**: backend는 non-loopback `API_HOST`에서 `DASHBOARD_CONTROL_TOKEN` 없으면 기동 실패. dashboard production runtime은 `DASHBOARD_PASSWORD`/`DASHBOARD_SESSION_SECRET` 없으면 인증 bypass가 아니라 500 설정 오류. `/ws/dashboard`도 header 또는 `?token=` 검증.
- **testnet risk state 개선**: testnet entry risk gate가 hardcoded daily PnL 0 대신 DB 기반 `load_account_risk_state` 사용. candle close로 open position mark 갱신. paper_exit 없는 closed position도 mark/entry 기준 realized PnL로 계산.
- **URL/시크릿 로그 누출 차단**: Binance signed URL/Telegram bot URL이 reqwest transport error 문자열에 포함되지 않도록 `without_url()` 적용.
- **운영 정리**: `PAPER_TRADING_ENABLED=true` + `TRADING_MODE!=paper` 조합 차단. 주요 numeric env 양수 검증. dashboard `lint`는 Next 16에서 제거된 `next lint` 대신 `tsc --noEmit --incremental false`. `configs/example.env`에 dashboard password/session secret 추가. unused workspace direct dep `futures-core` 제거.
- **검증 완료**: `cargo test --workspace` 통과(총 96 passed), `cargo clippy --workspace --all-targets` exit 0(잔여 warning 1개: 기존 `ai_repository::persist_ai_decision` 인자 수), `npm run lint` 통과, `npx tsc --noEmit` 통과, `npm run build` 통과.

## ▶ 재개 지점 — "남은 후속" (우선순위 순)
1. ~~**🟡 보호주문 e2e 라이브 확인**~~ ✅ **종결 (2026-06-15 01:46, 라이브 검증 완료)**: 봇 상주 중 ETHUSDT RSI 과매도(≈12) 자연 진입 발생 → **보호주문 전체 경로 성공**. risk_event(severity=**info**, action=order_submitted): `order_status:FILLED`·`protection_ok:true`·`stop_loss_order_id`/`take_profit_order_id` 발급. testnet 거래소 직접 확인: openAlgoOrders에 SL/TP 양쪽 `algoStatus:NEW`(TP triggerPrice 1698.1, clientAlgoId `...-sl`/`-tp`), positionRisk ETHUSDT amt 0.030 @1665.64. **-4120 LOCK 재발 없음** — 새 Algo API(`659f50e`) 라이브 확정. ⚠️**이 진입 직전 WS freeze 결함 발견·수정**(아래 신규).
- ✅ **신규 수정: WS silent-stall 재연결 (`077ee9e`)**: e2e 모니터링 중 Binance 시장스트림이 16:06경 **에러 없이 freeze**(half-open, read.next() 영구 hang) → 재연결 안 됨 → latency-gate가 진입 690건 차단(봇 진입불가). 3거래소(binance/bitget/bybit) WS read 루프에 `MARKET_STREAM_IDLE_TIMEOUT=30s`(stall 시 reconnect) + read 에러 시 return 추가. silent-WS 통합테스트. 봇 재시작 후 freeze 재발 0·Bitget 끊김→자동복구 확인. **latency-gate는 정상 작동했음**(stale 데이터 거래 차단=제 역할).
2. ~~**🟠 Bitget 8시간 anomaly**~~ ✅ **해결(`d389631`)**: 라이브 캡처로 근본원인 확정 — Bitget v2 ws candle 구독 시 `action:"snapshot"`으로 **500개 과거 캔들을 오래된순(asc)** 전송, `parse_kline`이 `.next()`로 가장 오래된 행(499분 전) 선택 → open_time 8h 과거. **수정=`.next_back()`**(최신 행 선택, 단일 update는 동일). RED→GREEN + 라이브 검증(Bitget latency 8h→0ms, block 0건). 캡처 스크립트=`/tmp/bitget_capture.py`
3. ~~**🟠 position sweep**~~ ✅ **기동 시 sweep 구현+라이브 검증 (2026-06-15, `7104483`)**: 라이브에서 결함 실증 — TP가 거래소에서 정상 체결돼 +0.75 익절(청산 e2e 거래소측 성공)했으나 **맥북 OS 절전(sleep) 중 봇 freeze로 청산 fill을 못 받아 DB가 고아 포지션 보유**. 재시작 시 `load_open_position_keys`가 DB만 믿어 stale 키로 재진입 차단 위험. **수정=기동 시 1회 sweep**(`testnet_runtime.rs`): DB 키 복원 후 `fetch_account_snapshot`→`exchange_open_position_keys`로 거래소 실제 오픈과 대조→`orphaned_position_keys`(거래소에 없는 DB 키)를 `close_orphaned_positions_for_key`(`execution_repository.rs`, status=`reconciled_closed_offline`)로 정리+in-memory set에서 제거. snapshot 실패 시 키 유지(보수적). 순수헬퍼 유닛테스트2+DB테스트2(idempotent·타모드불간섭). **라이브 검증: 재시작→고아 ETH 자동 closed(11:10:53)·DB↔거래소 일치**. ⚠️**남은 갭(선택)**: 절전 중 freeze는 idle-timeout(30s 타이머)이 절전 중엔 안 돎→깨어난 직후 stale, 그리고 sweep는 기동 시 1회뿐이라 **장시간 가동 중 발생하는 청산은 주기적 sweep 없으면 다음 재시작까지 미반영**. 주기적(캔들 루프 N분마다) sweep는 후속 과제.
4. **(선택) signal.id 안정화**: `strategy/src/lib.rs:100` `Uuid::new_v4()` 캔들마다 새 id. (현재 무해 — open_position_keys가 1차 가드)
5. **(선택) timeframe CI 가드**: 구독 채널 리터럴(1m)이 `timeframe_duration`에서 `Some` 반환하는지 검증하는 테스트. 비-1m 구독 추가 시 빌드 깨져 게이트 무음 회귀 방지(적대적 리뷰 제안, defer)

## 🔧 다음 세션 부팅 명령어 (그대로 복붙)
```sh
cd ~/Documents/Rust/trading-system

# 1) 테스트 DB 준비 (이미 있으면 CREATE는 "already exists"로 스킵됨 — DROP 금지: Bash 가드가 막음)
set -a; source .env; set +a
ADMIN_URL="$DATABASE_URL"
psql "$ADMIN_URL" -tAc "SELECT 1 FROM pg_database WHERE datname='trading_system_test';" | grep -q 1 \
  || psql "$ADMIN_URL" -c "CREATE DATABASE trading_system_test;"
export TEST_DATABASE_URL="${DATABASE_URL%/*}/trading_system_test"

# 2) 그린 베이스라인 확인 (반드시 96 passed / 0 failed 여야 함)
cargo test --workspace 2>&1 | grep -oE "[0-9]+ passed" | awk '{s+=$1} END{print s" passed"}'
cargo fmt --all -- --check && echo "fmt clean"
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^(warning|error):"   # 1 = 기존 ai_repository 인자 수 warning
```

## ⚠️ 함정 / 주의 (재발 방지)
- **git repo 루트 = 부모 `~/Documents/Rust`** (trading-system 아님). `git status`에 `trading_system_implementation_plan.html` 같이 잡히는 게 정상
- **`.env` 절대 커밋 금지** — `.gitignore`의 `/.env`가 `trading-system/.env`를 가림(검증됨). 커밋 전 항상 `git diff --cached`로 시크릿 스캔
- **Bash 안전가드**: `DROP DATABASE`·`rm -rf 홈` 등 차단됨. 테스트 DB는 DROP 말고 CREATE(있으면 스킵)로
- **trading-api는 binary crate** — 테스트 실행은 `cargo test -p trading-api --bin trading-api <필터>` (—lib 아님)
- **DB 통합 테스트**는 `TEST_DATABASE_URL` 없으면 조용히 skip됨. 0건 skip 확인할 것
- 보호가격 반올림 방향: **LONG=floor, SHORT=ceil** (스탑이 entry로 안 당겨지게). `round_protection_price(price, side)`
- 머니 코드 수정은 항상 **재현 테스트(RED) → 수정 → GREEN** + 적대적 워크플로 검증
- **타임아웃≠실패**: 주문 HTTP 타임아웃은 결과 UNKNOWN(체결됐을 수 있음). `TradingError::Timeout`으로 분류(`map_request_error`). 진입 타임아웃은 **reconcile**(`reconcile_entry_timeout`): query_order로 실제상태 조회→체결이면 보호, 미체결/없음이면 skip, 조회실패면 LOCK. 절대 일반 실패처럼 silent skip 금지
- **reconcile는 단일 조회 신뢰 금지**: 타임아웃 직후 1회 조회는 stale일 수 있음(체결됐는데 None/NEW). 반드시 `query_order_until_settled`로 재시도(미체결/없음만, Err은 즉시 surface). 미체결 결론은 재시도 소진 후에만
- 보호실패/타임아웃 복구·보호배치는 인라인 금지 → `handle_protection_failure`·`handle_entry_timeout`·`finalize_entry_with_protection`·`reconcile_entry_timeout` 함수로(테스트 가능 seam)
- **캔들 latency는 close_time 기준**(절대 open_time 아님): 1분봉 open_time=봉 시작이라 정상 틱도 7-10초로 읽혀 게이트가 모든 진입 차단. `MarketEvent::event_time()` Candle arm=`candle.close_time()`(=open_time+interval). 오더북은 진짜 event_time(불변). `timeframe_duration`은 대소문자 무시(Bitget `1H`)+미인식은 open_time 폴백(fail-safe). 머니게이트 진단은 **DB 실측 우선**(candles/order_books로 클럭스큐 배제)+적대적 리뷰 필수(초기 진단이 틀릴 수 있음)
- **Bitget 구독 스냅샷=500개 과거캔들 오래된순(asc)** → 캔들파서는 `.next_back()`(최신행). `.next()`면 8h 과거 open_time
- **보호주문(SL/TP)은 Algo API**: Binance가 2025-12-09부로 조건부주문(STOP_MARKET/TAKE_PROFIT_MARKET/STOP/TAKE_PROFIT/TRAILING_STOP_MARKET)을 Algo Service 이전. `/fapi/v1/order`로 보내면 **-4120 거부**. 반드시 `/fapi/v1/algoOrder` + `algoType=CONDITIONAL` + `triggerPrice`(stopPrice 아님). `order_id_from_value`는 이미 `algoId` 폴백 있음. testnet 검증=`/tmp/verify_protection_codepath.py`. 거래소 API 에러는 **공식 문서+실제 testnet 재현** 두 증거로 진단(추측 금지)
- 서브에이전트는 항상 `model: "opus"`

## 📚 핵심 문서
- `docs/verification-notes.md` — 검증 로그(이번 수정 전부 기록됨), `docs/phase3·6-*.md`, `docs/runbook.md`
