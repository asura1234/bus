use agile_board::{
    mock,
    model::{
        render_sanitized_markdown, BoardState, Column, NewTicketInput, UpdateTicketInput,
        ViewFilter, ALL_COLUMNS,
    },
    server, store,
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

fn bin() -> String {
    env!("CARGO_BIN_EXE_agile-board").to_string()
}

fn headless_chrome() -> Option<PathBuf> {
    let mac = PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    if mac.is_file() {
        return Some(mac);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .flat_map(|dir| ["chromium", "google-chrome"].map(|name| dir.join(name)))
        .find(|path| path.is_file())
}

#[test]
fn transition_matrix_enforces_normal_investigation_only_and_blocked_escape_hatch() {
    // 1. Exhaustive 7x7 transition matrix check for both investigation_only=false and true
    for &inv_only in &[false, true] {
        for &from in &ALL_COLUMNS {
            for &to in &ALL_COLUMNS {
                let res = from.validate_transition(to, inv_only, "T-99");
                if from == to {
                    let err = res.unwrap_err();
                    assert_eq!(err.0, 3);
                    assert!(err.1.contains("already in"));
                    continue;
                }
                let expected_ok = match (from, to) {
                    (_, Column::Blocked) => true,
                    (Column::Blocked, _) => true,
                    (Column::Todo, Column::Investigation) => true,
                    (Column::Investigation, Column::Implementation) => true,
                    (Column::Investigation, Column::Done) => inv_only,
                    (Column::Implementation, Column::Testing) => true,
                    (Column::Testing, Column::Review)
                    | (Column::Testing, Column::Implementation) => true,
                    (Column::Review, Column::Done) | (Column::Review, Column::Implementation) => {
                        true
                    }
                    _ => false,
                };
                assert_eq!(
                    res.is_ok(),
                    expected_ok,
                    "matrix mismatch for inv_only={inv_only}: {from} -> {to}"
                );
                if let Err(err) = res {
                    assert_eq!(err.0, 3);
                    assert!(err.1.contains("allowed targets from"));
                    if from == Column::Investigation && to == Column::Done && !inv_only {
                        assert!(err.1.contains("--investigation-only"));
                    }
                }
            }
        }
    }

    // 2. End-to-end stateful normal chain + rework loops + universal blocked escape hatch
    let mut state = BoardState::default();
    let now = 1_795_400_000_000_i64;
    state.add_room(1, "backend-api".into(), now).unwrap();

    // Non-investigation-only cannot be created directly in done
    assert!(state
        .add_ticket(
            NewTicketInput {
                room: "backend-api".into(),
                title: "Direct done".into(),
                description: None,
                column: Column::Done,
                mr_url: None,
                investigation_only: false,
                note: None,
            },
            now
        )
        .is_err());

    let t1 = state
        .add_ticket(
            NewTicketInput {
                room: "backend-api".into(),
                title: "Normal MR work".into(),
                description: Some("See [Spec](https://example.com/spec).".into()),
                column: Column::Todo,
                mr_url: None,
                investigation_only: false,
                note: None,
            },
            now,
        )
        .unwrap();

    // Normal chain: todo -> investigation -> implementation -> testing -> implementation (rework)
    // -> testing -> review -> implementation (rework) -> testing -> review -> done
    let chain = [
        Column::Investigation,
        Column::Implementation,
        Column::Testing,
        Column::Implementation,
        Column::Testing,
        Column::Review,
        Column::Implementation,
        Column::Testing,
        Column::Review,
        Column::Done,
    ];
    for (idx, col) in chain.into_iter().enumerate() {
        let moved = state
            .move_ticket(
                &t1.id,
                col,
                Some(format!("step {idx}")),
                now + ((idx as i64 + 1) * 10),
            )
            .unwrap();
        assert_eq!(moved.column, col);
    }

    // Universal blocked escape hatch: any state -> blocked -> any state
    for target in ALL_COLUMNS {
        if target == Column::Blocked {
            continue;
        }
        state
            .move_ticket(
                &t1.id,
                Column::Blocked,
                Some("Need decision".into()),
                now + 500,
            )
            .unwrap();
        let moved = state
            .move_ticket(&t1.id, target, Some("Unblocked".into()), now + 510)
            .unwrap();
        assert_eq!(moved.column, target);
    }

    // 3. Investigation-only ticket can jump investigation -> done directly
    let t2 = state
        .add_ticket(
            NewTicketInput {
                room: "1".into(),
                title: "Root cause spike".into(),
                description: None,
                column: Column::Investigation,
                mr_url: None,
                investigation_only: true,
                note: Some("Investigating flake".into()),
            },
            now + 600,
        )
        .unwrap();
    let done = state
        .move_ticket(
            &t2.id,
            Column::Done,
            Some("No code change needed".into()),
            now + 700,
        )
        .unwrap();
    assert_eq!(done.column, Column::Done);
}

#[test]
fn markdown_description_sanitizes_raw_html_and_renders_external_links() {
    let raw = "Fixes [Feishu buglist #4821](https://example.feishu.cn/wiki/wikcnBugList4821) and https://example.com/docs/spec <script>alert(1)</script> [bad](javascript:alert(2))";
    let html = render_sanitized_markdown(raw);
    assert!(html.contains(
        r#"<a href="https://example.feishu.cn/wiki/wikcnBugList4821" target="_blank" rel="noopener noreferrer">Feishu buglist #4821</a>"#
    ));
    assert!(html.contains(
        r#"<a href="https://example.com/docs/spec" target="_blank" rel="noopener noreferrer">https://example.com/docs/spec</a>"#
    ));
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!html.contains("javascript:"));
}

#[test]
fn persistence_round_trip_and_history_clear_preserve_rooms() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let now = 1_795_400_000_000_i64;

    store::mutate(&data_dir, |s| {
        s.add_room(7, "storage-engine".into(), now)?;
        let t = s.add_ticket(
            NewTicketInput {
                room: "storage-engine".into(),
                title: "Fix race".into(),
                description: Some("Initial [doc](https://example.com/doc)".into()),
                column: Column::Investigation,
                mr_url: None,
                investigation_only: false,
                note: Some("Initial note".into()),
            },
            now,
        )?;
        s.update_ticket(
            UpdateTicketInput {
                id: t.id,
                title: Some("Fix race in listener".into()),
                description: Some(
                    "Updated [Feishu bug](https://example.feishu.cn/wiki/123)".into(),
                ),
                mr_url: Some("https://github.com/example/platform/pull/555".into()),
                note: Some("Opened PR".into()),
            },
            now + 1000,
        )?;
        Ok(())
    })
    .unwrap();

    let loaded = store::load(&data_dir).unwrap();
    assert_eq!(loaded.rooms.len(), 1);
    assert_eq!(loaded.tickets.len(), 1);
    assert_eq!(loaded.tickets[0].title, "Fix race in listener");
    assert!(loaded.tickets[0]
        .description_html
        .contains("https://example.feishu.cn/wiki/123"));
    assert_eq!(
        loaded.tickets[0].mr_url.as_deref(),
        Some("https://github.com/example/platform/pull/555")
    );
    assert_eq!(loaded.session_history.len(), 3);

    // Clear session history
    let ((cleared_t, cleared_h), after) =
        store::mutate(&data_dir, |s| Ok(s.clear_session_history())).unwrap();
    assert_eq!(cleared_t, 1);
    assert_eq!(cleared_h, 3);
    assert_eq!(after.rooms.len(), 1);
    assert!(after.tickets.is_empty());
    assert!(after.session_history.is_empty());
}

#[test]
fn view_filters_day_week_and_session_match_documented_windows() {
    let mut state = BoardState::default();
    let now = 1_795_440_000_000_i64;
    mock::seed_mock_state(&mut state, now).unwrap();

    let day_count = state
        .tickets
        .iter()
        .filter(|t| ViewFilter::Day.matches(t, now))
        .count();
    let week_count = state
        .tickets
        .iter()
        .filter(|t| ViewFilter::Week.matches(t, now))
        .count();
    let session_count = state
        .tickets
        .iter()
        .filter(|t| ViewFilter::Session.matches(t, now))
        .count();

    assert!(day_count > 0);
    assert!(week_count > day_count);
    assert!(session_count > week_count);
    assert_eq!(session_count, state.tickets.len());
}

#[test]
fn cli_and_http_page_contract_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_str().unwrap().to_string();

    // Relative path is rejected with exit 2
    let rel = Command::new(bin())
        .args(["--data-dir", "relative/path", "room", "list"])
        .output()
        .unwrap();
    assert_eq!(rel.status.code(), Some(2));

    // Register section & ticket via CLI
    let add_room = Command::new(bin())
        .args([
            "--data-dir",
            &data_dir,
            "room",
            "add",
            "--id",
            "1",
            "--name",
            "backend-api",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(add_room.status.success());

    let add_ticket = Command::new(bin())
        .args([
            "--data-dir",
            &data_dir,
            "ticket",
            "add",
            "--room",
            "backend-api",
            "--title",
            "Test CLI ticket",
            "--description",
            "Tracks [Feishu item](https://example.feishu.cn/wiki/1) and [Product Doc](https://example.com/doc).",
            "--mr",
            "https://github.com/example/platform/pull/42",
            "--column",
            "investigation",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(add_ticket.status.success());

    // Disallowed move investigation -> done (without --investigation-only) returns exit 3
    let bad_move = Command::new(bin())
        .args([
            "--data-dir",
            &data_dir,
            "ticket",
            "move",
            "T-1",
            "--column",
            "done",
        ])
        .output()
        .unwrap();
    assert_eq!(bad_move.status.code(), Some(3));

    // Clear without --confirm returns exit 2
    let no_confirm = Command::new(bin())
        .args(["--data-dir", &data_dir, "history", "clear"])
        .output()
        .unwrap();
    assert_eq!(no_confirm.status.code(), Some(2));

    // Verify page HTML contract: Agile Board title, viewport scrolling (`html { overflow: auto }` and
    // `body { overflow: visible; min-height: 100vh }`), universal 7-column template,
    // global sticky header band + sticky left section sidebar (frozen row + frozen column),
    // near-square card min-height, plain solid 1px card border (no per-column card border accent),
    // double confirmation popups for Clear session history, no modal, no hidden/rendered ticket IDs,
    // and only the 2 topbar controls (view selector + clear history button)
    assert!(server::PAGE_HTML.contains("<title>Agile Board</title>"));
    assert!(server::PAGE_HTML.contains("overflow: auto;"));
    assert!(server::PAGE_HTML.contains("overflow: visible;"));
    assert!(!server::PAGE_HTML.contains("html, body"));
    assert!(server::PAGE_HTML.contains("--sidebar-width: 148px;"));
    assert!(server::PAGE_HTML.contains("--bg-sidebar: #141d2f;"));
    assert!(server::PAGE_HTML.contains("--col-width: 160px;"));
    assert!(server::PAGE_HTML.contains("container-type: inline-size;"));
    assert!(server::PAGE_HTML.contains("min-height: 86cqi;"));
    assert!(server::PAGE_HTML.contains(".kanban-column[data-col=\"blocked\"]"));
    assert!(server::PAGE_HTML.contains("rgba(244, 63, 94, 0.09)"));
    assert!(!server::PAGE_HTML.contains("aspect-ratio:"));
    assert!(server::PAGE_HTML.contains("position: sticky;"));
    assert!(server::PAGE_HTML.contains("left: 0;"));
    assert!(server::PAGE_HTML.contains("room-sidebar-cell"));
    assert!(server::PAGE_HTML.contains(
        "grid-template-columns: var(--sidebar-width) repeat(7, minmax(var(--col-width), 1fr));"
    ));
    assert!(server::PAGE_HTML.contains("border: 1px solid var(--border-strong);"));
    assert!(!server::PAGE_HTML.contains(".ticket-card[data-col="));
    assert!(!server::PAGE_HTML.contains("border-left-color"));
    assert!(server::PAGE_HTML.contains("id=\"clear-confirm-step1\""));
    assert!(server::PAGE_HTML.contains("id=\"clear-confirm-step2\""));
    assert!(server::PAGE_HTML.contains("id=\"confirm-step1-btn\""));
    assert!(server::PAGE_HTML.contains("id=\"confirm-step2-btn\""));
    assert!(!server::PAGE_HTML.contains("Bus"));
    assert!(!server::PAGE_HTML.contains("openTicketModal"));
    assert!(!server::PAGE_HTML.contains("modal-backdrop"));
    assert!(!server::PAGE_HTML.contains("t.id"));
    assert!(!server::PAGE_HTML.contains("data-ticket-id"));
    assert!(!server::PAGE_HTML.contains("toggle-history-btn"));
    assert!(!server::PAGE_HTML.contains("history-panel"));
    for col in ALL_COLUMNS {
        assert!(server::PAGE_HTML.contains(col.as_str()));
    }
    assert!(server::PAGE_HTML.contains("id=\"view-select\""));
    assert!(server::PAGE_HTML.contains("id=\"clear-history-btn\""));
    assert!(server::PAGE_HTML.contains("Clear session history"));

    // Serve on ephemeral port in thread and test HTTP endpoints (GET /, GET /api/board, POST /api/history/clear)
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();
    let dir_path = tmp.path().to_path_buf();

    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let handle = std::thread::spawn(move || {
        server::serve(&dir_path, port, Some(5), stop_clone).unwrap();
    });

    std::thread::sleep(Duration::from_millis(80));

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut root_resp = String::new();
    stream.read_to_string(&mut root_resp).unwrap();
    assert!(root_resp.starts_with("HTTP/1.1 200 OK"));
    assert!(root_resp.contains("Cache-Control: no-store"));

    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .write_all(b"GET /api/board?view=session HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200 OK"));
    assert!(resp.contains("\"Test CLI ticket\""));
    assert!(resp.contains("https://example.feishu.cn/wiki/1"));
    assert!(resp.contains("https://github.com/example/platform/pull/42"));

    // Clear history over HTTP without confirm -> 400 Bad Request
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let bad_body = b"{}";
    stream
        .write_all(
            format!(
                "POST /api/history/clear HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n",
                bad_body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    stream.write_all(bad_body).unwrap();
    let mut clear_bad_resp = String::new();
    stream.read_to_string(&mut clear_bad_resp).unwrap();
    assert!(clear_bad_resp.starts_with("HTTP/1.1 400 Bad Request"));

    // Clear history over HTTP with confirm:true -> 200 OK
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let ok_body = br#"{"confirm":true}"#;
    stream
        .write_all(
            format!(
                "POST /api/history/clear HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n",
                ok_body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    stream.write_all(ok_body).unwrap();
    let mut clear_ok_resp = String::new();
    stream.read_to_string(&mut clear_ok_resp).unwrap();
    assert!(clear_ok_resp.starts_with("HTTP/1.1 200 OK"));
    assert!(clear_ok_resp.contains("\"cleared_tickets\":1"));

    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();

    // If headless Chrome/Chromium is available, verify in a real browser engine that scrolling
    // window to x=600, y=1200 keeps `.sticky-header-band` top ~ 0 and `.room-sidebar-cell` left ~ 0
    if let Some(chrome) = headless_chrome() {
        let mut state = BoardState::default();
        mock::seed_mock_state(&mut state, 1_795_440_000_000).unwrap();
        let payload = serde_json::json!({
            "rooms": state.rooms,
            "tickets": state.tickets,
        });
        let probe_script = format!(
            r#"<script>
const INLINE_DATA = {payload};
window.addEventListener("DOMContentLoaded", () => {{
  renderAll(INLINE_DATA);
  document.body.style.minHeight = "3000px";
  window.scrollTo(600, 1200);
  const band = document.querySelector(".sticky-header-band").getBoundingClientRect();
  const side = document.querySelector(".room-sidebar-cell").getBoundingClientRect();
  const sideCells = Array.from(document.querySelectorAll(".room-sidebar-cell"));
  const sidebarContinuous = sideCells.length >= 2 && sideCells.every((cell, idx) => {{
    if (idx === 0) return true;
    const prev = sideCells[idx - 1].getBoundingClientRect();
    const cur = cell.getBoundingClientRect();
    return Math.abs(cur.top - prev.bottom) <= 2;
  }});
  const cards = Array.from(document.querySelectorAll(".ticket-card"));
  const allCardsFit = cards.length > 0 && cards.every(c => {{
    const fitHeight = c.scrollHeight <= c.clientHeight + 1;
    const mr = c.querySelector(".mr-link");
    if (!mr) return fitHeight;
    const cr = c.getBoundingClientRect();
    const mrRect = mr.getBoundingClientRect();
    const mrInside = mrRect.top >= cr.top && mrRect.bottom <= cr.bottom && mrRect.left >= cr.left && mrRect.right <= cr.right;
    return fitHeight && mrInside;
  }});
  const out = document.createElement("pre");
  out.id = "sticky-probe-result";
  out.textContent = JSON.stringify({{
    scrollX: Math.round(window.scrollX),
    scrollY: Math.round(window.scrollY),
    bandTop: Math.round(band.top),
    sideLeft: Math.round(side.left),
    sidebarContinuous,
    allCardsFit
  }});
  document.body.appendChild(out);
}});
</script></body>"#
        );
        let probe_html = server::PAGE_HTML.replace("</body>", &probe_script);
        let probe_path = tmp.path().join("sticky-probe.html");
        std::fs::write(&probe_path, probe_html).unwrap();

        for win_size in ["600,500", "1728,1117"] {
            let out = Command::new(&chrome)
                .args([
                    "--headless=new",
                    "--disable-gpu",
                    &format!("--window-size={win_size}"),
                    "--dump-dom",
                    &format!("file://{}", probe_path.display()),
                ])
                .output()
                .unwrap();
            if out.status.success() {
                let dom = String::from_utf8_lossy(&out.stdout);
                assert!(
                    dom.contains(r#""scrollY":1200,"bandTop":0,"sideLeft":0,"sidebarContinuous":true,"allCardsFit":true"#),
                    "unexpected probe DOM output at {win_size}: {dom}"
                );
            }
        }
    }
}

#[test]
fn headless_page_preserves_scroll_across_polls() {
    // Stub fetch and advance the page's scheduled poll explicitly so browser timing cannot race the assertions.
    assert_headless_board_probe(
        r#"<script>
let rooms = [{ id: 1, name: "Test room" }];
let tickets = Array.from({ length: 12 }, (_, i) => ({
  room_id: 1,
  column: "todo",
  title: `Ticket ${i}`
}));
let nowMs = 1000;
let poll;
let pollDelay;
window.fetch = async url => ({
  ok: url === "/api/board?view=session",
  json: async () => ({ rooms, tickets, now_ms: nowMs })
});
window.setInterval = (callback, delay) => {
  poll = callback;
  pollDelay = delay;
};

function check(condition, message) {
  if (!condition) throw new Error(message);
}

window.addEventListener("load", async () => {
  const result = document.createElement("pre");
  result.id = "board-probe-result";
  result.hidden = true;
  document.body.appendChild(result);
  try {
    const board = document.getElementById("board-container");
    const section = board.firstElementChild;
    check(section?.querySelector(".ticket-title")?.textContent === "Ticket 0",
      "initial board did not render");
    check(pollDelay === 3000, "expected a poll every 3000 ms");
    window.scrollTo(275, 550);
    const checkScroll = (x, y, phase) => check(
      window.scrollX === x && window.scrollY === y,
      `${phase}: expected scroll (${x}, ${y}), got (${window.scrollX}, ${window.scrollY})`
    );
    checkScroll(275, 550, "initial scroll");

    await poll();
    checkScroll(275, 550, "identical poll");
    check(board.firstElementChild === section, "identical poll rebuilt the board");

    nowMs += 3000;
    await poll();
    checkScroll(275, 550, "timestamp-only poll");
    check(board.firstElementChild === section, "timestamp-only poll rebuilt the board");

    tickets = [{ ...tickets[0], title: "Inserted ticket" }, ...tickets];
    await poll();
    check(board.firstElementChild !== section, "changed poll did not rebuild the board");
    check(board.querySelector(".ticket-title")?.textContent === "Inserted ticket",
      "changed ticket was not rendered");
    checkScroll(275, 550, "changed poll");
    check(getComputedStyle(board).overflowAnchor === "none", "board scroll anchoring is enabled");

    rooms = [];
    tickets = [];
    await poll();
    const scroll = document.scrollingElement;
    const maxY = scroll.scrollHeight - scroll.clientHeight;
    check(maxY < 550, "empty board did not shrink");
    checkScroll(275, maxY, "empty poll");
    check(board.querySelector(".room-name")?.textContent === "No sections", "empty board did not render");
    result.textContent = "PASS";
  } catch (error) {
    result.textContent = error.message;
  }
});
</script>"#,
    );
}

#[test]
fn headless_page_counts_visible_tickets_across_sections() {
    assert_headless_board_probe(
        r#"<script>
const rooms = [
  { id: 1, name: "First" },
  { id: 2, name: "Second" },
  { id: 3, name: "Empty" }
];
let tickets = [
  { room_id: 1, column: "todo", title: "First todo" },
  { room_id: 2, column: "todo", title: "Second todo" },
  { room_id: 2, column: "testing", title: "Second testing" }
];
const olderTicket = { room_id: 1, column: "done", title: "Outside the view" };
let poll;
window.setInterval = callback => { poll = callback; };
window.fetch = async url => ({
  ok: url === "/api/board?view=session",
  json: async () => ({ rooms, tickets, all_tickets: [...tickets, olderTicket] })
});

function checkTotals(expected, phase) {
  const labels = [...document.querySelectorAll(".column-header")].map(header => header.textContent);
  const expectedLabels = ["todo", "blocked", "investigation", "implementation", "testing", "review", "done"]
    .map((column, index) => `${column} ${expected[index]}`);
  if (labels.join() !== expectedLabels.join()) {
    throw new Error(`${phase}: expected ${expectedLabels.join(", ")}, got ${labels.join(", ")}`);
  }
}

window.addEventListener("load", async () => {
  const result = document.createElement("pre");
  result.id = "board-probe-result";
  result.hidden = true;
  document.body.appendChild(result);
  try {
    checkTotals([2, 0, 0, 0, 1, 0, 0], "initial totals");
    await poll();
    checkTotals([2, 0, 0, 0, 1, 0, 0], "identical poll");

    tickets = [tickets[0], { ...tickets[1], column: "testing" }, tickets[2]];
    await poll();
    checkTotals([1, 0, 0, 0, 2, 0, 0], "changed poll");

    tickets = [];
    await poll();
    checkTotals([0, 0, 0, 0, 0, 0, 0], "empty poll");
    result.textContent = "PASS";
  } catch (error) {
    result.textContent = error.message;
  }
});
</script>"#,
    );
}

#[test]
fn headless_page_labels_pull_requests_by_host() {
    assert_headless_board_probe(
        r#"<script>
const rooms = [{ id: 1, name: "Requests" }];
const cases = [
  ["https://github.com/example/platform/pull/41?tab=files", "PR #41"],
  ["https://codeup.aliyun.com/example/platform/merge_requests/42", "MR #42"],
  ["https://gitlab.com/example/platform/-/merge_requests/43", "MR #43"],
  ["https://forge.example.com/example/platform/-/merge_requests/44", "MR #44"],
  ["https://github.com.example.com/example/platform/pull/45", "MR #45"],
  ["https://example.com/changes/46", "MR / PR"]
];
const tickets = cases.map(([mr_url], index) => ({
  room_id: 1, column: "review", title: `Ticket ${index + 1}`, mr_url
}));
let poll;
window.setInterval = callback => { poll = callback; };
window.fetch = async url => ({
  ok: url === "/api/board?view=session",
  json: async () => ({ rooms, tickets })
});

function checkLinks(expected) {
  const links = [...document.querySelectorAll(".ticket-mr-row .mr-link")];
  if (links.length !== expected.length) throw new Error(`Expected ${expected.length} links, got ${links.length}`);
  expected.forEach(([url, label], index) => {
    const link = links[index];
    if (link.getAttribute("href") !== url || link.textContent !== `${label} ↗`) {
      throw new Error(`Expected ${label} ↗ for ${url}, got ${link.textContent} for ${link.href}`);
    }
  });
}

window.addEventListener("load", async () => {
  const result = document.createElement("pre");
  result.id = "board-probe-result";
  result.hidden = true;
  document.body.appendChild(result);
  try {
    checkLinks(cases);

    const changedUrl = "https://gitlab.com/example/platform/-/merge_requests/41";
    tickets[0] = { ...tickets[0], mr_url: changedUrl };
    await poll();
    checkLinks([[changedUrl, "MR #41"], ...cases.slice(1)]);
    result.textContent = "PASS";
  } catch (error) {
    result.textContent = error.message;
  }
});
</script>"#,
    );
}

fn assert_headless_board_probe(fixture: &str) {
    let Some(chrome) = headless_chrome() else {
        eprintln!("Skipping browser probe: Chrome or Chromium is unavailable");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let probe_path = tmp.path().join("board-probe.html");
    let probe_html = server::PAGE_HTML.replacen("<script>", &format!("{fixture}\n<script>"), 1);
    std::fs::write(&probe_path, probe_html).unwrap();
    let out = Command::new(chrome)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--window-size=600,500",
            "--dump-dom",
        ])
        .arg(format!(
            "--user-data-dir={}",
            tmp.path().join("profile").display()
        ))
        .arg(format!("file://{}", probe_path.display()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "Chrome failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dom = String::from_utf8_lossy(&out.stdout);
    let result = dom
        .lines()
        .find(|line| line.contains(r#"<pre id="board-probe-result""#));
    assert!(
        result.is_some_and(|line| line.contains(">PASS</pre>")),
        "unexpected board probe result: {result:?}"
    );
}
