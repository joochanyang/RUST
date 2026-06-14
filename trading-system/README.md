# Trading System

Rust 기반 코인 선물 AI 하이브리드 트레이딩 시스템입니다. 기본 실행 모드는 `paper`이며, `live` 모드는 명시적인 승인 플래그 없이는 시작되지 않습니다.

## Phase 1 Scope

- Rust workspace와 crate 경계
- 공통 도메인 타입
- 환경 변수 기반 설정 로더
- JSON structured logging
- PostgreSQL migration
- Axum health API

## Local Run

```sh
cp configs/example.env .env
cargo run -p trading-api
```

헬스체크:

```sh
curl http://127.0.0.1:8080/api/health
```

## Safety Defaults

- `TRADING_MODE=paper`가 기본값입니다.
- `TRADING_MODE=testnet`은 Binance Futures testnet 전용 모의투자 모드입니다. `BINANCE_TESTNET_ENABLED=true`와 testnet 전용 API 키가 필요합니다.
- `TRADING_MODE=live`는 `LIVE_TRADING_APPROVED=true` 없이는 서버가 시작되지 않습니다.
- `DASHBOARD_CONTROL_TOKEN`을 설정하면 운영 제어/증거 기록 POST API는 `x-dashboard-control-token` 헤더가 있어야 실행됩니다.
- Telegram 리모콘은 `TELEGRAM_ALLOWED_CHAT_ID`와 일치하는 채팅에서만 명령을 처리합니다.
- 실거래/API 키/Telegram 토큰은 저장소에 커밋하지 않습니다.
