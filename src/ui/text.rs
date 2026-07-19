use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::palette;

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
    diff.lines().count() + diff.lines().filter(|line| line.starts_with("@@")).count()
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

    for line in diff.lines() {
        if line.starts_with("@@") {
            if display_index >= start && display_index < end {
                lines.push(finish_line(Vec::new(), width, palette().panel));
            }
            display_index += 1;
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

fn finish_line(mut spans: Vec<Span<'static>>, width: usize, background: Color) -> Line<'static> {
    let used: usize = spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    Line::from(spans).style(Style::default().bg(background))
}

fn syntax_spans(code: &str, path: &str) -> Vec<Span<'static>> {
    let hash_comments = matches!(
        path.rsplit('.').next().unwrap_or_default(),
        "py" | "rb" | "sh" | "bash" | "zsh" | "toml" | "yaml" | "yml"
    );
    let mut spans = Vec::new();
    let mut cursor = 0;
    while cursor < code.len() {
        let rest = &code[cursor..];
        if rest.starts_with("//") || (hash_comments && rest.starts_with('#')) {
            spans.push(Span::styled(
                rest.to_owned(),
                Style::default().fg(palette().faint),
            ));
            break;
        }
        let character = rest.chars().next().expect("nonempty remainder");
        if character == '"' || character == '\'' {
            let mut escaped = false;
            let mut end = character.len_utf8();
            for next in rest[character.len_utf8()..].chars() {
                end += next.len_utf8();
                if next == character && !escaped {
                    break;
                }
                escaped = next == '\\' && !escaped;
                if next != '\\' {
                    escaped = false;
                }
            }
            spans.push(Span::styled(
                rest[..end].to_owned(),
                Style::default().fg(palette().yellow),
            ));
            cursor += end;
            continue;
        }
        if character.is_alphanumeric() || character == '_' {
            let end = rest
                .char_indices()
                .find_map(|(index, next)| {
                    (!(next.is_alphanumeric() || next == '_')).then_some(index)
                })
                .unwrap_or(rest.len());
            let token = &rest[..end];
            let following = rest[end..].trim_start();
            let color = if is_keyword(token) {
                palette().purple
            } else if token.chars().all(|next| next.is_ascii_digit()) {
                palette().orange
            } else if token.chars().next().is_some_and(char::is_uppercase)
                || following.starts_with('(')
            {
                palette().cyan
            } else {
                palette().ink
            };
            spans.push(Span::styled(token.to_owned(), Style::default().fg(color)));
            cursor += end;
            continue;
        }
        let (token, color) =
            if rest.starts_with("::") || rest.starts_with("->") || rest.starts_with("=>") {
                (&rest[..2], palette().cyan)
            } else {
                (&rest[..character.len_utf8()], palette().ink)
            };
        spans.push(Span::styled(token.to_owned(), Style::default().fg(color)));
        cursor += token.len();
    }
    spans
}

fn is_keyword(token: &str) -> bool {
    matches!(
        token,
        "as" | "async"
            | "await"
            | "break"
            | "class"
            | "const"
            | "continue"
            | "crate"
            | "def"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "from"
            | "function"
            | "if"
            | "impl"
            | "import"
            | "in"
            | "interface"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "new"
            | "none"
            | "null"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "throw"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "use"
            | "var"
            | "where"
            | "while"
            | "yield"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_source_diff_with_numbers_and_tinted_changes() {
        let lines = styled_diff(
            "@@ -1 +1 @@\n-let old_value = 1;\n+let new_value = 2;",
            "src/main.rs",
            100,
        );

        assert_eq!(lines.len(), 4);
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|span| span.content.trim().is_empty())
        );
        assert_eq!(lines[1].style.bg, Some(palette().surface_alt));
        assert_eq!(lines[2].style.bg, Some(palette().remove_bg));
        assert_eq!(lines[3].style.bg, Some(palette().add_bg));
        assert!(lines[2].spans[0].content.trim().is_empty());
        assert_eq!(lines[3].spans[0].content.trim(), "1");
        assert!(
            lines[3]
                .spans
                .iter()
                .any(|span| span.content == "let" && span.style.fg == Some(palette().purple))
        );
    }
}
