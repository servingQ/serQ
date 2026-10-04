//! Drawing an IR program: the deployment view produces a `figure::Figure`,
//! two writers paint it.

pub mod deployment;
pub mod figure;
pub mod svg;
pub mod tikz;

/// The formats a figure is written in, by name.
pub const FORMATS: [&str; 2] = ["tikz", "svg"];

/// The figure in the format named `format`, one of [`FORMATS`].
pub fn render(figure: &figure::Figure, format: &str) -> Result<String, String> {
    match format {
        "tikz" => Ok(tikz::render(figure)),
        "svg" => Ok(svg::render(figure)),
        f => Err(format!("unknown format `{f}` ({})", FORMATS.join(", "))),
    }
}
