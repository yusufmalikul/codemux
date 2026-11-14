//! A scrollable block that is as tall as its first N rows, and scrolls the rest.
//!
//! Sidebar rows are not all the same height (a tab row is taller than a
//! session row, and a search match adds a second line), so a fixed pixel
//! height can not show "exactly N rows". This wrapper lays the block out
//! once, reads where row N ends, and lays it out again cut at that point.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, tree, Tree, Widget};
use iced::advanced::{overlay, renderer, Clipboard, Shell};
use iced::{mouse, Element, Event, Length, Rectangle, Size, Vector};

/// `scrollable` must hold a column whose first child is the block's
/// header; the rows follow it. The block grows with its rows up to `rows`
/// of them, then scrolls.
pub(super) fn max_rows<'a, Message: 'a>(scrollable: impl Into<Element<'a, Message>>, rows: usize) -> Element<'a, Message> {
    Element::new(MaxRows { content: scrollable.into(), rows })
}

struct MaxRows<'a, Message> {
    content: Element<'a, Message>,
    rows: usize,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MaxRows<'_, Message> {
    // Fully see-through: the tree, layout and events are the scrollable's own.
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        Size { width: Length::Fill, height: Length::Shrink }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        let full = self.content.as_widget_mut().layout(tree, renderer, limits);
        // The scrollable's one child is the column: the header, then the rows.
        // Child 0 is the header, so row N is child N. Cut at its bottom only
        // when there is a row after it.
        let cut = full
            .children()
            .first()
            .map(|column| column.children())
            .filter(|kids| kids.len() > self.rows + 1)
            .map(|kids| kids[self.rows].bounds().y + kids[self.rows].bounds().height);
        match cut {
            Some(height) => self.content.as_widget_mut().layout(tree, renderer, &limits.max_height(height)),
            None => full,
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &iced::Renderer, operation: &mut dyn widget::Operation) {
        self.content.as_widget_mut().operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(tree, layout, renderer, viewport, translation)
    }
}
