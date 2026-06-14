# PROGRESS — trading-system (Rust 코인선물 AI 트레이딩)

> 마지막 갱신: 2026-06-14 (3차 세션). 다음 세션에서 이 파일부터 읽고 "재개 지점"으로 이동.

## 현재 상태 (한 줄)
머니 코렉트니스 6종(C1~H3) + 후속 #1·#2·#3 + **타임아웃 자동복구 reconcile(+재시도 하드닝)** 완료.
**테스트 78 통과/0 실패, fmt clean, clippy 13(신규 0).** DB 통합테스트 0건 skip 확인.
다음은 아래 "남은 후속" 참조(운영확인 + 선택적 position sweep).

## 위치 / 저장소
- 작업 경로: `~/Documents/Rust/trading-system` (⚠️ git repo 루트는 **부모** `~/Documents/Rust`)
- 원격: https://github.com/joochanyang/RUST.git (`main`, 추적됨)
- 마지막 커밋: `ce80038` "Fix money-correctness and order-safety bugs in trading system"
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

## 🚀 다음 세션 첫 액션 (clear 후 여기부터)
1. 위 "부팅 명령어" 복붙 → **78 passed / 0 failed · fmt clean · clippy 13** 확인(그린 베이스라인)
2. 그다음 아래 "재개 지점"을 **우선순위 순**으로. 단, **#1은 사용자 입회 필요**(testnet 키) — 코드 작업은 #2가 첫 후보
3. 트리거 문구: "rust 트레이딩 이어서 작업"

## ▶ 재개 지점 — "남은 후속" (우선순위 순)
1. **⚠️사용자 입회 필요 / 운영 확인(코드 아님)**: `demo-fapi.binance.com`이 실제 데모/테스트넷인지 testnet 키와 함께 1회 확인 (안전모델 전체가 이 host 문자열에 의존). → Claude 단독 진행 불가, 사용자가 키·확인 제공해야 함
2. **(코드, 다음 작업 1순위) 주기적 position sweep**: 캔들 루프에 주기적 `fetch_account_snapshot`로 실제 포지션 vs `open_position_keys` 대조 → 모든 경로의 무방비 포지션 사후 감지(defense-in-depth). reconcile 리뷰에서 제안된 (b)안, 별도 기능. TDD + 적대적 리뷰로 진행
3. **(선택) signal.id 안정화**: `strategy/src/lib.rs:100` `Uuid::new_v4()`가 캔들마다 새 id → 같은 시장조건 재진입은 다른 멱등키. (현재 무해 — open_position_keys가 1차 가드)

## 🔧 다음 세션 부팅 명령어 (그대로 복붙)
```sh
cd ~/Documents/Rust/trading-system

# 1) 테스트 DB 준비 (이미 있으면 CREATE는 "already exists"로 스킵됨 — DROP 금지: Bash 가드가 막음)
set -a; source .env; set +a
ADMIN_URL="$DATABASE_URL"
psql "$ADMIN_URL" -tAc "SELECT 1 FROM pg_database WHERE datname='trading_system_test';" | grep -q 1 \
  || psql "$ADMIN_URL" -c "CREATE DATABASE trading_system_test;"
export TEST_DATABASE_URL="${DATABASE_URL%/*}/trading_system_test"

# 2) 그린 베이스라인 확인 (반드시 78 passed / 0 failed 여야 함)
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
- 서브에이전트는 항상 `model: "opus"`

## 📚 핵심 문서
- `docs/verification-notes.md` — 검증 로그(이번 수정 전부 기록됨), `docs/phase3·6-*.md`, `docs/runbook.md`
