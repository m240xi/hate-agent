use std::fmt::Write as _;

const MAX_LINE_CHARS: usize = 600;

pub(crate) fn strip_ansi(text: &str) -> String {
    if !text.contains('\u{1b}') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(n) = chars.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            Some('(' | ')') => {
                chars.next();
            }
            Some(_) | None => {}
        }
    }
    out
}

pub(crate) fn clean(text: &str) -> String {
    let stripped = strip_ansi(text);
    let mut out = String::with_capacity(stripped.len());
    let mut prev: Option<&str> = None;
    let mut repeat = 0usize;
    let mut blank_pending = false;

    let flush = |out: &mut String, line: &str, repeat: usize| {
        push_capped(out, line);
        if repeat > 1 {
            let _ = write!(out, " (×{repeat})");
        }
        out.push('\n');
    };

    for raw in stripped.lines() {
        let line = raw.trim_end_matches('\r');
        let line = line.rsplit('\r').next().unwrap_or(line).trim_end();

        if line.is_empty() {
            blank_pending = prev.is_some();
            continue;
        }
        if prev == Some(line) && !blank_pending {
            repeat += 1;
            continue;
        }
        if let Some(p) = prev {
            flush(&mut out, p, repeat);
        }
        if blank_pending {
            out.push('\n');
            blank_pending = false;
        }
        prev = Some(line);
        repeat = 1;
    }
    if let Some(p) = prev {
        flush(&mut out, p, repeat);
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn push_capped(out: &mut String, line: &str) {
    if line.len() <= MAX_LINE_CHARS {
        out.push_str(line);
        return;
    }
    let total = line.chars().count();
    if total <= MAX_LINE_CHARS {
        out.push_str(line);
        return;
    }
    out.extend(line.chars().take(MAX_LINE_CHARS));
    let _ = write!(out, "…[+{} chars]", total - MAX_LINE_CHARS);
}

pub(crate) fn clip(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines && text.len() <= max_bytes {
        return text.to_string();
    }
    let head_lines = (max_lines / 4).max(1);
    let tail_lines = max_lines.saturating_sub(head_lines).max(1);
    let head_bytes = max_bytes / 4;
    let tail_bytes = max_bytes - head_bytes;

    let mut head_end = 0usize;
    let mut used = 0usize;
    while head_end < lines.len() && head_end < head_lines {
        let cost = lines[head_end].len() + 1;
        if used + cost > head_bytes && head_end > 0 {
            break;
        }
        used += cost;
        head_end += 1;
    }
    let mut tail_start = lines.len();
    let mut used = 0usize;
    while tail_start > head_end && lines.len() - tail_start < tail_lines {
        let cost = lines[tail_start - 1].len() + 1;
        if used + cost > tail_bytes && tail_start < lines.len() {
            break;
        }
        used += cost;
        tail_start -= 1;
    }
    if tail_start <= head_end {
        return text.to_string();
    }

    let omitted = tail_start - head_end;
    let mut out = String::with_capacity(max_bytes + 128);
    for l in &lines[..head_end] {
        out.push_str(l);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "[… {omitted} lines omitted — re-run piped through grep/head/tail to see them]"
    );
    out.push_str(&lines[tail_start..].join("\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_osc_and_keeps_text() {
        let s = "\u{1b}[1;31merror\u{1b}[0m: bad \u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7} ok";
        assert_eq!(strip_ansi(s), "error: bad link ok");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("x\u{1b}"), "x");
    }

    #[test]
    fn progress_redraws_collapse_to_last_frame() {
        let s = "Downloading 10%\rDownloading 50%\rDownloading 100%\r\ndone\r\n";
        assert_eq!(clean(s), "Downloading 100%\ndone");
    }

    #[test]
    fn folds_repeats_and_blank_runs() {
        let s = "\n\nwarn: x\nwarn: x\nwarn: x\n\n\n\nok\nok\nend\n\n";
        assert_eq!(clean(s), "warn: x (×3)\n\nok (×2)\nend");
    }

    #[test]
    fn repeats_across_a_blank_line_stay_separate() {
        assert_eq!(clean("a\n\na\n"), "a\n\na");
    }

    #[test]
    fn long_lines_are_cut_with_count() {
        let s = "x".repeat(MAX_LINE_CHARS + 50);
        let c = clean(&s);
        assert!(c.ends_with("…[+50 chars]"), "{c}");
        let m = "é".repeat(MAX_LINE_CHARS);
        assert_eq!(clean(&m), m);
    }

    #[test]
    fn clip_keeps_head_and_long_tail() {
        let text: String = (1..=1000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let c = clip(&text, 80, 64 * 1024);
        let lines: Vec<&str> = c.lines().collect();
        assert_eq!(lines.len(), 81, "80 kept + marker");
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[19], "line 20");
        assert!(lines[20].contains("920 lines omitted"), "{}", lines[20]);
        assert_eq!(lines[21], "line 941");
        assert_eq!(*lines.last().unwrap(), "line 1000");
    }

    #[test]
    fn clip_respects_byte_budget_and_small_input() {
        assert_eq!(clip("a\nb", 80, 1024), "a\nb");
        let text: String = (0..200)
            .map(|_| "y".repeat(100))
            .collect::<Vec<_>>()
            .join("\n");
        let c = clip(&text, 1000, 4096);
        assert!(c.len() <= 4096 + 200, "len {}", c.len());
        assert!(c.contains("lines omitted"));
    }

    #[test]
    fn clip_single_huge_line_does_not_loop_or_drop_everything() {
        let text = format!("{}\nlast", "z".repeat(10_000));
        let c = clip(&text, 10, 1024);
        assert!(c.ends_with("last"));
    }
}
