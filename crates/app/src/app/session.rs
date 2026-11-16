//! The GUI's part of a session (side bar shown, width, view, expanded folders): pushed to
//! Neovim whenever it changes, which saves it with the session (`session.lua`), and applied
//! when Neovim loads a session (`gui_session`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use nvim_bridge::gui::GuiSession;

use super::*;
use crate::side_bar::SideViews;

/// How long the state has to stay the same before it is sent: dragging the side bar's
/// edge or opening folders one after another is one message.
const SETTLE: Duration = Duration::from_millis(500);

/// What was last sent, and what waits to be.
#[derive(Default)]
pub(super) struct SessionSync {
    sent: Option<GuiSession>,
    pending: Option<(GuiSession, Instant)>,
}

impl SessionSync {
    /// `now` is the state: `Ok(Some(state))` when it is to be sent now, `Ok(None)` when
    /// there is nothing to send, `Err(wait)` when it changed and has to settle first.
    pub fn poll(&mut self, now: GuiSession, at: Instant) -> Result<Option<GuiSession>, Duration> {
        if self.sent.as_ref() == Some(&now) {
            self.pending = None;
            return Ok(None);
        }
        let since = match &self.pending {
            Some((p, since)) if *p == now => *since,
            _ => {
                self.pending = Some((now.clone(), at));
                at
            }
        };
        match SETTLE.checked_sub(at.saturating_duration_since(since)) {
            Some(wait) if !wait.is_zero() => Err(wait),
            _ => {
                self.pending = None;
                self.sent = Some(now.clone());
                Ok(Some(now))
            }
        }
    }

    /// Neovim may have lost what was sent (a session was loaded): send it again.
    pub fn forget(&mut self) {
        self.sent = None;
        self.pending = None;
    }
}

/// How the side bar's view is named in a session.
fn view_name(view: SideView, views: &SideViews) -> String {
    match view {
        SideView::Explorer => "explorer".into(),
        SideView::Git => "git".into(),
        SideView::Tree(id) => views.by_id(id).map(|i| i.view.clone()).unwrap_or_else(|| "explorer".into()),
    }
}

/// The side bar view a session names; `None` for a tree view that isn't registered (yet).
fn view_of(name: &str, views: &SideViews) -> Option<SideView> {
    match name {
        "" | "explorer" => Some(SideView::Explorer),
        "git" => Some(SideView::Git),
        other => views.by_view(other).map(|i| SideView::Tree(i.id)),
    }
}

impl VimApp {
    /// The side bar as it is now.
    fn gui_session(&self) -> GuiSession {
        GuiSession {
            visible: self.explorer.visible,
            width: self.side_width.round(),
            view: view_name(self.side, &self.side_views),
            expanded: self.explorer.expanded_dirs().iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            panel: self.panel_session(),
            agent: self.agent_session(),
        }
    }

    /// Send the side bar's state to Neovim once it has settled.
    pub(super) fn sync_session(&mut self, ctx: &egui::Context) {
        if !self.explorer_enabled {
            return;
        }
        match self.session_sync.poll(self.gui_session(), Instant::now()) {
            Ok(Some(state)) => {
                self.bridge.send(UiCommand::Lua { code: "return Eshvi.session.set_gui(...)".into(), args: vec![state.to_value()] });
            }
            Ok(None) => {}
            Err(wait) => ctx.request_repaint_after(wait),
        }
    }

    /// A session was loaded: show the side bar as it was saved.
    pub(super) fn apply_session(&mut self, ctx: &egui::Context, s: GuiSession) {
        if !self.explorer_enabled {
            return;
        }
        self.explorer.visible = s.visible;
        if s.width >= 140.0 {
            let width = s.width.clamp(140.0, 800.0);
            self.side_width = width;
            self.force_side_width = Some(width);
        }
        if let Some(p) = &s.panel {
            self.apply_panel_session(p);
        }
        if let Some(a) = &s.agent {
            self.apply_agent_session(ctx, a);
        }
        // A tree view that registers later (the Hierarchy, once a server attaches) is
        // switched to then.
        self.session_view = None;
        match view_of(&s.view, &self.side_views) {
            Some(view) => self.side = view,
            None => {
                self.side = SideView::Explorer;
                self.session_view = Some(s.view.clone());
            }
        }
        let dirs: Vec<PathBuf> = s.expanded.iter().map(PathBuf::from).collect();
        self.explorer.set_expanded_dirs(&dirs);
        if let Some(active) = self.active_file.clone() {
            self.explorer.reveal(&active);
        }
        // Neovim now has this state; if it was sent to it meanwhile, send it again.
        self.session_sync.forget();
        ctx.request_repaint();
    }

    /// A tree view registered: the session's side bar view, if it is this one.
    pub(super) fn session_view_registered(&mut self, view: &str) {
        if self.session_view.as_deref() == Some(view) {
            if let Some(info) = self.side_views.by_view(view) {
                self.side = SideView::Tree(info.id);
                self.session_view = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(width: f32) -> GuiSession {
        GuiSession { visible: true, width, view: "explorer".into(), expanded: vec!["/p/src".into()], panel: None, agent: None }
    }

    #[test]
    fn the_state_is_sent_once_it_has_settled_and_only_when_it_changed() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut sync = SessionSync::default();
        // The first state waits, then goes.
        assert_eq!(sync.poll(state(260.0), at(0)), Err(SETTLE));
        assert!(sync.poll(state(260.0), at(300)).is_err());
        assert_eq!(sync.poll(state(260.0), at(500)), Ok(Some(state(260.0))));
        // Unchanged: nothing more.
        assert_eq!(sync.poll(state(260.0), at(900)), Ok(None));
        // A drag changes it every frame: it waits for the last one.
        assert!(sync.poll(state(270.0), at(1000)).is_err());
        assert!(sync.poll(state(280.0), at(1300)).is_err());
        assert!(sync.poll(state(290.0), at(1600)).is_err());
        assert_eq!(sync.poll(state(290.0), at(2100)), Ok(Some(state(290.0))));
        // Back to what was sent before it settled: nothing to send.
        assert!(sync.poll(state(300.0), at(2200)).is_err());
        assert_eq!(sync.poll(state(290.0), at(2300)), Ok(None));
        // Loading a session makes it be sent again.
        sync.forget();
        assert!(sync.poll(state(290.0), at(3000)).is_err());
        assert_eq!(sync.poll(state(290.0), at(3500)), Ok(Some(state(290.0))));
    }

    #[test]
    fn views_are_named_and_found_again_by_name() {
        let mut views = SideViews::default();
        assert_eq!(view_of("hierarchy", &views), None);
        let id = views.register(
            "hierarchy",
            nvim_bridge::gui::TreeSide { title: "HIERARCHY".into(), icon: "type-hierarchy".into(), ..Default::default() },
        );
        assert_eq!(view_name(SideView::Tree(id), &views), "hierarchy");
        assert_eq!(view_of("hierarchy", &views), Some(SideView::Tree(id)));
        assert_eq!(view_name(SideView::Git, &views), "git");
        assert_eq!(view_of("git", &views), Some(SideView::Git));
        assert_eq!(view_of("explorer", &views), Some(SideView::Explorer));
        // A view that went away is the Explorer.
        assert_eq!(view_name(SideView::Tree(id + 1), &views), "explorer");
    }
}
