//! Validate the typed answer contract before any decision reaches its consumers.
use crate::core::models::*;
use crate::error::{Result, SentinelError};
use chrono::Utc;
use std::collections::HashMap;

fn invalid(field: &str) -> SentinelError {
    // Do not echo untrusted response values (or provider error bodies) to the UI.
    SentinelError::InvalidJevResponse(format!("missing or invalid {field}"))
}

fn unit(value: f64, field: &str) -> Result<()> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(invalid(field))
    }
}

pub fn validate_answers(
    response: &SystemOneResponse,
    questions: &HashMap<String, QuestionSpec>,
) -> Result<()> {
    for (id, question) in questions {
        let answer = response.answers.get(id).ok_or_else(|| invalid(id))?;
        match (question, answer) {
            (QuestionSpec::Noul { .. }, JevAnswer::Noul { noul }) => unit(*noul, id)?,
            (QuestionSpec::Score { .. }, JevAnswer::Score { score, confidence }) => {
                unit(*score, id)?;
                if let Some(c) = confidence {
                    unit(*c, id)?;
                }
            }
            (
                QuestionSpec::Choice { criteria, .. },
                JevAnswer::Choice {
                    choice,
                    confidence,
                    probabilities,
                },
            ) => {
                if !criteria.contains_key(choice) {
                    return Err(invalid(id));
                }
                if let Some(c) = confidence {
                    unit(*c, id)?;
                }
                if let Some(probs) = probabilities {
                    if probs.len() != criteria.len() {
                        return Err(invalid(id));
                    }
                    for (key, value) in probs {
                        if !criteria.contains_key(key) {
                            return Err(invalid(id));
                        }
                        unit(*value, id)?;
                    }
                    if (probs.values().sum::<f64>() - 1.0).abs() > 0.001 {
                        return Err(invalid(id));
                    }
                }
            }
            _ => return Err(invalid(id)),
        }
    }
    Ok(())
}

pub fn decode_decision(response: SystemOneResponse) -> Result<SentinelDecision> {
    let choice = |id: &str, allowed: &[&str]| -> Result<(String, Option<f64>)> {
        match response.answers.get(id) {
            Some(JevAnswer::Choice {
                choice, confidence, ..
            }) if allowed.contains(&choice.as_str()) => {
                if let Some(c) = confidence {
                    unit(*c, id)?;
                }
                Ok((choice.clone(), *confidence))
            }
            _ => Err(invalid(id)),
        }
    };
    let (system_health, health_confidence) =
        choice("system_health", &["healthy", "degraded", "critical"])?;
    let (suggested_action, _) = choice(
        "suggested_action",
        &[
            "none",
            "restart_unhealthy_service",
            "purge_cache",
            "alert_operator",
        ],
    )?;
    let risk_score = match response.answers.get("risk_score") {
        Some(JevAnswer::Score { score, confidence }) => {
            unit(*score, "risk_score")?;
            if let Some(c) = confidence {
                unit(*c, "risk confidence")?;
            }
            *score
        }
        _ => return Err(invalid("risk_score")),
    };
    let action_probability = match response.answers.get("action_required") {
        Some(JevAnswer::Noul { noul }) => {
            unit(*noul, "action_required")?;
            *noul
        }
        _ => return Err(invalid("action_required")),
    };
    Ok(SentinelDecision {
        timestamp: Utc::now(),
        system_health,
        health_confidence,
        risk_score,
        action_required: action_probability >= 0.5,
        action_probability,
        suggested_action,
        raw_answers: response.answers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response() -> SystemOneResponse {
        serde_json::from_value(serde_json::json!({"model":"test", "answers": {
            "system_health":{"type":"choice","choice":"healthy"},
            "risk_score":{"type":"score","score":0.0},
            "action_required":{"type":"noul","noul":0.0},
            "suggested_action":{"type":"choice","choice":"none"}
        }}))
        .unwrap()
    }
    #[test]
    fn zero_is_valid_but_missing_is_not() {
        let decision = decode_decision(response()).unwrap();
        assert_eq!(decision.risk_score, 0.0);
        assert_eq!(decision.health_confidence, None);
        for key in [
            "risk_score",
            "system_health",
            "suggested_action",
            "action_required",
        ] {
            let mut r = response();
            r.answers.remove(key);
            assert!(decode_decision(r).is_err(), "{key}");
        }
    }
    #[test]
    fn rejects_wrong_types_ranges_and_non_finite_values() {
        for score in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
            let mut r = response();
            r.answers.insert(
                "risk_score".into(),
                JevAnswer::Score {
                    score,
                    confidence: None,
                },
            );
            assert!(decode_decision(r).is_err());
        }
        let mut r = response();
        r.answers
            .insert("risk_score".into(), JevAnswer::Noul { noul: 0.1 });
        assert!(decode_decision(r).is_err());
        let mut r = response();
        r.answers.insert(
            "system_health".into(),
            JevAnswer::Choice {
                choice: "invented".into(),
                confidence: None,
                probabilities: None,
            },
        );
        assert!(decode_decision(r).is_err());
    }
    #[test]
    fn category_answer_must_belong_to_requested_choices() {
        let questions = HashMap::from([(
            "system_health".into(),
            QuestionSpec::Choice {
                instructions: String::new(),
                criteria: HashMap::from([("healthy".into(), String::new())]),
            },
        )]);
        let mut r = response();
        assert!(validate_answers(&r, &questions).is_ok());
        r.answers.insert(
            "system_health".into(),
            JevAnswer::Choice {
                choice: "healthy".into(),
                confidence: Some(1.1),
                probabilities: None,
            },
        );
        assert!(validate_answers(&r, &questions).is_err());
        r.answers.insert(
            "system_health".into(),
            JevAnswer::Choice {
                choice: "other".into(),
                confidence: None,
                probabilities: None,
            },
        );
        assert!(validate_answers(&r, &questions).is_err());
    }
}
