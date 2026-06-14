CREATE TABLE IF NOT EXISTS candles (
    id BIGSERIAL PRIMARY KEY,
    exchange TEXT NOT NULL,
    symbol TEXT NOT NULL,
    timeframe TEXT NOT NULL,
    open_time TIMESTAMPTZ NOT NULL,
    open NUMERIC NOT NULL,
    high NUMERIC NOT NULL,
    low NUMERIC NOT NULL,
    close NUMERIC NOT NULL,
    volume NUMERIC NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (exchange, symbol, timeframe, open_time)
);

CREATE TABLE IF NOT EXISTS order_books (
    id BIGSERIAL PRIMARY KEY,
    exchange TEXT NOT NULL,
    symbol TEXT NOT NULL,
    event_time TIMESTAMPTZ NOT NULL,
    best_bid NUMERIC NOT NULL,
    best_ask NUMERIC NOT NULL,
    bid_size NUMERIC NOT NULL,
    ask_size NUMERIC NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS signals (
    id UUID PRIMARY KEY,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    strategy TEXT NOT NULL,
    score NUMERIC NOT NULL,
    reason TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS orders (
    id UUID PRIMARY KEY,
    signal_id UUID REFERENCES signals(id),
    exchange TEXT NOT NULL,
    exchange_order_id TEXT,
    mode TEXT NOT NULL,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    order_type TEXT NOT NULL,
    status TEXT NOT NULL,
    price NUMERIC,
    quantity NUMERIC NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS order_fills (
    order_id UUID PRIMARY KEY REFERENCES orders(id),
    exchange TEXT NOT NULL,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    price NUMERIC NOT NULL,
    quantity NUMERIC NOT NULL,
    filled_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS positions (
    id UUID PRIMARY KEY,
    exchange TEXT NOT NULL,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    entry_price NUMERIC NOT NULL,
    mark_price NUMERIC NOT NULL,
    quantity NUMERIC NOT NULL,
    leverage NUMERIC NOT NULL,
    unrealized_pnl NUMERIC NOT NULL,
    opened_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS protection_orders (
    id UUID PRIMARY KEY,
    entry_order_id UUID NOT NULL REFERENCES orders(id),
    position_id UUID NOT NULL REFERENCES positions(id),
    stop_loss_price NUMERIC NOT NULL,
    take_profit_price NUMERIC NOT NULL,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS paper_exits (
    id UUID PRIMARY KEY,
    position_id UUID NOT NULL REFERENCES positions(id),
    entry_order_id UUID NOT NULL REFERENCES orders(id),
    exchange TEXT NOT NULL,
    symbol TEXT NOT NULL,
    trigger TEXT NOT NULL,
    exit_price NUMERIC NOT NULL,
    quantity NUMERIC NOT NULL,
    realized_pnl NUMERIC NOT NULL,
    triggered_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS risk_events (
    id UUID PRIMARY KEY,
    severity TEXT NOT NULL,
    rule TEXT NOT NULL,
    action TEXT NOT NULL,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    acknowledged_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS ai_decisions (
    id UUID PRIMARY KEY,
    signal_id UUID REFERENCES signals(id),
    source TEXT NOT NULL,
    score NUMERIC NOT NULL,
    decision TEXT NOT NULL,
    model TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS backtest_runs (
    id UUID PRIMARY KEY,
    strategy_version TEXT NOT NULL,
    period_start TIMESTAMPTZ NOT NULL,
    period_end TIMESTAMPTZ NOT NULL,
    metrics JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS live_readiness_checks (
    id UUID PRIMARY KEY,
    check_key TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    evidence JSONB NOT NULL DEFAULT '{}'::jsonb,
    verified_by TEXT NOT NULL,
    verified_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS failure_injection_runs (
    id UUID PRIMARY KEY,
    scenario TEXT NOT NULL,
    expected_action TEXT NOT NULL,
    observed_action TEXT NOT NULL,
    passed BOOLEAN NOT NULL,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS candles_symbol_time_idx ON candles (symbol, timeframe, open_time DESC);
CREATE INDEX IF NOT EXISTS order_books_symbol_time_idx ON order_books (symbol, event_time DESC);
CREATE INDEX IF NOT EXISTS signals_created_at_idx ON signals (created_at DESC);
CREATE INDEX IF NOT EXISTS orders_created_at_idx ON orders (created_at DESC);
CREATE INDEX IF NOT EXISTS protection_orders_entry_order_idx ON protection_orders (entry_order_id);
CREATE INDEX IF NOT EXISTS paper_exits_triggered_at_idx ON paper_exits (triggered_at DESC);
CREATE INDEX IF NOT EXISTS risk_events_created_at_idx ON risk_events (created_at DESC);
CREATE INDEX IF NOT EXISTS failure_injection_runs_created_at_idx
    ON failure_injection_runs (created_at DESC);
