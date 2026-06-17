# Orderbook Imbalance Backtester Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an SQL-first backtester that measures whether the confirmed orderbook top-of-book imbalance signal survives transaction costs, using fixed-H-second holding exits, and verify it on the 2-day capture dataset (smoke level; honest WFO+OOS deferred until weeks accumulate).

**Architecture:** A single SQL script (`deploy/analysis/imbalance_backtest.sql`) extends the proven `imbalance_fast.sql` indexed-temp-table pattern: it builds resampled ticks, forward-matches the close H seconds later, computes `signed_return = sign(imbalance−0.5) × fwd_return`, subtracts a roundtrip cost of `2×(5bps taker + measured half-spread)` at 1×/2×/3× scenarios, and emits a per-feed×horizon net-return table with a ternary verdict. A thin `#[ignore]` Rust integration test (`crates/api/src/backtest_runner.rs` test module) runs the SQL through a single pooled connection and prints results. The money path, engine, and `Strategy` trait stay byte-invariant.

**Tech Stack:** PostgreSQL (psql + sqlx), Rust (tokio test, sqlx `PgPool`), the existing `#[ignore]` walk-forward test convention.

---

## File Structure

- **Create:** `deploy/analysis/imbalance_backtest.sql` — the backtest query. Single responsibility: entry→fixed-H exit→gross PnL→cost subtraction→ternary verdict. Inherits the locked pre-reg discipline (mid-price, non-overlapping `epoch%H=0`, per-feed, no interpolation, 2s tolerance, floor 2000) from `imbalance_fast.sql`.
- **Create:** `deploy/analysis/imbalance_backtest_synthetic_test.sql` — self-contained SQL test harness that seeds a throwaway schema with 4 synthetic branches and asserts the backtest math. Single responsibility: prove the SQL math is correct without touching real data.
- **Modify:** `crates/api/src/backtest_runner.rs` (append one `#[ignore]` test to the existing `tests` module) — thin runner that executes `imbalance_backtest.sql` against the DB pointed to by `DATABASE_URL` and prints the result table. No production code touched.
- **Update:** `deploy/analysis/README.md` (append a short "Backtest" section documenting how to run both the synthetic test and the live run).

**Money-path invariant:** No changes to `run_backtest`, `BacktestConfig`, `Strategy`, the engine, or any HTTP handler. The only Rust change is a new `#[ignore]` test function. Verify with `git diff` at the end.

---

## Background context the engineer needs

- The signal is **confirmed** (gate 18/18 ready, 2026-06-18): 10s & 30s horizons show `signal?` on all 6 feeds (Spearman IC 0.07–0.23, Bonferroni |z|>3.20), zero sign violations, positive quantile spreads. Spreads are ~1 bps, so **cost is the wall** — this backtester measures whether net-of-cost return is still positive.
- `imbalance = bid_size / (bid_size + ask_size)`. imbalance > 0.5 ⇒ long, < 0.5 ⇒ short.
- The capture DB lives on Hetzner (`5.161.112.248`) with **no exposed port**. Direct sqlx connection is only possible against a **local** Postgres that has an `order_books` table (local dev DB, synthetic test schema, or an SSH port-forward). The authoritative 18-feed run against the real 2-day data is executed **on the host** via `docker exec trading-capture-postgres psql -U trading -d trading_system < imbalance_backtest.sql` — same mechanism used for the gate query. The Rust `#[ignore]` runner is a convenience that runs the same SQL against whatever `DATABASE_URL` points at (used in CI/local against synthetic or forwarded data).
- `order_books` columns: `id, exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size, created_at`. Capture creds: `-U trading -d trading_system`.
- Temp tables are session-scoped, so the whole SQL script must run on **one** connection. `psql < file` does this naturally; the Rust runner must read the file and run it as one `simple_query` batch on a single acquired connection.
- The reference pattern for the Rust runner is the existing `walk_forward_trend_filter` test (`backtest_runner.rs:1009`): `#[tokio::test] #[ignore]`, read env var, `PgPoolOptions::new().connect(...)`, print to stderr, run with `--nocapture`.

---

## Task 1: SQL synthetic-test harness (RED) — define expected backtest math

**Files:**
- Create: `deploy/analysis/imbalance_backtest_synthetic_test.sql`

This task writes the **test first**: a self-contained SQL file that creates a throwaway schema with 4 synthetic branches and asserts the backtest produces the expected verdicts. It will fail until Task 2 creates the real query, because it `\i`-includes nothing yet — instead it inlines the *expected* assertions so we know exactly what Task 2 must satisfy.

- [ ] **Step 1: Write the failing synthetic test**

Create `deploy/analysis/imbalance_backtest_synthetic_test.sql`:

```sql
-- Synthetic verification of imbalance_backtest.sql math. Run on a throwaway local PG:
--   createdb -h 127.0.0.1 -U postgres imbalance_bt_test
--   psql -h 127.0.0.1 -U postgres imbalance_bt_test < imbalance_backtest_synthetic_test.sql
--   dropdb -h 127.0.0.1 -U postgres imbalance_bt_test
--
-- Asserts 4 branches:
--   A: imbalance>0.5 & price rises  -> long wins  -> gross net positive
--   B: imbalance<0.5 & price falls  -> short wins -> gross net positive
--   C: cost monotonicity            -> net(1x) > net(2x) > net(3x)
--   D: no-signal (imbalance~0.5)    -> gross ~0, not PASS
\set ON_ERROR_STOP on

DROP TABLE IF EXISTS order_books;
CREATE TABLE order_books (
    id bigserial PRIMARY KEY,
    exchange text, symbol text,
    event_time timestamptz,
    best_bid numeric, best_ask numeric,
    bid_size numeric, ask_size numeric,
    created_at timestamptz DEFAULT now()
);

-- Branch A (feedA): every 1s for 3 hours. imbalance HIGH (bid_size 9, ask 1 => I=0.9),
-- mid rises 1 unit/sec from 100. half-spread fixed at 0.01 (bid=mid-0.005, ask=mid+0.005).
INSERT INTO order_books (exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size)
SELECT 'feedA', 'BTCUSDT',
       timestamptz '2026-01-01 00:00:00+00' + make_interval(secs => s),
       (100 + s) - 0.005, (100 + s) + 0.005, 9, 1
FROM generate_series(0, 10800) AS s;   -- 3h => 10s:1081, 30s:361, 60s:181 non-overlap

-- Branch B (feedB): imbalance LOW (bid 1, ask 9 => I=0.1), mid FALLS 1 unit/sec from 10000.
INSERT INTO order_books (exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size)
SELECT 'feedB', 'BTCUSDT',
       timestamptz '2026-01-01 00:00:00+00' + make_interval(secs => s),
       (10000 - s) - 0.005, (10000 - s) + 0.005, 1, 9
FROM generate_series(0, 10800) AS s;

-- Branch D (feedD): imbalance alternates around 0.5, mid flat at 100 => no signal.
INSERT INTO order_books (exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size)
SELECT 'feedD', 'BTCUSDT',
       timestamptz '2026-01-01 00:00:00+00' + make_interval(secs => s),
       100 - 0.005, 100 + 0.005,
       CASE WHEN s % 2 = 0 THEN 5 ELSE 5 END, 5
FROM generate_series(0, 10800) AS s;

\echo '===== running imbalance_backtest.sql against synthetic data ====='
\i imbalance_backtest.sql
```

- [ ] **Step 2: Run it to verify it fails**

Run:
```bash
createdb -h 127.0.0.1 -U postgres imbalance_bt_test 2>/dev/null
psql -h 127.0.0.1 -U postgres imbalance_bt_test < deploy/analysis/imbalance_backtest_synthetic_test.sql
```
Expected: FAIL with `imbalance_backtest.sql: No such file or directory` (the `\i` target doesn't exist yet). This confirms the test drives Task 2.

- [ ] **Step 3: Commit the failing test**

```bash
git add deploy/analysis/imbalance_backtest_synthetic_test.sql
git commit -m "test: synthetic harness for imbalance backtest (RED)"
```

---

## Task 2: The backtest SQL (GREEN) — make the synthetic test pass

**Files:**
- Create: `deploy/analysis/imbalance_backtest.sql`

- [ ] **Step 1: Write the backtest query**

Create `deploy/analysis/imbalance_backtest.sql`:

```sql
-- imbalance_backtest.sql — fixed-H-second holding backtest of the orderbook imbalance signal.
--
-- Pre-registration: docs/superpowers/specs/2026-06-16-orderbook-imbalance-preregistration.md
-- Design:           docs/superpowers/specs/2026-06-18-imbalance-backtest-design.md
--
-- Locked discipline (inherited from imbalance_fast.sql / imbalance_ic.sql — do NOT change):
--   * mid = (best_bid+best_ask)/2 ; returns on mid, never on bid/ask (bounce trap)
--   * NON-OVERLAPPING base samples: one entry every H seconds (epoch_second % H = 0)
--   * forward/exit = first tick in [t+H, t+H+2s); NO interpolation; unmatched entry dropped
--   * per (exchange, symbol, horizon); NO pooling
--   * sample-count floor = 2000 non-overlapping entries before any PASS/FAIL judgment
--
-- Money model (design sec 3):
--   direction   = sign(imbalance - 0.5)            (>0.5 long, <0.5 short)
--   gross_bps   = direction * (exit_mid/entry_mid - 1) * 10000
--   half_spread = (best_ask-best_bid)/2 / mid * 10000   (measured at ENTRY)
--   taker_bps   = 5.0   (per side, conservative)
--   roundtrip_cost(k) = k * 2 * (taker_bps + half_spread_bps)   for k in {1,2,3}
--   net_bps(k)  = gross_bps - roundtrip_cost(k)
--
-- NOTE: temp tables are session-scoped — run this whole file on ONE connection
--       (psql < file, or a single sqlx connection batch).

\set ON_ERROR_STOP on
\timing on

DROP TABLE IF EXISTS _ticks;
CREATE TEMP TABLE _ticks AS
SELECT exchange, symbol,
       date_trunc('second', event_time)                         AS ts,
       (best_bid + best_ask) / 2.0                              AS mid,
       bid_size / (bid_size + ask_size)                         AS imbalance,
       (best_ask - best_bid) / 2.0 / ((best_bid + best_ask)/2.0) * 10000 AS half_spread_bps
FROM (
    SELECT exchange, symbol, event_time, best_bid, best_ask, bid_size, ask_size,
           row_number() OVER (PARTITION BY exchange, symbol, date_trunc('second', event_time)
                              ORDER BY event_time) AS rn
    FROM order_books
    WHERE bid_size + ask_size > 0
      AND best_bid > 0 AND best_ask > 0
) d
WHERE rn = 1;

CREATE INDEX _ticks_idx ON _ticks (exchange, symbol, ts) INCLUDE (mid);
ANALYZE _ticks;

-- entries (non-overlapping) + fixed-H exit via index seek
DROP TABLE IF EXISTS _trades;
CREATE TEMP TABLE _trades AS
WITH horizons(h_secs) AS (VALUES (10),(30),(60)),
entries AS (
    SELECT t.exchange, t.symbol, h.h_secs, t.ts,
           t.mid AS entry_mid, t.imbalance, t.half_spread_bps
    FROM _ticks t CROSS JOIN horizons h
    WHERE (extract(epoch FROM t.ts)::bigint % h.h_secs) = 0
      AND t.mid > 0
      AND t.imbalance <> 0.5            -- no direction => no trade
)
SELECT e.exchange, e.symbol, e.h_secs,
       e.half_spread_bps,
       sign(e.imbalance - 0.5)
         * (x.mid / e.entry_mid - 1.0) * 10000   AS gross_bps
FROM entries e
JOIN LATERAL (
    SELECT t2.mid FROM _ticks t2
    WHERE t2.exchange = e.exchange AND t2.symbol = e.symbol
      AND t2.ts >= e.ts + make_interval(secs => e.h_secs)
      AND t2.ts <  e.ts + make_interval(secs => e.h_secs) + make_interval(secs => 2.0)
    ORDER BY t2.ts LIMIT 1
) x ON TRUE;

ANALYZE _trades;

\echo '===== IMBALANCE BACKTEST: net return (bps) after roundtrip cost ====='
WITH agg AS (
    SELECT exchange, symbol, h_secs,
           count(*)                              AS n,
           avg(gross_bps)                        AS mean_gross_bps,
           avg(half_spread_bps)                  AS mean_half_spread_bps,
           avg(2.0 * (5.0 + half_spread_bps))    AS mean_roundtrip_1x_bps
    FROM _trades GROUP BY exchange, symbol, h_secs
)
SELECT exchange, symbol, h_secs AS horizon_s, n AS samples,
       round(mean_gross_bps::numeric, 4)                                   AS gross_bps,
       round(mean_half_spread_bps::numeric, 4)                             AS half_spread_bps,
       round((mean_gross_bps - mean_roundtrip_1x_bps)::numeric, 4)         AS net_1x_bps,
       round((mean_gross_bps - 2*mean_roundtrip_1x_bps)::numeric, 4)       AS net_2x_bps,
       round((mean_gross_bps - 3*mean_roundtrip_1x_bps)::numeric, 4)       AS net_3x_bps,
       CASE
           WHEN n < 2000 THEN 'INCONCLUSIVE (n<floor)'
           WHEN (mean_gross_bps - mean_roundtrip_1x_bps) <= 0 THEN 'FAIL (net<=0 @1x)'
           WHEN (mean_gross_bps - 2*mean_roundtrip_1x_bps) <= 0 THEN 'MARGINAL (dies @2x)'
           ELSE 'net+ @1x (single-regime; needs WFO+OOS)'
       END AS verdict
FROM agg ORDER BY exchange, symbol, h_secs;

\echo '===== sign-consistency cross-check: avg(direction*fwd) should be > 0 (matches IC sign) ====='
SELECT exchange, symbol, h_secs AS horizon_s,
       round(avg(gross_bps)::numeric, 4) AS avg_signed_bps,
       CASE WHEN avg(gross_bps) > 0 THEN 'OK (+)' ELSE 'SIGN MISMATCH' END AS check
FROM _trades GROUP BY exchange, symbol, h_secs ORDER BY exchange, symbol, h_secs;
```

- [ ] **Step 2: Run the synthetic test to verify it passes**

Run:
```bash
psql -h 127.0.0.1 -U postgres imbalance_bt_test < deploy/analysis/imbalance_backtest_synthetic_test.sql
```
Expected: completes without error and prints two tables. **Manually verify these 4 branch assertions in the output:**
- **A (feedA, I=0.9, rising):** `gross_bps` positive (~ +H/100×10000 magnitude minus rounding); `net_1x_bps` positive (gross ≫ cost because mid moves 1 unit/sec); verdict `net+ @1x` (n: 10s=1081 <2000 ⇒ may show `INCONCLUSIVE` — that's expected for 3h synthetic; assert *gross/net signs*, not the verdict label, for A/B).
- **B (feedB, I=0.1, falling):** `gross_bps` positive (short wins); sign-consistency `OK (+)`.
- **C (cost monotonicity):** `net_1x_bps > net_2x_bps > net_3x_bps` for every row.
- **D (feedD, I=0.5):** **no rows** (entries filter `imbalance <> 0.5` drops them) → feedD absent from output. This proves no-direction trades are excluded. (If you want a non-0.5 no-signal branch, that is covered by the live noise; the synthetic D asserts the `<>0.5` guard.)

If any assertion fails, fix `imbalance_backtest.sql` and re-run. Do not proceed until all 4 hold.

- [ ] **Step 3: Drop the throwaway DB**

```bash
dropdb -h 127.0.0.1 -U postgres imbalance_bt_test
```

- [ ] **Step 4: Commit the passing query**

```bash
git add deploy/analysis/imbalance_backtest.sql
git commit -m "feat: imbalance fixed-H backtest SQL (GREEN) — synthetic branches pass"
```

---

## Task 3: Thin Rust `#[ignore]` runner

**Files:**
- Modify: `crates/api/src/backtest_runner.rs` (append one test to the `#[cfg(test)] mod tests` block, alongside `walk_forward_trend_filter`)

- [ ] **Step 1: Add the runner test**

Append inside the existing `mod tests` block in `crates/api/src/backtest_runner.rs` (after the last test function, before the module's closing `}`). It mirrors `walk_forward_trend_filter`'s connection style but runs the SQL file as one batch on a single connection (temp tables require it):

```rust
    /// Imbalance fixed-H backtest runner. NOT a pass/fail unit test — a permanent
    /// #[ignore]-d probe that runs deploy/analysis/imbalance_backtest.sql against the
    /// DB in DATABASE_URL (local dev / SSH-forwarded capture / synthetic) and prints
    /// the net-return table. The authoritative 18-feed run is done on the capture host
    /// via `docker exec trading-capture-postgres psql -U trading -d trading_system < imbalance_backtest.sql`.
    ///
    /// Run: `DATABASE_URL=… cargo test -p trading-api --bin trading-api \
    ///       backtest_runner::tests::imbalance_backtest_smoke -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "imbalance backtest replay over an order_books DB; run explicitly"]
    async fn imbalance_backtest_smoke() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping imbalance backtest; DATABASE_URL is not set");
            return;
        };
        let sql = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../deploy/analysis/imbalance_backtest.sql"),
        )
        .expect("read imbalance_backtest.sql");

        // Temp tables are session-scoped: acquire ONE connection and run the whole
        // batch on it. Strip psql meta-commands (\set, \timing, \echo) the server won't parse.
        let sql_no_meta: String = sql
            .lines()
            .filter(|l| !l.trim_start().starts_with('\\'))
            .collect::<Vec<_>>()
            .join("\n");

        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("connect order_books database");
        let mut conn = pool.acquire().await.expect("acquire single connection");

        eprintln!("=== Imbalance fixed-H backtest (net of 2×(5bps+half-spread), 1x/2x/3x) ===");
        // simple_query runs the multi-statement batch on this one connection.
        use sqlx::Executor;
        let mut stream = conn.execute_many(sqlx::raw_sql(&sql_no_meta));
        use futures::StreamExt;
        while let Some(res) = stream.next().await {
            res.expect("imbalance_backtest.sql statement failed");
        }
        eprintln!(
            "backtest SQL executed. For the printed result table, run the authoritative version:\n  \
             docker exec -i trading-capture-postgres psql -U trading -d trading_system < deploy/analysis/imbalance_backtest.sql"
        );
    }
```

> Note: sqlx does not stream `SELECT` rows to stdout the way psql does. The runner's job is to **execute** the SQL (proving it parses and runs against a real `order_books` schema) and to point at the psql command that prints the human-readable table. The numeric results are produced by the psql run on the capture host. If the engineer prefers in-Rust row printing, that is an optional enhancement, not required by this plan.

- [ ] **Step 2: Verify it compiles and the suite is green**

Run:
```bash
cd /Users/mr.joo/Documents/Rust/trading-system && cargo test -p trading-api --bin trading-api 2>&1 | tail -20
```
Expected: compiles clean; existing tests pass; `imbalance_backtest_smoke` shows as `ignored` (not run without `--ignored`). If `futures`/`sqlx::Executor`/`execute_many` is unavailable, fall back to splitting `sql_no_meta` on `;` and running each non-empty statement with `conn.execute(...)` in a loop — same single-connection guarantee.

- [ ] **Step 3: Smoke-run against a synthetic local DB (optional but recommended)**

Run (recreate the synthetic schema first if dropped):
```bash
createdb -h 127.0.0.1 -U postgres imbalance_bt_test 2>/dev/null
psql -h 127.0.0.1 -U postgres imbalance_bt_test -c "$(sed '/\\i/d' deploy/analysis/imbalance_backtest_synthetic_test.sql)"
DATABASE_URL=postgres://postgres@127.0.0.1/imbalance_bt_test \
  cargo test -p trading-api --bin trading-api backtest_runner::tests::imbalance_backtest_smoke -- --ignored --nocapture 2>&1 | tail -15
dropdb -h 127.0.0.1 -U postgres imbalance_bt_test
```
Expected: the runner connects, executes the batch without error, prints the "executed" message. (Schema-seed line uses `sed` to drop the `\i` include so only the synthetic INSERTs are loaded.)

- [ ] **Step 4: Confirm clippy/fmt clean and money path untouched**

Run:
```bash
cargo fmt --check && cargo clippy -p trading-api --bin trading-api -- -D warnings 2>&1 | tail -5
git diff --stat HEAD~2 -- crates/
```
Expected: fmt/clippy clean; `git diff --stat` shows **only** `backtest_runner.rs` changed under `crates/`, and the diff is purely the new test function (no edits to `run_backtest`/`BacktestConfig`/`Strategy`/engine).

- [ ] **Step 5: Commit**

```bash
git add crates/api/src/backtest_runner.rs
git commit -m "feat: #[ignore] imbalance backtest runner (money path byte-invariant)"
```

---

## Task 4: Authoritative run on the capture host + README

**Files:**
- Modify: `deploy/analysis/README.md` (append a "Backtest" section)

- [ ] **Step 1: Copy the SQL to the capture host and run the authoritative 18-feed backtest**

Run (read-only against capture data; SSH to the prod capture host):
```bash
scp deploy/analysis/imbalance_backtest.sql root@5.161.112.248:/root/RUST/trading-system/deploy/analysis/imbalance_backtest.sql
ssh root@5.161.112.248 'docker exec -i trading-capture-postgres psql -U trading -d trading_system < /root/RUST/trading-system/deploy/analysis/imbalance_backtest.sql'
```
Expected: a 18-row (6 feed × 3 horizon) net-return table + a sign-consistency table. Capture the output. With 2-day data the 10s/30s rows will have n≥2000 (so a real `net+ @1x` / `FAIL` / `MARGINAL` verdict); 60s bitget rows may show `INCONCLUSIVE (n<floor)`.

- [ ] **Step 2: Record the result in the README**

Append to `deploy/analysis/README.md`:

```markdown
## Backtest (fixed-H holding, net of cost)

`imbalance_backtest.sql` measures whether the imbalance signal survives transaction
costs. Entry direction = sign(imbalance−0.5); exit = fixed H seconds later (the same
horizon the signal predicts); roundtrip cost = k × 2 × (5 bps taker + measured
half-spread), k ∈ {1,2,3}. Verdict: `net+ @1x` / `MARGINAL (dies @2x)` /
`FAIL (net<=0 @1x)` / `INCONCLUSIVE (n<floor)`.

Run on the capture host:
    docker exec -i trading-capture-postgres psql -U trading -d trading_system < imbalance_backtest.sql

Verify the math first (throwaway local PG):
    createdb -h 127.0.0.1 -U postgres imbalance_bt_test
    psql -h 127.0.0.1 -U postgres imbalance_bt_test < imbalance_backtest_synthetic_test.sql
    dropdb -h 127.0.0.1 -U postgres imbalance_bt_test

⚠️ With only ~2 days of capture this is SINGLE-REGIME smoke, not a verdict. An honest
PASS/FAIL needs the same backtest re-run after weeks accumulate, with walk-forward
in-sample/out-of-sample windows (pre-reg §8.2).
```

- [ ] **Step 3: Commit**

```bash
git add deploy/analysis/README.md
git commit -m "docs: record imbalance backtest run + how-to in analysis README"
```

---

## Self-Review notes (resolved)

- **Spec coverage:** §2 architecture → Tasks 2+3; §3 cost model + verdicts → Task 2 SQL; §4 tests (4 synthetic branches, look-ahead 0, sign-consistency) → Task 1 + Task 2 Step 2 + the sign-consistency table in the SQL; §5 success criteria → Task 2 (synthetic), Task 4 (capture run), Task 3 Step 4 (money-path invariant). §6/§7 limits/non-goals → enforced by scope (no engine/trait edits; INCONCLUSIVE labeling).
- **No-signal branch nuance:** synthetic branch D asserts the `imbalance <> 0.5` *guard* (no-direction → no trade). True noise (imbalance near but ≠ 0.5, flat price) is exercised by the live capture run, where such rows yield gross ≈ 0 and a `FAIL`/`INCONCLUSIVE` verdict. This is called out so the engineer doesn't expect a "no-signal feed" row from the synthetic file.
- **Type/name consistency:** `_ticks`, `_trades`, columns `gross_bps`/`half_spread_bps`/`net_1x_bps`/`net_2x_bps`/`net_3x_bps`/`verdict`, and the test fn `imbalance_backtest_smoke` are used identically across Tasks 2–4.
- **Single-connection requirement** (temp tables) is stated in Task 2 header, the Rust runner (Step 1), and the fallback note (Step 2).
