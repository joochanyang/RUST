//! Telegram notification message formatting.
//!
//! Pure string builders for the testnet trading runtime's Telegram alerts.
//! Kept separate from `testnet_runtime` so the money-path logic is untouched:
//! call sites only swap the `format!(...)` argument passed to `notify(...)`.
//!
//! Messages target Telegram **Markdown** (legacy) parse mode: `*bold*` and
//! `` `code` ``. Values that may contain Markdown-special characters are wrapped
//! in inline code (backticks) so they render verbatim without escaping.

/// One labelled row in a notification body, e.g. `📊 수량: ` + `` `0.05` ``.
fn row(icon: &str, label: &str, value: &str) -> String {
    format!("{icon} {label}: `{value}`\n")
}

/// Wraps a notification with a title line and a divider, returning the full
/// Markdown message. `title` is already bold-marked by the caller.
fn frame(title: &str, body: &str) -> String {
    format!("{title}\n──────────\n{body}")
}

/// A framed message whose body is a single plain line (no labelled rows).
pub fn frame_simple(title: &str, line: &str) -> String {
    frame(title, line)
}

/// A standalone `📝 사유: ...` row, for lock bodies built at the call site.
pub fn reason_row(reason: &str) -> String {
    row("📝", "사유", reason)
}

/// Shared body for the CRITICAL lock alerts: symbol/side/qty rows plus a
/// free-form `extra` line (already Markdown-formatted by the caller).
pub fn lock_body(symbol: &str, side: &str, qty: impl std::fmt::Display, extra: &str) -> String {
    format!(
        "{}{}{}{extra}",
        row("🪙", "심볼", symbol),
        row("📊", "방향", side),
        row("🐦", "수량", &qty.to_string()),
    )
}

/// 🟢 진입 주문 제출 (entry submitted with protection).
pub fn entry_submitted(
    symbol: &str,
    side: &str,
    qty: impl std::fmt::Display,
    status: &str,
    stop: impl std::fmt::Display,
    take: impl std::fmt::Display,
) -> String {
    let body = format!(
        "{}{}{}{}{}",
        row("📊", "방향", side),
        row("🐦", "수량", &qty.to_string()),
        row("✅", "상태", status),
        row("🛡", "손절(SL)", &stop.to_string()),
        row("🎯", "익절(TP)", &take.to_string()),
    );
    frame(&format!("🟢 *진입 주문 제출* — {symbol}"), &body)
}

/// ✅ 진입 타임아웃 후 체결로 reconcile (placing protection).
pub fn entry_reconciled_filled(symbol: &str, side: &str, qty: impl std::fmt::Display) -> String {
    let body = format!(
        "{}{}{}",
        row("📊", "방향", side),
        row("🐦", "수량", &qty.to_string()),
        "⏳ 타임아웃됐으나 체결 확인 — 보호주문 발주 중…\n",
    );
    frame(&format!("✅ *진입 reconcile: 체결* — {symbol}"), &body)
}

/// ⏭ 진입 타임아웃 후 미체결로 reconcile (safe to skip).
pub fn entry_reconciled_not_filled(symbol: &str, side: &str) -> String {
    let body = format!(
        "{}{}",
        row("📊", "방향", side),
        "✔️ 타임아웃됐으나 미체결 확인 — 안전하게 건너뜀\n",
    );
    frame(&format!("⏭ *진입 reconcile: 미체결* — {symbol}"), &body)
}

/// 🤖 AI 게이트 차단.
pub fn ai_block(symbol: &str, side: &str, reason: &str) -> String {
    let body = format!("{}{}", row("📊", "방향", side), row("📝", "사유", reason),);
    frame(&format!("🤖 *AI 차단* — {symbol}"), &body)
}

/// ⛔ 리스크 게이트 신호 차단.
pub fn signal_blocked(symbol: &str, side: &str, reason: &str) -> String {
    let body = format!("{}{}", row("📊", "방향", side), row("📝", "사유", reason),);
    frame(&format!("⛔ *신호 차단* — {symbol}"), &body)
}

/// ⚠️ 거래소 최소 수량/명목가 미달로 진입 스킵.
pub fn entry_skipped_minimums(
    symbol: &str,
    side: &str,
    qty: impl std::fmt::Display,
    notional: impl std::fmt::Display,
) -> String {
    let body = format!(
        "{}{}{}{}",
        row("📊", "방향", side),
        row("🐦", "수량", &qty.to_string()),
        row("💵", "명목가", &notional.to_string()),
        "ℹ️ 거래소 최소치 미달 — 진입 스킵\n",
    );
    frame(&format!("⚠️ *진입 스킵(최소치 미달)* — {symbol}"), &body)
}

/// ❌ 주문 발주 실패 (non-locking).
pub fn order_failed(symbol: &str, side: &str, reason: &str) -> String {
    let body = format!("{}{}", row("📊", "방향", side), row("📝", "사유", reason),);
    frame(&format!("❌ *주문 실패* — {symbol}"), &body)
}

/// 🔄 기동 시 position sweep 으로 고아 포지션 정리.
pub fn position_sweep(key: &str, closed: impl std::fmt::Display) -> String {
    let body = format!(
        "{}{}",
        row("🔑", "키", key),
        row("🧹", "정리된 DB 포지션", &closed.to_string()),
    );
    frame("🔄 *Position Sweep*", &body)
}

/// ℹ️ 기동 시 거래소 필터 로드 성공.
pub fn filters_loaded(symbols: impl std::fmt::Display) -> String {
    frame(
        "ℹ️ *거래소 필터 로드*",
        &row("🔢", "심볼 수", &symbols.to_string()),
    )
}

/// ℹ️ 기동 시 오픈 포지션 키 복원.
pub fn positions_restored(count: impl std::fmt::Display) -> String {
    frame(
        "ℹ️ *오픈 포지션 복원*",
        &row("🔢", "복원된 키 수", &count.to_string()),
    )
}

/// 🚨 CRITICAL — 런타임 LOCK. `extra` 는 락 종류별 추가 본문(이미 row 형식).
pub fn critical_lock(title: &str, body: &str) -> String {
    frame(&format!("🚨 *CRITICAL: {title}*"), body)
}

/// 🚨 계정 조회 실패 (기동 시, non-locking warn).
pub fn account_check_failed(error: &str) -> String {
    frame("🚨 *계정 조회 실패*", &row("📝", "사유", error))
}

/// ⏱ 마켓 데이터 지연 — 진입 게이트 차단(non-locking warn).
pub fn market_latency_warning(
    exchange: &str,
    symbol: &str,
    latency_ms: impl std::fmt::Display,
) -> String {
    let body = format!(
        "{}{}{}",
        row("🏦", "거래소", exchange),
        row("🪙", "심볼", symbol),
        row("⏱", "지연(ms)", &latency_ms.to_string()),
    );
    frame("⏱ *마켓 지연 — 진입 차단*", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_submitted_is_markdown_framed() {
        let m = entry_submitted("ETHUSDT", "Buy", "0.05", "NEW", "2450.5", "2600.0");
        assert!(m.starts_with("🟢 *진입 주문 제출* — ETHUSDT"));
        assert!(m.contains("──────────"));
        assert!(m.contains("🐦 수량: `0.05`"));
        assert!(m.contains("🛡 손절(SL): `2450.5`"));
        assert!(m.contains("🎯 익절(TP): `2600.0`"));
    }

    #[test]
    fn values_are_code_wrapped_so_special_chars_are_safe() {
        // A reason containing Markdown specials must not break rendering: it is
        // wrapped in backticks, kept verbatim inside the code span.
        let m = signal_blocked("BTCUSDT", "Sell", "daily loss limit (*) hit_now");
        assert!(m.contains("📝 사유: `daily loss limit (*) hit_now`"));
    }

    #[test]
    fn critical_lock_carries_alarm_and_title() {
        let m = critical_lock("testnet 보호 실패", &row("🐦", "수량", "0.05"));
        assert!(m.starts_with("🚨 *CRITICAL: testnet 보호 실패*"));
        assert!(m.contains("🐦 수량: `0.05`"));
    }

    #[test]
    fn reconciled_variants_state_outcome() {
        assert!(entry_reconciled_filled("ETHUSDT", "Buy", "0.05").contains("보호주문 발주 중"));
        assert!(entry_reconciled_not_filled("ETHUSDT", "Buy").contains("안전하게 건너뜀"));
    }

    #[test]
    fn market_latency_warning_wraps_underscored_label_value_safely() {
        // Regression: the old plain-text message had a bare `latency_ms:` whose
        // underscore broke Markdown parsing once parse_mode was enabled. The
        // formatted version code-wraps every value, so no stray entity remains.
        let m = market_latency_warning("Binance", "BTCUSDT", 4011);
        assert!(m.starts_with("⏱ *마켓 지연 — 진입 차단*"));
        assert!(m.contains("⏱ 지연(ms): `4011`"));
        // The asterisks are balanced (title only) and there are no unbalanced
        // backticks: every backtick opens and closes a value span.
        assert_eq!(m.matches('`').count() % 2, 0);
    }
}
