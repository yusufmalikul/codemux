//! One-line text that ends in "…" when it is wider than its space.
//! iced 0.14 has no such option, so the text is cut here, at layout time,
//! with the real font, so the cut always matches what is drawn.
//! It can also mark the places that match the search text.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::text::{self, Paragraph as _};
use iced::advanced::widget::{self, tree, Tree, Widget};
use codemux_core::textmatch;
use iced::{alignment, mouse, Element, Length, Pixels, Rectangle, Size};
use std::ops::Range;

/// Background behind text that matches the search.
const MARK: iced::Color = iced::Color::from_rgba8(0xf0, 0xb4, 0x29, 0.4);

pub(super) fn ellipsis(content: impl Into<String>) -> Ellipsis {
    Ellipsis { content: content.into(), size: None, font: None, color: None, hug: false, mark: String::new() }
}

/// Takes all the width it gets (`Length::Fill`), so in a row the other
/// items keep their full size and only this text is cut.
pub(super) struct Ellipsis {
    content: String,
    size: Option<Pixels>,
    font: Option<iced::Font>,
    color: Option<iced::Color>,
    hug: bool,
    /// Search text (one lowercase line) to mark wherever it shows.
    mark: String,
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

    pub(super) fn color(mut self, color: iced::Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Mark every place the search text `query` shows, ignoring case.
    pub(super) fn mark(mut self, query: &str) -> Self {
        self.mark = textmatch::normalize_query(query);
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

/// What the text was made from, so it is laid out again only when one of
/// these changes: the text, the width, the size, the font and the mark.
type MadeFrom = (String, f32, Pixels, iced::Font, String);

/// The text as shown (maybe cut).
#[derive(Default)]
struct State<P> {
    made_from: Option<MadeFrom>,
    shown: String,
    paragraph: P,
    /// Indexes of the paragraph's spans that match the search.
    marked: Vec<usize>,
}

/// `shown` cut into pieces in order: each with whether it matches `query`.
/// Joined, the pieces give back `shown`.
pub(super) fn pieces(shown: &str, query: &str) -> Vec<(Range<usize>, bool)> {
    let mut out = Vec::new();
    let mut at = 0;
    for r in textmatch::match_ranges(shown, query) {
        if r.start > at {
            out.push((at..r.start, false));
        }
        at = r.end;
        out.push((r, true));
    }
    if at < shown.len() || out.is_empty() {
        out.push((at..shown.len(), false));
    }
    out
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
            .is_some_and(|(c, m, s, f, k)| *c == self.content && *m == max && *s == size && *f == font && *k == self.mark);
        if !unchanged {
            let base = text::Text {
                content: "",
                bounds: Size::INFINITE,
                size,
                line_height: text::LineHeight::default(),
                font,
                align_x: text::Alignment::Default,
                align_y: alignment::Vertical::Top,
                shaping: text::Shaping::default(),
                wrapping: text::Wrapping::None,
            };
            state.shown = fit(&self.content, max, |s| Renderer::Paragraph::with_text(base.with_content(s)).min_width());
            let pieces = pieces(&state.shown, &self.mark);
            let spans: Vec<text::Span<'_, (), iced::Font>> =
                pieces.iter().map(|(r, _)| text::Span::new(&state.shown[r.clone()])).collect();
            state.paragraph = Renderer::Paragraph::with_spans(base.with_content(spans.as_slice()));
            state.marked = pieces.iter().enumerate().filter(|(_, (_, m))| *m).map(|(i, _)| i).collect();
            state.made_from = Some((self.content.clone(), max, size, font, self.mark.clone()));
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
        let origin = layout.bounds().position() - iced::Point::ORIGIN;
        for &i in &state.marked {
            for bounds in state.paragraph.span_bounds(i) {
                let quad = renderer::Quad { bounds: bounds + origin, border: iced::border::rounded(3), ..Default::default() };
                renderer.fill_quad(quad, MARK);
            }
        }
        let color = self.color.unwrap_or(style.text_color);
        renderer.fill_paragraph(&state.paragraph, layout.bounds().position(), color, *viewport);
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
    use super::{fit, pieces};

    #[test]
    fn pieces_split_out_every_match() {
        let shown = "Fix Webhook, webhook…";
        let got: Vec<(&str, bool)> = pieces(shown, "webhook").into_iter().map(|(r, m)| (&shown[r], m)).collect();
        assert_eq!(got, [("Fix ", false), ("Webhook", true), (", ", false), ("webhook", true), ("…", false)]);
        // Nothing to mark: one plain piece, even for empty text.
        assert_eq!(pieces("Fix", ""), [(0..3, false)]);
        assert_eq!(pieces("", "x"), [(0..0, false)]);
    }

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
