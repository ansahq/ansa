use gpui::{
    App, Bounds, Div, Element, ElementId, GlobalElementId, InspectorElementId, LayoutId, Pixels,
    Role, SharedString, Window, div, prelude::*, px,
};

pub(super) struct LiveStatus {
    label: SharedString,
    content: Div,
}

impl LiveStatus {
    pub(super) fn new(label: String) -> Self {
        let label: SharedString = label.into();
        Self {
            label: label.clone(),
            content: div().px_4().py_2().text_size(px(12.)).child(label),
        }
    }
}

impl Element for LiveStatus {
    type RequestLayoutState = <Div as Element>::RequestLayoutState;
    type PrepaintState = <Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some("candidate-live-status".into())
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn a11y_role(&self) -> Option<Role> {
        Some(Role::Status)
    }
    fn write_a11y_info(&self, node: &mut gpui::accesskit::Node) {
        node.set_label(self.label.to_string());
        // Role::Status alone does not imply a live setting in this AccessKit version.
        node.set_live(gpui::accesskit::Live::Polite);
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.content.request_layout(None, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.content
            .prepaint(None, inspector, bounds, state, window, cx)
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.content
            .paint(None, inspector, bounds, state, prepaint, window, cx);
    }
}
