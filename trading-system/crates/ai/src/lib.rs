use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use trading_core::{Result, Side, Signal, TradingError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroDecision {
    pub macro_score: Decimal,
    pub risk_level: String,
    pub long_bias: Decimal,
    pub short_bias: Decimal,
    pub halt_reason: Option<String>,
    pub model: String,
    pub input_hash: String,
}

impl Default for MacroDecision {
    fn default() -> Self {
        Self {
            macro_score: Decimal::ZERO,
            risk_level: "neutral".to_owned(),
            long_bias: Decimal::ZERO,
            short_bias: Decimal::ZERO,
            halt_reason: None,
            model: "manual-default".to_owned(),
            input_hash: "not-provided".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternDecision {
    pub pattern_confidence: Decimal,
    pub historical_win_rate: Decimal,
    pub similar_cases: Vec<String>,
    pub model: String,
    pub input_hash: String,
}

impl Default for PatternDecision {
    fn default() -> Self {
        Self {
            pattern_confidence: Decimal::new(70, 0),
            historical_win_rate: Decimal::new(70, 0),
            similar_cases: Vec::new(),
            model: "local-candle-default".to_owned(),
            input_hash: "not-provided".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AiEntryContext {
    pub macro_decision: Option<MacroDecision>,
    pub pattern_decision: Option<PatternDecision>,
}

#[derive(Debug, Clone)]
pub struct AiGateConfig {
    pub enabled: bool,
    pub fail_closed: bool,
    pub macro_halt_threshold: Decimal,
    pub pattern_min_confidence: Decimal,
}

impl Default for AiGateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            fail_closed: true,
            macro_halt_threshold: Decimal::new(-70, 0),
            pattern_min_confidence: Decimal::new(65, 0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiGateDecision {
    Allow,
    Block { reason: String },
}

impl AiGateDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Block { .. } => "block",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Allow => None,
            Self::Block { reason } => Some(reason),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AiEntryGate {
    config: AiGateConfig,
}

impl AiEntryGate {
    pub fn new(config: AiGateConfig) -> Self {
        Self { config }
    }

    pub fn evaluate(&self, signal: &Signal, context: &AiEntryContext) -> Result<AiGateDecision> {
        if !self.config.enabled {
            return Ok(AiGateDecision::Allow);
        }

        let macro_decision = match &context.macro_decision {
            Some(decision) => decision,
            None if self.config.fail_closed => {
                return Ok(AiGateDecision::Block {
                    reason: "missing Claude macro decision".to_owned(),
                });
            }
            None => return Ok(AiGateDecision::Allow),
        };

        if let Some(reason) = &macro_decision.halt_reason {
            return Ok(AiGateDecision::Block {
                reason: format!("Claude macro halt: {reason}"),
            });
        }

        if macro_decision.macro_score <= self.config.macro_halt_threshold {
            return Ok(AiGateDecision::Block {
                reason: format!(
                    "Claude macro score {} is at or below halt threshold {}",
                    macro_decision.macro_score, self.config.macro_halt_threshold
                ),
            });
        }

        if signal.side == Side::Buy && macro_decision.long_bias < Decimal::ZERO {
            return Ok(AiGateDecision::Block {
                reason: "Claude macro long bias is negative".to_owned(),
            });
        }

        if signal.side == Side::Sell && macro_decision.short_bias < Decimal::ZERO {
            return Ok(AiGateDecision::Block {
                reason: "Claude macro short bias is negative".to_owned(),
            });
        }

        let pattern_decision = match &context.pattern_decision {
            Some(decision) => decision,
            None if self.config.fail_closed => {
                return Ok(AiGateDecision::Block {
                    reason: "missing Candle pattern decision".to_owned(),
                });
            }
            None => return Ok(AiGateDecision::Allow),
        };

        if pattern_decision.pattern_confidence <= self.config.pattern_min_confidence {
            return Ok(AiGateDecision::Block {
                reason: format!(
                    "Candle pattern confidence {} is at or below minimum {}",
                    pattern_decision.pattern_confidence, self.config.pattern_min_confidence
                ),
            });
        }

        if pattern_decision.historical_win_rate <= self.config.pattern_min_confidence {
            return Ok(AiGateDecision::Block {
                reason: format!(
                    "Candle historical win rate {} is at or below minimum {}",
                    pattern_decision.historical_win_rate, self.config.pattern_min_confidence
                ),
            });
        }

        Ok(AiGateDecision::Allow)
    }

    pub fn evaluate_or_block(&self, signal: &Signal, context: &AiEntryContext) -> AiGateDecision {
        match self.evaluate(signal, context) {
            Ok(decision) => decision,
            Err(error) => AiGateDecision::Block {
                reason: format!("AI gate error: {error}"),
            },
        }
    }
}

pub trait AiDecisionProvider: Send + Sync {
    fn decisions_for_signal(&self, signal: &Signal) -> Result<AiEntryContext>;
}

#[derive(Debug, Clone)]
pub struct StaticAiDecisionProvider {
    macro_decision: Option<MacroDecision>,
    pattern_decision: Option<PatternDecision>,
}

impl StaticAiDecisionProvider {
    pub fn new(
        macro_decision: Option<MacroDecision>,
        pattern_decision: Option<PatternDecision>,
    ) -> Self {
        Self {
            macro_decision,
            pattern_decision,
        }
    }
}

impl AiDecisionProvider for StaticAiDecisionProvider {
    fn decisions_for_signal(&self, _signal: &Signal) -> Result<AiEntryContext> {
        Ok(AiEntryContext {
            macro_decision: self.macro_decision.clone(),
            pattern_decision: self.pattern_decision.clone(),
        })
    }
}

pub fn conservative_ai_error(error: impl std::fmt::Display) -> TradingError {
    TradingError::RiskBlocked(format!("AI decision unavailable: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use trading_core::{Side, Symbol};
    use uuid::Uuid;

    fn signal(side: Side) -> Signal {
        Signal {
            id: Uuid::new_v4(),
            symbol: Symbol::new("BTCUSDT"),
            side,
            strategy: "test".to_owned(),
            score: Decimal::new(80, 0),
            reason: "test".to_owned(),
            created_at: Utc::now(),
        }
    }

    fn enabled_gate() -> AiEntryGate {
        AiEntryGate::new(AiGateConfig {
            enabled: true,
            ..AiGateConfig::default()
        })
    }

    #[test]
    fn disabled_gate_allows_entry() {
        let gate = AiEntryGate::new(AiGateConfig::default());
        let decision = gate
            .evaluate(
                &signal(Side::Buy),
                &AiEntryContext {
                    macro_decision: None,
                    pattern_decision: None,
                },
            )
            .unwrap();

        assert_eq!(decision, AiGateDecision::Allow);
    }

    #[test]
    fn enabled_gate_blocks_missing_decisions() {
        let decision = enabled_gate()
            .evaluate(
                &signal(Side::Buy),
                &AiEntryContext {
                    macro_decision: None,
                    pattern_decision: None,
                },
            )
            .unwrap();

        assert_eq!(decision.as_str(), "block");
    }

    #[test]
    fn enabled_fail_closed_gate_blocks_missing_pattern_when_macro_present() {
        let decision = enabled_gate()
            .evaluate(
                &signal(Side::Buy),
                &AiEntryContext {
                    macro_decision: Some(MacroDecision {
                        long_bias: Decimal::new(80, 0),
                        short_bias: Decimal::new(80, 0),
                        ..MacroDecision::default()
                    }),
                    pattern_decision: None,
                },
            )
            .unwrap();

        assert_eq!(decision.as_str(), "block");
        assert_eq!(decision.reason(), Some("missing Candle pattern decision"));
    }

    #[test]
    fn enabled_gate_blocks_low_pattern_confidence() {
        let decision = enabled_gate()
            .evaluate(
                &signal(Side::Buy),
                &AiEntryContext {
                    macro_decision: Some(MacroDecision::default()),
                    pattern_decision: Some(PatternDecision {
                        pattern_confidence: Decimal::new(60, 0),
                        ..PatternDecision::default()
                    }),
                },
            )
            .unwrap();

        assert_eq!(decision.as_str(), "block");
    }
}
