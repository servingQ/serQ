//! Drawing an IR program: the deployment view produces a `figure::Figure`,
//! two writers paint it.

pub mod deployment;
pub mod figure;
pub mod svg;
pub mod tikz;

/// A writer: a figure as the text of one format.
pub type Writer = fn(&figure::Figure) -> String;

/// The formats a figure is written in: each name with its writer.
pub const FORMATS: [(&str, Writer); 2] = [("tikz", tikz::render), ("svg", svg::render)];

/// The names of [`FORMATS`], in order.
pub fn format_names() -> Vec<&'static str> {
    FORMATS.iter().map(|&(name, _)| name).collect()
}

/// The figure in the format named `format`, one of [`FORMATS`].
pub fn render(figure: &figure::Figure, format: &str) -> Result<String, String> {
    match FORMATS.iter().find(|&&(name, _)| name == format) {
        Some(&(_, write)) => Ok(write(figure)),
        None => Err(format!(
            "unknown format `{format}` ({})",
            format_names().join(", ")
        )),
    }
}
