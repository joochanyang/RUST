# Phase 4 AI Filter Integration

## Implemented

- AI entry gate that can be enabled with `AI_FILTER_ENABLED=true`.
- Claude macro decision contract:
  - Blocks when `macro_score <= -70`.
  - Blocks when `halt_reason` is present.
  - Blocks long entries when `long_bias` is negative.
  - Blocks short entries when `short_bias` is negative.
- Candle pattern decision contract:
  - Blocks when `pattern_confidence <= 65`.
  - Blocks when `historical_win_rate <= 65`.
- Fail-closed behavior through `AI_FILTER_FAIL_CLOSED=true`.
- Static decision provider for local/paper validation before external Claude API integration.
- `ai_decisions` persistence with `signal_id`, source, score, decision, model, input hash, and details.
- AI block reasons persisted to `risk_events`.
- Paper strategy loop now runs signal -> AI gate -> risk gate -> paper broker.

## Environment

```sh
AI_FILTER_ENABLED=false
AI_FILTER_FAIL_CLOSED=true
AI_MACRO_SCORE=0
AI_LONG_BIAS=0
AI_SHORT_BIAS=0
AI_PATTERN_CONFIDENCE=70
AI_HISTORICAL_WIN_RATE=70
```

## Remaining Work

1. Replace the static provider with a Claude macro provider that runs on a slow/event-driven schedule, not every tick.
2. Replace default pattern confidence with a real local Candle pattern matcher.
3. Add integration tests that prove blocked signals appear in `signals`, `ai_decisions`, and `risk_events`.

