//! One-line text that ends in "…" when it is wider than its space.
//! iced 0.14 has no such option, so the text is cut here, at layout time,
//! with the real font, so the cut always matches what is drawn.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::{self, tree, Tree, Widget};
use iced::{alignment, mouse, Element, Length, Pixels, Rectangle, Size};

pub(super) fn ellipsis(content: impl Into<String>) -> Ellipsis {
    Ellipsis { content: content.into(), size: None, font: None, hug: false }
}

/// Takes all the width it gets (`Length::Fill`), so in a row the other
/// items keep their full size and only this text is cut.
pub(super) struct Ellipsis {
    content: String,
    size: Option<Pixels>,
    font: Option<iced::Font>,
    hug: bool,
}

impl Ellipsis {
    pub(super) fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.size = Some(size.into());
        self
    }

    pub(super) fn font(mut self, font: iced::Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Use only the width of the shown text, so what comes after it in a
    /// row follows right after it instead of at the far end. It still asks
    /// for `Fill`, so a row gives the other items their space first.
    pub(super) fn hug(mut self) -> Self {
        self.hug = true;
        self
    }
}

/// The text as shown (maybe cut), and what it was made from, so it is
/// cut again only when the text, font or width changes.
#[derive(Default)]
struct State<P> {
    made_from: Option<(String, f32, Pixels, iced::Font)>,
    shown: String,
    paragraph: P,
}

/// One line of `content`: line breaks and runs of spaces become one space.
/// When it is wider than `max`, the longest start of it that fits with
/// "…" after it. `width` measures a string in the real font.
pub(super) fn fit(content: &str, max: f32, width: impl Fn(&str) -> f32) -> String {
    let line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if width(&line) <= max {
        return line;
    }
    let cut = |chars: usize| {
        let end = line.char_indices().nth(chars).map_or(line.len(), |(i, _)| i);
        format!("{}…", line[..end].trim_end())
    };
    // Binary search for the most characters that still fit.
    let (mut lo, mut hi) = (0, line.chars().count());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width(&cut(mid)) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for Ellipsis
where
    Renderer: text::Renderer<Font = iced::Font>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::<Renderer::Paragraph>::default())
    }

    fn size(&self) -> Size<Length> {
        Size { width: Length::Fill, height: Length::Shrink }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let state = tree.state.downcast_mut::<State<Renderer::Paragraph>>();
        let max = limits.max().width;
        let size = self.size.unwrap_or_else(|| renderer.default_size());
        let font = self.font.unwrap_or_else(|| renderer.default_font());
        let unchanged = state
            .made_from
            .as_ref()
            .is_some_and(|(c, m, s, f)| *c == self.content && *m == max && *s == size && *f == font);
        if !unchanged {
            let paragraph = |content: &str| {
                Renderer::Paragraph::with_text(text::Text {
                    content,
                    bounds: Size::INFINITE,
                    size,
                    line_height: text::LineHeight::default(),
                    font,
                    align_x: text::Alignment::Default,
                    align_y: alignment::Vertical::Top,
                    shaping: text::Shaping::default(),
                    wrapping: text::Wrapping::None,
                })
            };
            state.shown = fit(&self.content, max, |s| paragraph(s).min_width());
            state.paragraph = paragraph(&state.shown);
            state.made_from = Some((self.content.clone(), max, size, font));
        }
        let size = limits.resolve(Length::Fill, Length::Shrink, state.paragraph.min_bounds());
        // A row gives a `Fill` item a fixed width (min = max), so hugging
        // the text is done here, not through the limits.
        let width = if self.hug { state.paragraph.min_width().min(size.width) } else { size.width };
        layout::Node::new(Size::new(width, size.height))
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        renderer.fill_paragraph(&state.paragraph, layout.bounds().position(), style.text_color, *viewport);
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, _renderer: &Renderer, operation: &mut dyn widget::Operation) {
        let state = tree.state.downcast_ref::<State<Renderer::Paragraph>>();
        operation.text(None, layout.bounds(), &state.shown);
    }
}

impl<'a, Message, Theme, Renderer> From<Ellipsis> for Element<'a, Message, Theme, Renderer>
where
    Renderer: text::Renderer<Font = iced::Font> + 'a,
{
    fn from(e: Ellipsis) -> Self {
        Element::new(e)
    }
}

#[cfg(test)]
mod tests {
    use super::fit;

    /// Every character is 1 wide, so widths are easy to count.
    fn chars(s: &str) -> f32 {
        s.chars().count() as f32
    }

    #[test]
    fn short_text_is_kept_whole_on_one_line() {
        assert_eq!(fit("Fix  webhook\nretry", 20.0, chars), "Fix webhook retry");
    }

    #[test]
    fn long_text_is_cut_with_dots_and_fits() {
        let cut = fit("Fix webhook retry", 10.0, chars);
        assert_eq!(cut, "Fix webho…");
        // No space left before the dots.
        assert_eq!(fit("Fix webhook retry", 5.0, chars), "Fix…");
    }

    #[test]
    fn no_room_leaves_only_the_dots() {
        assert_eq!(fit("Fix", 0.0, chars), "…");
        assert_eq!(fit("ÄÖÜ ÄÖÜ", 3.0, chars), "ÄÖ…");
    }
}
