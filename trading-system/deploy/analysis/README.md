# Order-book imbalance signal study (pre-registered)

Pre-registration: [`docs/superpowers/specs/2026-06-16-orderbook-imbalance-preregistration.md`](../../docs/superpowers/specs/2026-06-16-orderbook-imbalance-preregistration.md)

This is a **signal study, not a strategy.** It asks one question before any code is
written: does top-of-book imbalance `I = bid_size/(bid_size+ask_size)` predict the
forward mid-price return at 10s / 30s / 60s? The hypothesis, sign, horizons, and
PASS/FAIL/INCONCLUSIVE thresholds are **locked in the pre-reg doc** — do not change
them after seeing numbers (that is what made the price-direction families produce
false positives).

## When to run

**Not yet.** The capture needs weeks of 1s top-of-book first. Run the gate query
below; only when every feed clears the 2,000 non-overlapping-sample floor (per
horizon) is the data sufficient to judge. Until then the verdicts read
`INCONCLUSIVE (n<floor)` by design.

## Files

| File | What it answers |
|------|-----------------|
| `imbalance_ic.sql` | Spearman rank IC per (exchange, symbol, horizon) + Fisher-z significance vs Bonferroni (18 tests, α/18 ⇒ \|z\|>3.20). |
| `imbalance_quantiles.sql` | Imbalance quintile → mean forward return (bps). Shows the *shape*: a signal must be monotone Q1→Q5 with positive Q5−Q1 spread, not just a positive IC. |

Both enforce the locked discipline: **mid-price** returns (not close — bid-ask
bounce trap), **non-overlapping** samples (one base point every H seconds — overlap
inflates significance), **per-feed** (no cross-exchange pooling), **no interpolation**
of missing forward rows.

## How to run (on the capture host, 5.161.112.248)

The capture DB has no exposed port; query it through the container. From the repo on
the host (`deploy/analysis/`):

```sh
# 0. DATA-SUFFICIENCY GATE — run this FIRST. Judge nothing until every feed clears
#    the floor at every horizon. (floor = 2000 non-overlapping samples; 10s ≈ 5.5h
#    of continuous data, 60s ≈ 33h.)
docker exec -i trading-capture-postgres psql -U trading -d trading_system <<'SQL'
WITH horizons(h) AS (VALUES (10),(30),(60)),
ticks AS (
  SELECT exchange, symbol, date_trunc('second', event_time) AS ts
  FROM order_books WHERE bid_size + ask_size > 0
  GROUP BY 1,2,3
)
SELECT t.exchange, t.symbol, h.h AS horizon_s,
       count(*) FILTER (WHERE extract(epoch FROM t.ts)::bigint % h.h = 0) AS nonoverlap_samples,
       CASE WHEN count(*) FILTER (WHERE extract(epoch FROM t.ts)::bigint % h.h = 0) >= 2000
            THEN 'ready' ELSE 'wait' END AS gate
FROM ticks t CROSS JOIN horizons h
GROUP BY 1,2,3 ORDER BY 1,2,3;
SQL

# 1. IC (only meaningful once the gate says 'ready')
docker exec -i trading-capture-postgres psql -U trading -d trading_system < imbalance_ic.sql

# 2. Quantile shape
docker exec -i trading-capture-postgres psql -U trading -d trading_system < imbalance_quantiles.sql
```

## Reading the verdicts (per pre-reg §5)

A feed×horizon has a **signal** only if ALL hold, and only if the pattern is
**consistent across several feeds** (not one feed winning by chance out of 18):

- `imbalance_ic.sql`: `samples ≥ 2000` AND `spearman_ic > 0` AND `|fisher_z| > 3.20`
  (Bonferroni). A negative-but-significant IC is `SIGN VIOLATION`, **not** a
  "reverse signal" — that needs a fresh holdout before it counts (§7).
- `imbalance_quantiles.sql`: `monotone_up = true` AND `spread_q5_q1_bps > 0`.

`no-signal` with sufficient samples ⇒ clean falsification ⇒ **STOP** (do not proceed
to "but maybe a strategy would work"). `INCONCLUSIVE` ⇒ accumulate more data, re-run.

⚠️ **Statistical vs economic significance.** Microstructure IC is typically small
(0.01–0.05 is strong). Even a Bonferroni-significant IC may not survive taker fees
(~0.04% × round trip = 0.08%). This study judges *predictive power exists* only.
Economic significance (real PnL after fees/slippage) is the next stage, behind the
same walk-forward + OOS + fee + adversarial-review gauntlet the price-direction
families went through. See pre-reg §8 — it requires extending `Strategy::evaluate`
(currently candle-only) to carry order-book signal, which is a money-path change.

## Backtest (fixed-H holding, net of cost)

`imbalance_backtest.sql` measures whether the imbalance signal survives transaction
costs. Entry direction = `sign(imbalance − 0.5)`; exit = fixed H seconds later (the
same horizon the signal predicts); roundtrip cost = `k × 2 × (5 bps taker + measured
half-spread)`, k ∈ {1,2,3}. Verdict: `net+ @1x` / `MARGINAL (dies @2x)` /
`FAIL (net<=0 @1x)` / `INCONCLUSIVE (n<floor)`.

Run on the capture host:

    docker exec -i trading-capture-postgres psql -U trading -d trading_system < imbalance_backtest.sql

Verify the math first (throwaway local PG):

    createdb -h 127.0.0.1 -U postgres imbalance_bt_test
    (cd deploy/analysis && psql -h 127.0.0.1 -U postgres imbalance_bt_test < imbalance_backtest_synthetic_test.sql)
    dropdb -h 127.0.0.1 -U postgres imbalance_bt_test

Or via the Rust runner (executes the SQL against `DATABASE_URL`; does not print the
table — sqlx discards rows. Use the psql command above for the numbers):

    DATABASE_URL=… cargo test -p trading-api --bin trading-api \
      backtest_runner::tests::imbalance_backtest_smoke -- --ignored --nocapture

### Result — first run, 2026-06-18 (~2 days of capture)

**The signal is real but does NOT survive taker fees.** All 18 feed×horizon cells:

- **sign-consistency 18/18 `OK (+)`** — direction is correct everywhere; gross return
  is positive (+0.13 … +0.57 bps), matching the confirmed IC sign. The signal predicts.
- **net 18/18 FAIL** (16 `FAIL (net<=0 @1x)` + 2 `INCONCLUSIVE (n<floor)` at bitget 60s).
  Gross ≈ 0.3–0.6 bps; measured half-spread is tiny (~0.008–0.029 bps — top-of-book is
  very tight), so roundtrip cost ≈ **10 bps, almost entirely the 5 bps/side taker fee**.
  Gross is ~1/20th of cost. Even at a 2 bps/side fee tier the gap is ~4 bps vs ~0.4 bps.

This is exactly the "statistical vs economic significance" warning above: predictive
power exists, but a fixed-H taker round trip cannot monetise a sub-1-bps edge.

⚠️ This is **SINGLE-REGIME smoke** (~2 days), not a final verdict — but the failure
margin (~25×) is so large that a regime shift is very unlikely to flip it. An honest
PASS would need the same backtest re-run after weeks accumulate, with walk-forward
in-sample/out-of-sample windows (pre-reg §8.2). Per pre-reg §8.4, a signal study that
dies under cost ⇒ **STOP** rather than tuning variants into a false positive. A maker-
only / queue-position model is a structurally different hypothesis (different fill
assumptions), not a tweak of this one.
