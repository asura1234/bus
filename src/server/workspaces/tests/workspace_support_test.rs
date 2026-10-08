impl Workspace {
    pub(crate) fn test_new(name: &str) -> Self {
        let (events, _) = mpsc::channel(64);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(RenderSignal::new());
        let identity_cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
        let (layout, root_id) = TileLayout::new();
        let terminal_id = TerminalId::alloc();
        let mut panes = HashMap::new();
        panes.insert(root_id, PaneState::new(terminal_id));
        let tab = Tab {
            custom_name: None,
            number: 1,
            root_pane: root_id,
            layout,
            panes,
            runtimes: HashMap::new(),
            zoomed: false,
            events,
            render_notify,
            render_dirty,
        };
        let mut public_pane_numbers = HashMap::new();
        public_pane_numbers.insert(tab.root_pane, 1);
        Self {
            id: generate_workspace_id(),
            custom_name: Some(name.to_string()),
            identity_cwd: identity_cwd.clone(),
            cached_identity_cwd: identity_cwd.clone(),
            cached_auto_label: fallback_label_from_cwd(&identity_cwd),
            public_pane_numbers,
            next_public_pane_number: 2,
            next_public_tab_number: 2,
            tabs: vec![tab],
            active_tab: 0,
            test_runtimes: HashMap::new(),
        }
    }

    pub(crate) fn insert_test_runtime(&mut self, pane_id: PaneId, runtime: TerminalRuntime) {
        self.test_runtimes.insert(pane_id, runtime);
    }

    pub(crate) fn test_split(&mut self, direction: Direction) -> PaneId {
        let tab = self.active_tab_mut().expect("workspace must have tab");
        let new_id = tab.layout.split_focused(direction);
        tab.panes
            .insert(new_id, PaneState::new(TerminalId::alloc()));
        self.register_new_pane(new_id);
        new_id
    }

    pub(crate) fn test_add_tab(&mut self, name: Option<&str>) -> usize {
        let (events, _) = mpsc::channel(64);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(RenderSignal::new());
        let (layout, root_id) = TileLayout::new();
        let mut panes = HashMap::new();
        panes.insert(root_id, PaneState::new(TerminalId::alloc()));
        let tab = Tab {
            custom_name: name.map(str::to_string),
            number: self.next_public_tab_number,
            root_pane: root_id,
            layout,
            panes,
            runtimes: HashMap::new(),
            zoomed: false,
            events,
            render_notify,
            render_dirty,
        };
        self.next_public_tab_number += 1;
        self.register_new_pane(root_id);
        self.tabs.push(tab);
        self.tabs.len() - 1
    }

    pub(crate) fn test_adversarial_identity_state() -> Self {
        let mut ws = Self::test_new("adversarial-identity");
        let removed_pane = ws.test_split(Direction::Horizontal);
        ws.test_split(Direction::Vertical);
        assert!(!ws.close_pane(removed_pane));
        let _unused_raw_id = PaneId::alloc();
        let later_pane = ws.test_split(Direction::Horizontal);

        let removed_tab = ws.test_add_tab(Some("removed"));
        let survivor_tab = ws.test_add_tab(None);
        let final_tab = ws.test_add_tab(None);
        let survivor_root = ws.tabs[survivor_tab].root_pane;
        let final_root = ws.tabs[final_tab].root_pane;
        assert!(ws.close_tab(removed_tab));
        ws.tabs.rotate_left(1);
        ws.switch_tab(
            ws.find_tab_index_for_pane(survivor_root)
                .expect("survivor tab should still exist"),
        );

        assert_ne!(
            ws.active_tab + 1,
            ws.tabs[ws.active_tab].number,
            "adversarial active tab must distinguish position from public tab number"
        );
        assert_ne!(
            later_pane.raw() as usize,
            ws.public_pane_number(later_pane).unwrap(),
            "adversarial pane must distinguish raw pane id from public pane number"
        );
        assert_eq!(ws.find_tab_index_for_pane(final_root), Some(1));
        ws
    }

    pub(crate) fn assert_invariants_for_test(&self) {
        assert!(
            !self.tabs.is_empty(),
            "workspace {} must contain at least one tab",
            self.id
        );
        assert!(
            self.active_tab < self.tabs.len(),
            "workspace {} active_tab {} out of bounds for {} tabs",
            self.id,
            self.active_tab,
            self.tabs.len()
        );

        let mut tab_numbers = std::collections::HashSet::new();
        let mut max_tab_number = 0usize;
        let mut live_panes = std::collections::HashSet::new();
        let mut terminal_ids = std::collections::HashSet::new();

        for (tab_idx, tab) in self.tabs.iter().enumerate() {
            assert!(
                tab.number > 0,
                "workspace {} tab {} has invalid public tab number 0",
                self.id,
                tab_idx
            );
            assert!(
                tab_numbers.insert(tab.number),
                "workspace {} has duplicate public tab number {}",
                self.id,
                tab.number
            );
            max_tab_number = max_tab_number.max(tab.number);
            assert!(
                tab.panes.contains_key(&tab.root_pane),
                "workspace {} tab {} root pane {:?} is missing from tab panes",
                self.id,
                tab_idx,
                tab.root_pane
            );

            let layout_panes = tab.layout.pane_ids();
            let layout_set: std::collections::HashSet<_> = layout_panes.iter().copied().collect();
            assert_eq!(
                layout_panes.len(),
                layout_set.len(),
                "workspace {} tab {} layout contains duplicate pane ids",
                self.id,
                tab_idx
            );
            assert!(
                layout_set.contains(&tab.layout.focused()),
                "workspace {} tab {} focused pane {:?} is not in layout",
                self.id,
                tab_idx,
                tab.layout.focused()
            );
            let pane_set: std::collections::HashSet<_> = tab.panes.keys().copied().collect();
            assert_eq!(
                layout_set, pane_set,
                "workspace {} tab {} layout panes must exactly match pane states",
                self.id, tab_idx
            );

            for (pane_id, pane) in &tab.panes {
                assert!(
                    live_panes.insert(*pane_id),
                    "workspace {} pane {:?} appears in more than one tab",
                    self.id,
                    pane_id
                );
                assert!(
                    self.public_pane_numbers.contains_key(pane_id),
                    "workspace {} live pane {:?} has no public pane number",
                    self.id,
                    pane_id
                );
                assert!(
                    terminal_ids.insert(pane.attached_terminal_id.clone()),
                    "workspace {} terminal {} is attached to multiple panes",
                    self.id,
                    pane.attached_terminal_id
                );
            }
        }

        assert!(
            self.next_public_tab_number > 0,
            "workspace {} next_public_tab_number must be greater than 0",
            self.id
        );
        assert!(
            self.next_public_tab_number > max_tab_number,
            "workspace {} next_public_tab_number {} must be greater than max live public tab number {}",
            self.id,
            self.next_public_tab_number,
            max_tab_number
        );

        let public_pane_keys: std::collections::HashSet<_> =
            self.public_pane_numbers.keys().copied().collect();
        assert_eq!(
            public_pane_keys, live_panes,
            "workspace {} public pane map must exactly match live panes",
            self.id
        );

        let mut pane_numbers = std::collections::HashSet::new();
        let mut max_pane_number = 0usize;
        for (pane_id, pane_number) in &self.public_pane_numbers {
            assert!(
                *pane_number > 0,
                "workspace {} pane {:?} has invalid public pane number 0",
                self.id,
                pane_id
            );
            assert!(
                pane_numbers.insert(*pane_number),
                "workspace {} duplicate public pane number {} for pane {:?}",
                self.id,
                pane_number,
                pane_id
            );
            max_pane_number = max_pane_number.max(*pane_number);
        }
        assert!(
            self.next_public_pane_number > 0,
            "workspace {} next_public_pane_number must be greater than 0",
            self.id
        );
        assert!(
            self.next_public_pane_number > max_pane_number,
            "workspace {} next_public_pane_number {} must be greater than max live public pane number {}",
            self.id,
            self.next_public_pane_number,
            max_pane_number
        );
    }
}
