# PROGRESS — trading-system (Rust 코인선물 AI 트레이딩)

> 마지막 갱신: 2026-06-15 (★보호주문 e2e 라이브 검증 종결 + WS silent-stall 재연결 수정). 다음 세션에서 이 파일부터 읽고 "재개 지점"으로 이동.

## 현재 상태 (한 줄)
머니6종 + 후속 + reconcile + **latency-gate + Bitget 8h + 보호주문 Algo API(+e2e 라이브 종결) + 출시 전 감사 P0/P1 + runbook 정합성 + WS silent-stall 재연결 완료**.
**전체 테스트 99 통과/0 실패 (병렬 `cargo test` 반복 green — flaky 해소됨), fmt clean, clippy warning 1(기존 ai_repository 인자 수).** ⚠️**DB 통합테스트는 `TEST_DATABASE_URL` 필요**(미설정 시 skip). 로컬 검증용 DB=`trading_system_test`(로컬 PG).
✅ **보호주문 e2e 라이브 종결 (2026-06-15 01:46)**: 봇 상주 중 ETHUSDT 자연 진입(RSI≈12) → 보호주문 LOCK 없이 통과·SL/TP algoOrder 거래소 등록(algoStatus:NEW) 확정. 상세는 "재개 지점" §1.
✅ **WS silent-stall 재연결 (`077ee9e`)**: 3거래소 WS read 루프에 idle 30s 타임아웃+read에러 시 reconnect. e2e 모니터링 중 Binance freeze 발견→수정. 상세는 "재개 지점" §1 하위.
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

## 🚀 다음 세션 첫 액션 (clear 후 여기부터)
1. 위 "부팅 명령어" 복붙 → **96 passed / 0 failed · fmt clean · clippy warning 1** 확인(그린 베이스라인)
2. **세 버그 모두 해결·검증됨**(latency-gate / Bitget 8h / 보호주문 Algo). git `main` = `f936ab9` 동기화 완료.
3. **⚙️ testnet 봇은 직전 세션 종료 시 kill 완료**(실행 중 아님). 다시 띄우려면 "재개 지점" 위의 봇 실행 명령 또는 `set -a; source .env; set +a && RUST_LOG=trading_api=debug,info cargo run -p trading-api --bin trading-api`. 검증 로그는 `/tmp/trading-bot-run3.log`(latency 0~128ms 확인 가능, 참고용).
4. **다음 할 일(선택)**: 아래 "재개 지점"의 ①보호주문 e2e 라이브(진입→보호 전체경로, 시장 과매도 시 자연발생) ②position sweep ③signal.id 안정화 ④timeframe CI 가드. **급한 버그 없음** — 추가 작업 안 하면 현 상태로 종결 가능.
5. 트리거 문구: "rust 트레이딩 이어서 작업"

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
3. **(선택) 주기적 position sweep**: 캔들 루프에 주기적 `fetch_account_snapshot`로 실제 포지션 vs `open_position_keys` 대조(defense-in-depth). reconcile 리뷰 (b)안
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
