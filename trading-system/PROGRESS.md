# PROGRESS — trading-system (Rust 코인선물 AI 트레이딩)

> 마지막 갱신: 2026-06-14. 다음 세션에서 이 파일부터 읽고 "재개 지점"으로 이동.

## 현재 상태 (한 줄)
전체 코드리뷰 + 머니 코렉트니스 6종(C1·C2·C3·H1·H2·H3) 수정 완료, 커밋·푸시 끝.
**테스트 59 통과/0 실패, fmt clean, clippy 신규경고 0.** 다음은 아래 "남은 후속" 중 선택.

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

## ▶ 재개 지점 — "남은 후속" (우선순위 순, 이번 범위 밖이었음)
1. **M: HTTP 요청 타임아웃 부재** — `binance.rs` `Client::new()`에 `.timeout(10s)` 추가. 주문이 전략 루프 무한 블록 방지. (가장 쉬움·실효성 큼)
2. **M: 멱등성 키(newClientOrderId) 부재** — `binance.rs` `place_market_order`에 `signal_id` 기반 결정적 client order id 추가. 타임아웃 재시도 시 중복주문/대사 방지
3. **C3 end-to-end 테스트** — 보호실패 전체 시퀀스(lock→flatten→CRITICAL risk_event→alert→open_key 미등록) 통합 테스트. 현재 `flatten_position` 단위 테스트만 있음
4. **운영 확인(코드 아님)**: `demo-fapi.binance.com`이 실제 데모/테스트넷인지 testnet 키와 함께 1회 확인 (안전모델 전체가 이 host 문자열에 의존)

## 🔧 다음 세션 부팅 명령어 (그대로 복붙)
```sh
cd ~/Documents/Rust/trading-system

# 1) 테스트 DB 준비 (이미 있으면 CREATE는 "already exists"로 스킵됨 — DROP 금지: Bash 가드가 막음)
set -a; source .env; set +a
ADMIN_URL="$DATABASE_URL"
psql "$ADMIN_URL" -tAc "SELECT 1 FROM pg_database WHERE datname='trading_system_test';" | grep -q 1 \
  || psql "$ADMIN_URL" -c "CREATE DATABASE trading_system_test;"
export TEST_DATABASE_URL="${DATABASE_URL%/*}/trading_system_test"

# 2) 그린 베이스라인 확인 (반드시 59 passed / 0 failed 여야 함)
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
- 서브에이전트는 항상 `model: "opus"`

## 📚 핵심 문서
- `docs/verification-notes.md` — 검증 로그(이번 수정 전부 기록됨), `docs/phase3·6-*.md`, `docs/runbook.md`
