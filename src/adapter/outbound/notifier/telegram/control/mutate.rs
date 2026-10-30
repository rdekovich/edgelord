use crate::port::inbound::runtime::RuntimeRiskLimitKind;

use super::TelegramControl;

impl TelegramControl {
    pub(super) fn set_risk_text(
        &self,
        kind: RuntimeRiskLimitKind,
        value: rust_decimal::Decimal,
    ) -> String {
        match self.state.set_risk_limit(kind, value) {
            Ok(limits) => format!(
                "✅ Updated {} to {}\n\n\
                ⚙️ Current limits:\n\
                • 💰 min_profit: {}\n\
                • 📉 max_slippage: {}\n\
                • 📊 max_position: ${}\n\
                • 💼 max_exposure: ${}",
                kind.as_str(),
                value,
                limits.min_profit_threshold,
                limits.max_slippage,
                limits.max_position_per_market,
                limits.max_total_exposure
            ),
            Err(err) => format!("❌ Error: cannot update {}: {}", kind.as_str(), err),
        }
    }

    pub(super) fn pause_text(&self) -> String {
        if self.state.is_circuit_breaker_active() {
            let reason = self
                .state
                .circuit_breaker_reason()
                .unwrap_or_else(|| "unknown".to_string());
            return format!("⏸️ Already paused: {}", reason);
        }

        self.state.activate_circuit_breaker("paused via Telegram");
        "⏸️ Trading paused".to_string()
    }

    pub(super) fn resume_text(&self) -> String {
        if !self.state.is_circuit_breaker_active() {
            return "▶️ Trading already active".to_string();
        }

        self.state.reset_circuit_breaker();
        if self.state.is_circuit_breaker_active() {
            return "Trading remains halted: execution reconciliation is required".into();
        }
        "▶️ Trading resumed".to_string()
    }
}
