//! Message formatting for Telegram notifications.

use crate::port::outbound::notifier::Event;

use super::notifier::TelegramConfig;

/// Format an event into a Telegram message, or None if the event should be skipped.
pub fn format_event_message(event: &Event, config: &TelegramConfig) -> Option<String> {
    match event {
        Event::OpportunityDetected(e) if config.notify_opportunities => {
            let question = truncate(&e.question, 60);

            Some(format!(
                "🎯 *Opportunity Detected*\n\
                \n\
                {}\n\
                Edge: `{:.2}%`\n\
                Contracts per leg: `{:.2}`\n\
                Estimated gross profit: `{}`\nBefore fees; execution and settlement not confirmed",
                escape_markdown(&question),
                e.edge * rust_decimal::Decimal::from(100),
                e.volume,
                signed_usd(e.expected_profit)
            ))
        }
        Event::ExecutionCompleted(e) if config.notify_executions => {
            let (emoji, title) = if e.success {
                ("✅", "Trade Executed")
            } else {
                ("❌", "Execution Failed")
            };

            let market_display = truncate(&e.market_id, 16);

            Some(format!(
                "{} *{}*\n\
                \n\
                Market: {}\n\
                {}",
                emoji,
                title,
                escape_markdown(&market_display),
                escape_markdown(&e.details)
            ))
        }
        Event::RiskRejected(e) if config.notify_risk_rejections => {
            let market_display = truncate(&e.market_id, 16);

            Some(format!(
                "⚠️ *Risk Check Failed*\n\
                \n\
                Market: {}\n\
                Reason: {}",
                escape_markdown(&market_display),
                escape_markdown(&e.reason)
            ))
        }
        Event::CircuitBreakerActivated { reason } => Some(format!(
            "🛑 *Circuit Breaker Activated*\n\
            \n\
            Reason: {}\n\
            Trading halted",
            escape_markdown(reason)
        )),
        Event::CircuitBreakerReset => Some(
            "✅ *Circuit Breaker Reset*\n\
            \n\
            Trading resumed"
                .to_string(),
        ),
        Event::DailySummary(e) => Some(format!(
            "📊 *Daily Summary — {}*\n\
            \n\
            Opportunities: `{}`\n\
            Trades: `{}`\n\
            Successful: `{}`\n\
            Profit: `{}`\n\
            Exposure: `${:.2}`",
            escape_markdown(&e.date.to_string()),
            e.opportunities_detected,
            e.trades_executed,
            e.trades_successful,
            signed_usd(e.total_profit),
            e.current_exposure
        )),
        Event::RelationsDiscovered(e) => {
            if e.relations.is_empty() {
                return None;
            }

            let mut msg = format!(
                "🔗 *Relations Discovered*\n\
                \n\
                Advisory only — requires review before trading\nFound `{}` relation\\(s\\)\n",
                e.relations_count
            );

            for (i, rel) in e.relations.iter().take(5).enumerate() {
                let type_emoji = match rel.relation_type.as_str() {
                    "mutually_exclusive" => "🔀",
                    "implies" => "➡️",
                    "exactly_one" => "☝️",
                    _ => "🔗",
                };

                let type_display = match rel.relation_type.as_str() {
                    "mutually_exclusive" => "Mutually Exclusive",
                    "implies" => "Implies",
                    "exactly_one" => "Exactly One",
                    other => other,
                };

                msg.push_str(&format!(
                    "\n{}\\. {} *{}* \\({}%\\)\n",
                    i + 1,
                    type_emoji,
                    escape_markdown(type_display),
                    (rel.confidence * 100.0) as u32
                ));

                for q in &rel.market_questions {
                    let q_truncated = truncate(q, 50);
                    msg.push_str(&format!("   • {}\n", escape_markdown(&q_truncated)));
                }

                if !rel.reasoning.is_empty() {
                    let reasoning = truncate(&rel.reasoning, 60);
                    msg.push_str(&format!("   💡 _{}_\n", escape_markdown(&reasoning)));
                }
            }

            if e.relations.len() > 5 {
                msg.push_str(&format!("\n\\.\\.\\.and {} more", e.relations.len() - 5));
            }

            Some(msg)
        }
        _ => None,
    }
}

fn signed_usd(value: rust_decimal::Decimal) -> String {
    let sign = if value.is_sign_negative() { "-" } else { "+" };
    format!("{sign}${:.2}", value.abs())
}

/// Truncate a string with ellipsis (Unicode-safe).
pub fn truncate(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count > max_chars {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

/// Escape special characters for Telegram `MarkdownV2`.
pub fn escape_markdown(text: &str) -> String {
    let special_chars = [
        '\\', '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.',
        '!',
    ];
    let mut result = String::with_capacity(text.len() * 2);

    for c in text.chars() {
        if special_chars.contains(&c) {
            result.push('\\');
        }
        result.push(c);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rust_decimal_macros::dec;

    use crate::port::outbound::notifier::{
        ExecutionEvent, OpportunityEvent, RelationDetail, RelationsEvent, RiskEvent, SummaryEvent,
    };

    #[test]
    fn negative_profit_and_literal_backslashes_render_correctly() {
        assert_eq!(signed_usd(dec!(-12.50)), "-$12.50");
        assert_eq!(escape_markdown(r"a\b"), r"a\\b");
        let message = format_event_message(
            &Event::DailySummary(SummaryEvent {
                date: NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
                opportunities_detected: 0,
                trades_executed: 1,
                trades_successful: 0,
                total_profit: dec!(-12.50),
                current_exposure: dec!(0),
            }),
            &test_config(true, true, true),
        )
        .unwrap();
        assert!(message.contains("`-$12.50`"));
        assert!(!message.contains("+$-"));
    }

    // -------------------------------------------------------------------------
    // escape_markdown tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_escape_markdown() {
        assert_eq!(escape_markdown("hello"), "hello");
        assert_eq!(escape_markdown("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown("*bold*"), "\\*bold\\*");
        assert_eq!(escape_markdown("test.com"), "test\\.com");
    }

    #[test]
    fn test_escape_markdown_all_special_chars() {
        // All special characters: _ * [ ] ( ) ~ ` > # + - = | { } . !
        assert_eq!(escape_markdown("_"), "\\_");
        assert_eq!(escape_markdown("*"), "\\*");
        assert_eq!(escape_markdown("["), "\\[");
        assert_eq!(escape_markdown("]"), "\\]");
        assert_eq!(escape_markdown("("), "\\(");
        assert_eq!(escape_markdown(")"), "\\)");
        assert_eq!(escape_markdown("~"), "\\~");
        assert_eq!(escape_markdown("`"), "\\`");
        assert_eq!(escape_markdown(">"), "\\>");
        assert_eq!(escape_markdown("#"), "\\#");
        assert_eq!(escape_markdown("+"), "\\+");
        assert_eq!(escape_markdown("-"), "\\-");
        assert_eq!(escape_markdown("="), "\\=");
        assert_eq!(escape_markdown("|"), "\\|");
        assert_eq!(escape_markdown("{"), "\\{");
        assert_eq!(escape_markdown("}"), "\\}");
        assert_eq!(escape_markdown("."), "\\.");
        assert_eq!(escape_markdown("!"), "\\!");
    }

    #[test]
    fn test_escape_markdown_multiple_chars() {
        assert_eq!(
            escape_markdown("**bold** _italic_"),
            "\\*\\*bold\\*\\* \\_italic\\_"
        );
        assert_eq!(escape_markdown("[link](url)"), "\\[link\\]\\(url\\)");
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    #[test]
    fn test_escape_markdown_no_special_chars() {
        assert_eq!(escape_markdown("Hello World 123"), "Hello World 123");
    }

    #[test]
    fn test_escape_markdown_unicode() {
        // Unicode characters should pass through unchanged
        assert_eq!(escape_markdown("日本語"), "日本語");
        assert_eq!(escape_markdown("emoji 🎯"), "emoji 🎯");
        assert_eq!(escape_markdown("café"), "café");
    }

    // -------------------------------------------------------------------------
    // truncate tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello...");
        assert_eq!(truncate("ab", 2), "ab");
    }

    #[test]
    fn test_truncate_unicode() {
        // Handles multi-byte UTF-8 characters without panic
        assert_eq!(truncate("日本語テスト", 3), "日本語...");
        assert_eq!(truncate("café", 4), "café");
        assert_eq!(truncate("🎯🚀💰", 2), "🎯🚀...");
    }

    #[test]
    fn test_truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_one_over() {
        assert_eq!(truncate("hello!", 5), "hello...");
    }

    #[test]
    fn test_truncate_empty() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn test_truncate_zero_max() {
        assert_eq!(truncate("hello", 0), "...");
    }

    #[test]
    fn test_truncate_one_char() {
        assert_eq!(truncate("hello", 1), "h...");
    }

    // -------------------------------------------------------------------------
    // Helper function to create test config
    // -------------------------------------------------------------------------

    fn test_config(
        notify_opportunities: bool,
        notify_executions: bool,
        notify_risk_rejections: bool,
    ) -> TelegramConfig {
        TelegramConfig {
            bot_token: "test-token".to_string(),
            chat_id: 12345,
            notify_opportunities,
            notify_executions,
            notify_risk_rejections,
            position_display_limit: 10,
        }
    }

    // -------------------------------------------------------------------------
    // OpportunityDetected event formatting
    // -------------------------------------------------------------------------

    #[test]
    fn format_opportunity_detected_when_enabled() {
        let config = test_config(true, true, true);
        let event = Event::OpportunityDetected(OpportunityEvent {
            market_id: "market-123".to_string(),
            question: "Will it rain tomorrow?".to_string(),
            edge: dec!(0.05),
            volume: dec!(100),
            expected_profit: dec!(5.25),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Opportunity Detected"));
        assert!(msg.contains("rain tomorrow"));
        assert!(msg.contains("Edge:"));
        assert!(msg.contains("5.00%")); // 0.05 * 100
        assert!(msg.contains("Contracts per leg:"));
        assert!(msg.contains("`100.00`"));
        assert!(!msg.contains("$100"));
        assert!(msg.contains("Estimated gross profit:"));
        assert!(msg.contains("+$5.25"));
    }

    #[test]
    fn format_opportunity_detected_when_disabled() {
        let config = test_config(false, true, true);
        let event = Event::OpportunityDetected(OpportunityEvent {
            market_id: "market-123".to_string(),
            question: "Will it rain tomorrow?".to_string(),
            edge: dec!(0.05),
            volume: dec!(100),
            expected_profit: dec!(5.25),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_none());
    }

    #[test]
    fn format_opportunity_truncates_long_question() {
        let config = test_config(true, true, true);
        let long_question =
            "This is a very long question that exceeds the maximum character limit for display";
        let event = Event::OpportunityDetected(OpportunityEvent {
            market_id: "market-123".to_string(),
            question: long_question.to_string(),
            edge: dec!(0.05),
            volume: dec!(100),
            expected_profit: dec!(5.25),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());
        let msg = result.unwrap();
        // Question should be truncated to 60 chars + "..." (escaped as \.\.\.)
        assert!(
            msg.contains("\\.\\.\\.") || msg.contains("..."),
            "Expected truncation ellipsis in: {}",
            msg
        );
    }

    // -------------------------------------------------------------------------
    // ExecutionCompleted event formatting
    // -------------------------------------------------------------------------

    #[test]
    fn format_execution_success_when_enabled() {
        let config = test_config(true, true, true);
        let event = Event::ExecutionCompleted(ExecutionEvent {
            market_id: "market-abc123def".to_string(),
            success: true,
            details: "Orders: order1, order2".to_string(),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Trade Executed"));
        assert!(msg.contains("market\\-abc123def")); // Market should be escaped
        assert!(msg.contains("Orders:"));
    }

    #[test]
    fn format_execution_failure_when_enabled() {
        let config = test_config(true, true, true);
        let event = Event::ExecutionCompleted(ExecutionEvent {
            market_id: "market-123".to_string(),
            success: false,
            details: "Failed: insufficient balance".to_string(),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Execution Failed"));
        assert!(msg.contains("insufficient balance"));
    }

    #[test]
    fn format_execution_when_disabled() {
        let config = test_config(true, false, true);
        let event = Event::ExecutionCompleted(ExecutionEvent {
            market_id: "market-123".to_string(),
            success: true,
            details: "Orders: order1".to_string(),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_none());
    }

    #[test]
    fn format_execution_truncates_long_market_id() {
        let config = test_config(true, true, true);
        let event = Event::ExecutionCompleted(ExecutionEvent {
            market_id: "very-long-market-id-that-exceeds-limit".to_string(),
            success: true,
            details: "OK".to_string(),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());
        let msg = result.unwrap();
        // Market ID should be truncated to 16 chars + "..." (escaped as \.\.\.)
        assert!(
            msg.contains("\\.\\.\\.") || msg.contains("..."),
            "Expected truncation ellipsis in: {}",
            msg
        );
    }

    // -------------------------------------------------------------------------
    // RiskRejected event formatting
    // -------------------------------------------------------------------------

    #[test]
    fn format_risk_rejected_when_enabled() {
        let config = test_config(true, true, true);
        let event = Event::RiskRejected(RiskEvent {
            market_id: "market-123".to_string(),
            reason: "Exceeds max position limit".to_string(),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Risk Check Failed"));
        assert!(msg.contains("market\\-123"));
        assert!(msg.contains("Exceeds max position limit"));
    }

    #[test]
    fn format_risk_rejected_when_disabled() {
        let config = test_config(true, true, false);
        let event = Event::RiskRejected(RiskEvent {
            market_id: "market-123".to_string(),
            reason: "Exceeds max position limit".to_string(),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_none());
    }

    // -------------------------------------------------------------------------
    // CircuitBreaker events formatting
    // -------------------------------------------------------------------------

    #[test]
    fn format_circuit_breaker_activated() {
        let config = test_config(true, true, true);
        let event = Event::CircuitBreakerActivated {
            reason: "Too many failed trades".to_string(),
        };

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Circuit Breaker Activated"));
        assert!(msg.contains("Too many failed trades"));
        assert!(msg.contains("Trading halted"));
    }

    #[test]
    fn format_circuit_breaker_activated_escapes_special_chars() {
        let config = test_config(true, true, true);
        let event = Event::CircuitBreakerActivated {
            reason: "Error: API_RATE_LIMIT (code: 429)".to_string(),
        };

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        // Parentheses should be escaped
        assert!(msg.contains("\\(code: 429\\)"));
    }

    #[test]
    fn format_circuit_breaker_reset() {
        let config = test_config(true, true, true);
        let event = Event::CircuitBreakerReset;

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Circuit Breaker Reset"));
        assert!(msg.contains("Trading resumed"));
    }

    // -------------------------------------------------------------------------
    // DailySummary event formatting
    // -------------------------------------------------------------------------

    #[test]
    fn format_daily_summary() {
        let config = test_config(true, true, true);
        let event = Event::DailySummary(SummaryEvent {
            date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
            opportunities_detected: 50,
            trades_executed: 25,
            trades_successful: 20,
            total_profit: dec!(150.50),
            current_exposure: dec!(500),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Daily Summary"));
        assert!(msg.contains("2024\\-01\\-15")); // Date escaped
        assert!(msg.contains("50")); // opportunities
        assert!(msg.contains("25")); // trades
        assert!(msg.contains("20")); // successful
        assert!(msg.contains("+$150.50")); // profit
        assert!(msg.contains("$500")); // exposure
    }

    // -------------------------------------------------------------------------
    // RelationsDiscovered event formatting
    // -------------------------------------------------------------------------

    #[test]
    fn format_relations_discovered() {
        let config = test_config(true, true, true);
        let event = Event::RelationsDiscovered(RelationsEvent {
            relations_count: 2,
            relations: vec![
                RelationDetail {
                    relation_type: "mutually_exclusive".to_string(),
                    confidence: 0.95,
                    market_questions: vec!["Will A win?".to_string(), "Will B win?".to_string()],
                    reasoning: "A and B cannot both win".to_string(),
                },
                RelationDetail {
                    relation_type: "implies".to_string(),
                    confidence: 0.80,
                    market_questions: vec!["If X then Y".to_string(), "Y outcome".to_string()],
                    reasoning: "X implies Y".to_string(),
                },
            ],
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Relations Discovered"));
        assert!(msg.contains("2"));
        assert!(msg.contains("Mutually Exclusive"));
        assert!(msg.contains("95%"));
        assert!(msg.contains("Implies"));
        assert!(msg.contains("80%"));
    }

    #[test]
    fn format_relations_discovered_empty() {
        let config = test_config(true, true, true);
        let event = Event::RelationsDiscovered(RelationsEvent {
            relations_count: 0,
            relations: vec![],
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_none()); // Empty relations should return None
    }

    #[test]
    fn format_relations_discovered_exactly_one() {
        let config = test_config(true, true, true);
        let event = Event::RelationsDiscovered(RelationsEvent {
            relations_count: 1,
            relations: vec![RelationDetail {
                relation_type: "exactly_one".to_string(),
                confidence: 0.99,
                market_questions: vec!["Option A".to_string(), "Option B".to_string()],
                reasoning: "Exactly one must happen".to_string(),
            }],
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("Exactly One"));
        assert!(msg.contains("99%"));
    }

    #[test]
    fn format_relations_discovered_unknown_type() {
        let config = test_config(true, true, true);
        let event = Event::RelationsDiscovered(RelationsEvent {
            relations_count: 1,
            relations: vec![RelationDetail {
                relation_type: "custom_relation".to_string(),
                confidence: 0.75,
                market_questions: vec!["Q1".to_string()],
                reasoning: "Custom logic".to_string(),
            }],
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        // Unknown relation types should use the type name directly
        assert!(msg.contains("custom_relation") || msg.contains("custom\\_relation"));
    }

    #[test]
    fn format_relations_discovered_truncates_many() {
        let config = test_config(true, true, true);
        // Create 7 relations (more than the 5 shown)
        let relations: Vec<RelationDetail> = (0..7)
            .map(|i| RelationDetail {
                relation_type: "mutually_exclusive".to_string(),
                confidence: 0.9,
                market_questions: vec![format!("Question {}", i)],
                reasoning: format!("Reason {}", i),
            })
            .collect();

        let event = Event::RelationsDiscovered(RelationsEvent {
            relations_count: 7,
            relations,
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("and 2 more")); // 7 - 5 = 2 more
    }

    #[test]
    fn format_relations_discovered_truncates_long_questions() {
        let config = test_config(true, true, true);
        let long_question =
            "This is a very long market question that exceeds the display limit and should be truncated";

        let event = Event::RelationsDiscovered(RelationsEvent {
            relations_count: 1,
            relations: vec![RelationDetail {
                relation_type: "implies".to_string(),
                confidence: 0.85,
                market_questions: vec![long_question.to_string()],
                reasoning: "Some reasoning".to_string(),
            }],
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("\\.\\.\\.")); // Escaped ellipsis
    }

    #[test]
    fn format_relations_discovered_truncates_long_reasoning() {
        let config = test_config(true, true, true);
        let long_reasoning =
            "This is very detailed reasoning that explains the logical relationship between the markets in great detail and exceeds the truncation limit";

        let event = Event::RelationsDiscovered(RelationsEvent {
            relations_count: 1,
            relations: vec![RelationDetail {
                relation_type: "implies".to_string(),
                confidence: 0.85,
                market_questions: vec!["Q1".to_string()],
                reasoning: long_reasoning.to_string(),
            }],
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());
        // Reasoning should be truncated
    }

    // -------------------------------------------------------------------------
    // Config combinations
    // -------------------------------------------------------------------------

    #[test]
    fn format_all_notifications_disabled() {
        let config = test_config(false, false, false);

        // These should all return None when disabled
        let opportunity = Event::OpportunityDetected(OpportunityEvent {
            market_id: "m1".to_string(),
            question: "Q".to_string(),
            edge: dec!(0.01),
            volume: dec!(10),
            expected_profit: dec!(0.1),
        });
        assert!(format_event_message(&opportunity, &config).is_none());

        let execution = Event::ExecutionCompleted(ExecutionEvent {
            market_id: "m1".to_string(),
            success: true,
            details: "OK".to_string(),
        });
        assert!(format_event_message(&execution, &config).is_none());

        let risk = Event::RiskRejected(RiskEvent {
            market_id: "m1".to_string(),
            reason: "Rejected".to_string(),
        });
        assert!(format_event_message(&risk, &config).is_none());

        // Circuit breaker events are always sent regardless of config
        let breaker = Event::CircuitBreakerActivated {
            reason: "Test".to_string(),
        };
        assert!(format_event_message(&breaker, &config).is_some());

        let reset = Event::CircuitBreakerReset;
        assert!(format_event_message(&reset, &config).is_some());

        // Daily summary is always sent
        let summary = Event::DailySummary(SummaryEvent {
            date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            opportunities_detected: 0,
            trades_executed: 0,
            trades_successful: 0,
            total_profit: dec!(0),
            current_exposure: dec!(0),
        });
        assert!(format_event_message(&summary, &config).is_some());
    }

    // -------------------------------------------------------------------------
    // Special character escaping in various fields
    // -------------------------------------------------------------------------

    #[test]
    fn format_opportunity_escapes_special_chars_in_question() {
        let config = test_config(true, true, true);
        let event = Event::OpportunityDetected(OpportunityEvent {
            market_id: "market-123".to_string(),
            question: "Will the price go *up* (or down)?".to_string(),
            edge: dec!(0.05),
            volume: dec!(100),
            expected_profit: dec!(5),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        // Check that special characters are escaped
        assert!(msg.contains("\\*up\\*"));
        assert!(msg.contains("\\(or down\\)"));
    }

    #[test]
    fn format_execution_escapes_special_chars_in_details() {
        let config = test_config(true, true, true);
        let event = Event::ExecutionCompleted(ExecutionEvent {
            market_id: "market-123".to_string(),
            success: true,
            details: "Orders: [order_1, order_2]".to_string(),
        });

        let result = format_event_message(&event, &config);
        assert!(result.is_some());

        let msg = result.unwrap();
        assert!(msg.contains("\\[order\\_1, order\\_2\\]"));
    }
}
