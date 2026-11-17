//! Polite live status node for the upstream accessibility candidate.
//!
//! `Role::Status` alone does not make this AccessKit version announce changes;
//! the node must also carry `Live::Polite`. GPUI's fluent `aria_*` helpers do
//! not expose that property, so this element delegates layout and painting to
//! its inner `Div` and adds the role, label and live setting in
//! `write_a11y_info`.
//!
//! The pattern is the same one Ansa uses in the isolated accessibility spike:
//! a renderer-independent status element, not a second copy of the launcher.

use gpui::{
    App, Bounds, Div, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId,
    ParentElement, Pixels, Role, SharedString, Styled, Window, div,
};

pub(crate) struct LiveStatus {
    label: SharedString,
    content: Div,
}

impl LiveStatus {
    pub(crate) fn new(label: impl Into<SharedString>, content: Div) -> Self {
        Self {
            label: label.into(),
            content: div().flex_1().min_w_0().child(content),
        }
    }
}

impl Element for LiveStatus {
    type RequestLayoutState = <Div as Element>::RequestLayoutState;
    type PrepaintState = <Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some("ansa-live-status".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn a11y_role(&self) -> Option<Role> {
        Some(Role::Status)
    }

    fn write_a11y_info(&self, node: &mut gpui::accesskit::Node) {
        node.set_label(self.label.to_string());
        node.set_live(gpui::accesskit::Live::Polite);
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.content.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.content
            .prepaint(None, inspector_id, bounds, state, window, cx)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.content
            .paint(None, inspector_id, bounds, state, prepaint, window, cx);
    }
}
