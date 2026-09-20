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

pub fn validate_raw_score(score: f64, criteria_count: usize, field_id: &str) -> Result<()> {
    if !(2..=10).contains(&criteria_count) {
        return Err(invalid(field_id));
    }
    let max = (criteria_count - 1) as f64;
    if !score.is_finite() || score < 0.0 || score > max {
        return Err(invalid(field_id));
    }
    Ok(())
}

pub fn normalize_score(score: f64, criteria_count: usize, field_id: &str) -> Result<f64> {
    validate_raw_score(score, criteria_count, field_id)?;
    let max = (criteria_count - 1) as f64;
    Ok(score / max)
}

pub fn validate_answers(
    response: &SystemOneResponse,
    questions: &HashMap<String, QuestionSpec>,
) -> Result<()> {
    for (id, question) in questions {
        let answer = response.answers.get(id).ok_or_else(|| invalid(id))?;
        match (question, answer) {
            (QuestionSpec::Noul { .. }, JevAnswer::Noul { noul }) => unit(*noul, id)?,
            (QuestionSpec::Score { criteria, .. }, JevAnswer::Score { score, confidence }) => {
                validate_raw_score(*score, criteria.len(), id)?;
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

pub fn decode_decision(
    response: SystemOneResponse,
    questions: &HashMap<String, QuestionSpec>,
) -> Result<SentinelDecision> {
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
    let risk_score = match (
        questions.get("risk_score"),
        response.answers.get("risk_score"),
    ) {
        (
            Some(QuestionSpec::Score { criteria, .. }),
            Some(JevAnswer::Score { score, confidence }),
        ) => {
            if let Some(c) = confidence {
                unit(*c, "risk confidence")?;
            }
            normalize_score(*score, criteria.len(), "risk_score")?
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
    fn default_test_questions() -> HashMap<String, QuestionSpec> {
        HashMap::from([
            (
                "system_health".into(),
                QuestionSpec::Choice {
                    instructions: String::new(),
                    criteria: HashMap::from([
                        ("healthy".into(), String::new()),
                        ("degraded".into(), String::new()),
                        ("critical".into(), String::new()),
                    ]),
                },
            ),
            (
                "risk_score".into(),
                QuestionSpec::Score {
                    instructions: String::new(),
                    criteria: vec!["Nominal".into(), "Moderate".into(), "Critical".into()],
                },
            ),
            (
                "action_required".into(),
                QuestionSpec::Noul {
                    instructions: String::new(),
                    criteria: HashMap::new(),
                },
            ),
            (
                "suggested_action".into(),
                QuestionSpec::Choice {
                    instructions: String::new(),
                    criteria: HashMap::from([
                        ("none".into(), String::new()),
                        ("restart_unhealthy_service".into(), String::new()),
                    ]),
                },
            ),
        ])
    }

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
    fn test_score_normalization_and_validation() {
        // 3 criteria -> scale [0.0, 2.0]
        assert_eq!(normalize_score(0.0, 3, "risk_score").unwrap(), 0.0);
        assert_eq!(normalize_score(1.0, 3, "risk_score").unwrap(), 0.5);
        assert_eq!(normalize_score(1.6, 3, "risk_score").unwrap(), 0.8);
        assert_eq!(normalize_score(2.0, 3, "risk_score").unwrap(), 1.0);

        // Invalid scores outside scale
        assert!(normalize_score(-0.01, 3, "risk_score").is_err());
        assert!(normalize_score(2.01, 3, "risk_score").is_err());
        assert!(normalize_score(f64::NAN, 3, "risk_score").is_err());
        assert!(normalize_score(f64::INFINITY, 3, "risk_score").is_err());

        // Criteria count out of bounds [2, 10]
        assert!(normalize_score(0.0, 1, "risk_score").is_err());
        assert!(normalize_score(0.0, 11, "risk_score").is_err());
    }

    #[test]
    fn zero_is_valid_but_missing_is_not() {
        let q = default_test_questions();
        let decision = decode_decision(response(), &q).unwrap();
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
            assert!(decode_decision(r, &q).is_err(), "{key}");
        }
    }

    #[test]
    fn rejects_wrong_types_ranges_and_non_finite_values() {
        let q = default_test_questions();
        // With 3 criteria scale is [0.0, 2.0]; verify boundary rejections
        for score in [-0.01, 2.01, f64::NAN, f64::INFINITY] {
            let mut r = response();
            r.answers.insert(
                "risk_score".into(),
                JevAnswer::Score {
                    score,
                    confidence: None,
                },
            );
            assert!(decode_decision(r, &q).is_err());
        }
        let mut r = response();
        r.answers
            .insert("risk_score".into(), JevAnswer::Noul { noul: 0.1 });
        assert!(decode_decision(r, &q).is_err());
        let mut r = response();
        r.answers.insert(
            "system_health".into(),
            JevAnswer::Choice {
                choice: "invented".into(),
                confidence: None,
                probabilities: None,
            },
        );
        assert!(decode_decision(r, &q).is_err());
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
