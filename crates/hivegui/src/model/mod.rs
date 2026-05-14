pub mod conversation;
pub mod tools;

/// Sanitise raw text typed by the engineer before it crosses the
/// HiveClaw boundary. Per FR-011: trim, normalise line endings, and
/// reject control characters other than `\n`, `\t`.
pub fn sanitize_user_input(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    // Normalise CRLF / CR to LF first.
    let normalised = raw.replace("\r\n", "\n").replace('\r', "\n");
    for c in normalised.chars() {
        if c == '\n' || c == '\t' {
            out.push(c);
            continue;
        }
        if (c as u32) < 0x20 {
            // Drop ASCII control characters.
            continue;
        }
        if (c as u32) == 0x7f {
            // Drop DEL.
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_whitespace() {
        assert_eq!(sanitize_user_input("  hello  "), "hello");
    }

    #[test]
    fn normalises_crlf() {
        assert_eq!(sanitize_user_input("a\r\nb"), "a\nb");
        assert_eq!(sanitize_user_input("a\rb"), "a\nb");
    }

    #[test]
    fn keeps_tab_and_newline() {
        assert_eq!(sanitize_user_input("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn drops_control_chars() {
        assert_eq!(sanitize_user_input("a\x01b\x7fc"), "abc");
    }
}
