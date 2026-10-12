//! The ratatui widget that draws a [`crate::SlintPane`].

use crate::pane::PaneRender;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::StatefulWidget;

/// Draws a pane's latest encoded frame into its area.
///
/// ```ignore
/// frame.render_stateful_widget(SlintWidget, area, &mut pane);
/// ```
///
/// Rendering never runs Slint or the encoder. If the area differs from the pane's last layout,
/// the pane adopts it and the next frame renders at the new size.
#[derive(Clone, Copy, Debug, Default)]
pub struct SlintWidget;

impl StatefulWidget for SlintWidget {
    type State = dyn PaneRender;

    fn render(self, area: Rect, buf: &mut Buffer, pane: &mut Self::State) {
        pane.render_pane(area, buf);
    }
}
