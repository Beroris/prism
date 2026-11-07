//! File names for saved captures.
//!
//! Placeholders: `{date}` (2026-11-07), `{time}` (14.05.09), `{yyyy}`, `{MM}`, `{dd}`, `{HH}`,
//! `{mm}`, `{ss}`, `{w}` and `{h}` (capture size). Unknown placeholders are kept as typed.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

pub fn format_name(pattern: &str, t: Stamp, w: u32, h: u32) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut rest = pattern;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find('}') else {
            out.push_str(after);
            rest = "";
            break;
        };
        let key = &after[1..end];
        let value = match key {
            "date" => Some(format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)),
            "time" => Some(format!("{:02}.{:02}.{:02}", t.hour, t.minute, t.second)),
            "yyyy" => Some(format!("{:04}", t.year)),
            "MM" => Some(format!("{:02}", t.month)),
            "dd" => Some(format!("{:02}", t.day)),
            "HH" => Some(format!("{:02}", t.hour)),
            "mm" => Some(format!("{:02}", t.minute)),
            "ss" => Some(format!("{:02}", t.second)),
            "w" => Some(w.to_string()),
            "h" => Some(h.to_string()),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&after[..=end]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    sanitize_file_name(&out)
}

/// Replace characters Windows doesn't allow in file names, and trim what it strips silently.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '-' } else { c }).collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() { "Screenshot".to_string() } else { trimmed.to_string() }
}

/// The first of `name.ext`, `name (2).ext`, `name (3).ext`… for which `exists` is false.
pub fn unique_name(name: &str, ext: &str, exists: impl Fn(&str) -> bool) -> String {
    let first = format!("{name}.{ext}");
    if !exists(&first) {
        return first;
    }
    (2..10_000).map(|n| format!("{name} ({n}).{ext}")).find(|c| !exists(c)).unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Stamp = Stamp { year: 2026, month: 11, day: 7, hour: 9, minute: 5, second: 3 };

    #[test]
    fn formats_placeholders() {
        assert_eq!(format_name("Screenshot {date} {time}", T, 10, 20), "Screenshot 2026-11-07 09.05.03");
        assert_eq!(format_name("{yyyy}{MM}{dd}_{HH}{mm}{ss}_{w}x{h}", T, 10, 20), "20261107_090503_10x20");
    }

    #[test]
    fn keeps_unknown_and_unclosed() {
        assert_eq!(format_name("a {nope} b", T, 1, 1), "a {nope} b");
        assert_eq!(format_name("a {date", T, 1, 1), "a {date");
    }

    #[test]
    fn sanitizes() {
        assert_eq!(format_name("x: y/z?.", T, 1, 1), "x- y-z-");
        assert_eq!(sanitize_file_name("  ...  "), "Screenshot");
    }

    #[test]
    fn unique_suffix() {
        let taken = ["a.png", "a (2).png"];
        assert_eq!(unique_name("a", "png", |n| taken.contains(&n)), "a (3).png");
        assert_eq!(unique_name("b", "png", |n| taken.contains(&n)), "b.png");
    }
}
