//! A pane: one of the plots stacked in a [`Plot`](super::Plot), sharing
//! its time window. Each pane has value axes of its own, with what was
//! decided and drawn about them this frame; the traces, the time window
//! and everything else are the plot's.

use super::ValueAxis;

/// A plot area with its own value axes.
pub(super) struct Pane {
    /// Tells it from the others (a trace's `pane`), for as long as the
    /// plot lives: undo brings a pane back with its id.
    pub id: u32,
    /// Its share of the height, against the others' (1 each, equal).
    pub weight: f32,
    /// Its value axes, in the order they are laid out (those with no
    /// visible trace aside).
    pub axes: Vec<ValueAxis>,
    /// The axes drawn this frame, as ids in `axes`.
    pub drawn: Vec<u32>,
    /// Each axis' name, by id, as of this frame.
    pub axis_names: Vec<(u32, String)>,
    /// Each trace's value axis this frame (an index into `drawn`), in
    /// the plot's trace order; `None` for one not drawn here (hidden, not
    /// settled, or in another pane).
    pub axis_of: Vec<Option<usize>>,
    /// The axis clicked: its traces are picked out, the others dimmed.
    pub axis_highlight: Option<u32>,
    /// `axis_highlight` before the last click on an axis, to undo when
    /// that click was the first of a double-click (renaming).
    pub highlight_before: Option<Option<u32>>,
}

impl Pane {
    pub fn new(id: u32) -> Self {
        Self::with_axes(id, 1.0, Vec::new())
    }

    pub fn with_axes(id: u32, weight: f32, axes: Vec<ValueAxis>) -> Self {
        Self {
            id,
            weight,
            axes,
            drawn: Vec::new(),
            axis_names: Vec::new(),
            axis_of: Vec::new(),
            axis_highlight: None,
            highlight_before: None,
        }
    }

    /// Its axis with id `id`.
    pub fn axis(&self, id: u32) -> Option<&ValueAxis> {
        self.axes.iter().find(|a| a.id == id)
    }

    pub fn axis_mut(&mut self, id: u32) -> Option<&mut ValueAxis> {
        self.axes.iter_mut().find(|a| a.id == id)
    }

    /// The name axis `id` was drawn with this frame.
    pub fn axis_name(&self, id: u32) -> Option<&str> {
        (self.axis_names.iter())
            .find(|(a, _)| *a == id)
            .map(|(_, n)| n.as_str())
    }
}
