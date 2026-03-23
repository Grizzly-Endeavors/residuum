//! Markdown rendering for agent responses using termimad.

/// Renders markdown text for terminal display.
pub struct MarkdownRenderer {
    skin: termimad::MadSkin,
}

impl MarkdownRenderer {
    /// Create a new renderer.
    ///
    /// When `color_enabled` is true, applies styled headings, bold, code blocks, etc.
    /// When false, uses a plain skin with no ANSI codes.
    #[must_use]
    pub fn new(color_enabled: bool) -> Self {
        let skin = if color_enabled {
            Self::styled_skin()
        } else {
            termimad::MadSkin::no_style()
        };
        Self { skin }
    }

    /// Render markdown content to a styled terminal string.
    #[must_use]
    pub fn render(&self, content: &str) -> String {
        let width = terminal_width();
        self.skin.text(content, Some(width)).to_string()
    }

    fn styled_skin() -> termimad::MadSkin {
        use termimad::crossterm::style::{Attribute, Color};

        let mut skin = termimad::MadSkin::default();
        skin.bold.add_attr(Attribute::Bold);
        skin.italic.add_attr(Attribute::Italic);
        skin.inline_code.set_fg(Color::Green);
        skin.code_block.set_fg(Color::Green);
        // Headings in cyan bold
        skin.headers[0].set_fg(Color::Cyan);
        skin.headers[1].set_fg(Color::Cyan);
        skin
    }
}

fn terminal_width() -> usize {
    let w = termimad::terminal_size().0 as usize;
    w.max(80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_plain_text_passthrough() {
        let renderer = MarkdownRenderer::new(false);
        let output = renderer.render("hello world");
        assert!(
            output.contains("hello world"),
            "plain renderer should preserve text content"
        );
    }

    #[test]
    fn render_with_bold() {
        let renderer = MarkdownRenderer::new(false);
        let output = renderer.render("this is **bold** text");
        assert!(
            output.contains("bold"),
            "rendered output should contain bold word"
        );
        assert!(
            !output.contains("**"),
            "markdown markers should be stripped by renderer"
        );
    }

    #[test]
    fn render_colored_returns_non_empty() {
        let plain_renderer = MarkdownRenderer::new(false);
        let colored_renderer = MarkdownRenderer::new(true);
        let content = "# heading\n\nsome text";
        let plain_output = plain_renderer.render(content);
        let colored_output = colored_renderer.render(content);
        assert!(
            !colored_output.is_empty(),
            "colored render should return non-empty string"
        );
        assert_ne!(
            colored_output, plain_output,
            "colored render should produce different output than plain render"
        );
    }

    #[test]
    fn render_code_block() {
        let renderer = MarkdownRenderer::new(false);
        let output = renderer.render("```\nlet x = 1;\n```");
        assert!(
            output.contains("let x = 1"),
            "code block content should be preserved"
        );
    }
}
