//! Sidebar status, labels, animation colors and frame assembly.
use super::super::deletion::DeleteTarget;
use super::super::{
    forms::{Form, RenameTarget},
    BusUi,
};
use super::geometry::layout;
use super::text::cells;
use super::{cell_width, provider, status, wrap, Action, View, ACCENT};
use crate::messaging::model::{BusState, Room, RoomAgent, RoomId, RoomKind, RuntimeStatus};
use ratatui::{layout::Rect, style::Color};

const WORKING_MID: Color = Color::Rgb(68, 190, 84);
const WORKING_DIM: Color = Color::Rgb(36, 112, 52);
const BLOCKED_BRIGHT: Color = Color::Rgb(255, 92, 102);
const BLOCKED_MID: Color = Color::Rgb(205, 64, 72);
const BLOCKED_DIM: Color = Color::Rgb(128, 44, 52);

fn agent_status(agent: &RoomAgent) -> &'static str {
    if !agent.hook_setup_confirmed && !agent.session_binding_invalidated && !agent.deletion_pending
    {
        "Not ready"
    } else {
        status(agent.shown_status())
    }
}

/// The agent's identity color for the current color vision setting.
pub(super) fn identity_color(
    agent: &RoomAgent,
    settings: &crate::messaging::prefs::settings::BusSettings,
) -> Color {
    let [r, g, b] = if settings.color_blind_mode {
        agent.accessible_color
    } else {
        agent.color
    };
    Color::Rgb(r, g, b)
}

/// The color of an agent's name in a message: in a work room, every MASTER
/// orchestrator is drawn in You's green, which agent allocation (standard and
/// color blind) keeps clear of, so it never shares a worker's color. MASTER
/// itself keeps identity colors to tell its orchestrators apart.
pub(super) fn message_name_color(
    state: &BusState,
    open_room: Option<RoomId>,
    agent: &RoomAgent,
    settings: &crate::messaging::prefs::settings::BusSettings,
) -> Color {
    let in_master = |room| state.master_room().is_some_and(|master| master.id == room);
    if in_master(agent.room_id) && !open_room.is_some_and(in_master) {
        ACCENT
    } else {
        identity_color(agent, settings)
    }
}

fn midpoint_color(first: Color, second: Color) -> Color {
    let midpoint = |first: u8, second: u8| (u16::from(first) + u16::from(second)).div_ceil(2) as u8;
    match (first, second) {
        (Color::Rgb(fr, fg, fb), Color::Rgb(sr, sg, sb)) => {
            Color::Rgb(midpoint(fr, sr), midpoint(fg, sg), midpoint(fb, sb))
        }
        _ => first,
    }
}

fn working_status_colors(word: &str, keyframe: usize) -> Vec<Color> {
    let head = keyframe % word.len();
    word.chars()
        .enumerate()
        .map(|(index, _)| {
            if index == head {
                ACCENT
            } else if index.abs_diff(head) == 1 {
                WORKING_MID
            } else {
                WORKING_DIM
            }
        })
        .collect()
}

/// Animated colors for a status word shown on an agent or room row.
fn animated_status_colors(word: &str, phase: u8) -> Option<Vec<Color>> {
    match word {
        "Working" => {
            let keyframe = usize::from(phase / 2);
            let current = working_status_colors(word, keyframe);
            if phase.is_multiple_of(2) {
                Some(current)
            } else {
                let next = working_status_colors(word, keyframe + 1);
                Some(
                    current
                        .into_iter()
                        .zip(next)
                        .map(|(current, next)| midpoint_color(current, next))
                        .collect(),
                )
            }
        }
        "Blocked" => {
            let keyframe = phase / 2;
            let color_at = |keyframe| match keyframe % 6 {
                0 | 5 => BLOCKED_DIM,
                2 | 3 => BLOCKED_BRIGHT,
                _ => BLOCKED_MID,
            };
            let current = color_at(keyframe);
            let color = if phase.is_multiple_of(2) {
                current
            } else {
                midpoint_color(current, color_at(keyframe + 1))
            };
            Some(vec![color; word.len()])
        }
        _ => None,
    }
}

/// Rows taken by the MASTER room entry and the gap before ROOMS.
const MASTER_SECTION_ROWS: usize = 2;

/// Rows of a MASTER agent: name, provider, orchestrated room and a gap.
const MASTER_AGENT_ROWS: usize = 4;

/// The logical row of the ROOMS header while `selected` is open. With MASTER
/// open, its AGENTS section sits between MASTER and ROOMS.
fn sidebar_rooms_header(state: &BusState, selected: Option<RoomId>) -> usize {
    let Some(master) = state.master_room().map(|master| master.id) else {
        return 1;
    };
    if selected != Some(master) {
        return 1 + MASTER_SECTION_ROWS;
    }
    let agents = state
        .agents()
        .filter(|agent| agent.room_id == master)
        .count();
    // AGENTS header, gap, the agents, then the divider.
    1 + MASTER_SECTION_ROWS + 2 + agents * MASTER_AGENT_ROWS + 1
}

/// The logical sidebar row of a room while `selected` is open: MASTER is the
/// first entry, alone above the rest; work rooms follow the ROOMS header.
pub(in crate::client::rooms) fn sidebar_room_row(
    state: &BusState,
    room: RoomId,
    selected: Option<RoomId>,
) -> Option<usize> {
    if state.master_room().is_some_and(|master| master.id == room) {
        return Some(1);
    }
    let first = sidebar_rooms_header(state, selected) + 2;
    state
        .rooms()
        .filter(|candidate| candidate.kind == RoomKind::Work)
        .position(|candidate| candidate.id == room)
        .map(|index| first + index)
}

/// A room's sidebar label, its name truncated to fit in `width` cells.
pub(in crate::client::rooms) fn room_label(room: &Room, width: usize) -> String {
    let room_width = width.saturating_sub(2);
    let name = if cells(&room.name) <= room_width {
        room.name.clone()
    } else {
        let mut name = String::new();
        let mut used = 0;
        for c in room.name.chars() {
            if used + cell_width(c) + 1 > room_width {
                break;
            }
            used += cell_width(c);
            name.push(c);
        }
        name.push('…');
        name
    };
    format!("# {name}")
}

/// The MASTER line under an agent's provider: the room it orchestrates, or
/// `# unassigned` for a saved orchestrator whose room failed the load checks.
/// Written like the ROOMS list, `# name`.
pub(in crate::client::rooms) fn orchestrated_room_label(
    state: &BusState,
    agent: &RoomAgent,
) -> String {
    match agent.orchestrates.and_then(|room| state.room(room)) {
        Some(room) => format!("# {}", room.name),
        None => "# unassigned".into(),
    }
}

/// Measured agent rows, shared by height calculation and visible-row drawing.
struct SidebarAgent<'a> {
    agent: &'a RoomAgent,
    in_master: bool,
    paths: Vec<String>,
    height: usize,
}

#[derive(Clone, Copy)]
struct SidebarViewport {
    sidebar: Rect,
    offset: usize,
    visible_end: usize,
}
impl SidebarViewport {
    fn at(self, x: u16, y: usize, width: u16) -> Rect {
        if (self.offset..self.visible_end).contains(&y) {
            Rect::new(x, (y - self.offset) as u16, width, 1)
        } else {
            Rect::default()
        }
    }
}

impl BusUi {
    pub fn cursor(&self) -> Option<crate::protocol::wire::CursorState> {
        self.view.cursor.clone()
    }

    pub fn compute_view(&mut self, cols: u16, rows: u16) {
        self.sync_toast();
        let key = super::super::ViewKey {
            room: self.room,
            terminal: self.terminal,
            form: self.form.as_ref().map(std::mem::discriminant),
            deletion: self.deletion.is_some(),
            alert: self.alert.is_some(),
        };
        if self
            .view_key
            .replace(key)
            .is_some_and(|previous| previous != key)
        {
            self.full_repaint = true;
        }
        let layout = layout(cols, rows);
        let sidebar = layout.sidebar;
        let main = layout.pane_surface;
        let mut view = View {
            sidebar,
            ..View::default()
        };
        self.sidebar_view(&mut view, rows);
        if let Some(form) = &self.form {
            self.form_view(&mut view, main, form);
        } else if self.terminal.is_none() {
            self.room_view(&mut view, main);
        } else {
            view.lines(
                Rect::new(main.x + 2, 2, main.width.saturating_sub(4), 3),
                "Opening agent… Room remains available in the sidebar.",
                None,
                true,
            );
            if let Some(error) = self.visible_error() {
                view.lines(
                    Rect::new(main.x + 2, 6, main.width.saturating_sub(4), 8),
                    error,
                    None,
                    false,
                );
            }
        }
        if self.deletion.is_some() {
            self.delete_view(&mut view, Rect::new(0, 0, cols, rows));
        }
        if self.alert.is_some() {
            self.alert_view(&mut view, Rect::new(0, 0, cols, rows));
        }
        // Kitty images draw above text, so hide them under anything that can
        // cover the history. A notice is not one: the room view shows it on
        // its own status line under the composer, and the other screens that
        // show it are covers already.
        if view.dialog.width > 0
            || self.form.is_some()
            || self.deletion.is_some()
            || self.terminal.is_some()
            || self.rename.is_some()
        {
            view.thumbnails.clear();
        }
        self.view = view;
    }
    fn sidebar_view(&mut self, view: &mut View, rows: u16) {
        let sidebar = view.sidebar;
        let sw = sidebar.width.saturating_sub(3);
        let rooms: Vec<_> = self.snapshot.state.rooms().collect();
        let agents = Self::sidebar_measure_agents(&self.snapshot.state, self.room, sw);
        let master_header_rows = if self.snapshot.state.master_room().is_some() {
            MASTER_SECTION_ROWS - 1
        } else {
            0
        };
        let content_height = 6
            + master_header_rows
            + rooms.len()
            + agents.iter().map(|entry| entry.height).sum::<usize>();
        let notices = if self.force_exit_available {
            2
        } else {
            u16::from(self.quitting.is_some())
        };
        // The settings divider and button stay pinned below any exit notices.
        view.sidebar_body = Rect::new(0, 0, sidebar.width, rows.saturating_sub(notices + 2));
        view.sidebar_max_scroll =
            content_height.saturating_sub(usize::from(view.sidebar_body.height));
        self.sidebar_scroll = self.sidebar_scroll.min(view.sidebar_max_scroll);
        let offset = self.sidebar_scroll;
        let visible_end = offset + usize::from(view.sidebar_body.height);
        let viewport = SidebarViewport {
            sidebar,
            offset,
            visible_end,
        };
        // A work room lists ROOMS, then its AGENTS; MASTER lists its
        // orchestrator AGENTS first, so it is clear whose agents they are.
        let master_open = self
            .snapshot
            .state
            .master_room()
            .is_some_and(|master| Some(master.id) == self.room);
        let rooms_header = sidebar_rooms_header(&self.snapshot.state, self.room);
        let work_rooms = rooms
            .iter()
            .filter(|room| room.kind == RoomKind::Work)
            .count();
        let (divider_row, agents_header) = if master_open {
            (rooms_header - 1, 1 + MASTER_SECTION_ROWS)
        } else {
            let divider = rooms_header + 2 + work_rooms;
            (divider, divider + 1)
        };
        self.sidebar_room_rows(view, viewport, &rooms, rooms_header);
        view.sidebar_divider = viewport.at(1, divider_row, sw);
        self.sidebar_agent_rows(view, viewport, agents, agents_header);
        self.sidebar_footer(view, rows);
    }

    fn sidebar_measure_agents(
        state: &BusState,
        room: Option<RoomId>,
        sw: u16,
    ) -> Vec<SidebarAgent<'_>> {
        // Measure logical sidebar rows once; only visible rows become labels
        // and hit targets. Expanded paths supply both height and display lines.
        state
            .agents()
            .filter(|a| Some(a.room_id) == room)
            .map(|agent| {
                // MASTER agents show name, provider and orchestrated room only.
                if state
                    .master_room()
                    .is_some_and(|master| master.id == agent.room_id)
                {
                    return SidebarAgent {
                        agent,
                        in_master: true,
                        paths: Vec::new(),
                        height: MASTER_AGENT_ROWS,
                    };
                }
                let paths = if agent.details_disclosed {
                    wrap(&agent.cwd.to_string_lossy(), sw)
                } else {
                    Vec::new()
                };
                let height = 3
                    + usize::from(agent.details_disclosed && agent.branch.is_some())
                    + paths.len();
                SidebarAgent {
                    agent,
                    in_master: false,
                    paths,
                    height,
                }
            })
            .collect()
    }

    fn sidebar_room_rows(
        &self,
        view: &mut View,
        viewport: SidebarViewport,
        rooms: &[&Room],
        rooms_header: usize,
    ) {
        let sw = viewport.sidebar.width.saturating_sub(3);
        view.row(viewport.at(1, rooms_header, sw), "ROOMS", None, false, true);
        view.row(
            viewport.at(viewport.sidebar.width.saturating_sub(3), rooms_header, 1),
            "+",
            Some(Action::NewRoom),
            false,
            false,
        );
        for room in rooms {
            let Some(row) = sidebar_room_row(&self.snapshot.state, room.id, self.room) else {
                continue;
            };
            // A work room's status sits left of its delete button, as on agent
            // rows; MASTER shows just its name.
            let right = match room.kind {
                RoomKind::Work => status(self.snapshot.state.room_status(room.id)),
                RoomKind::Master => "",
            };
            let name_width = if right.is_empty() {
                sw.saturating_sub(2)
            } else {
                sw.saturating_sub(right.len() as u16 + 3)
            };
            let rect = viewport.at(1, row, name_width);
            if rect.height == 0 {
                continue;
            }
            let label = room_label(room, usize::from(name_width));
            if let Some(rename) = self
                .rename
                .as_ref()
                .filter(|r| r.target == RenameTarget::Room(room.id))
            {
                view.editor(rect, &rename.editor, Some(Action::Room(room.id)), true);
            } else {
                view.row(
                    rect,
                    label,
                    Some(Action::Room(room.id)),
                    Some(room.id) == self.room && self.terminal.is_none(),
                    false,
                );
            }
            if !right.is_empty() {
                let status_rect = viewport.at(
                    viewport
                        .sidebar
                        .width
                        .saturating_sub(right.len() as u16 + 4),
                    row,
                    right.len() as u16,
                );
                view.row(status_rect, right, Some(Action::Room(room.id)), false, true);
                if let Some(colors) = animated_status_colors(right, self.status_animation_phase) {
                    view.color_last_row_characters(status_rect, colors);
                }
            }
            // MASTER is permanent, so it never offers a delete button.
            if Some(room.id) == self.room && room.kind == RoomKind::Work {
                view.row(
                    viewport.at(viewport.sidebar.width.saturating_sub(3), row, 1),
                    "×",
                    Some(Action::Delete(DeleteTarget::Room(room.id))),
                    false,
                    true,
                );
            }
        }
    }

    fn sidebar_agent_rows(
        &self,
        view: &mut View,
        viewport: SidebarViewport,
        agents: Vec<SidebarAgent<'_>>,
        agents_header: usize,
    ) {
        let sw = viewport.sidebar.width.saturating_sub(3);
        let mut y = agents_header;
        view.row(viewport.at(1, y, sw), "AGENTS", None, false, true);
        view.row(
            viewport.at(viewport.sidebar.width.saturating_sub(3), y, 1),
            "+",
            Some(Action::NewAgent),
            false,
            false,
        );
        y += 2;
        for SidebarAgent {
            agent,
            in_master,
            paths,
            height,
        } in agents
        {
            if y >= viewport.visible_end {
                break;
            }
            if y + height <= viewport.offset {
                y += height;
                continue;
            }
            self.sidebar_agent_heading(view, viewport, agent, y);
            y += 1;
            view.row(
                viewport.at(1, y, sw.saturating_sub(2)),
                provider(agent.provider),
                Some(Action::Agent(agent.id)),
                false,
                true,
            );
            if in_master {
                y += 1;
                // An orchestrator keeps its room for life, so the room line
                // opens that room instead of offering to reassign it.
                view.row(
                    viewport.at(1, y, sw),
                    orchestrated_room_label(&self.snapshot.state, agent),
                    agent.orchestrates.map(Action::Room),
                    false,
                    true,
                );
                y += 2;
                continue;
            }
            view.row(
                viewport.at(viewport.sidebar.width.saturating_sub(3), y, 1),
                if agent.details_disclosed { "v" } else { ">" },
                Some(Action::Details(agent.id)),
                false,
                true,
            );
            y += 1;
            if agent.details_disclosed {
                if let Some(branch) = &agent.branch {
                    view.row(viewport.at(1, y, sw), branch, None, false, true);
                    y += 1;
                }
                for line in paths {
                    view.row(viewport.at(1, y, sw), line, None, false, true);
                    y += 1;
                }
            }
            y += 1;
        }
    }

    fn sidebar_agent_heading(
        &self,
        view: &mut View,
        viewport: SidebarViewport,
        agent: &RoomAgent,
        y: usize,
    ) {
        let sw = viewport.sidebar.width.saturating_sub(3);
        let right = agent_status(agent);
        let name_width = sw.saturating_sub(right.len() as u16 + 3);
        let rect = viewport.at(1, y, name_width);
        if let Some(rename) = self
            .rename
            .as_ref()
            .filter(|r| r.target == RenameTarget::Agent(agent.id))
        {
            view.editor(rect, &rename.editor, Some(Action::Agent(agent.id)), true);
        } else {
            view.row(
                rect,
                &agent.name,
                Some(Action::Agent(agent.id)),
                self.terminal == Some(agent.id),
                false,
            );
            view.color_last_row(rect, identity_color(agent, &self.settings));
        }
        let status_rect = viewport.at(
            viewport
                .sidebar
                .width
                .saturating_sub(right.len() as u16 + 4),
            y,
            right.len() as u16,
        );
        view.row(
            status_rect,
            right,
            Some(Action::Agent(agent.id)),
            false,
            true,
        );
        if let Some(colors) = animated_status_colors(right, self.status_animation_phase) {
            view.color_last_row_characters(status_rect, colors);
        }
        view.row(
            viewport.at(viewport.sidebar.width.saturating_sub(3), y, 1),
            "×",
            Some(Action::Delete(DeleteTarget::Agent(agent.id))),
            false,
            true,
        );
    }

    fn sidebar_footer(&self, view: &mut View, rows: u16) {
        let sw = view.sidebar.width.saturating_sub(3);
        view.row(
            Rect::new(1, rows.saturating_sub(4), sw, 1),
            if self.force_exit_available {
                "Unsaved: Ctrl+Shift+Q exits"
            } else {
                ""
            },
            None,
            false,
            true,
        );
        view.row(
            Rect::new(1, rows.saturating_sub(3), sw, 1),
            if self.quitting.is_some() {
                "Saving before exit…"
            } else if self.force_exit_available {
                "Ctrl+Q retries saving"
            } else {
                ""
            },
            None,
            false,
            true,
        );
        view.settings_divider = Rect::new(1, rows.saturating_sub(2), sw, 1);
        let settings_width = ("Settings".len() as u16).min(sw);
        view.row(
            Rect::new(1, rows.saturating_sub(1), settings_width, 1),
            "Settings",
            Some(Action::Settings),
            matches!(self.form, Some(Form::Settings)),
            false,
        );
    }
}
pub(super) fn status_name(value: RuntimeStatus) -> &'static str {
    status(value)
}
