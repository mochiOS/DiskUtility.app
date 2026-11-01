use crate::model::{self, Id, Kind, Model};
use appcore::prelude::*;
use appcore::viewkit::event::{EventContext, EventResult, ViewEvent};
use appcore::viewkit::view::{Constraints, MeasureContext, PaintContext};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Action {
    Select(Id),
    Show(Id),
    Toggle(Id),
    Refresh,
    Back,
    Mount,
    Partition,
    Erase,
}
struct Core {
    model: RefCell<Model>,
    receiver: RefCell<Option<Receiver<appcore::Result<appcore::storage::Snapshot>>>>,
    pending: Rc<Cell<Option<Action>>>,
    buttons: RefCell<BTreeMap<Action, ButtonInteractionState>>,
    toolbar: ActionToolbarState<Action>,
    tree_scroll: ScrollState,
    detail_scroll: ScrollState,
}
impl Core {
    fn interaction(&self, action: Action) -> ButtonInteractionState {
        self.buttons.borrow_mut().entry(action).or_default().clone()
    }
    fn button(&self, label: &str, action: Action) -> Button {
        let interaction = self.interaction(action);
        let pending = self.pending.clone();
        Button::with_interaction_and_label(interaction, label)
            .size(ButtonSize::Small)
            .style(ButtonStyle::Ghost)
            .on_click(move || pending.set(Some(action)))
    }
    fn row(&self, label: &str, id: Id) -> ListRow {
        let pending = self.pending.clone();
        ListRow::with_interaction(self.interaction(Action::Show(id)), label)
            .on_select(move || pending.set(Some(Action::Show(id))))
    }
    fn apply(&self) -> bool {
        let Some(action) = self.pending.take() else {
            return false;
        };
        if action == Action::Refresh && !self.model.borrow().preview {
            self.start_refresh();
            return true;
        }
        let mut m = self.model.borrow_mut();
        match action {
            Action::Select(id) | Action::Show(id) => {
                if m.select(id) {
                    self.detail_scroll.reset();
                }
            }
            Action::Toggle(id) => m.toggle(id),
            Action::Back => m.detail_open = false,
            Action::Refresh => m.refresh(),
            Action::Mount | Action::Partition | Action::Erase => {}
        }
        true
    }
    fn start_refresh(&self) {
        if self.receiver.borrow().is_some() {
            return;
        }
        self.model.borrow_mut().loading = true;
        let (sender, receiver) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("disk-inventory".into())
            .spawn(move || {
                let _ = sender.send(appcore::storage::inventory());
            }) {
            Ok(_) => *self.receiver.borrow_mut() = Some(receiver),
            Err(_) => self
                .model
                .borrow_mut()
                .load_failed(appcore::Error::System(5)),
        }
    }
    fn poll(&self) {
        let result = self
            .receiver
            .borrow()
            .as_ref()
            .map(|receiver| receiver.try_recv());
        match result {
            Some(Ok(result)) => {
                self.receiver.borrow_mut().take();
                match result {
                    Ok(snapshot) => self.model.borrow_mut().apply_snapshot(snapshot),
                    Err(error) => self.model.borrow_mut().load_failed(error),
                }
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.receiver.borrow_mut().take();
                self.model
                    .borrow_mut()
                    .load_failed(appcore::Error::System(5));
            }
            _ => {}
        }
    }
    fn tree(self: &Rc<Self>) -> Scroll {
        Scroll::new(self.tree_scroll.clone()).content(Tree(self.clone()))
    }
    fn detail(self: &Rc<Self>) -> Scroll {
        Scroll::new(self.detail_scroll.clone()).content(Detail(self.clone()))
    }
}
#[derive(Clone)]
pub struct DiskView {
    core: Rc<Core>,
}
const COMPACT_WIDTH: f32 = 700.0;
fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(x, y, w.max(0.0), h.max(0.0))
}
fn fill(b: Rect, color: Color, c: &mut PaintContext<'_>) {
    Rectangle::new()
        .color(RectangleColor::Custom(color))
        .paint(b, c);
}
impl DiskView {
    pub fn new() -> Self {
        let view = Self::with_model(Model::new());
        if !view.core.model.borrow().preview {
            view.core.start_refresh();
        }
        view
    }
    fn with_model(model: Model) -> Self {
        Self {
            core: Rc::new(Core {
                model: RefCell::new(model),
                receiver: RefCell::new(None),
                pending: Rc::new(Cell::new(None)),
                buttons: RefCell::new(BTreeMap::new()),
                toolbar: ActionToolbarState::new(),
                tree_scroll: ScrollState::new(),
                detail_scroll: ScrollState::new(),
            }),
        }
    }
    fn toolbar(&self, compact_detail: bool) -> Toolbar<ActionToolbar<Action>> {
        let m = self.core.model.borrow();
        let mut actions =
            ActionToolbar::new(self.core.toolbar.clone()).context_key(m.selected.unwrap_or(0));
        if !compact_detail {
            actions = actions.title("Disk Utility");
        }
        if compact_detail {
            actions = actions.leading(
                Action::Back,
                self.core
                    .button("Disks", Action::Back)
                    .leading_symbol(SymbolName::ChevronLeft),
            );
        }
        actions = actions.trailing(
            Action::Refresh,
            self.core
                .button("Refresh", Action::Refresh)
                .leading_symbol(SymbolName::Refresh)
                .enabled(!m.loading),
        );
        // These actions remain unavailable until the storage owner supports them.
        let mounted = m
            .selected
            .and_then(|id| m.node(id))
            .is_some_and(|n| n.mount.is_some());
        for (action, label) in [
            (Action::Mount, if mounted { "Unmount" } else { "Mount" }),
            (Action::Partition, "Partition"),
            (Action::Erase, "Erase"),
        ] {
            actions = actions.trailing(action, self.core.button(label, action).enabled(false));
        }
        Toolbar::new(actions)
    }

    fn content(&self, width: f32, theme: &Theme) -> VStack {
        let m = self.core.model.borrow();
        let compact_detail = width < COMPACT_WIDTH && m.detail_open;
        let body: Box<dyn View> = if m.nodes.is_empty() {
            Box::new(
                Surface::pane().content(ContentArea::pane(
                    VStack::new()
                        .alignment(StackAlignment::Stretch)
                        .gap(StackGap::Medium)
                        .child(Text::styled(
                            if m.loading {
                                "Loading disks…"
                            } else if m.error.is_some() {
                                "Disk information unavailable"
                            } else {
                                "No disks found"
                            },
                            TextRole::TitleSmall,
                        ))
                        .child(
                            Text::body(m.error.as_deref().unwrap_or(if m.loading {
                                "Reading disk and mounted-volume information."
                            } else {
                                "No storage devices or mounted volumes were reported."
                            }))
                            .color(theme.colors.text_secondary),
                        ),
                )),
            )
        } else if width >= COMPACT_WIDTH {
            Box::new(
                NavigationSplitView::new(self.core.tree(), self.core.detail())
                    .flexible_sidebar(200.0, theme.layout.navigation_sidebar_width, 280.0)
                    .minimum_detail_width(420.0)
                    .shows_divider(false),
            )
        } else if compact_detail {
            Box::new(Surface::pane().content(self.core.detail()))
        } else {
            Box::new(
                Surface::sidebar().content(
                    Padding::symmetric(theme.spacing.medium, theme.spacing.large)
                        .content(self.core.tree()),
                ),
            )
        };
        let status = if m.preview {
            "Preview · Sample disks. Disk changes are disabled."
        } else if m.loading {
            if m.nodes.is_empty() {
                "Reading disk information…"
            } else {
                "Refreshing… Displayed information may be out of date."
            }
        } else if m.error.is_some() {
            if m.nodes.is_empty() {
                m.error.as_deref().unwrap()
            } else {
                "Refresh failed. Displayed information is out of date; try Refresh again."
            }
        } else if m.partial {
            "Some disk details are unavailable. Available disk and mounted-volume information is shown."
        } else {
            "Read-only disk inspection."
        };
        VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::None)
            .child(self.toolbar(compact_detail))
            .child(body.layout().flex_grow(1.0).flex_shrink(1.0))
            .child(
                Surface::sidebar()
                    .content(
                        Padding::symmetric(theme.spacing.extra_large, theme.spacing.small)
                            .content(Text::caption(status).color(theme.colors.text_secondary)),
                    )
                    .layout()
                    .flex_shrink(0.0),
            )
    }
}
impl View for DiskView {
    fn measure(&self, c: Constraints, _: &mut MeasureContext<'_>) -> Size {
        c.constrain(c.maximum)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        self.core.poll();
        if self.core.model.borrow().loading {
            c.request_redraw_in_at(b, Instant::now() + Duration::from_millis(80));
        }
        Surface::app()
            .content(self.content(b.size.width, c.theme))
            .paint(b, c);
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        if b.size.width < COMPACT_WIDTH
            && self.core.model.borrow().detail_open
            && matches!(
                e,
                ViewEvent::KeyPressed {
                    key: appcore::viewkit::platform::Key::Escape,
                    ..
                }
            )
        {
            self.core.pending.set(Some(Action::Back));
        }
        let result = Surface::app()
            .content(self.content(b.size.width, c.theme()))
            .handle_event(b, e, c);
        if self.core.apply() {
            c.request_redraw_in(b);
            EventResult::Consumed
        } else {
            result
        }
    }
}

struct Tree(Rc<Core>);
impl Tree {
    fn content(&self, theme: &Theme) -> VStack {
        let m = self.0.model.borrow();
        let mut stack = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::ExtraSmall);
        for (id, depth) in m.visible() {
            let n = m.node(id).unwrap();
            let children = m.children(id).next().is_some();
            let mut row = HStack::new()
                .alignment(StackAlignment::Center)
                .gap(StackGap::None);
            let disclosure_width = theme.controls.small_height;
            if children {
                row = row.child(
                    self.0
                        .button("", Action::Toggle(id))
                        .leading_symbol(if m.expanded.contains(&id) {
                            SymbolName::ChevronDown
                        } else {
                            SymbolName::ChevronRight
                        })
                        .accessibility_label(format!(
                            "{} {}",
                            if m.expanded.contains(&id) {
                                "Collapse"
                            } else {
                                "Expand"
                            },
                            n.name
                        ))
                        .frame(disclosure_width, disclosure_width),
                );
            } else {
                row = row.child(
                    Spacer::new()
                        .into_stack_child()
                        .width(disclosure_width)
                        .flex_grow(0.0),
                );
            }
            let pending = self.0.pending.clone();
            let mut selection =
                SidebarItem::with_interaction(self.0.interaction(Action::Select(id)), &n.name)
                    .reserve_symbol_space()
                    .selected(m.selected == Some(id))
                    .on_select(move || pending.set(Some(Action::Select(id))));
            selection = match n.kind {
                Kind::Disk => selection.symbol(SymbolName::Internaldrive),
                Kind::Volume => selection.symbol(SymbolName::Folder),
                _ => selection,
            };
            row = row.child(selection.layout().flex_grow(1.0));
            stack = stack.child(
                Padding::only(
                    if depth == 0 { theme.spacing.small } else { 0.0 },
                    0.0,
                    0.0,
                    depth as f32 * theme.spacing.medium,
                )
                .content(row)
                .layout()
                .flex_shrink(0.0),
            );
        }
        stack
    }
}
impl View for Tree {
    fn measure(&self, c: Constraints, ctx: &mut MeasureContext<'_>) -> Size {
        self.content(ctx.theme).measure(c, ctx)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        self.content(c.theme).paint(b, c);
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        self.content(c.theme()).handle_event(b, e, c)
    }
}
struct Detail(Rc<Core>);
fn info(label: &str, value: impl Into<String>) -> SettingsRow {
    settings_value(label, "", value).compact()
}
impl Detail {
    fn content(&self, theme: &Theme) -> ContentArea<VStack> {
        let m = self.0.model.borrow();
        let mut stack = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::Medium);
        let Some(n) = m.selected.and_then(|id| m.node(id)) else {
            return ContentArea::pane(stack.child(Text::label("Select a disk or volume")));
        };
        stack = stack
            .child(Text::styled(&n.name, TextRole::TitleMedium))
            .child(
                Text::caption(format!("{} · {}", n.kind.label(), model::bytes(n.bytes)))
                    .color(theme.colors.text_secondary),
            );
        if n.kind == Kind::Disk {
            stack = stack.child(Text::label("Partition layout")).child(
                PartitionMap {
                    core: self.0.clone(),
                    disk: n.id,
                }
                .layout()
                .height(48.0),
            );
            let mut partitions = List::new();
            for part in m.children(n.id) {
                partitions = partitions.row(
                    self.0
                        .row(&part.name, part.id)
                        .trailing(model::bytes(part.bytes))
                        .layout()
                        .height(theme.layout.settings_row_height)
                        .flex_shrink(0.0),
                );
            }
            stack = stack.child(partitions);
            stack = stack
                .child(Divider::new())
                .child(info(
                    "Connection",
                    match n.removable {
                        Some(true) => "Removable",
                        Some(false) => "Internal",
                        None => "Unknown",
                    },
                ))
                .child(info(
                    "Partition map",
                    if m.preview {
                        "GPT (sample)"
                    } else {
                        n.format.as_deref().unwrap_or("Unknown")
                    },
                ));
        }
        if n.kind == Kind::Volume {
            stack = stack.child(Text::label("Storage usage"));
            if let (Some(used), Some(free)) = (n.used, n.free()) {
                stack = stack
                    .child(
                        ProgressBar::new(if n.bytes == 0 {
                            0.0
                        } else {
                            used as f32 / n.bytes as f32
                        })
                        .accessibility_label("Storage usage"),
                    )
                    .child(info("Used", model::bytes(used)))
                    .child(info("Available", model::bytes(free)));
            } else {
                stack = stack.child(Text::body("Usage unavailable"));
            }
            stack = stack
                .child(Divider::new())
                .child(info(
                    "File system",
                    n.format.as_deref().unwrap_or("Unknown"),
                ))
                .child(info(
                    "Mount point",
                    n.mount.as_deref().unwrap_or("Not mounted"),
                ));
        }
        if n.kind == Kind::Unallocated {
            stack = stack.child(Text::body("This space is outside the disk's partitions. It may include partition-table metadata."));
        }
        if n.kind == Kind::Partition {
            let volume = m.children(n.id).find(|node| node.kind == Kind::Volume);
            stack = stack
                .child(info(
                    "File system",
                    volume
                        .and_then(|node| node.format.as_deref())
                        .unwrap_or("Unknown"),
                ))
                .child(info(
                    "Mount point",
                    volume
                        .and_then(|node| node.mount.as_deref())
                        .unwrap_or(if m.preview { "Not mounted" } else { "Unknown" }),
                ))
                .child(Text::label("Volumes"));
            if volume.is_none() {
                stack = stack.child(Text::body(
                    "Mount and file system information is unavailable.",
                ));
            }
            let mut volumes = List::new();
            for volume in m.children(n.id) {
                volumes = volumes.row(
                    self.0
                        .row(&volume.name, volume.id)
                        .icon(SymbolName::Folder)
                        .layout()
                        .height(theme.layout.settings_row_height)
                        .flex_shrink(0.0),
                );
            }
            stack = stack.child(volumes);
        }
        stack = stack.child(info(
            if n.capacity_from_partition_map {
                "GPT capacity"
            } else {
                "Capacity"
            },
            model::bytes(n.bytes),
        ));
        if n.capacity_from_partition_map {
            stack = stack.child(
                Text::caption(
                    "Capacity reported by the partition table; hardware capacity is unavailable.",
                )
                .color(theme.colors.text_secondary),
            );
        }
        if n.kind != Kind::Unallocated {
            stack = stack.child(info(
                "Access",
                match n.read_only {
                    Some(true) => "Read-only",
                    Some(false) => {
                        if m.preview {
                            "Read/write (sample)"
                        } else {
                            "Read/write"
                        }
                    }
                    None => "Unknown",
                },
            ));
        }
        if let Some(start) = n.start_sector {
            stack = stack.child(info("First sector", start.to_string()));
        }
        if let Some(sector_bytes) = n.sector_bytes {
            stack = stack.child(info("Sector size", format!("{sector_bytes} bytes")));
        }
        if let Some(disk) = m.disk_for(n.id).filter(|d| d.id != n.id) {
            stack = stack.child(info("Disk", &disk.name));
        }
        stack = stack
            .child(Divider::new())
            .child(Text::label("Disk management"))
            .child(
                Text::caption(
                    "Mounting, erasing and partition editing are not available in this version.",
                )
                .color(theme.colors.text_secondary),
            );
        ContentArea::pane(stack)
    }
}
impl View for Detail {
    fn measure(&self, c: Constraints, ctx: &mut MeasureContext<'_>) -> Size {
        self.content(ctx.theme).measure(c, ctx)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        self.content(c.theme).paint(b, c);
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        self.content(c.theme()).handle_event(b, e, c)
    }
}
struct PartitionMap {
    core: Rc<Core>,
    disk: Id,
}
impl View for PartitionMap {
    fn measure(&self, c: Constraints, _: &mut MeasureContext<'_>) -> Size {
        c.constrain(Size::new(c.maximum.width, 48.0))
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        let m = self.core.model.borrow();
        let Some(disk) = m.node(self.disk).filter(|d| d.bytes > 0) else {
            return;
        };
        let mut x = b.origin.x;
        for part in m.children(disk.id) {
            let width = b.size.width * (part.bytes as f64 / disk.bytes as f64) as f32;
            let color = if part.kind == Kind::Unallocated {
                c.theme.colors.surface_subtle
            } else if part.read_only == Some(true) {
                c.theme.colors.text_secondary
            } else {
                c.theme.colors.accent
            };
            fill(rect(x, b.origin.y + 8.0, width, 24.0), color, c);
            if x > b.origin.x && width > 1.0 {
                fill(
                    rect(x, b.origin.y + 8.0, 1.0, 24.0),
                    c.theme.colors.background,
                    c,
                );
            }
            x += width;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use appcore::viewkit::draw_command::DisplayList;
    use appcore::viewkit::typography::TextMeasurer;
    fn paint(
        view: &DiskView,
        width: f32,
        theme: &Theme,
    ) -> (
        DisplayList,
        Vec<appcore::viewkit::accessibility::AccessibilityNode>,
    ) {
        let mut list = DisplayList::new();
        let mut nodes = Vec::new();
        let mut text = TextMeasurer::new();
        let bounds = rect(0.0, 0.0, width, 640.0);
        view.paint(
            bounds,
            &mut PaintContext::new(&mut list, theme, &theme.typography, &mut text)
                .with_viewport_bounds(bounds)
                .with_accessibility_nodes(&mut nodes),
        );
        (list, nodes)
    }
    #[test]
    fn native_toolbar_and_panes_stay_reachable_in_both_layouts_and_themes() {
        use appcore::viewkit::draw_command::DrawCommand;
        let view = DiskView::with_model(Model::sample());
        for theme in [Theme::LIGHT, Theme::DARK] {
            for width in [320.0, 699.0, 700.0, 960.0, 1600.0] {
                for open in [false, true] {
                    view.core.model.borrow_mut().detail_open = open;
                    let (list, nodes) = paint(&view, width, &theme);
                    let toolbar = nodes
                        .iter()
                        .find(|n| n.role == AccessibilityRole::Toolbar)
                        .unwrap()
                        .bounds;
                    assert_eq!(toolbar.origin.y, 0.0);
                    assert!(
                        nodes
                            .iter()
                            .any(|n| n.label.as_deref() == Some("Refresh") && n.enabled)
                    );
                    assert_eq!(
                        nodes.iter().any(|n| n.label.as_deref() == Some("Disks")),
                        width < COMPACT_WIDTH && open
                    );
                    for node in nodes.iter().filter(|n| n.role == AccessibilityRole::Button) {
                        assert!(node.bounds.origin.x >= 0.0);
                        assert!(node.bounds.origin.x + node.bounds.size.width <= width + 0.01);
                        if node.bounds.origin.y < toolbar.size.height {
                            assert!(
                                node.bounds.origin.y + node.bounds.size.height
                                    <= toolbar.size.height
                            );
                        }
                    }
                    if width >= COMPACT_WIDTH {
                        assert!(list.commands().iter().any(|cmd| matches!(cmd,
                            DrawCommand::PushRoundedCornersClip { rect, radii }
                                if rect.origin.x > 0.0 && rect.origin.y == toolbar.size.height
                                    && radii[0] == theme.radius.extra_large
                        )));
                    }
                }
            }
        }
    }
    #[test]
    fn compact_back_button_preserves_the_selected_volume() {
        use appcore::viewkit::platform::PointerButton;
        let view = DiskView::with_model(Model::sample());
        view.core.model.borrow_mut().select(41);
        let b = rect(0.0, 0.0, 360.0, 480.0);
        let theme = Theme::LIGHT;
        let (_, nodes) = paint(&view, 360.0, &theme);
        let back = nodes
            .iter()
            .find(|n| n.label.as_deref() == Some("Disks"))
            .unwrap()
            .bounds;
        let position = Point::new(
            back.origin.x + back.size.width / 2.0,
            back.origin.y + back.size.height / 2.0,
        );
        let mut measurer = TextMeasurer::new();
        let mut c = EventContext::new(&theme, &theme.typography, &mut measurer);
        for e in [
            ViewEvent::PointerPressed {
                position,
                button: PointerButton::Primary,
            },
            ViewEvent::PointerReleased {
                position,
                button: PointerButton::Primary,
            },
        ] {
            view.handle_event(b, &e, &mut c);
        }
        assert!(!view.core.model.borrow().detail_open);
        assert_eq!(view.core.model.borrow().selected, Some(41));
        assert!(
            view.core
                .model
                .borrow()
                .visible()
                .iter()
                .any(|(id, _)| *id == 41)
        );
    }
    #[test]
    fn unconnected_backend_is_empty_and_never_becomes_a_sample_on_refresh() {
        let mut m = Model::sample();
        m.nodes.clear();
        m.selected = None;
        m.preview = false;
        m.refresh();
        assert!(m.nodes.is_empty());
        let view = DiskView::with_model(m);
        let mut list = DisplayList::new();
        let theme = Theme::DARK;
        let mut measurer = TextMeasurer::new();
        view.paint(
            rect(0.0, 0.0, 320.0, 480.0),
            &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer),
        );
        assert!(!list.commands().is_empty());
    }
    #[test]
    fn preview_paints_all_node_kinds_at_both_widths() {
        let view = DiskView::with_model(Model::sample());
        let theme = Theme::LIGHT;
        for width in [320.0, 960.0] {
            for id in [1, 10, 11, 41, 50, 111] {
                view.core.model.borrow_mut().select(id);
                let mut list = DisplayList::new();
                let mut measurer = TextMeasurer::new();
                view.paint(
                    rect(0.0, 0.0, width, 480.0),
                    &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut measurer),
                );
                assert!(!list.commands().is_empty());
            }
        }
    }

    #[test]
    fn native_sidebar_selection_survives_reconstruction_between_press_and_release() {
        use appcore::viewkit::platform::PointerButton;
        let view = DiskView::with_model(Model::sample());
        let theme = Theme::LIGHT;
        view.core.model.borrow_mut().toggle(40);
        let name = view.core.model.borrow().node(41).unwrap().name.clone();
        let (_, nodes) = paint(&view, 960.0, &theme);
        let row = nodes
            .iter()
            .filter(|n| {
                n.label.as_deref() == Some(name.as_str()) && n.role == AccessibilityRole::Button
            })
            .max_by(|a, b| a.bounds.origin.x.total_cmp(&b.bounds.origin.x))
            .unwrap()
            .bounds;
        let position = Point::new(
            row.origin.x + row.size.width / 2.0,
            row.origin.y + row.size.height / 2.0,
        );
        let mut text = TextMeasurer::new();
        let mut context = EventContext::new(&theme, &theme.typography, &mut text);
        for event in [
            ViewEvent::PointerPressed {
                position,
                button: PointerButton::Primary,
            },
            ViewEvent::PointerReleased {
                position,
                button: PointerButton::Primary,
            },
        ] {
            view.clone()
                .handle_event(rect(0.0, 0.0, 960.0, 640.0), &event, &mut context);
        }
        assert_eq!(view.core.model.borrow().selected, Some(41));
        let (_, nodes) = paint(&view, 960.0, &theme);
        assert!(
            nodes
                .iter()
                .any(|n| n.label.as_deref() == Some("Storage usage")
                    && n.role == AccessibilityRole::ProgressIndicator)
        );
    }

    #[test]
    fn storage_mutations_remain_disabled_and_refresh_does_not_reenable_them() {
        let view = DiskView::with_model(Model::sample());
        for selected in [1, 41] {
            view.core.model.borrow_mut().select(selected);
            for refresh in [false, true] {
                if refresh {
                    view.core.pending.set(Some(Action::Refresh));
                    view.core.apply();
                }
                let (_, nodes) = paint(&view, 960.0, &Theme::LIGHT);
                let mutations: Vec<_> = nodes
                    .iter()
                    .filter(|n| {
                        n.label.as_deref().is_some_and(|label| {
                            matches!(label, "Mount" | "Unmount" | "Partition" | "Erase")
                        })
                    })
                    .collect();
                assert_eq!(mutations.len(), 3);
                assert!(mutations.iter().all(|n| !n.enabled));
            }
        }
    }
}
