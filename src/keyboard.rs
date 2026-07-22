#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    MoveSelection(i32),
    Insert(char),
    Backspace,
    DeleteWord,
    CursorLeft,
    CursorRight,
    ClearQuery,
    OpenRepo,
    OpenBranches,
    OpenBranch,
    BackToRepos,
    DismissToast,
    Noop,
}
