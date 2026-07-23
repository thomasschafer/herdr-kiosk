use ratatui::text::Line;

pub mod branch_picker;
pub mod dialog;
pub mod error_toast;
pub mod help;
pub mod list_row;
pub mod new_branch;
pub mod repo_list;
pub mod search_bar;
pub mod setup;

pub(crate) fn wrapped_content_height(lines: &[Line<'_>], text_width: u16) -> u16 {
    lines.iter().fold(0, |height, line| {
        let width = u16::try_from(line.width()).unwrap_or(u16::MAX);
        height.saturating_add(width.max(1).div_ceil(text_width.max(1)))
    })
}
