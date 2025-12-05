use std::time::Duration;

/// Format a number with two digits (adding leading zero if needed)
pub fn format_two_digits(v: u64) -> String {
    if v < 10 {
        format!("0{}", v)
    } else {
        v.to_string()
    }
}

/// Format a duration as HH:MM:SS or MM:SS
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}:{}:{}", h, format_two_digits(m), format_two_digits(s))
    } else {
        format!("{}:{:02}", m, s)
    }
}

/// Sanitize a window title by removing forbidden characters
pub fn sanitize_window_title(input: &str) -> String {
    let mut filtered = String::with_capacity(input.len());
    for ch in input.chars() {
        if is_forbidden_title_char(ch) {
            continue;
        }
        filtered.push(ch);
    }
    let collapsed = filtered.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    let mut s = if trimmed.is_empty() {
        "Archive".to_string()
    } else {
        trimmed.to_string()
    };
    if s.chars().count() > 128 {
        s = s.chars().take(128).collect();
    }
    s
}

/// Check if a character is forbidden in window titles
fn is_forbidden_title_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'
                | '\u{200C}'
                | '\u{200D}'
                | '\u{200E}'
                | '\u{200F}'
                | '\u{202A}'
                | '\u{202B}'
                | '\u{202C}'
                | '\u{202D}'
                | '\u{202E}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{2060}'
                | '\u{2066}'
                | '\u{2067}'
                | '\u{2068}'
                | '\u{2069}'
                | '\u{FEFF}'
        )
}
