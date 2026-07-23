use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct SearchBarStyle<'a> {
    pub title: &'a str,
    pub placeholder: &'a str,
    pub border_color: Color,
    pub muted_color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisibleSlice {
    start: usize,
    end: usize,
    cursor_col: u16,
}

fn visible_slice(text: &str, cursor: usize, max_width: u16) -> VisibleSlice {
    if max_width == 0 || text.is_empty() {
        return VisibleSlice {
            start: 0,
            end: 0,
            cursor_col: 0,
        };
    }
    let graphemes: Vec<_> = text.grapheme_indices(true).collect();
    let mut boundaries: Vec<_> = graphemes.iter().map(|(index, _)| *index).collect();
    boundaries.push(text.len());
    let cursor = cursor.min(text.len());
    let boundary_index = match boundaries.binary_search(&cursor) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    };
    let mut widths = Vec::with_capacity(boundaries.len());
    let mut width = 0;
    widths.push(0);
    for (_, grapheme) in &graphemes {
        width += grapheme.width();
        widths.push(width);
    }
    let max_width = usize::from(max_width);
    let scroll_column = widths[boundary_index].saturating_sub(max_width.saturating_sub(1));
    let start_index = widths
        .iter()
        .rposition(|column| *column <= scroll_column)
        .unwrap_or_default()
        .min(graphemes.len().saturating_sub(1));
    let mut end_index = start_index;
    let mut visible_width = 0;
    while let Some((_, grapheme)) = graphemes.get(end_index) {
        if visible_width + grapheme.width() > max_width {
            break;
        }
        visible_width += grapheme.width();
        end_index += 1;
    }
    VisibleSlice {
        start: boundaries[start_index],
        end: boundaries[end_index],
        cursor_col: u16::try_from(
            widths[boundary_index]
                .saturating_sub(widths[start_index])
                .min(max_width.saturating_sub(1)),
        )
        .unwrap_or(u16::MAX),
    }
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    style: &SearchBarStyle<'_>,
    search_text: &str,
    cursor: usize,
) {
    let title = crate::display::sanitize(style.title);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(style.border_color));
    let inner = block.inner(area);
    if search_text.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                style.placeholder,
                Style::default().fg(style.muted_color),
            )))
            .block(block),
            area,
        );
        if inner.width > 0 && inner.height > 0 {
            frame.set_cursor_position((inner.x, inner.y));
        }
        return;
    }
    let mut cursor = cursor.min(search_text.len());
    while !search_text.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    let sanitized_cursor = crate::display::sanitize(&search_text[..cursor]).len();
    let sanitized = crate::display::sanitize(search_text);
    let slice = visible_slice(&sanitized, sanitized_cursor, inner.width);
    frame.render_widget(
        Paragraph::new(&sanitized[slice.start..slice.end]).block(block),
        area,
    );
    if inner.width > 0 && inner.height > 0 {
        frame.set_cursor_position((inner.x.saturating_add(slice.cursor_col), inner.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_slice_scrolls_without_splitting_graphemes() {
        let text = "A👩‍💻BC";
        let slice = visible_slice(text, "A👩‍💻B".len(), 3);
        assert_eq!(&text[slice.start..slice.end], "👩‍💻B");
        assert_eq!(slice.cursor_col, 2);
    }
}
