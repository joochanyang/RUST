# PROGRESS — trading-system (Rust 코인선물 AI 트레이딩)

> 마지막 갱신: 2026-06-16 (📡**오더북-임밸런스 가설용 장기 캐처 — 선결 코드 3건 완료·푸시**: per-stream staleness 재연결(`6056de3`)+무거래 템플릿(`7f05862`)+1초 다운샘플(`2536828`). 거래/전략/캔들/latency 경로 전부 불변(적대적 검증). 133 passed. **결정 완료(사용자)**: 배포=**Hetzner**, 저장=**1초 샘플링**(Hetzner 63G 여유라 raw면 ~2주 꿉참). **★Hetzner 라이브 배포 완료(2026-06-16)**: 무거래 캐처 가동 중(6피드 ~1.0 row/sec·1초 샘플·주문0·RestartCount=0). deploy key로 clone→compose up. **헬스 재점검 2026-06-16: 전부 healthy**(RestartCount=0·6피드 ~1.0row/s·디스크58%·watchdog OK). **✅imbalance 사전등록 분석 미리 작성+로컬검증 완료**(spec md + `deploy/analysis/` IC·분위수 SQL·README, 합성데이터로 4판정브랜치 실측, ✅**커밋됨**(이전 메모 "미커밋"은 stale)). **다음=몇 주 방치하며 적재 → 게이트 'ready'면 분석 실행**(지금은 INCONCLUSIVE). 모니터법=`deploy/README.md`. 상세=「📡 진행 중」섹션. ▼이전: 🛑**가격방향 탐색 종료**(4계열 falsify 수용, 검증 인프라 영구 보존). 상세=🛑 박스. ▼더 이전: ★★★**일봉(1d) 돌파도 effectively-FAIL — 돌파/평균회귀 계열 4번째 falsify, BTC/ETH 2023-26 레짐에서 가격방향 엣지 없음 확정**. 1d WFO 3윈도우(IS12mo→OOS6mo, 60일 pre-roll, n=3 verdict) 실행: 기계판정=INCONCLUSIVE(2윈도우 floor미달)였으나, **풀링하면 powered FAIL**: 53거래·15승(28.3%)·1x PnL−66.46·P(≤15승|공정동전)=0.0011. floor 미달 윈도우 둘 다 0근처 아닌 **유의한 음수**(−46.89/−29.51)라 floor가 증거를 은폐 중이었음. 유일 양수 W3(+9.94)은 2×비용서 −0.08 사망. IS-best 3개 다 음수+파라미터 텔레포트=오버핏 시그니처. 라이브 불변). **★★다음 세션=구조적으로 다른(가격방향이 아닌) 가설 or 멈춤** — "🚀 다음 세션 첫 액션" 읽기. ⭐일봉 검증 인프라(pre-roll `eval_start`·n=3 verdict·1d 롤업)는 영구 유지, 커밋됨.

## 🛑 결정 — 가격방향 탐색 정직하게 종료 (2026-06-16, 사용자 확정)
**트리거 문구: "rust 트레이딩 이어서 작업" → 먼저 이 박스를 읽을 것**

> **사용자 결정(2026-06-16)**: 위 분기점에서 **"여기서 멈춤(정직한 멈춤)"** 선택. 가격방향 전략 4계열이 1m/5m/1h/1d 전부 OOS 무엣지로 falsify된 증거를 받아들여, **"BTC/ETH 2023-26 레짐 + 현 도구셋(캔들 OHLCV)으로 착취 가능한 가격방향 엣지 없음"으로 결론짓고 종료**. PROGRESS.md가 최고 EV로 플래그했던 선택지.
> **시스템 상태**: 클린·`main` `b0d2003`·origin 동기. 127 passed/0 failed/4 ignored. 봇 안 떠있음·미결 testnet 포지션 0.
> **영구 보존된 검증 인프라(재개 시 그대로 재사용)**: 워크포워드+OOS+수수료(1×/2×/3×)+3진판정(PASS/FAIL/INCONCLUSIVE)+거래수 floor+pre-roll(`eval_start`)+적대적 설계/판정 리뷰. 코드는 전부 `#[ignore]` 영구 테스트로 남아있음(`walk_forward_*` in `backtest_runner.rs`).
> **재개 조건(아무거나 충족 시에만)**: ⚠️**가격방향 변형(돌파/평균회귀/추세추종)을 같은 BTC/ETH 레짐에서 또 돌리는 것은 금지** — family-wise 거짓양성률만 키움(메타 교훈, 아래). 재개하려면 둘 중 하나가 필요: (a) **구조적으로 다른(가격방향과 직교한) 가설** + **사전등록 데이터근거**(엣지를 *예측하는* 분석. 후보=펀딩비/캐리·횡단면 상대가치·변동성 레짐), 또는 (b) **새 데이터/레짐**(다른 종목·다른 거시국면·오더북/펀딩비 등 캔들 외 신호 → `Strategy::evaluate` trait 확장 필요). 근거 없는 시도는 시작하지 말 것.
> ▼아래는 멈춤 직전의 "다음 후보" 상세 메모(재개 시 참고용으로 보존).

## 📡 진행 중 — 오더북-임밸런스 가설용 장기 캐처 (2026-06-16~) **★Hetzner 라이브 배포 완료**
**재개 트리거: "rust 트레이딩 이어서 작업" → 이 섹션이 활성 작업**

> **🟢 라이브 상태(2026-06-16 배포)**: Hetzner(`5.161.112.248`)에 **무거래 캐처 가동 중**. 컨테이너 `trading-capture`+`trading-capture-postgres`(전용 DB) RestartCount=0·healthy. 6피드(binance/bybit/bitget×BTC/ETH) **~1.0 row/sec**(1초 샘플 정확 작동), row 신선도 <0.5s. `paper_trading=false`·주문 0(캐처-only 확인). Bitget WS 리셋→자동재연결 1회 관측(정상). 배포: deploy key(read-only, id 154537679)로 git clone→`docker compose --env-file .env -f deploy/docker-compose.capture.yml up -d --build`.
> **🛡 watchdog 상주(2026-06-16, `973d370`)**: `/opt/trading-capture/watchdog.sh` + cron `*/5`. 컨테이너 down→`up -d`, `order_books` freshness>180s(stall)→capture restart, 디스크>85%→경보(로그만). 로그 `/var/log/trading-capture-watchdog.log`. **검증**: 헬시=무동작 OK 1줄 / `docker stop` 강제→감지·자동복구·row 재개 실측. compose `restart:unless-stopped`+staleness 자가복구가 못 잡는 케이스(non-crash·PG정지·디스크)를 메움. 스크립트는 repo `deploy/watchdog.sh`에 버전관리.
> **모니터/재시작 = `deploy/README.md`**. **다음 = 몇 주 데이터 쌓일 때까지 방치(watchdog가 지킴) → 충분해지면 imbalance 사전등록 분석**(아래 4번).
> **✅ 헬스 재점검(2026-06-16 후속 세션)**: RestartCount=0·6피드 ~1.0 row/sec(bitget 0.89, delta채널 정상)·freshness<1s·디스크 58%(84/150G)·watchdog 매5분 `OK` 연속·pgdata 69MB. 전부 healthy, 코드/git 변경 0. ⚠적재율 실측=초반 ~90MB/h(=하루 ~2GB, README 낙관치 50-100MB/일보다 ~20배 — 단 PG초기 오버헤드 포함이라 며칠 뒤 재측정 필요. 최악이어도 61G여유=~30일).
> **✅ 헬스+게이트 점검(2026-06-16 11:30 KST = 02:32 UTC, "rust 작업 이어서")**: SSH read-only 진단. 서버 109일 uptime·디스크 **25%**(109G free, 이전 58%→Docker 클린업 효과)·RAM 4.4G avail(swap 1.8/2.0 빡빡하나 안정)·캐처 컨테이너 둘 다 **Up 9h·RestartCount=0**·freshness <1s·6피드 라이브.
>   - 🔴**중대 발견 — 데이터 적재 윈도우가 9.5h뿐**: `order_books` span = 06-15 17:03 ~ 06-16 02:32 UTC = **9.5시간**, 총 192,712행(33MB). "몇 주 적재" 아직 시작 못 함. **원인**: 이전 세션(Hetzner Docker 디스크 클린업, 06-16 03:07~04:43 KST = 18:00~19:40 UTC)이 캐처 스택을 recreate(RestartCount=0=크래시 아님)하며 **전용 PG 데이터가 리셋**된 것으로 강하게 추정(데이터 시작 17:03 UTC가 컨테이너 "Up 9h"와 일치). 즉 캐처는 정상 가동 중이나 **시계가 9.5h 전부터 다시 셈**. ⚠️클린업이 캐처를 건드릴 수 있음=함정(다음 클린업 시 캐처 PG 볼륨 보호 필요).
>   - **게이트 = 6/18 ready**(10s horizon만 floor 2000 통과: 2787~3423샘플). 30s(904~1141)·60s(439~570) 전부 `wait`. **바인딩 제약=60s horizon**(README: 60s floor 2000 ≈ 33h 연속). 9.5h→60s 570샘플뿐. 모든 horizon powered까지 **최소 며칠~계획대로 몇 주** 더 필요.
>   - 🔧**정정(메모 틀림)**: ①캐처 PG creds = user **`trading`** / db **`trading_system`**(`postgres` 아님 — README 게이트 쿼리는 `-U trading`로 이미 맞음, 헬스점검 명령만 주의). ②`order_books` 시각 컬럼 = **`event_time`**(`recorded_at` 아님). ③컬럼=id/exchange/symbol/event_time/best_bid/best_ask/bid_size/ask_size/created_at. ④현 디스크 **25%**(이전 메모 58% 갱신).
>   - ⚠️**watchdog 게이트체크 미배포 확인**: 서버 `/opt/trading-capture/watchdog.sh` = 옛 버전(`grep -c gate`=0). `431a0897`(게이트체크판)이 repo엔 있으나 서버 미반영 → 다음 SSH 점검 때 `install -D -m755 deploy/watchdog.sh /opt/trading-capture/watchdog.sh`로 갱신. 현재 watchdog는 헬스(컨테이너/freshness/디스크)만 정상 감시 중(`OK` 연속).
>   - **다음**: 계속 방치 적재. **3진 게이트가 60s까지 `ready`(18/18)** 되는 시점(이번 9.5h 기준 ~며칠 더, 단 재리셋 없어야)에 사전등록 IC·분위수 분석 실행. 그 전엔 INCONCLUSIVE. 재점검 트리거="rust 캐처 적재율이랑 게이트 확인해줘".
> **✅ 헬스+게이트 재점검(2026-06-16 14:31 KST = 05:34 UTC, "rust프로젝트 확인" 세션)**: SSH read-only. 서버 109일 uptime·디스크 **25%**(109G free)·RAM 4.5G avail·캐처 컨테이너 둘 다 RestartCount=0·healthy.
>   - **적재 윈도우 = 12.48h**(`order_books` 06-15 17:03:14 ~ 06-16 05:32 UTC, 253k행, 44MB). ⚠️**재리셋은 없었음** — 시작시각 17:03 UTC가 이전 세션(9.5h)과 **동일** → 같은 클린업 시점부터 끊김 없이 계속 적재 중(9.5h→12.5h). 즉 추가 데이터 유실 없음.
>   - **게이트 = 6/18 ready**(10s horizon 4503샘플 전부 통과 / 30s 1501·60s 750 전부 wait). **바인딩=60s horizon**(floor 2000≈33h 연속, 현재 12.5h). 18/18까지 며칠 더.
>   - **6피드 전부 라이브**: binance/bybit ~1.0 row/s·freshness<1.1s, bitget 0.82 row/s. ⚠️**bitget freshness 일시 ~20s 관측됐으나 직후 2분 윈도우서 0.74s·178 distinct secs로 정상 확인**(델타채널 순간 갭, stall 아님). 워치독 5분마다 `OK`+게이트로그(`imbalance gate waiting: 6/18`).
>   - **✅ watchdog 게이트체크 배포 확인**: 이전 세션 미배포였던 게이트체크판이 서버에 반영됨(`grep -c gate`=20, 게이트로그 실측). ready-latch(`/tmp/trading-capture-gate.ready`) 없음=정상(18/18 아님). **이전 세션 "watchdog 게이트 미배포" 우려 해소.**
>   - **로컬**: testnet 봇 가동 중(pid 86562, 진입0=정상 횡보), git clean `a90e6619`, 분석스크립트+spec 전부 커밋됨, 3일 리마인더 launchd 로드됨.
> **✅ testnet 실거래 배관 검증 + 텔레그램 알림 리포맷 (2026-06-16, 커밋·푸시 `7ac93b19`)**: 사용자 "testnet으로 실거래 한 번 돌려봐" → 로컬 봇 기동(`cargo run -p trading-api`, `.env`는 **testnet 거래모드**=`TRADING_MODE=testnet`+`BINANCE_TESTNET_ENABLED=true`+`PAPER_TRADING_ENABLED=false`). **배관 전부 정상 검증**: testnet 키 유효(signed balance HTTP 200, 5281 USDT)·`paper_trading:false` 루프 spawn(`main.rs:161` `if mode==Testnet`)·6피드 마켓데이터 latency 0~12ms·latency-gate block 0·position sweep 통과(risk_events 0=고아 없음)·lock 0. **진입 주문은 미발생**(횡보 — `VolatilityBreakoutStrategy` lookback=20 윈도우 돌파해야 신호, signals 0건). ⚠️**정상**: 메모리 박제 "주문 0=과매도 아니라 정상 횡보"와 동일. 즉 **진입 빼고 전 배관(API연결·주문·보호·sweep·락) 작동 확인**, 진입은 시장 돌파 待. ⚠️Bitget WS 리셋→자동재연결(정상). **★텔레그램 알림 리포맷**(사용자 "보기좋게"): 알림 기능은 이미 다 있었음(진입/체결/청산/락/sweep/reconcile 16곳 `notify()`)·plain text·parse_mode 없었음. **신규 `notify_format.rs`**(순수 문자열 빌더: 굵은 제목·구분선·이모지+라벨 row·값은 백틱코드로 감싸 Markdown특수문자 verbatim) + `send_message`에 `parse_mode:"Markdown"`. **16곳 notify 전부 헬퍼로 교체, 머니패스 로직 byte불변**(notify() 인자 문자열만 변경). TDD 4테스트, **59 passed/0 failed/4 ignored**, fmt/clippy clean. 실제 텔레그램 샘플 2종 발송 렌더 확인. 봇 새 바이너리로 재시작(기동알림 새포맷). ⚠️로컬 봇은 백그라운드 가동 중(PID 파일 `/tmp/rust-testnet-bot.pid`, 로그 `/tmp/rust-testnet-bot.log`) — **종료=`kill $(cat /tmp/rust-testnet-bot.pid)`**. testnet이라 돈위험 없으나 방치 시 진입 발생 가능(횡보 끝나면). 캐처(Hetzner)와 무관한 별개 로컬 프로세스.
> **🔴 함정 — 텔레그램 Markdown 깨짐 (리포맷이 만든 회귀, 즉시 수정 `d2500c54`)**: `parse_mode:"Markdown"`을 전역으로 켜자, **포맷터로 안 바꾼 단 하나의 notify**(`market_ingestion.rs`의 latency 경고 `latency_ms:`)의 **밑줄 `_`이 Markdown 이탤릭 마커**로 해석돼 텔레그램 HTTP 400(`can't parse entities`)이 latency 차단마다 폭주(피드 1개 지연 시 초당 수 건). **근본수정 2겹**: ①`send_message`가 Markdown 발송 실패 시 **plain text로 자동 재발송**(어떤 값에 `*`/`_`/백틱 있어도 알림 절대 유실 안 됨) ②latency 경고도 `notify_format`(값 코드래핑)로. **교훈: parse_mode를 켜면 plain text로 보내던 ALL notify가 Markdown 파싱됨 — 한 곳이라도 안 고치면 깨짐. 전역 parse_mode = 전역 영향.** 실측검증: 옛 plain+Markdown→HTTP400 재현·새포맷→ok·폴백→ok. 60 passed.
> **🔴 함정 — 텔레그램 latency 알림 throttle 없음 → 스팸 폭주 (수정 `c4de7033`)**: latency 경고 `notify()`가 **throttle 없이** `latency_ms>2000`인 **모든 마켓이벤트마다**(초당 수백) 텔레그램 발송 → 피드 1개 지연 시 동일 알림 수천 건 폭주(실측 2026-06-16, 사용자 스크린샷). **원래 설계 결함**(내 리포맷이 만든 게 아님 — 단 이전엔 Binance 지연 0~128ms라 block 자체가 드물어 안 드러났음). **수정**: `market_ingestion.rs`에 `LatencyAlertThrottle`(거래소×심볼당 60s 1건). **risk_event DB 기록은 매 breach마다 그대로**(진단용)—텔레그램만 제한. TDD(윈도우/per-key/리셋). **교훈: 고빈도 경로의 notify는 반드시 throttle. DB persist(진단)와 chat alert(사람)는 빈도 분리.**
> **🔴 함정+근본수정 — Binance kline 소켓 staleness 미보호 (조사확정+수정 `c4de7033`)**: testnet 봇서 **Binance 캔들만 48~70초 지연**(latency_ms 48673/68660 등), 자가복구 안 되고 재시작만 해결. **★서브에이전트 근본원인 조사(opus)로 확정**: Binance는 **소켓 2개 분리**(`/market/stream?...@kline_1m`=캔들전용 `expects_orderbook=false` / `/public/stream?...@bookTicker`=오더북전용 `=true`). per-stream staleness 재연결(`6056de3`)이 **오더북 프레임에만 클럭 무장** → **캔들 소켓은 staleness 체크 영구 inert**(`last_orderbook_at=None`→항상 healthy). 캔들 partial-stall(캔들만 멈추고 ping 살아있음) 시 ①30s idle 타임아웃=ping이 리셋해 안 걸림 ②오더북 staleness=캔들소켓엔 inert → **자가복구 경로 전무**. bybit/bitget은 캔들+오더북 한 소켓 멀티플렉스라 이미 보호됨(그래서 Binance만 터짐). **수정**: staleness 가드를 **소켓별 "주(primary) 프레임"**으로 일반화(`binance.rs:run_public_market_stream_once`) — 오더북소켓=OrderBook, 캔들소켓=Candle로 클럭 무장, connect-time 앵커. 30s 임계는 kline_1m이 매 체결마다 partial-tick 푸시라 안전. **교훈: 거래소가 피드를 여러 소켓에 분리하면 각 소켓의 주 프레임마다 staleness 가드 필요 — `6056de3`은 오더북 소켓만 고쳐 캔들 소켓 갭이 남아있었음.** ⚠게이트는 의도대로 작동(오래된 가격 진입 차단=돈 안전). 재시작 즉시 해소 확인.
> **✅ imbalance 사전등록 분석 스크립트 미리 작성·로컬검증 완료(2026-06-16, 미커밋·untracked)**: 데이터 기다리는 동안 분석을 **사전등록(pre-register)**. 산출물 3+1: `docs/superpowers/specs/2026-06-16-orderbook-imbalance-preregistration.md`(가설·부호·horizon·판정기준·다중비교를 **숫자 보기 전** 못박음) + `deploy/analysis/imbalance_ic.sql`(Spearman IC) + `deploy/analysis/imbalance_quantiles.sql`(분위수 스프레드·단조성) + `deploy/analysis/README.md`(실행법+**데이터충분성 게이트 쿼리**). **사전등록 핵심 못박음**: 가설=`I=bid/(bid+ask)`가 forward **mid**수익률을 부호(+)·단조 예측, horizon **10/30/60s**, **거래소×심볼별 독립**(풀링 금지), **비중복 표본**(overlap=유의성 과대), **mid-price**(bid-ask bounce trap 회피), **보간 금지**, floor=2000 비중복표본, **본페로니 α/18=0.0028**(6피드×3horizon). 3진판정 signal/no-signal/INCONCLUSIVE. **★로컬 throwaway PG(127.0.0.1, `imbalance_sqltest_a`)에 합성데이터로 4브랜치 전부 실측검증**: 양(+)관계→IC+0.996·`signal?`, 무관계→IC~0·`no-signal`, 역(−)관계→IC−0.999·`SIGN VIOLATION`, 저표본→`INCONCLUSIVE(n<floor)`. 비중복표본수(10s→1079·30s→359·60s→179)·단조Q1→Q5·Fisher-z 전부 정상. ⚠로컬 테스트DB는 남아있음(DROP DATABASE 가드로 자동삭제 안 됨, 무해·수동정리 가능: `dropdb -h 127.0.0.1 -U postgres imbalance_sqltest_a`). **이 분석은 신호연구(예측력 존재여부)까지만** — 살아남으면 전략구현(`Strategy::evaluate` trait 확장=머니패스)+동일 WFO+OOS+수수료+적대적리뷰(사전등록 §8). **⚠실행시점=캐처 몇주 후 게이트 쿼리가 'ready'일 때**(지금 돌리면 INCONCLUSIVE).
> **⏰ 3일 간격 점검 리마인더(2026-06-16 설정)**: macOS launchd `~/Library/LaunchAgents/com.mrjoo.rust-capture-gate-reminder.plist`(StartInterval 259200s=3일). 데스크탑 알림만 띄움(Claude/SSH는 클라우드서 불가 — 로컬키·로컬파일 접근 필요해서 cloud routine 대신 로컬 리마인더 선택). 알림 뜨면 터미널서 **"rust 캐처 적재율이랑 게이트 확인해줘"** → 그때 SSH로 헬스+적재율+게이트(`deploy/analysis/README.md` 게이트 쿼리) 점검. 해제: `launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.mrjoo.rust-capture-gate-reminder.plist` + plist 삭제. 로그 `/tmp/rust-capture-gate-reminder.log`.
> **🛡 watchdog 게이트 체크 추가(2026-06-16)**: `deploy/watchdog.sh`에 **#4 imbalance 게이트 체크**(서버 자체서 무인 감시 — 맥북 무관). 매시간(throttle, `GATE_CHECK_SECS=3600`) 게이트 쿼리 돌려 18피드×horizon 중 floor(`GATE_FLOOR=2000`) 통과 개수 로그: `INFO imbalance gate waiting: N/18` / `18/18`이면 `GATE ready ... 분석 돌릴 때` + latch(`/tmp/trading-capture-gate.ready`)로 반복 중단. **로그-only**(분석 실행=사람 결정, 사전등록 규율). throttle 스탬프 `/tmp/trading-capture-gate.stamp`. 확인: `grep GATE /var/log/trading-capture-watchdog.log`. **검증**: bash -n+shellcheck clean, 게이트 SQL 로컬 합성DB로 0/18(3h)·18/18(34h) 실측, throttle/latch 4케이스 실측. ⚠**서버 재배포 필요**: 코드는 repo에 있으나 Hetzner `/opt/trading-capture/watchdog.sh`는 옛 버전 — 다음 SSH 점검 때 `install -D -m755 deploy/watchdog.sh /opt/trading-capture/watchdog.sh`로 갱신.

> **방향(사용자 선택 2026-06-16)**: 멈춤 직후, 가격방향과 **직교한** 후속으로 **오더북 top-of-book 불균형(imbalance) 가설**을 노리기로 함. 단 검증하려면 데이터가 필요한데 DB 실측 결과:
> - `funding_rates` 테이블 없음 → 펀딩비/캐리 가설은 데이터 적재부터(보류).
> - `order_books`는 **62시간치뿐**(2026-06-13~15, top-of-book: best_bid/ask+size). 정직한 walk-forward OOS엔 **몇 주~몇 달 무중단 캐처** 필요.
> **★선결 코드 작업 완료(커밋 모두 푸시)**:
> - **per-stream 오더북 staleness 재연결**(`6056de3`): partial-stall 버그(오더북만 죽고 ping/kline 트리클이 idle 타이머 영구 리셋) 근본수정 + connect-time 앵커(재연결 후 오더북 영영 안 와도 잡힘). TDD+적대적리뷰 2회.
> - **무거래 캐처 템플릿**(`7f05862`): `configs/capture-only.env`(`cp` 한 줄로 무주문 캐처).
> - **1초 다운샘플**(`2536828`): `MARKET_DATA_ORDERBOOK_SAMPLE_SECS`(기본 0=전부, 캐처는 1)로 (거래소,심볼)당 초당 1행만 `order_books` 적재. 디스크 ~50-100배↓. **거래/전략 포워더/캔들/latency 경로 불변**(적대적 7항목 라인검증). TDD.
>
> **★결정 완료(사용자 2026-06-16)**: 배포=**Hetzner**(`5.161.112.248`)에 캐처+DB 함께. 저장=**1초 샘플링**(풀해상도 raw 대신 — Hetzner 디스크 63G 여유뿐, raw면 ~2주에 꿉참; 1초면 몇 달 OK). 리텐션 정책=1초 샘플링으로 해소(추가 프루닝 불필요).
>
> **★배포 아티팩트 완료·빌드검증·푸시(`cb6218e`)**: `deploy/Dockerfile`(멀티스테이지)+`deploy/docker-compose.capture.yml`(전용 `trading-capture-postgres`+capture 서비스·무거래 env·1초 샘플링)+`deploy/README.md`(런북)+`.dockerignore`. **로컬 풀 릴리스 빌드 통과(1m01s·167MB 런타임 이미지)**. ⚠빌더는 `rust:1-slim-bookworm`(1.78은 의존성 edition2024 요구로 실패). 마이그레이션 컴파일타임 임베드→런타임 이미지에 `migrations/` 불필요, `RUN_MIGRATIONS=true` 기동 시 적용. **Hetzner docker build 됨**(credsStore 함정=홈서버 전용, Hetzner엔 없음·smoke 통과).
>
> **🚀 다음 세션 첫 액션 — Hetzner에서 런북 실행 (전부 운영 작업, `deploy/README.md` 그대로)**:
> 1. **코드 가져오기**: Hetzner에서 `git clone`(remote=`joochanyang/RUST`). ⚠️**rsync 워킹트리 금지**(안전분류기가 exfiltration으로 막음·실측) → git clone만. 서버 git 인증 필요.
> 2. **기동**: `export CAPTURE_DB_PASSWORD=...` → `cd trading-system && docker compose -f deploy/docker-compose.capture.yml up -d --build`. 전용 Postgres 새로 띄움(기존 4개 PG와 격리), 마이그레이션 자동.
> 3. **검증·모니터**: 헬스(`exec capture wget -qO- 127.0.0.1:8080/api/health`)·insert율/freshness(`exec capture-postgres psql ...GROUP BY exchange,symbol`)·디스크 증가율(1초 샘플링이 ~50-100MB/일급인지). 절벽=stall(staleness 수정이 자가복구, 안 되면 재시작).
> 4. **데이터 충분해지면**(몇 주 후): imbalance 가설을 **사전등록 데이터분석부터**(imbalance가 다음 N분 수익률 예측? IC·분위수) → 신호 있으면 전략 구현+동일 WFO+OOS+수수료+적대적리뷰, 없으면 근거 있는 기각. ⚠️같은 family-wise 규율.
> **⚠️미결(다음 세션 첫 결정)**: Hetzner 서버 git 인증 방식(deploy key/PAT). 그 외 배포 결정은 다 끝남(전용 PG·이미지 빌드 방식·env 전부 아티팩트에 박제).
> **⚠️비차단 한계(캐처엔 무방, 라이브 전 재검토)**: bybit `tickers.*`=델타채널→top-of-book 안 변해도 staleness/샘플 갱신됨. 백프레셔(DB 멈춤) 동안 staleness 탐지 지연. 상세=메모리 파일.

### 🚀 (참고·보존) 멈춤 직전 다음 액션 메모 — ★가격방향이 아닌 구조적으로 다른 가설, or 멈춤

> **목표(사용자)**: "실거래 할 수 있는 전략". 검증 가능 목표=**OOS에서 수수료 반영 후 일관 양(+)**.
> **현재 결론(2026-06-15 확정)**: **가격이 레벨을 돌파/회귀하는 모든 전략(순수돌파·추세필터돌파·평균회귀)이 1m·5m·1h·1d 전부 OOS 엣지 없음** — 같은 BTC/ETH 2023-26 레짐에서 **4번 falsify**. 일봉(마지막·최선의 돌파 시도: 일봉·수수료반영·pre-roll·추세필터·12조합 WFO)도 풀링 OOS 53거래 −66.46(P=0.0011로 유의한 LOSE). 사전등록 데이터분석이 이미 예측한 그대로(lag-1 자기상관≈0=모멘텀 없음, k0.5 적중<50%=휩쏘).
> **★메타 교훈**: 같은 레짐에서 "가격이 레벨을 넘는다" 변형을 더 돌리는 건 거짓양성률만 키움 → **가격방향 차원에서 멈출 것**. 엣지가 있다면 가격방향이 아닌 다른 차원에 있음.
> **다음 후보(계속한다면 — 가격방향과 직교):** ①**펀딩비/캐리 하베스팅**(perp funding=가격예측과 다른 신호원, 시장구조 엣지) ②**횡단면 상대가치**(바스켓 내 강세롱/약세숏 — 시계열 방향 아님, 멀티심볼=trait 확장 필요) ③**변동성 레짐/기간구조**. ⚠️**단 사전등록 데이터근거(엣지를 예측하는)가 없으면 시작 말 것** — 이번 일봉 spec이 실패를 정확히 예측했듯, 근거 없는 시도는 family-wise 거짓양성만 누적. 최고 EV 선택지는 **"이 레짐엔 이 도구셋으로 착취할 가격방향 엣지 없음"으로 결론짓고 멈추는 것**일 수 있음.
> **검증 인프라(영구 재사용)**: 워크포워드+OOS+수수료(1×/2×/3×)+3진판정+거래수 floor+pre-roll(`eval_start`)+적대적 설계/판정 리뷰. 새 가설도 반드시 동일 규율(IS 믿지 말 것·OOS+수수료로만·family-wise 새 홀드아웃).

### ⚠️ 먼저 읽을 것 — 과최적화 함정 (★세 번 실증됨, 재발 절대 금지)
1. **순수 변동성돌파**(lookback/k): 워크포워드 OOS 평균 **+0.00%**(수수료 전). IS-최적이 OOS서 무너짐.
2. **추세필터 변동성돌파**(lookback/k/ma_period, +수수료): 워크포워드 OOS **평균 −136.74·1/4 윈도우만 +**, 그 1개도 2×비용서 음전. IS-best 파라미터 윈도우마다 바뀜=노이즈.
3. **평균회귀(RSI+볼린저)**(2026-06-15, `TechnicalStrategy`, 평균회귀-적합 출구로 검증): OOS **평균 −185.40·positive 0/4·expectancy 0/4·거래수 337~596(충분)**. IS 27조합 전부 음수(−298~−4,964), 음수가 거래수에 비례(rsi7→1.5만거래→−4,964)=**스냅백이 taker수수료+슬리피지 못 이김**. ⭐**핵심 교훈: 검증 전 적대적 설계리뷰 필수** — 원래 계획은 돌파용 2:1 브래킷 출구를 평균회귀에 그대로 씌워 "잘못된 출구로 테스트"할 뻔했음(3에이전트 만장일치 CRITICAL). 출구를 중간밴드복귀/RSI50/하드스톱/시간스톱으로 고쳐서야 정직한 FAIL 판정 가능.
4. **추세필터 돌파 5m/1h 재검증**(2026-06-15, 1m→롤업 캔들, 코드 변경 0·하니스 `timeframe`만 바꿈): OOS 2mo → **5m=FAIL**(평균 −92.84·1/4+·거래수 60~419), **1h=INCONCLUSIVE**(평균 −26.08·2/4+, **W2 16거래<floor20**). → **OOS를 3mo로 넓혀(진입조건 불변=과최적화 아님, 거래수 확보 목적) 재판정**: **5m=FAIL**(평균 −155.84·1/4), **1h=FAIL**(평균 −50·**0/4**·거래수 25~189 충분). ⭐⭐**핵심 교훈: 저거래 윈도우의 "양전"은 노이즈** — 1h 2mo의 W2(+1.96,16거래)·W3(+3.56)가 거래수 늘자(25·177) 둘 다 음전(W2 expectancy −1.30). INCONCLUSIVE를 "엣지 없음"으로 단정 안 하고 **거래수 늘려 재판정한 게 정답**(2/4 양전은 가짜였음). **타임프레임↑→손실↓(−136→−93→−50)=수수료드래그 가설 맞으나 양전 전환 안 됨**.
5. **일봉(1d) 추세필터 변동성돌파**(2026-06-15, `walk_forward_breakout_daily`, 12조합 WFO 3윈도우 IS12mo→OOS6mo·60일 pre-roll·n=3 verdict·2026-01~now 미사용 홀드아웃): 기계판정=**INCONCLUSIVE**(3윈도우 중 2개 20거래 floor 미달)였으나 **★진짜 결론=effectively-FAIL(엣지 없음)**. OOS PnL [−46.89, −29.51, +9.94]·expectancy [−3.35, −1.55, +0.50]·거래수 [14,19,20].
   - ⭐⭐**floor가 보호가 아니라 은폐 중이었음**: floor 목적=희소 윈도우가 0근처에 떨어져 "중립" 오독되는 fake-zero 차단. 그런데 미달 두 윈도우는 0근처가 아니라 **결정적 음수**(W1 14거래 승14.3% −46.89 P(≤2|공정동전)=0.0065, W2 19거래 승26.3% −29.51 P(≤5)=0.0318) → 개별 유의한 음수. **풀링하면**(같은 패밀리·종목·레짐이라 풀링이 정직): 53거래 15승(28.3%) 1x −66.46 기대값−1.254/거래 **P(≤15승|공정동전)=0.0011**(p=0.45에도 0.0096)=통계적으로 지는 게 확정. 유일 양수 W3(+9.94)은 2×비용서 −0.08 사망, P(3윈도우 중 ≥1 양수|엣지없음)=0.875=노이즈가 예측한 그대로.
   - **IS-best 3개 다 음수**(−5.60/−25.50/−40.47)=셀렉터가 12조합×12mo 후견으로도 흑자 못 찾고 "least-bad" 선택 + 파라미터 텔레포트(lb10k0.5ma50→lb20k0.3ma50→lb20k0.3ma30)=오버핏 시그니처(trap#2 재현).
   - **설계 결함 아님 검증완료**: pre-roll·버퍼·now-clamp제거·홀드아웃 전부 정상. `signals==trades`(14=14 등)로 웜업 정상 입증, SQL 교차검증(W1 trend-filter통과=14=하니스 signals)으로 pre-roll 작동 확인. 추세 SMA게이트가 raw돌파 60~73% 제거(6mo당 ~52→14-20)=일봉은 게이트 후 구조적 저빈도.
   - **rerun(OOS 9~12mo, 룰 허용 구조변경)**: floor는 다 통과시키나 결과 이미 확정(W1/W2 기대값 음수→거래 늘면 더 음수). 신지식 0 → "기록상 깨끗한 FAIL 스탬프 필요할 때만". **절대금지: floor 낮추기·그리드 확장·파라미터 추가로 거래수 인위증가**.
   - ⭐**핵심 교훈: floor 미달 ≠ INCONCLUSIVE. 미달 윈도우가 "0근처"면 보호(INCONCLUSIVE), "유의한 음수"면 floor는 가장 damning한 증거를 숨기는 것 → 풀링해서 powered FAIL로 읽어라. no-data(증거없음)와 no-edge(증거가 불리)를 혼동 말 것.**
- **교훈**: "백테스트 수익 좋아질 때까지 파라미터 돌리기"=과최적화 정의 그대로. **IS 숫자 믿지 말 것. OOS+수수료로만 판정.** **출구가 전략 논리와 맞는지 먼저 확인**(진입만 보면 안 됨). **거래수 floor로 INCONCLUSIVE/FAIL 구분**(저빈도 타임프레임은 가짜 0 위험) — **단 미달 윈도우가 유의한 음수면 풀링해 FAIL로 읽을 것(함정#5)**. 새 전략도 반드시 같은 워크포워드+OOS+수수료+적대적리뷰로 검증.

### ✅ 이번 세션 산출물 (일봉 1d 돌파 검증 — 2026-06-15)
- **1d 캔들 DB 적재**(순수 데이터, 코드 0): binance BTC/ETH 1m→1d 롤업, 같은 `candles` 테이블 `timeframe='1d'`. epoch-floor 버킷(N=86400), **완전한 1440개 버킷만**(`HAVING count=1440`), idempotent(`ON CONFLICT DO NOTHING`). 각 심볼 **1,095행**(2023-06-13~2026-06-11), OHLCV 무결성 손계산 검증통과(BTC 2024-01-01 open=42314 close=44230.20=1m 첫/마지막행 일치). 롤업 SQL은 5m/1h와 동일 패턴(N만 86400).
- **백테스트 warm-up pre-roll 인프라**(`backtest_runner.rs`, TDD RED→GREEN): `BacktestConfig.eval_start: Option<DateTime<Utc>>` 추가(옵셔널=HTTP API/serde 하위호환, `deny_unknown_fields` 없음). `run_backtest` 진입 루프에 `entry_allowed_at(candle_time, eval_start)` 가드 — `Some(t)`면 t 이전 캔들은 버퍼만 워밍(MA 계산용)·진입/signals_seen 카운트 제외, `None`이면 기존과 100% 동일(프로덕션·1m/5m/1h 불변). `run_combo_tf`에 `pre_roll_days: Option<i64>` 인자(Some→period_start 당기고 eval_start=start). 순수헬퍼 단위테스트 1(`entry_allowed_at_gates_only_pre_eval_start_candles`).
  - ⚠️**왜 필요**: `load_candles`가 `[start,end)`만 로드·pre-roll 없음 → SMA50이 윈도우 첫 ~50일 잡아먹어 거래수 과소집계→가짜 INCONCLUSIVE. pre-roll 60일로 해소(SQL 교차검증: W1 trend-filter통과=14=하니스 signals=14로 작동 확인).
- **일봉 워크포워드 하니스**(`#[ignore]` 영구): `walk_forward_breakout_daily` + `walk_forward_breakout_daily_test`. 3윈도우 IS12mo→OOS6mo(now-clamp 없음, 2026-01~now 홀드아웃 예약), `pre_roll_days=60`, n=3 verdict(공통 `walk_forward_breakout_for_timeframe`의 `==4` 블록 2곳 **byte-for-byte 불변**=박제된 5m/1h falsify 보호 위해 복사), signals_seen+IS-best 조합 출력, 컴파일타임 버퍼 헤드룸 단언(`const _: () = assert!(BACKTEST_HISTORY_LIMIT > DAILY_MAX_MA_PERIOD)`). 실행 ~0.7s. 실행: `cargo test -p trading-api --bin trading-api backtest_runner::tests::walk_forward_breakout_daily_test -- --ignored --nocapture`.
- **결과**(위 함정#5): effectively-FAIL. 테스트 **127 passed/0 failed/4 ignored**, fmt clean, clippy 신규0(기존 ai_repository 인자수 1만 잔존).
- **적대적 리뷰 2회**(설계 7에이전트 propose/attack/synthesize + 판정 4에이전트): 설계는 warm-up pre-roll·now-clamp제거·n=3 freeze-safe를 잡아냄, 판정은 INCONCLUSIVE→effectively-FAIL 재해석(floor 은폐 발견)을 3렌즈 만장일치로 확정.

### ✅ 이번 세션 산출물 (평균회귀 검증 — 커밋·푸시 완료 `91ac208`)
- **`TechnicalStrategy` 파라미터화**(`crates/strategy/src/lib.rs`): `new(rsi_period, bollinger_period, oversold, overbought)`+getter 추가. `Default` 불변. 라이브 미사용(라이브=`VolatilityBreakoutStrategy::default()` 유지). TDD 2테스트(비기본 임계값이 신호를 실제로 바꿈 검증).
- **평균회귀 전용 백테스트 코어**(`backtest_runner.rs` 테스트모듈): `run_mean_reversion_backtest(pool,config,&TechnicalStrategy)` — 진입은 BasicRiskGate로 사이징하되 **출구는 평균회귀-적합**(하드스톱1%→중간밴드복귀(SMA)→RSI50재크로스→시간스톱60봉, 우선순위순). 프로덕션 `run_backtest`·`BacktestConfig`·HTTP API **전부 불변**(strategy 디스크리미네이터 추가 안 함 — strategy_version 오라벨·dyn디스패치·config비대화 회피). MR 출구 4단위테스트.
- **평균회귀 워크포워드 하니스**(`#[ignore]` 영구): 27조합×4윈도우, IS최소거래(≥20)충족 조합 중 IS-best 선택, OOS 1×/2×/3×, 윈도우별 trades/wins/losses/per-trade expectancy/IS-best PnL+OOS 진단, **3진 판정(PASS/FAIL/INCONCLUSIVE)**. 실행: `cargo test -p trading-api --bin trading-api backtest_runner::tests::walk_forward_mean_reversion -- --ignored --nocapture`(DATABASE_URL 필요, ~815s).
- spec: `docs/superpowers/specs/2026-06-15-mean-reversion-validation-design.md`(적대적 리뷰 반영판 + 실행결과 §8.1). **적대적 설계리뷰가 치명결함(추세용 출구) 잡아냄 — 이게 정직한 판정의 핵심.**
- 테스트 **126 passed / 0 failed / 2 ignored**. fmt clean, clippy 신규0(기존 ai_repository 인자수 1만 잔존).

### ✅ 이번 세션 추가 산출물 (5m/1h 타임프레임 확장 — 커밋 `77a6ef0` + 미커밋 하니스)
- **5m/1h 캔들 DB 적재**(커밋 `77a6ef0`, 순수 데이터): binance BTC/ETH 1m→5m/1h 롤업, 같은 `candles` 테이블 `timeframe='5m'/'1h'`. **완전한 버킷만**(`HAVING count=5/60`), idempotent. 각 심볼 **5m≈315.7k·1h=26,310행**(2023-06~2026-06 풀커버), OHLCV 무결성 검증통과. **롤업 SQL**(재실행/심볼확장 그대로): `to_timestamp(floor(extract(epoch from open_time)/N)*N)` 버킷(5m=300·1h=3600), open=`array_agg ORDER BY open_time ASC[1]`·close=`DESC[1]`·high=max·low=min·vol=sum.
- **타임프레임 파라미터 돌파 하니스**(`backtest_runner.rs` 테스트모듈, `#[ignore]`): `walk_forward_breakout_higher_timeframes`(5m+1h 순차) + `walk_forward_breakout_for_timeframe`/`run_combo_tf`. 기존 `walk_forward_trend_filter`(1m) **불변**(복제 안 함). 거래수 진단 + 3진판정. **OOS 3mo로 설정**(1h INCONCLUSIVE 해소용 — 진입조건 불변, 거래수 확보 목적). 실행 ~101s. 테스트 **126 passed/0 failed/3 ignored**, fmt/clippy clean.
- **결과**(위 함정#4): 5m=FAIL(평균 −155.84·1/4), **1h=FAIL**(평균 −50·**0/4**·거래수 25~189). OOS 2mo서 보인 1h 2/4 양전은 저거래(16) 노이즈였음(3mo서 음전). 타임프레임↑→손실↓ 방향성은 확인, 엣지엔 미달.

### ⏭ 다음 세션 — ★정통 일봉(1d) 돌파 검증 (사용자 선택, clear 후 여기부터)
**트리거: "rust 트레이딩 이어서 작업" / "일봉 전략 이어서". 사용자 결정=정통 일봉 전략.**

> ⭐**정정(이전 메모 틀림)**: 백테스트 일봉 검증에 **엔진 수정 불필요**. `backtest_runner`는 엔진 버퍼(`PAPER_MAX_CANDLES_PER_KEY`)와 **무관** — 자체 `BACKTEST_HISTORY_LIMIT=64`만 씀(소스 검증). 일봉 룩백 20~50일은 64 안에 들어가므로 **그대로 검증 가능**. 엔진 버퍼 확장은 *라이브 봇이 일봉을 실행할 때만* 필요(=엣지 증명 후의 일).

**⚠️ 핵심 제약 — 일봉은 데이터가 적다**: 1m→1d 롤업 시 **~1,095 일봉**(3년, BTC). 현재 4윈도우(IS 4mo≈120일·OOS 3mo≈90일)로는 **돌파 발화가 드물어 거래수 floor(20) 크게 미달→INCONCLUSIVE 확정 위험**. 일봉 돌파는 OOS 90일에 수 건뿐일 수 있음(1h 2.7일 룩백서도 W2 25거래였음).

**clear 후 첫 액션 순서**:
1. **그린 베이스라인**(아래 §0): `cargo test --workspace` = 126 passed/0 failed/3 ignored.
2. **1d 롤업**(순수 데이터, 코드 0): `floor(extract(epoch from open_time)/86400)*86400` 버킷, **완전한 1440개 버킷만**(`HAVING count(*)=1440`), idempotent `ON CONFLICT DO NOTHING`. binance BTC/ETH. (5m/1h 롤업 SQL 그대로, N=86400.) → verify: ~1,095행/심볼.
3. **거래수 먼저 가늠**(코드 짜기 전): 일봉 돌파가 윈도우당 몇 건 발화하는지. floor 미달 확정이면 → **윈도우 재설계**(IS/OOS를 년 단위로 — 예: IS 18mo·OOS 9mo·2윈도우, 또는 전체기간 단일 IS/OOS). ⚠️**진입조건 튜닝 금지**(과최적화), 윈도우/룩백만 데이터 양에 맞춤.
4. **하니스**: `walk_forward_breakout_for_timeframe`에 "1d" 추가하거나 별도 윈도우셋 필요(일봉은 4윈도우 구조 안 맞을 가능성). 기존 1m/5m/1h 하니스 불변. 같은 3진판정·수수료·거래수 진단.
5. **적대적 리뷰**(돈/판정 영향): 일봉 윈도우 설계가 정직한지(거래수·family-wise·룩백편향) 검증 후 실행.

남은 다른 방향(일봉도 무엣지면):
- **다른 데이터**(오더북 불균형/펀딩비) — `Strategy::evaluate`가 캔들만 받으므로 trait 확장 필요.
- **멈춤** — 검증 인프라(워크포워드+수수료+3진판정+적대적리뷰) 완비, 향후 가설 재사용.
- ⚠️ 보장 없음. **family-wise 과최적화 주의**(같은 BTC/ETH·같은 거시국면 반복=거짓양성 누적). 마진 PASS면 한 번도 안 쓴 새 홀드아웃 필수.

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
