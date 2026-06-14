# PROGRESS — trading-system (Rust 코인선물 AI 트레이딩)

> 마지막 갱신: 2026-06-14 (4차 세션). 다음 세션에서 이 파일부터 읽고 "재개 지점"으로 이동.

## 현재 상태 (한 줄)
머니 코렉트니스 6종 + 후속 #1·#2·#3 + reconcile + **latency-gate 버그 수정 완료**(4차 세션).
**테스트 85 통과/0 실패, fmt clean, clippy 13(신규 0).** DB 통합테스트 0건 skip 확인.
✅ **운영 차단 버그(latency gate) 해결**: 캔들 freshness를 `close_time` 기준으로 측정하도록 수정 → 정상 캔들 latency≈0, 진입 차단 해소. 커밋·푸시 완료(`0fee96f`).

## 위치 / 저장소
- 작업 경로: `~/Documents/Rust/trading-system` (⚠️ git repo 루트는 **부모** `~/Documents/Rust`)
- 원격: https://github.com/joochanyang/RUST.git (`main`, 추적됨)
- 마지막 커밋: `0fee96f` "Fix latency gate blocking all entries by measuring candle freshness against close_time"
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
1. 위 "부팅 명령어" 복붙 → **85 passed / 0 failed · fmt clean · clippy 13** 확인(그린 베이스라인)
2. latency-gate 버그는 **해결됨**. 다음은 아래 "재개 지점"의 운영 재구동(testnet >21분 런 후 주문 발생 확인) 또는 선택 후속(Bitget 8h·position sweep)
3. 트리거 문구: "rust 트레이딩 이어서 작업"

## ✅ 운영 확인 완료 (2026-06-14, 사용자 키로 검증)
- **`demo-fapi.binance.com` = 진짜 testnet 확정**: 사용자 testnet 키로 signed `GET /fapi/v3/account` → HTTP 200 + **testnet 가짜잔고**(USDT 5282·USDC 5000·BTC 0.01). 같은 키를 실거래 `fapi.binance.com`에 치면 **HTTP 401 `-2015`**(거부) → testnet 전용 확정, 실거래 자금 위험 없음
- **testnet 모드 ON**: `.env`(gitignore, 커밋 안 됨) → `BINANCE_TESTNET_API_KEY/SECRET`(각 64자) 설정 + `BINANCE_TESTNET_ENABLED=true` + `TRADING_MODE=testnet` + `MARKET_DATA_ENABLED=true`(testnet 게이트 필수). `BINANCE_TESTNET_MAX_ORDER_NOTIONAL=50`(주문당 상한). ⚠️채팅으로 평문 전송된 키라 rotate 권장
- 부팅 검증: `cargo run -p trading-api --bin trading-api` → settings 게이트 통과·API listening·시장스트림 수신 OK

## ▶ 재개 지점 — "남은 후속" (우선순위 순)
1. **🟡 운영 재구동 검증 (latency 수정 후)**: `cargo run -p trading-api --bin trading-api`로 testnet 재구동 → 로그에서 캔들 `latency_ms`가 ~0으로 떨어지는지 + **>21분 런** 후(TechnicalStrategy는 ≥21 캔들 워밍업 필요, 분당 1캔들) 실제 주문 발생하는지 확인. 주문 여전히 0이면 워밍업 부족/심볼필터 min-notional 스킵(testnet_runtime.rs:201-228)/AI 게이트 의심 — latency 잔여 버그 아님
2. **🟠 Bitget 8시간 anomaly (조사→수정, 별개 커밋)**: `candles` DB 실측 = bitget `08:38/08:39` 고립행 **29,946,427ms(~8h19m)**. 가설=구독 스냅샷 배치에서 `bitget.rs` `parse_kline`이 `data.into_iter().next()`로 **가장 오래된 행** 선택(`action:"snapshot"` 미파싱). 1단계(동작보존): `BitgetKlineEnvelope`에 `#[serde(default)] action: String` + `tracing::warn!(rows, action, first_ts, last_ts)` 로깅 커밋. 2단계(운영): 재구동·재연결로 8h가 `action=="snapshot"`·`data.len()>>1`과 1:1 상관 확인. 3단계(확인 후, RED 테스트): 오래된순이면 `.next()`→최신행 선택. **⚠️ 정렬(asc/desc) 라이브 캡처로 확인 후 커밋**(latency-gate는 이미 8h를 차단하므로 긴급도 낮음 — 위험은 stale price 마크/시그널)
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

# 2) 그린 베이스라인 확인 (반드시 85 passed / 0 failed 여야 함)
cargo test --workspace 2>&1 | grep -oE "[0-9]+ passed" | awk '{s+=$1} END{print s" passed"}'
cargo fmt --all -- --check && echo "fmt clean"
cargo clippy --workspace --all-targets 2>&1 | grep -cE "^(warning|error):"   # 13 = 기존 baseline (신규 0)
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
- 서브에이전트는 항상 `model: "opus"`

## 📚 핵심 문서
- `docs/verification-notes.md` — 검증 로그(이번 수정 전부 기록됨), `docs/phase3·6-*.md`, `docs/runbook.md`
