use sillage_core::SimplifiedTree;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Ansi,
    Html,
    Json,
}

pub fn render(
    _tree: &SimplifiedTree,
    _query_match: &[bool],
    _query: Option<&str>,
    _style: Style,
) -> String {
    todo!("Slice D")
}
