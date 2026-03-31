use crate::types::{ContextBucket, ContextClassification, Rule};

pub fn known_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: "ccc_estimate".to_string(),
            app_keywords: vec![],
            title_keywords: vec![
                "ccc".to_string(),
                "ccc one".to_string(),
                "cccone".to_string(),
                "estimate".to_string(),
            ],
            title_keywords_all: vec![],
            priority: 1,
            cooldown_secs: 8,
            workflow_kind: "ccc".to_string(),
        },
        Rule {
            id: "ccc_supplement".to_string(),
            app_keywords: vec![],
            title_keywords: vec![
                "ccc".to_string(),
                "ccc one".to_string(),
                "cccone".to_string(),
                "supplement".to_string(),
            ],
            title_keywords_all: vec![],
            priority: 2,
            cooldown_secs: 8,
            workflow_kind: "ccc".to_string(),
        },
        Rule {
            id: "ccc_final_bill".to_string(),
            app_keywords: vec![],
            title_keywords: vec![
                "ccc".to_string(),
                "ccc one".to_string(),
                "cccone".to_string(),
                "final bill".to_string(),
            ],
            title_keywords_all: vec![],
            priority: 3,
            cooldown_secs: 8,
            workflow_kind: "ccc".to_string(),
        },
        Rule {
            id: "ccc_open".to_string(),
            app_keywords: vec![],
            title_keywords: vec![
                "ccc".to_string(),
                "ccc one".to_string(),
                "cccone".to_string(),
                "repair order".to_string(),
                "ro ".to_string(),
                "ro#".to_string(),
                "ro-".to_string(),
            ],
            title_keywords_all: vec![],
            priority: 4,
            cooldown_secs: 8,
            workflow_kind: "ccc".to_string(),
        },
    ]
}

/// Browser / desktop titles for non-CCC shop systems (screenshot-first). Lower priority than CCC rules.
pub fn vendor_workflow_rules() -> Vec<Rule> {
    vec![
        Rule {
            id: "tesla_epc_catalog".to_string(),
            app_keywords: vec![],
            title_keywords: vec![],
            title_keywords_all: vec!["tesla".to_string(), "epc".to_string()],
            priority: 10,
            cooldown_secs: 8,
            workflow_kind: "tesla_epc".to_string(),
        },
        Rule {
            id: "partstrader_open".to_string(),
            app_keywords: vec![],
            title_keywords: vec!["partstrader".to_string(), "parts trader".to_string()],
            title_keywords_all: vec![],
            priority: 11,
            cooldown_secs: 8,
            workflow_kind: "parts_trader".to_string(),
        },
        Rule {
            id: "ops_trax_open".to_string(),
            app_keywords: vec![],
            title_keywords: vec!["ops trax".to_string(), "opstrax".to_string()],
            title_keywords_all: vec![],
            priority: 12,
            cooldown_secs: 8,
            workflow_kind: "ops_trax".to_string(),
        },
    ]
}

pub fn all_built_in_rules() -> Vec<Rule> {
    let mut v = known_rules();
    v.extend(vendor_workflow_rules());
    v
}

pub fn known_rule_by_id(rule_id: &str) -> Option<Rule> {
    all_built_in_rules()
        .into_iter()
        .find(|r| r.id == rule_id)
}

pub fn candidate_patterns() -> Vec<Rule> {
    vec![
        Rule {
            id: "workflow_portal_candidate".to_string(),
            app_keywords: vec!["chrome".to_string(), "msedge".to_string(), "firefox".to_string()],
            title_keywords: vec!["portal".to_string(), "workflow".to_string()],
            title_keywords_all: vec![],
            priority: 1,
            cooldown_secs: 0,
            workflow_kind: String::new(),
        },
        Rule {
            id: "email_candidate".to_string(),
            app_keywords: vec!["outlook".to_string(), "chrome".to_string(), "msedge".to_string()],
            title_keywords: vec!["inbox".to_string(), "mail".to_string()],
            title_keywords_all: vec![],
            priority: 2,
            cooldown_secs: 0,
            workflow_kind: String::new(),
        },
    ]
}

fn match_rules(app_name_lower: &str, title_lower: &str, rules: &[Rule]) -> Option<(Rule, String)> {
    let mut ordered = rules.to_vec();
    ordered.sort_by_key(|r| r.priority);

    for rule in ordered {
        let app_match = if rule.app_keywords.is_empty() {
            true
        } else {
            rule.app_keywords
                .iter()
                .any(|k| app_name_lower.contains(k.as_str()))
        };
        let title_match = if !rule.title_keywords_all.is_empty() {
            rule.title_keywords_all
                .iter()
                .all(|k| title_lower.contains(k.as_str()))
        } else if rule.title_keywords.is_empty() {
            true
        } else {
            rule.title_keywords
                .iter()
                .any(|k| title_lower.contains(k.as_str()))
        };

        if app_match && title_match {
            let has_title = !rule.title_keywords.is_empty() || !rule.title_keywords_all.is_empty();
            let reason = if !rule.app_keywords.is_empty() && has_title {
                "app+title"
            } else if !rule.app_keywords.is_empty() {
                "app"
            } else if !rule.title_keywords_all.is_empty() {
                "title_all"
            } else {
                "title"
            };
            return Some((rule, reason.to_string()));
        }
    }
    None
}

pub fn classify_context(app_name_lower: &str, title_lower: &str) -> ContextClassification {
    if let Some((rule, reason)) = match_rules(app_name_lower, title_lower, &known_rules()) {
        return ContextClassification {
            bucket: ContextBucket::Known,
            rule_id: rule.id,
            reason,
            cooldown_secs: rule.cooldown_secs,
            action_allowed: true,
            rule_source: "local".to_string(),
        };
    }

    if let Some((rule, reason)) =
        match_rules(app_name_lower, title_lower, &vendor_workflow_rules())
    {
        return ContextClassification {
            bucket: ContextBucket::Known,
            rule_id: rule.id,
            reason,
            cooldown_secs: rule.cooldown_secs,
            action_allowed: true,
            rule_source: "local".to_string(),
        };
    }

    if let Some((rule, reason)) = match_rules(app_name_lower, title_lower, &candidate_patterns()) {
        return ContextClassification {
            bucket: ContextBucket::Candidate,
            rule_id: rule.id,
            reason,
            cooldown_secs: 0,
            action_allowed: false,
            rule_source: "local".to_string(),
        };
    }

    ContextClassification {
        bucket: ContextBucket::Unknown,
        rule_id: "none".to_string(),
        reason: "none".to_string(),
        cooldown_secs: 0,
        action_allowed: false,
        rule_source: "local".to_string(),
    }
}

/// Maps rule ids (built-in, trained, remote) to a stable workflow key for the UI and capture profile.
pub fn workflow_kind_from_rule_id(rule_id: &str) -> String {
    let id = rule_id.to_lowercase();
    if id == "none" {
        return "unknown".to_string();
    }
    if id == "excluded" {
        return "excluded".to_string();
    }
    if id.starts_with("ccc_exclude_") {
        return "excluded".to_string();
    }
    if id.starts_with("ccc_trained") {
        return "ccc".to_string();
    }
    if id.starts_with("ccc_") {
        return "ccc".to_string();
    }
    if id.starts_with("tesla_epc_") {
        return "tesla_epc".to_string();
    }
    if id.starts_with("partstrader_") || id.starts_with("parts_trader_") {
        return "parts_trader".to_string();
    }
    if id.starts_with("ops_trax_") || id.starts_with("opstrax_") {
        return "ops_trax".to_string();
    }
    if id.starts_with("trained_") {
        let rest = id.strip_prefix("trained_").unwrap_or("");
        if let Some(end) = rest.rfind('_') {
            let maybe_ts = &rest[end + 1..];
            if maybe_ts.chars().all(|c| c.is_ascii_digit()) && !maybe_ts.is_empty() {
                let wf = &rest[..end];
                if !wf.is_empty() {
                    return wf.to_string();
                }
            }
        }
    }
    if id.starts_with("workflow_") || id.starts_with("email_") {
        return "unknown".to_string();
    }
    "unknown".to_string()
}

pub fn preferred_capture_mode_for_rule(rule_id: &str) -> String {
    if workflow_kind_from_rule_id(rule_id).as_str() == "ccc" {
        "pdf".to_string()
    } else {
        "screenshot".to_string()
    }
}

pub fn sticky_browser_eligible_rule_id(rule_id: &str) -> bool {
    let id = rule_id.to_lowercase();
    if id == "none" || id == "excluded" {
        return false;
    }
    let k = workflow_kind_from_rule_id(rule_id);
    k != "unknown" && k != "excluded"
}
