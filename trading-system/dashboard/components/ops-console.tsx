"use client";

import {
  Ban,
  BarChart3,
  CircleDollarSign,
  Lock,
  LogOut,
  RefreshCcw,
  ShieldCheck,
  TrendingDown,
  TrendingUp,
  Unlock
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";

type Mode = "paper" | "testnet" | "live" | "locked";

type Account = {
  mode: Mode;
  locked_reason: string | null;
  configured_equity: string;
  open_positions: number;
  daily_realized_pnl: string;
  market_data_enabled: boolean;
  paper_trading_enabled: boolean;
};

type Position = {
  id: string;
  exchange: string;
  symbol: string;
  side: string;
  entry_price: string;
  mark_price: string;
  quantity: string;
  leverage: string;
  unrealized_pnl: string;
  opened_at: string;
  stop_loss_price: string | null;
  take_profit_price: string | null;
  protection_status: string | null;
};

type Performance = {
  total_entries: number;
  open_positions: number;
  closed_trades: number;
  winning_trades: number;
  losing_trades: number;
  take_profit_count: number;
  stop_loss_count: number;
  manual_close_count: number;
  panic_close_count: number;
  realized_pnl: string;
  daily_realized_pnl: string;
  unrealized_pnl: string;
  net_pnl: string;
  win_rate_pct: string;
  average_pnl: string;
  best_trade_pnl: string;
  worst_trade_pnl: string;
};

type TradeHistory = {
  position_id: string;
  exchange: string;
  symbol: string;
  side: string;
  strategy: string | null;
  entry_price: string;
  mark_price: string;
  quantity: string;
  opened_at: string;
  closed_at: string | null;
  exit_price: string | null;
  realized_pnl: string | null;
  exit_trigger: string | null;
  status: string;
};

type RiskEvent = {
  id: string;
  severity: string;
  rule: string;
  action: string;
  details: Record<string, unknown>;
  created_at: string;
  acknowledged_at: string | null;
};

type Snapshot = {
  account: Account | null;
  positions: Position[];
  performance: Performance | null;
  tradeHistory: TradeHistory[];
  riskEvents: RiskEvent[];
};

type DashboardWsPayload = {
  snapshot?: Snapshot;
  account?: Account;
  error?: string;
};

const emptySnapshot: Snapshot = {
  account: null,
  positions: [],
  performance: null,
  tradeHistory: [],
  riskEvents: []
};

const dashboardWsUrl = process.env.NEXT_PUBLIC_DASHBOARD_WS_URL;

export function OpsConsole({ authRequired }: { authRequired: boolean }) {
  const [snapshot, setSnapshot] = useState<Snapshot>(emptySnapshot);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [account, positions, performance, tradeHistory, riskEvents] = await Promise.all([
        getJson<Account>("/api/account"),
        getJson<Position[]>("/api/positions"),
        getJson<Performance>("/api/performance"),
        getJson<TradeHistory[]>("/api/trade-history?limit=100"),
        getJson<RiskEvent[]>("/api/risk-events?limit=20")
      ]);

      setSnapshot({ account, positions, performance, tradeHistory, riskEvents });
      setError(null);
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : "대시보드 데이터를 불러오지 못했습니다");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    let socket: WebSocket | undefined;

    const startPolling = () => {
      if (timer === undefined) {
        timer = window.setInterval(load, 5000);
      }
    };

    load();

    if (dashboardWsUrl) {
      socket = new WebSocket(dashboardWsUrl);
      socket.onmessage = (event) => {
        try {
          const payload = JSON.parse(event.data) as DashboardWsPayload;
          if (payload.error) {
            setError(payload.error);
          } else if (payload.snapshot) {
            setSnapshot(payload.snapshot);
            setError(null);
            setLoading(false);
          } else if (payload.account) {
            setSnapshot((current) => ({ ...current, account: payload.account ?? null }));
            setError(null);
            setLoading(false);
          }
        } catch {
          setError("실시간 대시보드 메시지를 해석하지 못했습니다");
        }
      };
      socket.onerror = () => {
        setError("실시간 대시보드 연결에 실패했습니다");
      };
      socket.onclose = () => {
        if (active) {
          startPolling();
        }
      };
    } else {
      startPolling();
    }

    return () => {
      active = false;
      if (timer !== undefined) {
        window.clearInterval(timer);
      }
      socket?.close();
    };
  }, [load]);

  const onControl = async (action: "lock" | "unlock" | "panic-close") => {
    setBusyAction(action);
    try {
      await postJson(`/api/control/${action}`);
      await load();
    } catch (controlError) {
      setError(controlError instanceof Error ? controlError.message : "제어 요청에 실패했습니다");
    } finally {
      setBusyAction(null);
    }
  };

  const closePosition = async (id: string) => {
    setBusyAction(id);
    try {
      await postJson(`/api/positions/${id}/close`);
      await load();
    } catch (controlError) {
      setError(controlError instanceof Error ? controlError.message : "포지션 종료에 실패했습니다");
    } finally {
      setBusyAction(null);
    }
  };

  const acknowledgeRiskEvent = async (id: string) => {
    setBusyAction(id);
    try {
      await postJson(`/api/risk-events/${id}/ack`);
      await load();
    } catch (controlError) {
      setError(controlError instanceof Error ? controlError.message : "리스크 확인 처리에 실패했습니다");
    } finally {
      setBusyAction(null);
    }
  };

  const logout = async () => {
    await fetch("/api/session", { method: "DELETE" });
    window.location.reload();
  };

  const performance = snapshot.performance;
  const criticalRisk = useMemo(
    () => snapshot.riskEvents.find((event) => event.severity === "critical" && !event.acknowledged_at),
    [snapshot.riskEvents]
  );

  return (
    <main className="shell">
      <header className="topbar">
        <div>
          <p className="eyebrow">Trading Operations</p>
          <h1>운영 대시보드</h1>
        </div>
        <div className="topbar-actions">
          <button className="icon-button" onClick={load} disabled={loading} title="새로고침">
            <RefreshCcw size={18} />
          </button>
          {authRequired ? (
            <button className="icon-button" onClick={logout} disabled={busyAction !== null} title="로그아웃">
              <LogOut size={18} />
            </button>
          ) : null}
          <button className="control-button lock" onClick={() => onControl("lock")} disabled={busyAction !== null}>
            <Lock size={16} />
            잠금
          </button>
          <button className="control-button unlock" onClick={() => onControl("unlock")} disabled={busyAction !== null}>
            <Unlock size={16} />
            해제
          </button>
          <button className="control-button panic" onClick={() => onControl("panic-close")} disabled={busyAction !== null}>
            <Ban size={16} />
            전체 종료
          </button>
        </div>
      </header>

      {error ? <div className="error-strip">{error}</div> : null}

      <section className="status-grid" aria-label="Trading performance">
        <Metric
          icon={<ShieldCheck size={18} />}
          label="운용 모드"
          value={formatMode(snapshot.account?.mode)}
          tone={snapshot.account?.mode === "locked" ? "danger" : "ok"}
          detail={snapshot.account?.locked_reason ?? "ready"}
        />
        <Metric
          icon={<CircleDollarSign size={18} />}
          label="총 손익"
          value={formatMoney(performance?.net_pnl)}
          tone={toneForMoney(performance?.net_pnl)}
          detail={`실현 ${formatMoney(performance?.realized_pnl)}`}
        />
        <Metric
          icon={<BarChart3 size={18} />}
          label="총 진입"
          value={String(performance?.total_entries ?? 0)}
          tone="neutral"
          detail={`청산 ${performance?.closed_trades ?? 0} / 보유 ${performance?.open_positions ?? 0}`}
        />
        <Metric
          icon={<TrendingUp size={18} />}
          label="승률"
          value={`${formatPercent(performance?.win_rate_pct)}%`}
          tone={Number(performance?.win_rate_pct ?? 0) >= 50 ? "ok" : "warning"}
          detail={`수익 ${performance?.winning_trades ?? 0} / 손실 ${performance?.losing_trades ?? 0}`}
        />
      </section>

      <section className="compact-grid">
        <MiniStat label="오늘 실현" value={formatMoney(performance?.daily_realized_pnl)} tone={toneForMoney(performance?.daily_realized_pnl)} />
        <MiniStat label="미실현" value={formatMoney(performance?.unrealized_pnl)} tone={toneForMoney(performance?.unrealized_pnl)} />
        <MiniStat label="평균 손익" value={formatMoney(performance?.average_pnl)} tone={toneForMoney(performance?.average_pnl)} />
        <MiniStat label="최고 수익" value={formatMoney(performance?.best_trade_pnl)} tone="ok" />
        <MiniStat label="최대 손실" value={formatMoney(performance?.worst_trade_pnl)} tone="danger" />
        <MiniStat label="익절 / 손절" value={`${performance?.take_profit_count ?? 0} / ${performance?.stop_loss_count ?? 0}`} tone="neutral" />
      </section>

      <section className="panel positions">
        <PanelTitle title="실시간 포지션" subtitle={`${snapshot.positions.length}개 보유`} />
        <PositionTable positions={snapshot.positions} busyAction={busyAction} onClose={closePosition} />
      </section>

      <section className="panel history">
        <PanelTitle title="거래 내역" subtitle={`최근 ${snapshot.tradeHistory.length}건`} />
        <TradeHistoryTable rows={snapshot.tradeHistory} />
      </section>

      <section className="panel risk">
        <PanelTitle
          title="리스크 기록"
          subtitle={criticalRisk ? `긴급: ${criticalRisk.action}` : "최근 제어 및 리스크 이벤트"}
        />
        <RiskEventTable events={snapshot.riskEvents} busyAction={busyAction} onAck={acknowledgeRiskEvent} />
      </section>
    </main>
  );
}

function PositionTable({
  positions,
  busyAction,
  onClose
}: {
  positions: Position[];
  busyAction: string | null;
  onClose: (id: string) => void;
}) {
  if (positions.length === 0) {
    return <div className="empty">보유 중인 포지션 없음</div>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            {["심볼", "방향", "진입가", "현재가", "수량", "손익", "익절", "손절", "종료"].map((column) => (
              <th key={column}>{column}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {positions.map((position) => (
            <tr key={position.id}>
              <td className="strong-cell">{position.symbol}</td>
              <td><SideBadge side={position.side} /></td>
              <td>{formatNumber(position.entry_price)}</td>
              <td>{formatNumber(position.mark_price)}</td>
              <td>{formatNumber(position.quantity)}</td>
              <td className={moneyClass(position.unrealized_pnl)}>{formatMoney(position.unrealized_pnl)}</td>
              <td>{formatNumber(position.take_profit_price)}</td>
              <td>{formatNumber(position.stop_loss_price)}</td>
              <td>
                <button className="row-action" disabled={busyAction !== null} onClick={() => onClose(position.id)}>
                  종료
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function TradeHistoryTable({ rows }: { rows: TradeHistory[] }) {
  if (rows.length === 0) {
    return <div className="empty">거래 내역 없음</div>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            {["진입", "심볼", "방향", "진입가", "청산가", "수량", "손익", "사유", "상태"].map((column) => (
              <th key={column}>{column}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((trade) => (
            <tr key={trade.position_id}>
              <td>{formatDateTime(trade.opened_at)}</td>
              <td className="strong-cell">{trade.symbol}</td>
              <td><SideBadge side={trade.side} /></td>
              <td>{formatNumber(trade.entry_price)}</td>
              <td>{formatNumber(trade.exit_price ?? trade.mark_price)}</td>
              <td>{formatNumber(trade.quantity)}</td>
              <td className={moneyClass(trade.realized_pnl)}>{trade.realized_pnl ? formatMoney(trade.realized_pnl) : "-"}</td>
              <td>{formatTrigger(trade.exit_trigger)}</td>
              <td><span className={`status-pill ${trade.status}`}>{formatStatus(trade.status)}</span></td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function RiskEventTable({
  events,
  busyAction,
  onAck
}: {
  events: RiskEvent[];
  busyAction: string | null;
  onAck: (id: string) => void;
}) {
  if (events.length === 0) {
    return <div className="empty">리스크 기록 없음</div>;
  }

  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            {["시간", "등급", "규칙", "조치", "상세", "확인"].map((column) => (
              <th key={column}>{column}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {events.map((event) => (
            <tr key={event.id}>
              <td>{formatDateTime(event.created_at)}</td>
              <td><span className={`severity ${event.severity}`}>{event.severity}</span></td>
              <td>{event.rule}</td>
              <td>{event.action}</td>
              <td>{stringifyDetails(event.details)}</td>
              <td>
                {event.acknowledged_at ? (
                  <span className="ack-text">완료</span>
                ) : (
                  <button className="row-action" disabled={busyAction !== null} onClick={() => onAck(event.id)}>
                    확인
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function Metric({
  icon,
  label,
  value,
  detail,
  tone
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  detail: string;
  tone: "ok" | "warning" | "danger" | "neutral";
}) {
  return (
    <article className={`metric ${tone}`}>
      <div className="metric-icon">{icon}</div>
      <div>
        <span>{label}</span>
        <strong>{value}</strong>
        <small>{detail}</small>
      </div>
    </article>
  );
}

function MiniStat({ label, value, tone }: { label: string; value: string; tone: "ok" | "warning" | "danger" | "neutral" }) {
  return (
    <article className={`mini-stat ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </article>
  );
}

function SideBadge({ side }: { side: string }) {
  const normalized = side.toLowerCase();
  const isLong = normalized === "long" || normalized === "buy";
  return (
    <span className={`side-badge ${isLong ? "long" : "short"}`}>
      {isLong ? <TrendingUp size={13} /> : <TrendingDown size={13} />}
      {isLong ? "LONG" : "SHORT"}
    </span>
  );
}

function PanelTitle({ title, subtitle }: { title: string; subtitle: string }) {
  return (
    <div className="panel-title">
      <h2>{title}</h2>
      <span>{subtitle}</span>
    </div>
  );
}

async function getJson<T>(path: string): Promise<T> {
  const response = await fetch(path, { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`${path} returned ${response.status}`);
  }
  return response.json() as Promise<T>;
}

async function postJson(path: string): Promise<void> {
  const csrfToken = await getCsrfToken();
  const response = await fetch(path, {
    method: "POST",
    headers: { "x-csrf-token": csrfToken }
  });
  if (!response.ok) {
    throw new Error(`${path} returned ${response.status}`);
  }
}

async function getCsrfToken() {
  const response = await fetch("/api/csrf", { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`/api/csrf returned ${response.status}`);
  }
  const payload = await response.json() as { token?: string };
  if (!payload.token) {
    throw new Error("CSRF token missing");
  }
  return payload.token;
}

function formatMoney(value: string | number | null | undefined) {
  const numeric = Number(value ?? 0);
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 2
  }).format(numeric);
}

function formatPercent(value: string | number | null | undefined) {
  return new Intl.NumberFormat("en-US", {
    maximumFractionDigits: 1
  }).format(Number(value ?? 0));
}

function formatNumber(value: string | number | null | undefined) {
  if (value === null || value === undefined) {
    return "-";
  }
  return new Intl.NumberFormat("en-US", {
    maximumFractionDigits: 8
  }).format(Number(value));
}

function formatDateTime(value: string) {
  return new Intl.DateTimeFormat("ko-KR", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit"
  }).format(new Date(value));
}

function formatMode(value: Mode | null | undefined) {
  if (!value) {
    return "unknown";
  }
  switch (value) {
    case "paper":
      return "페이퍼";
    case "testnet":
      return "테스트넷";
    case "live":
      return "실거래";
    case "locked":
      return "잠금";
    default:
      return value;
  }
}

function formatStatus(value: string) {
  switch (value) {
    case "open":
      return "보유";
    case "closed":
      return "청산";
    default:
      return value;
  }
}

function formatTrigger(value: string | null) {
  if (!value) {
    return "-";
  }
  switch (value) {
    case "take_profit":
      return "익절";
    case "stop_loss":
      return "손절";
    case "manual_close":
      return "수동 종료";
    case "panic_close":
      return "전체 종료";
    default:
      return value.replaceAll("_", " ");
  }
}

function toneForMoney(value: string | number | null | undefined): "ok" | "warning" | "danger" | "neutral" {
  const numeric = Number(value ?? 0);
  if (numeric > 0) {
    return "ok";
  }
  if (numeric < 0) {
    return "danger";
  }
  return "neutral";
}

function moneyClass(value: string | number | null | undefined) {
  const numeric = Number(value ?? 0);
  if (numeric > 0) {
    return "money positive";
  }
  if (numeric < 0) {
    return "money negative";
  }
  return "money";
}

function stringifyDetails(details: Record<string, unknown>) {
  return Object.entries(details)
    .slice(0, 3)
    .map(([key, value]) => `${key}: ${String(value)}`)
    .join(" | ");
}
