use crate::context_rules::{
    classify_context, known_rule_by_id, preferred_capture_mode_for_rule,
    sticky_browser_eligible_rule_id, workflow_kind_from_rule_id,
};
use crate::memory_store::{
    append_candidate_log, load_custom_rules, load_exclude_rules, load_memory, save_custom_rules,
    save_exclude_rules, save_memory,
};
use crate::types::{ContextBucket, ContextClassification, Rule, WatchContext};
use active_win_pos_rs::get_active_window;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_unix_ms() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    dur.as_millis() as i64
}

fn is_browser_app(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("chrome") || n.contains("msedge") || n.contains("firefox")
}

pub fn current_window_info() -> (String, String) {
    match get_active_window() {
        Ok(window) => {
            let app_name = if window.app_name.is_empty() {
                "unknown".to_string()
            } else {
                window.app_name
            };
            let title = if window.title.is_empty() {
                "unknown".to_string()
            } else {
                window.title
            };
            (app_name, title)
        }
        Err(_) => ("unknown".to_string(), "unknown".to_string()),
    }
}

pub fn classify_current_context(app_name: &str, window_title: &str) -> ContextClassification {
    classify_context(&app_name.to_lowercase(), &window_title.to_lowercase())
}

fn match_custom_rule(app_name_lower: &str, title_lower: &str, rule: &Rule) -> bool {
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
    app_match && title_match
}

fn title_signature_for_training(window_title: &str) -> String {
    let lower = window_title.to_lowercase();
    let without_suffix = lower
        .split(" - google chrome")
        .next()
        .unwrap_or(&lower)
        .split(" - microsoft edge")
        .next()
        .unwrap_or(&lower)
        .split(" - mozilla firefox")
        .next()
        .unwrap_or(&lower)
        .to_string();

    let normalized: String = without_suffix
        .chars()
        .map(|c| {
            if c.is_ascii_alphabetic() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();

    let stop_words = [
        "google",
        "chrome",
        "microsoft",
        "edge",
        "mozilla",
        "firefox",
        "tab",
        "new",
        "home",
        "page",
    ];
    let tokens: Vec<&str> = normalized
        .split_whitespace()
        .filter(|t| t.len() >= 3 && !stop_words.contains(t))
        .take(4)
        .collect();
    if tokens.is_empty() {
        "repair order".to_string()
    } else {
        tokens.join(" ")
    }
}

pub fn evaluate_context(app: &tauri::AppHandle) -> Result<WatchContext, String> {
    let (source_app, window_title) = current_window_info();
    let source_app_lower = source_app.to_lowercase();
    let title_lower = window_title.to_lowercase();
    let mut classification = classify_current_context(&source_app, &window_title);
    if !classification.action_allowed {
        let custom_rules = load_custom_rules(app)?;
        if let Some(rule) = custom_rules
            .iter()
            .find(|rule| match_custom_rule(&source_app_lower, &title_lower, rule))
        {
            classification = ContextClassification {
                bucket: ContextBucket::Known,
                rule_id: rule.id.clone(),
                reason: "trained".to_string(),
                cooldown_secs: 8,
                action_allowed: true,
                rule_source: "local".to_string(),
            };
        }
    }
    let bucket_str = match classification.bucket {
        ContextBucket::Known => "known",
        ContextBucket::Candidate => "candidate",
        ContextBucket::Unknown => "unknown",
    };
    let context_key = format!(
        "{}|{}",
        source_app.to_lowercase(),
        window_title.to_lowercase()
    );

    let mut memory = load_memory(app)?;
    let now_ms = now_unix_ms();

    // Small stability guard: keep prior known context briefly if sampling drops to unknown.
    let is_browser_like = source_app_lower.contains("chrome")
        || source_app_lower.contains("msedge")
        || source_app_lower.contains("firefox");
    if matches!(classification.bucket, ContextBucket::Unknown)
        && is_browser_like
        && memory.last_bucket == "known"
        && memory
            .last_reaction_time_unix_ms
            .map(|last| now_ms - last <= 20_000)
            .unwrap_or(false)
    {
        if let Some(rule) = known_rule_by_id(&memory.last_matched_rule) {
            classification = ContextClassification {
                bucket: ContextBucket::Known,
                rule_id: rule.id,
                reason: "sticky_known".to_string(),
                cooldown_secs: rule.cooldown_secs,
                action_allowed: true,
                rule_source: "local".to_string(),
            };
        } else if memory.last_matched_rule.starts_with("ccc_trained")
            || memory.last_matched_rule.starts_with("trained_")
        {
            classification = ContextClassification {
                bucket: ContextBucket::Known,
                rule_id: memory.last_matched_rule.clone(),
                reason: "sticky_known".to_string(),
                cooldown_secs: 8,
                action_allowed: true,
                rule_source: "local".to_string(),
            };
        }
    }

    // Extra sticky behavior for browser-based workflows (CCC, Tesla EPC, etc.) when the title flickers.
    if matches!(classification.bucket, ContextBucket::Unknown)
        && is_browser_app(&source_app)
        && sticky_browser_eligible_rule_id(&memory.last_matched_rule)
        && memory.last_bucket == "known"
        && memory
            .last_reaction_time_unix_ms
            .map(|last| now_ms - last <= 120_000)
            .unwrap_or(false)
    {
        if let Some(rule) = known_rule_by_id(&memory.last_matched_rule) {
            classification = ContextClassification {
                bucket: ContextBucket::Known,
                rule_id: rule.id,
                reason: "sticky_workflow_browser".to_string(),
                cooldown_secs: rule.cooldown_secs,
                action_allowed: true,
                rule_source: "local".to_string(),
            };
        } else if memory.last_matched_rule.starts_with("ccc_trained")
            || memory.last_matched_rule.starts_with("trained_")
        {
            classification = ContextClassification {
                bucket: ContextBucket::Known,
                rule_id: memory.last_matched_rule.clone(),
                reason: "sticky_workflow_browser".to_string(),
                cooldown_secs: 8,
                action_allowed: true,
                rule_source: "local".to_string(),
            };
        }
    }

    // Local exclude list wins over built-in rules, training, and sticky heuristics.
    let exclude_rules = load_exclude_rules(app)?;
    if exclude_rules
        .iter()
        .any(|r| match_custom_rule(&source_app_lower, &title_lower, r))
    {
        classification = ContextClassification {
            bucket: ContextBucket::Unknown,
            rule_id: "excluded".to_string(),
            reason: "excluded".to_string(),
            cooldown_secs: 0,
            action_allowed: false,
            rule_source: "local".to_string(),
        };
    }

    let changed =
        memory.last_matched_rule != classification.rule_id || memory.last_window_title != window_title;

    let within_cooldown = classification.action_allowed
        && !changed
        && memory
            .last_reaction_time_unix_ms
            .map(|last| now_ms - last < (classification.cooldown_secs as i64 * 1000))
            .unwrap_or(false);

    memory.last_matched_rule = classification.rule_id.clone();
    memory.last_window_title = window_title.clone();
    memory.last_bucket = bucket_str.to_string();
    memory.last_context_key = context_key.clone();
    memory.last_in_cooldown = within_cooldown;
    if classification.action_allowed && !within_cooldown {
        memory.last_reaction_time_unix_ms = Some(now_ms);
    }

    if changed && !classification.action_allowed {
        let counter = memory
            .candidate_counts
            .entry(context_key)
            .and_modify(|count| *count += 1)
            .or_insert(1);
        let log_line = json!({
            "ts": now_ms,
            "bucket": bucket_str,
            "source_app": source_app,
            "window_title": window_title,
            "rule_id": classification.rule_id,
            "seen_count": counter,
        })
        .to_string();
        let _ = append_candidate_log(app, &log_line);
    }

    save_memory(app, &memory)?;

    let preferred_capture_mode = preferred_capture_mode_for_rule(&classification.rule_id);
    let workflow_kind = workflow_kind_from_rule_id(&classification.rule_id);
    Ok(WatchContext {
        source_app: source_app.clone(),
        window_title: window_title.clone(),
        matched: classification.action_allowed,
        matched_rule: classification.rule_id,
        workflow_kind,
        preferred_capture_mode,
        bucket: classification.bucket,
        action_allowed: classification.action_allowed,
        changed,
        in_cooldown: within_cooldown,
    })
}

pub fn mark_capture(app: &tauri::AppHandle) -> Result<(), String> {
    let mut memory = load_memory(app)?;
    memory.last_capture_time_unix_ms = Some(now_unix_ms());
    save_memory(app, &memory)
}

pub fn train_current_context_as_ccc(app: &tauri::AppHandle) -> Result<String, String> {
    let (source_app, window_title) = current_window_info();
    let (app_keywords, title_signature) =
        training_keywords_for_current_window(&source_app, &window_title);

    let mut rules = load_custom_rules(app)?;
    let duplicate = rules.iter().any(|r| {
        r.id.starts_with("ccc_trained_")
            && r.app_keywords == app_keywords
            && r.title_keywords == vec![title_signature.clone()]
    });
    if !duplicate {
        let id = format!("ccc_trained_{}", now_unix_ms());
        rules.push(Rule {
            id,
            app_keywords,
            title_keywords: vec![title_signature.clone()],
            title_keywords_all: vec![],
            priority: 5,
            cooldown_secs: 8,
            workflow_kind: "ccc".to_string(),
        });
        save_custom_rules(app, &rules)?;
    }

    Ok(format!("Trained CCC pattern: {}", title_signature))
}

pub fn train_current_context_for_workflow(
    app: &tauri::AppHandle,
    workflow: String,
) -> Result<String, String> {
    let wf = workflow.to_lowercase().trim().replace(' ', "_");
    let allowed = ["ccc", "tesla_epc", "parts_trader", "ops_trax", "generic"];
    if !allowed.iter().any(|a| *a == wf.as_str()) {
        return Err(format!(
            "Unknown workflow: {}. Use: {}",
            workflow,
            allowed.join(", ")
        ));
    }
    if wf == "ccc" {
        return train_current_context_as_ccc(app);
    }

    let (source_app, window_title) = current_window_info();
    let (app_keywords, title_signature) =
        training_keywords_for_current_window(&source_app, &window_title);

    let mut rules = load_custom_rules(app)?;
    let prefix = format!("trained_{}_", wf);
    let duplicate = rules.iter().any(|r| {
        r.id.starts_with(&prefix)
            && r.app_keywords == app_keywords
            && r.title_keywords == vec![title_signature.clone()]
    });
    if !duplicate {
        let id = format!("trained_{}_{}", wf, now_unix_ms());
        rules.push(Rule {
            id,
            app_keywords,
            title_keywords: vec![title_signature.clone()],
            title_keywords_all: vec![],
            priority: 5,
            cooldown_secs: 8,
            workflow_kind: wf.clone(),
        });
        save_custom_rules(app, &rules)?;
    }

    Ok(format!("Trained {} pattern: {}", wf, title_signature))
}

/// Remove trained rules (`ccc_trained_*`, `trained_<workflow>_*`) that match the current foreground window.
pub fn forget_trained_rules_for_current_context(app: &tauri::AppHandle) -> Result<String, String> {
    let (source_app, window_title) = current_window_info();
    let source_app_lower = source_app.to_lowercase();
    let title_lower = window_title.to_lowercase();

    let rules = load_custom_rules(app)?;
    let before = rules.len();
    let kept: Vec<Rule> = rules
        .into_iter()
        .filter(|r| {
            let is_trained =
                r.id.starts_with("ccc_trained") || r.id.starts_with("trained_");
            if !is_trained {
                return true;
            }
            !match_custom_rule(&source_app_lower, &title_lower, r)
        })
        .collect();
    let removed = before - kept.len();
    save_custom_rules(app, &kept)?;
    if removed == 0 {
        Ok("No matching trained rules to remove.".to_string())
    } else {
        Ok(format!(
            "Removed {} trained pattern(s) for this app/title.",
            removed
        ))
    }
}

fn training_keywords_for_current_window(
    source_app: &str,
    window_title: &str,
) -> (Vec<String>, String) {
    let app_lower = source_app.to_lowercase();
    let title_signature = title_signature_for_training(window_title);
    let browser_hint = app_lower.contains("chrome")
        || app_lower.contains("msedge")
        || app_lower.contains("firefox");
    let app_keywords = if browser_hint {
        vec![
            "chrome".to_string(),
            "msedge".to_string(),
            "firefox".to_string(),
        ]
    } else if app_lower.is_empty() || app_lower == "unknown" {
        vec![]
    } else {
        vec![app_lower]
    };
    (app_keywords, title_signature)
}

/// Never treat this app/title as an active CCC context (overrides training and stickies).
pub fn exclude_current_context_from_ccc(app: &tauri::AppHandle) -> Result<String, String> {
    let (source_app, window_title) = current_window_info();
    let (app_keywords, title_signature) =
        training_keywords_for_current_window(&source_app, &window_title);

    let mut rules = load_exclude_rules(app)?;
    let duplicate = rules.iter().any(|r| {
        r.id.starts_with("ccc_exclude_")
            && r.app_keywords == app_keywords
            && r.title_keywords == vec![title_signature.clone()]
    });
    if !duplicate {
        let id = format!("ccc_exclude_{}", now_unix_ms());
        rules.push(Rule {
            id,
            app_keywords,
            title_keywords: vec![title_signature.clone()],
            title_keywords_all: vec![],
            priority: 5,
            cooldown_secs: 0,
            workflow_kind: String::new(),
        });
        save_exclude_rules(app, &rules)?;
    }

    Ok(format!(
        "This window will not match CCC (excluded): {}",
        title_signature
    ))
}

/// Drop exclude rules that match the current foreground window.
pub fn clear_exclude_rules_for_current_context(app: &tauri::AppHandle) -> Result<String, String> {
    let (source_app, window_title) = current_window_info();
    let source_app_lower = source_app.to_lowercase();
    let title_lower = window_title.to_lowercase();

    let rules = load_exclude_rules(app)?;
    let before = rules.len();
    let kept: Vec<Rule> = rules
        .into_iter()
        .filter(|r| {
            if !r.id.starts_with("ccc_exclude_") {
                return true;
            }
            !match_custom_rule(&source_app_lower, &title_lower, r)
        })
        .collect();
    let removed = before - kept.len();
    save_exclude_rules(app, &kept)?;
    if removed == 0 {
        Ok("No matching exclude rules to remove.".to_string())
    } else {
        Ok(format!("Removed {} exclude rule(s) for this app/title.", removed))
    }
}
