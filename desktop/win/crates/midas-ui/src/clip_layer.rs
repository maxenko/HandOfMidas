//! `ClipLayer` — wraps content in an iced renderer layer pinned to its own bounds.
//!
//! Why this exists
//! ===============
//!
//! `iced::widget::container::clip(true)` only narrows the iced `viewport`
//! parameter passed to child draws — it does NOT push a renderer layer.
//! Quads/text/image primitives respect that viewport, but custom **shader
//! primitives** ignore it entirely. Their `clip_bounds` (used by `iced_wgpu`
//! to set the wgpu scissor + viewport) is computed as
//! `(instance.bounds * scale) ∩ (layer.bounds * scale)`, where `layer.bounds`
//! defaults to `Rectangle::INFINITE` for the root layer.
//!
//! When a shader-bearing widget's laid-out bounds extend beyond a parent
//! pane (e.g. a fixed-width Chart cell wider than the watchlist column it
//! lives in), the shader paints across the overflow into sibling panes.
//! Wrapping the offending subtree in `ClipLayer` calls
//! [`Renderer::start_layer`] / [`Renderer::end_layer`] so iced records the
//! children inside a new layer whose bounds are exactly this widget's
//! laid-out rect — clamping every shader's `clip_bounds` accordingly.
//!
//! See `iced/examples/custom_shader` and PR iced-rs/iced#2738 for the
//! corresponding fix on the shader-side primitive (returning `false` from
//! `Primitive::draw` and using `Primitive::render` with explicit clip
//! bounds). `ClipLayer` is the wrapper-side complement.

use iced::advanced::layout::{Layout, Limits, Node};
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Wrap `content` in a renderer layer pinned to its own laid-out bounds.
pub struct ClipLayer<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> ClipLayer<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    pub fn new(content: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            content: content.into(),
        }
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ClipLayer<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn state(&self) -> tree::State {
        tree::State::None
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &Limits) -> Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        // Intersect with the parent viewport so we never expand the clip
        // region — only shrink it. If our bounds don't intersect the
        // viewport we have nothing to draw.
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                &clip,
            );
        });
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<ClipLayer<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(layer: ClipLayer<'a, Message, Theme, Renderer>) -> Self {
        Element::new(layer)
    }
}

/// Convenience constructor mirroring iced's `container(...)` style.
pub fn clip_layer<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> ClipLayer<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    ClipLayer::new(content)
}
