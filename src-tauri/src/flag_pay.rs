//! Extract labor / flag hours from CCC and Mitchell work-order PDFs and titles
//! so FileWisely can set RO repair flags to the printed hours.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FlagPayLaborLine {
    pub op: String,
    pub hours: f64,
    pub tech: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FlagPayExtract {
    pub ok: bool,
    pub source_system: String,
    pub is_work_order: bool,
    pub repair_order_number: String,
    pub flag_hours_total: Option<f64>,
    pub labor_lines: Vec<FlagPayLaborLine>,
    pub evidence: Vec<String>,
    pub text_excerpt: String,
}

fn infer_source_system(app: &str, title: &str, path: &str) -> String {
    let blob = format!("{app} {title} {path}").to_ascii_lowercase();
    if blob.contains("mitchell") || blob.contains("ultramate") || blob.contains("cloud estimating")
    {
        return "mitchell".to_string();
    }
    if blob.contains("ccc") || blob.contains("workfile") || blob.contains("\\ccc\\") {
        return "ccc".to_string();
    }
    "unknown".to_string()
}

fn looks_like_work_order(title: &str, path: &str, text: &str) -> bool {
    let blob = format!("{title} {path} {text}").to_ascii_lowercase();
    blob.contains("work order")
        || blob.contains("workorder")
        || blob.contains("labor assign")
        || blob.contains("flag hour")
        || blob.contains("tech hour")
        || blob.contains("labor hour")
        || blob.contains("wo#")
        || blob.contains("wo-")
        || blob.contains("w.o")
}

fn extract_ro(text: &str) -> String {
    let t = text.replace('\u{00a0}', " ");
    let patterns: [&str; 6] = [
        r"(?i)\bWO[#:\s-]*(\d{4,7})\b",
        r"(?i)\bW\.?O\.?[#:\s-]*(\d{4,7})\b",
        r"(?i)\bRO[#:\s-]*(\d{4,7})\b",
        r"(?i)\bRepair\s*Order[#:\s]*(\d{4,7})\b",
        r"(?i)\bWork\s*Order[#:\s-]*(\d{4,7})\b",
        r"^(\d{4,6})\b",
    ];
    for pat in patterns {
        if let Some(n) = simple_first_capture(pat, &t) {
            return n;
        }
    }
    String::new()
}

/// Tiny subset matcher: find first `(digits)` capture for the patterns we ship.
fn simple_first_capture(kind: &str, text: &str) -> Option<String> {
    let lower_kind = kind.to_ascii_lowercase();
    let prefixes: &[&str] = if lower_kind.contains("work") && lower_kind.contains("order") {
        &["Work Order", "work order", "WORK ORDER"]
    } else if lower_kind.contains("repair") {
        &["Repair Order", "repair order", "REPAIR ORDER"]
    } else if lower_kind.contains("wo") && !lower_kind.contains("work") {
        &["WO#", "WO:", "WO-", "WO ", "W.O.", "wo#", "wo-"]
    } else if lower_kind.contains("ro") {
        &["RO#", "RO:", "RO-", "RO ", "ro#", "ro-"]
    } else {
        &[""]
    };
    if prefixes == [""] {
        let bytes = text.as_bytes();
        if bytes.len() >= 4 && bytes[0].is_ascii_digit() {
            let mut i = 0;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if (4..=6).contains(&i) {
                return Some(text[..i].to_string());
            }
        }
        return None;
    }
    let search = text;
    for pref in prefixes {
        if pref.is_empty() {
            continue;
        }
        if let Some(idx) = find_ignore_ascii_case(search, pref) {
            let rest = search[idx + pref.len()..].trim_start_matches(['#', ':', '-', ' ', '\t']);
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if (4..=7).contains(&digits.len()) {
                return Some(digits);
            }
        }
    }
    None
}

fn find_ignore_ascii_case(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    h.find(&n)
}

fn parse_hour(raw: &str) -> Option<f64> {
    let n: f64 = raw.parse().ok()?;
    if !(0.1..=40.0).contains(&n) {
        return None;
    }
    Some((n * 100.0).round() / 100.0)
}

fn infer_op(near: &str) -> String {
    let t = near.to_ascii_lowercase();
    if t.contains("refinish") || t.contains("paint") {
        "refinish".to_string()
    } else if t.contains("body") || t.contains("sheet metal") {
        "body".to_string()
    } else if t.contains("mech") {
        "mechanical".to_string()
    } else if t.contains("frame") || t.contains("struct") {
        "frame".to_string()
    } else if t.contains("detail") {
        "detail".to_string()
    } else if t.contains("aluminum") {
        "aluminum".to_string()
    } else {
        "labor".to_string()
    }
}

fn extract_explicit_total(text: &str) -> Option<f64> {
    let lower = text.to_ascii_lowercase();
    for marker in ["flag hours", "flagged hours", "labor hours", "tech hours"] {
        let mut search_from = 0;
        while let Some(rel) = lower[search_from..].find(marker) {
            let idx = search_from + rel + marker.len();
            let rest = text[idx..].trim_start_matches([':', '-', ' ', '\t']);
            let token: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Some(h) = parse_hour(&token) {
                return Some(h);
            }
            search_from = idx;
        }
    }
    None
}

fn extract_hours(text: &str) -> (Option<f64>, Vec<FlagPayLaborLine>) {
    let mut lines = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let token = &text[start..i];
            if let Some(hours) = parse_hour(token) {
                let ctx_start = start.saturating_sub(14);
                let ctx_end = (i + 8).min(text.len());
                let near = &text[ctx_start..ctx_end];
                let near_l = near.to_ascii_lowercase();
                if near_l.contains("flag") {
                    i += 1;
                    continue;
                }
                let looks_hours = near_l.contains("hr")
                    || near_l.contains("hour")
                    || near_l.contains("labor")
                    || near_l.contains("body")
                    || near_l.contains("refinish")
                    || near_l.contains("paint")
                    || near_l.contains("mech")
                    || near_l.contains("frame");
                if looks_hours {
                    let op = infer_op(near);
                    let key = format!("{op}:{hours}");
                    if seen.insert(key) {
                        lines.push(FlagPayLaborLine {
                            op,
                            hours,
                            tech: String::new(),
                        });
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    if let Some(total) = extract_explicit_total(text) {
        return (Some(total), lines);
    }
    if lines.is_empty() {
        return (None, lines);
    }
    let ops: Vec<f64> = lines
        .iter()
        .filter(|l| l.op != "labor")
        .map(|l| l.hours)
        .collect();
    let mut total = if !ops.is_empty() {
        ops.iter().sum()
    } else {
        lines.iter().map(|l| l.hours).fold(0.0_f64, f64::max)
    };
    if total > 80.0 {
        total = lines.iter().map(|l| l.hours).fold(0.0_f64, f64::max);
    }
    (Some((total * 100.0).round() / 100.0), lines)
}

/// Pull printable ASCII runs out of a PDF (good enough for CCC/Mitchell labor pages).
pub fn extract_pdf_text(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut run = Vec::new();
    for &b in bytes {
        if (32..127).contains(&b) || b == b'\n' || b == b'\r' || b == b'\t' {
            run.push(b);
        } else if run.len() >= 4 {
            if !out.is_empty() {
                out.push(' ');
            }
            if let Ok(s) = std::str::from_utf8(&run) {
                out.push_str(s);
            }
            run.clear();
        } else {
            run.clear();
        }
    }
    if run.len() >= 4 {
        if !out.is_empty() {
            out.push(' ');
        }
        if let Ok(s) = std::str::from_utf8(&run) {
            out.push_str(s);
        }
    }
    if out.len() > 12_000 {
        out.truncate(12_000);
    }
    out
}

pub fn analyze_work_order(app: &str, title: &str, path: &str, pdf_text: &str) -> FlagPayExtract {
    let (title_hours, title_lines) = extract_hours(&format!("{title} {path}"));
    let (pdf_hours, pdf_lines) = extract_hours(pdf_text);
    let labor_lines = if !pdf_lines.is_empty() {
        pdf_lines
    } else {
        title_lines
    };
    let flag_hours_total = pdf_hours.or(title_hours);
    let is_wo = looks_like_work_order(title, path, pdf_text);
    let mut ro = extract_ro(title);
    if ro.is_empty() {
        ro = extract_ro(path);
    }
    if ro.is_empty() {
        ro = extract_ro(pdf_text);
    }
    let mut evidence = Vec::new();
    if title_hours.is_some() {
        evidence.push("window_or_filename".to_string());
    }
    if pdf_hours.is_some() {
        evidence.push("pdf_text".to_string());
    }
    if is_wo {
        evidence.push("work_order_surface".to_string());
    }
    let excerpt: String = pdf_text.chars().take(280).collect();
    FlagPayExtract {
        ok: is_wo || flag_hours_total.is_some(),
        source_system: infer_source_system(app, title, path),
        is_work_order: is_wo,
        repair_order_number: ro,
        flag_hours_total,
        labor_lines,
        evidence,
        text_excerpt: excerpt,
    }
}

#[tauri::command]
pub fn uce_extract_flag_pay(
    path: String,
    window_title: Option<String>,
    source_app: Option<String>,
) -> Result<FlagPayExtract, String> {
    let title = window_title.unwrap_or_default();
    let app = source_app.unwrap_or_default();
    let p = PathBuf::from(path.trim());
    let mut pdf_text = String::new();
    if p.exists()
        && p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false)
    {
        let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
        pdf_text = extract_pdf_text(&bytes);
    }
    let path_s = p.to_string_lossy().into_owned();
    Ok(analyze_work_order(&app, &title, &path_s, &pdf_text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_hours_from_ccc_work_order_text() {
        let (total, lines) = extract_hours("Work Order 90066 Body 4.0 Refinish 2.5 Flag hours 6.5");
        assert_eq!(total, Some(6.5));
        assert!(lines.iter().any(|l| l.op == "body" && (l.hours - 4.0).abs() < f64::EPSILON));
    }

    #[test]
    fn reads_ro_from_mitchell_title() {
        assert_eq!(
            extract_ro("Mitchell Cloud Estimating - Work Order 44112"),
            "44112"
        );
    }

    #[test]
    fn analyze_marks_updateable_work_order() {
        let got = analyze_work_order(
            "ccc.exe",
            "90066 Work Order Flag hours 6.5",
            r"C:\FileWisely\Incoming\WO_90066.pdf",
            "",
        );
        assert!(got.is_work_order);
        assert_eq!(got.source_system, "ccc");
        assert_eq!(got.repair_order_number, "90066");
        assert_eq!(got.flag_hours_total, Some(6.5));
    }
}
