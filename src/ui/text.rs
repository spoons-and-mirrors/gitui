use super::palette;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

mod syntax;
use syntax::syntax_spans;

pub(super) fn styled_source(source: &str, path: &str, width: usize) -> Vec<Line<'static>> {
    styled_source_window(source, path, width, 0, usize::MAX)
}

pub(super) fn styled_source_window(
    source: &str,
    path: &str,
    width: usize,
    start: usize,
    count: usize,
) -> Vec<Line<'static>> {
    let numbered = width >= 72;
    source
        .lines()
        .enumerate()
        .skip(start)
        .take(count)
        .map(|(index, line)| {
            let mut spans = if numbered {
                vec![Span::styled(
                    format!("{:>5}  ", index + 1),
                    Style::default().fg(palette().faint),
                )]
            } else {
                Vec::new()
            };
            spans.extend(syntax_spans(line, path));
            finish_line(spans, width, palette().panel)
        })
        .collect()
}

pub(super) fn styled_diff(diff: &str, path: &str, width: usize) -> Vec<Line<'static>> {
    styled_diff_window(diff, path, width, 0, usize::MAX)
}

pub(super) fn diff_display_line_count(diff: &str) -> usize {
    let has_hunks = diff.lines().any(|line| line.starts_with("@@"));
    let mut in_hunk = false;
    let mut count = 0;
    for line in diff.lines() {
        let hunk_header = line.starts_with("@@");
        if has_hunks && !in_hunk && !hunk_header {
            continue;
        }
        if hunk_header {
            count += usize::from(in_hunk);
            in_hunk = true;
        }
        count += 1;
    }
    count
}

pub(super) fn wrapped_preview_line_starts(
    content: &str,
    is_diff: bool,
    width: usize,
) -> Vec<usize> {
    let width = width.max(1);
    let numbered = width >= 72;
    let has_hunks = is_diff && content.lines().any(|line| line.starts_with("@@"));
    let mut in_hunk = false;
    let mut starts = vec![0_usize];
    for line in content.lines() {
        let hunk_header = line.starts_with("@@");
        if has_hunks && !in_hunk && !hunk_header {
            continue;
        }
        if hunk_header {
            if in_hunk {
                starts.push(starts.last().copied().unwrap_or(0).saturating_add(1));
            }
            in_hunk = true;
        }
        let prefix = if !is_diff {
            usize::from(numbered) * 7
        } else if in_hunk
            && !hunk_header
            && !line.starts_with("+++")
            && !line.starts_with("---")
            && (line.starts_with('+') || line.starts_with('-') || line.starts_with(' '))
        {
            usize::from(numbered) * 5 + 1
        } else {
            0
        };
        let payload = if prefix > 0 { &line[1..] } else { line };
        let display_width = prefix.saturating_add(UnicodeWidthStr::width(payload));
        let line_height = display_width.max(1).div_ceil(width);
        starts.push(
            starts
                .last()
                .copied()
                .unwrap_or(0)
                .saturating_add(line_height),
        );
    }
    starts
}

pub(super) fn styled_diff_window(
    diff: &str,
    path: &str,
    width: usize,
    start: usize,
    count: usize,
) -> Vec<Line<'static>> {
    let numbered = width >= 72;
    let mut old_line = None;
    let mut new_line = None;
    let end = start.saturating_add(count);
    let mut display_index = 0;
    let mut lines = Vec::new();
    let has_hunks = diff.lines().any(|line| line.starts_with("@@"));
    let mut in_hunk = false;

    for line in diff.lines() {
        let hunk_header = line.starts_with("@@");
        if has_hunks && !in_hunk && !hunk_header {
            continue;
        }
        if hunk_header {
            if in_hunk {
                if display_index >= start && display_index < end {
                    lines.push(finish_line(Vec::new(), width, palette().panel));
                }
                display_index += 1;
            }
            in_hunk = true;
        }
        if display_index >= end {
            break;
        }
        if display_index >= start {
            lines.push(styled_diff_line(
                line,
                path,
                width,
                numbered,
                &mut old_line,
                &mut new_line,
            ));
        } else {
            advance_diff_line(line, &mut old_line, &mut new_line);
        }
        display_index += 1;
    }
    lines
}

fn styled_diff_line(
    line: &str,
    path: &str,
    width: usize,
    numbered: bool,
    old_line: &mut Option<u32>,
    new_line: &mut Option<u32>,
) -> Line<'static> {
    if line.starts_with("@@") {
        if let Some((old, new)) = parse_hunk_lines(line) {
            *old_line = Some(old);
            *new_line = Some(new);
        }
        return finish_line(
            vec![Span::styled(
                line.to_owned(),
                Style::default()
                    .fg(palette().cyan)
                    .add_modifier(Modifier::BOLD),
            )],
            width,
            palette().surface_alt,
        );
    }
    if line.starts_with("diff --git") {
        return finish_line(
            vec![Span::styled(
                line.to_owned(),
                Style::default()
                    .fg(palette().accent)
                    .add_modifier(Modifier::BOLD),
            )],
            width,
            palette().panel,
        );
    }
    if line.starts_with("index ") {
        return finish_line(
            vec![Span::styled(
                line.to_owned(),
                Style::default().fg(palette().faint),
            )],
            width,
            palette().panel,
        );
    }
    if line.starts_with("---") || line.starts_with("+++") {
        let color = if line.starts_with("---") {
            palette().red
        } else {
            palette().green
        };
        return finish_line(
            vec![Span::styled(line.to_owned(), Style::default().fg(color))],
            width,
            palette().panel,
        );
    }
    if line.starts_with("\\ No newline") {
        return finish_line(
            vec![Span::styled(
                line.to_owned(),
                Style::default().fg(palette().yellow),
            )],
            width,
            palette().panel,
        );
    }
    if line.starts_with("Untracked file:") || line.starts_with("Binary untracked file") {
        return finish_line(
            vec![Span::styled(
                line.to_owned(),
                Style::default()
                    .fg(palette().yellow)
                    .add_modifier(Modifier::BOLD),
            )],
            width,
            palette().panel,
        );
    }

    let (marker, payload, background, new_number) = if let Some(payload) = line.strip_prefix('+') {
        let number = *new_line;
        *new_line = new_line.map(|value| value + 1);
        ("+", payload, palette().add_bg, number)
    } else if let Some(payload) = line.strip_prefix('-') {
        *old_line = old_line.map(|value| value + 1);
        ("-", payload, palette().remove_bg, None)
    } else if let Some(payload) = line.strip_prefix(' ')
        && old_line.is_some()
    {
        let new = *new_line;
        *old_line = old_line.map(|value| value + 1);
        *new_line = new_line.map(|value| value + 1);
        (" ", payload, palette().panel, new)
    } else {
        return finish_line(syntax_spans(line, path), width, palette().panel);
    };

    let mut spans = if numbered {
        line_number(new_number)
    } else {
        Vec::new()
    };
    spans.push(Span::styled(
        marker.to_owned(),
        Style::default()
            .fg(if marker == "+" {
                palette().green
            } else if marker == "-" {
                palette().red
            } else {
                palette().faint
            })
            .add_modifier(Modifier::BOLD),
    ));
    spans.extend(syntax_spans(payload, path));
    finish_line(spans, width, background)
}

fn advance_diff_line(line: &str, old_line: &mut Option<u32>, new_line: &mut Option<u32>) {
    if line.starts_with("@@") {
        if let Some((old, new)) = parse_hunk_lines(line) {
            *old_line = Some(old);
            *new_line = Some(new);
        }
    } else if line.starts_with("+++") || line.starts_with("---") {
    } else if line.starts_with('+') {
        *new_line = new_line.map(|value| value + 1);
    } else if line.starts_with('-') {
        *old_line = old_line.map(|value| value + 1);
    } else if line.starts_with(' ') && old_line.is_some() {
        *old_line = old_line.map(|value| value + 1);
        *new_line = new_line.map(|value| value + 1);
    }
}

fn parse_hunk_lines(line: &str) -> Option<(u32, u32)> {
    let mut fields = line.split_whitespace();
    fields.next()?;
    let old = fields
        .next()?
        .trim_start_matches('-')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new = fields
        .next()?
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old, new))
}

fn line_number(new: Option<u32>) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!(
            "{:>4} ",
            new.map_or_else(String::new, |value| value.to_string())
        ),
        Style::default().fg(palette().faint),
    )]
}

fn finish_line(spans: Vec<Span<'static>>, _width: usize, background: Color) -> Line<'static> {
    Line::from(spans).style(Style::default().bg(background))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_source_diff_with_numbers_and_tinted_changes() {
        let lines = styled_diff(
            concat!(
                "diff --git a/src/main.rs b/src/main.rs\n",
                "index 1234567..abcdef0 100644\n",
                "--- a/src/main.rs\n",
                "+++ b/src/main.rs\n",
                "@@ -1 +1 @@\n",
                "-let old_value = 1;\n",
                "+let new_value = 2;",
            ),
            "src/main.rs",
            100,
        );

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].style.bg, Some(palette().surface_alt));
        assert_eq!(lines[1].style.bg, Some(palette().remove_bg));
        assert_eq!(lines[2].style.bg, Some(palette().add_bg));
        assert!(lines[1].spans[0].content.trim().is_empty());
        assert_eq!(lines[2].spans[0].content.trim(), "1");
        assert!(
            lines[2]
                .spans
                .iter()
                .any(|span| span.content == "let" && span.style.fg == Some(palette().purple))
        );
    }

    #[test]
    fn wrapped_line_index_matches_styled_diff_heights() {
        let diff = concat!(
            "diff --git a/src/main.rs b/src/main.rs\n",
            "@@ -1 +1 @@\n",
            "+a line that wraps\n",
            "@@ -3 +3 @@\n",
            " context\n",
            "diff --git a/very-long-old-name.rs b/very-long-new-name.rs\n",
            "--- a/very-long-old-name.rs\n",
            "+++ b/very-long-new-name.rs\n",
            "@@ -1 +1 @@\n",
            "+emoji 👩‍💻 line",
        );
        let width = 10;
        let lines = styled_diff(diff, "src/main.rs", width);
        let starts = wrapped_preview_line_starts(diff, true, width);

        assert_eq!(starts.len(), lines.len() + 1);
        for (index, line) in lines.iter().enumerate() {
            let display_width = line
                .spans
                .iter()
                .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                .sum::<usize>();
            assert_eq!(
                starts[index + 1] - starts[index],
                display_width.max(1).div_ceil(width)
            );
        }
    }
}
