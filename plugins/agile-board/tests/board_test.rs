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
    // 1. Exhaustive transition matrix check for both investigation_only=false and true
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
                    (_, Column::Blocked | Column::WontWork) => true,
                    (Column::Blocked, _) => true,
                    (Column::WontWork, Column::Todo) => true,
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
                author_model: None,
                reviewer_model: None,
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
                author_model: None,
                reviewer_model: None,
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
                author_model: None,
                reviewer_model: None,
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
                author_model: None,
                reviewer_model: None,
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
                author_model: None,
                reviewer_model: None,
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

    let move_output = Command::new(bin())
        .args([
            "--data-dir",
            &data_dir,
            "ticket",
            "move",
            "T-1",
            "--column",
            "wont-work",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(move_output.status.success(), "{:?}", move_output);
    let moved: serde_json::Value = serde_json::from_slice(&move_output.stdout).unwrap();
    assert_eq!(moved["ticket"]["column"], "wont_work");

    let list_output = Command::new(bin())
        .args([
            "--data-dir",
            &data_dir,
            "ticket",
            "list",
            "--column",
            "wont_work",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(list_output.status.success(), "{:?}", list_output);
    let listed: serde_json::Value = serde_json::from_slice(&list_output.stdout).unwrap();
    assert_eq!(listed["tickets"][0]["id"], "T-1");
    assert_eq!(listed["tickets"][0]["column"], "wont_work");

    // Clear without --confirm returns exit 2
    let no_confirm = Command::new(bin())
        .args(["--data-dir", &data_dir, "history", "clear"])
        .output()
        .unwrap();
    assert_eq!(no_confirm.status.code(), Some(2));

    // Verify page HTML contract: Agile Board title, viewport scrolling (`html { overflow: auto }` and
    // `body { overflow: visible; min-height: 100vh }`), global sticky header band + sticky left
    // section sidebar, near-square card min-height, plain solid 1px card border, double confirmation
    // for Clear session history, no modal, and no hidden/rendered ticket IDs.
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
const frameCallbacks = [];
window.requestAnimationFrame = callback => frameCallbacks.push(callback);
function advanceFrame() {
  for (const callback of frameCallbacks.splice(0)) callback(performance.now());
}
window.fetch = async url => ({
  ok: url === "/api/board?view=session",
  json: async () => ({ rooms, tickets, now_ms: nowMs })
});
window.setInterval = (callback, delay) => {
  if (callback.name === "fetchBoard") {
    poll = callback;
    pollDelay = delay;
  }
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
    advanceFrame();
    window.scrollTo(275, 550);
    const checkScroll = (x, y, phase) => check(
      window.scrollX === x && window.scrollY === y,
      `${phase}: expected scroll (${x}, ${y}), got (${window.scrollX}, ${window.scrollY})`
    );
    checkScroll(275, 550, "initial scroll");

    await poll();
    checkScroll(275, 550, "identical poll");
    check(board.firstElementChild === section, "identical poll rebuilt the board");
    check(frameCallbacks.length === 0, "identical poll scheduled a scroll restore");

    nowMs += 3000;
    await poll();
    checkScroll(275, 550, "timestamp-only poll");
    check(board.firstElementChild === section, "timestamp-only poll rebuilt the board");
    check(frameCallbacks.length === 0, "timestamp-only poll scheduled a scroll restore");

    // Reproduce a transient zero scroll range immediately after the DOM swap.
    const innerHTML = Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML");
    Object.defineProperty(board, "innerHTML", {
      configurable: true,
      set(value) {
        innerHTML.set.call(this, value);
        board.style.display = "none";
        const root = document.scrollingElement;
        check(root.scrollHeight === root.clientHeight, "transient scroll range did not collapse");
        requestAnimationFrame(() => { board.style.display = ""; });
      },
    });
    tickets = [{ ...tickets[0], title: "Inserted ticket" }, ...tickets];
    await poll();
    delete board.innerHTML;
    advanceFrame();
    check(board.firstElementChild !== section, "changed poll did not rebuild the board");
    check(board.querySelector(".ticket-title")?.textContent === "Inserted ticket",
      "changed ticket was not rendered");
    checkScroll(275, 550, "changed poll after transient layout collapse");
    for (const element of [document.documentElement, document.body, board]) {
      check(getComputedStyle(element).overflowAnchor === "none", `${element.tagName} scroll anchoring is enabled`);
    }

    rooms = [];
    tickets = [];
    await poll();
    advanceFrame();
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
window.setInterval = (callback, delay) => { if (delay === 3000) poll = callback; };
window.fetch = async url => ({
  ok: url === "/api/board?view=session",
  json: async () => ({ rooms, tickets, all_tickets: [...tickets, olderTicket] })
});

function checkTotals(expected, phase) {
  const labels = [...document.querySelectorAll(".column-header .column-label")].map(label => label.textContent);
  const expectedLabels = ["todo", "blocked", "investigation", "implementation", "testing", "review", "done", "Won't Work"]
    .map((column, index) => `${column} ${expected[index] ?? 0}`);
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
  ["https://codeup.aliyun.com/663dc7b313a39cc55c4cb5b8/mobile/libtv-app/change/1936", "MR #1936"],
  ["https://codeup.aliyun.com/example/platform/merge_requests/42", "MR #42"],
  ["https://gitlab.com/example/platform/-/merge_requests/43", "MR #43"],
  ["https://forge.example.com/example/platform/-/merge_requests/44", "MR #44"],
  ["https://github.com.example.com/example/platform/pull/45", "MR #45"],
  ["https://example.com/change/46", "MR / PR"]
];
const tickets = cases.map(([mr_url], index) => ({
  room_id: 1, column: "review", title: `Ticket ${index + 1}`, mr_url
}));
let poll;
window.setInterval = (callback, delay) => { if (delay === 3000) poll = callback; };
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

#[test]
fn terminal_column_headers_include_hide_buttons() {
    for (column, name) in [("done", "Done"), ("wont_work", "Won't Work")] {
        let (_, rest) = server::PAGE_HTML
            .split_once(&format!(
                r#"<div class="column-header" data-col="{column}">"#
            ))
            .unwrap();
        let (header, _) = rest.split_once("</div>").unwrap();
        assert!(header.contains(r#"<span class="column-label">"#));
        assert!(header.contains(r#"<button type="button" class="column-toggle""#));
        assert!(header.contains(&format!(r#"aria-label="Hide {name} cards">Hide</button>"#)));
    }
}

#[test]
fn headless_page_fits_visible_columns_and_remembers_both_toggles() {
    let fixture = r#"<script>
const rooms = [{ id: 1, name: "First" }, { id: 2, name: "Second" }];
let tickets = [
  { room_id: 1, column: "done", title: "Completed one" },
  { room_id: 2, column: "done", title: "Completed two" },
  { room_id: 1, column: "wont_work", title: "Closed one" },
  { room_id: 2, column: "wont_work", title: "Closed two" }
];
let poll;
window.setInterval = (callback, delay) => { if (delay === 3000) poll = callback; };
window.fetch = async () => ({ ok: true, json: async () => ({ rooms, tickets }) });

window.addEventListener("load", async () => {
  const result = document.createElement("pre");
  result.id = "board-probe-result";
  result.hidden = true;
  document.body.appendChild(result);
  try {
    const done = document.querySelector('.column-header[data-col="done"]');
    const wontWork = document.querySelector('.column-header[data-col="wont_work"]');
    const check = (condition, message) => { if (!condition) throw new Error(message); };
    const visible = element => element.getClientRects().length > 0;
    const doneToggle = done.querySelector("button");
    const wontWorkToggle = wontWork.querySelector("button");
    const checkColumn = (header, name, shouldShow, count) => {
      const column = header.dataset.col;
      const toggle = header.querySelector("button");
      const label = header.querySelector(".column-label");
      const cells = [...document.querySelectorAll(`.kanban-column[data-col="${column}"]`)];
      const cards = cells.flatMap(cell => [...cell.querySelectorAll(".ticket-card")]);
      const action = shouldShow ? "Hide" : "Show";
      check(visible(header) && visible(toggle) && label.textContent === `${name === "Done" ? "done" : name} ${count}`,
        `${name} header, toggle, and count should remain visible`);
      check(toggle.textContent === action && toggle.getAttribute("aria-label") === `${action} ${name} cards`,
        `Expected ${action} ${name} cards`);
      check(cells.length === rooms.length && cells.every(cell => visible(cell) === shouldShow) &&
        cards.length === count && cards.every(card => visible(card) === shouldShow),
        `${name} visibility did not match across all rooms`);
    };
    const checkColumns = (showDone, showWontWork, count = 2) => {
      checkColumn(done, "Done", showDone, count);
      checkColumn(wontWork, "Won't Work", showWontWork, count);
    };
    const viewport = document.documentElement.clientWidth;
    const grid = document.querySelector(".global-columns-bar");
    const clearHistory = document.getElementById("clear-history-btn");
    const restoreGroup = document.getElementById("collapsed-columns");
    const gridHeaders = () => [...grid.querySelectorAll(".column-header")];
    const section = grid.querySelector(".sidebar-corner-header");
    const gap = parseFloat(getComputedStyle(grid).columnGap);
    const fits = element => {
      const rect = element.getBoundingClientRect();
      return rect.left >= -1 && rect.right <= viewport + 1;
    };
    const checkLayout = (showDone, showWontWork) => {
      const headers = gridHeaders();
      const visibleColumns = ["todo", "blocked", "investigation", "implementation", "testing", "review"];
      if (showDone) visibleColumns.push("done");
      if (showWontWork) visibleColumns.push("wont_work");
      check(headers.map(header => header.dataset.col).join() === visibleColumns.join(),
        `Unexpected header order for ${visibleColumns.join()}`);
      const viewportColumns = Math.min(headers.length, 7);
      const expectedWidth = (viewport - section.offsetWidth - gap) / viewportColumns - gap;
      check(headers.every(header => Math.abs(header.getBoundingClientRect().width - expectedWidth) <= 1),
        `Expected ${viewportColumns} full columns across the viewport`);
      for (const room of document.querySelectorAll(".room-section")) {
        const cells = [...room.querySelectorAll(".kanban-column")].filter(visible);
        check(cells.length === headers.length && cells.every((cell, i) => {
          const rect = cell.getBoundingClientRect();
          const header = headers[i].getBoundingClientRect();
          return Math.abs(rect.left - header.left) <= 1 && Math.abs(rect.right - header.right) <= 1;
        }), "Room columns did not match the header tracks");
      }
      const inView = headers.slice(0, viewportColumns);
      check(inView.every(fits) && Math.abs(inView.at(-1).getBoundingClientRect().right - (viewport - gap)) <= 1,
        `The ${viewportColumns} visible tracks should fill the viewport`);
      if (showDone && showWontWork) {
        check(wontWork.getBoundingClientRect().left >= viewport - 1 && !fits(wontWork),
          "Won't Work should overflow to the right when all eight columns are visible");
      } else {
        check(window.scrollX === 0 && document.documentElement.scrollWidth <= viewport + 1,
          "The board should fit without horizontal scrolling after hiding a column");
      }
      const restoreHeaders = [done, wontWork].filter(header => !headers.includes(header));
      check(visible(restoreGroup) === (restoreHeaders.length > 0),
        "The restore row should only be visible when a column is hidden");
      if (restoreHeaders.length > 0) {
        const clearRect = clearHistory.getBoundingClientRect();
        const groupRect = restoreGroup.getBoundingClientRect();
        const rects = restoreHeaders.map(header => header.getBoundingClientRect());
        check(Math.abs(Math.max(...rects.map(rect => rect.right)) - clearRect.right) <= 1,
          "Restore tags should align with the right edge of Clear session history");
        check(rects.every(rect => rect.top >= clearRect.bottom && rect.bottom <= groupRect.bottom) &&
          groupRect.bottom <= grid.getBoundingClientRect().top,
          "Restore tags should sit between Clear session history and the board columns");
        for (const header of restoreHeaders) {
          check(fits(header) && fits(header.querySelector("button")), "A restore control is outside the viewport");
        }
      }
    };
    const clickToggle = toggle => {
      const rect = toggle.getBoundingClientRect();
      const target = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
      check(target === toggle, `${toggle.getAttribute("aria-label")} could not be clicked`);
      target.click();
    };
    const doneKey = "agile-board.hide-done";
    const wontWorkKey = "agile-board.hide-wont-work";
    if (performance.getEntriesByType("navigation")[0].type === "reload") {
      checkColumns(false, false);
      checkLayout(false, false);
      clickToggle(doneToggle);
      checkColumns(true, false);
      checkLayout(true, false);
      check(localStorage.getItem(doneKey) === "false" && localStorage.getItem(wontWorkKey) === "true",
        "Showing Done changed the Won't Work preference");
      clickToggle(wontWorkToggle);
      checkColumns(true, true);
      checkLayout(true, true);
      check(localStorage.getItem(wontWorkKey) === "false", "Show Won't Work preference was not saved");
      result.textContent = "PASS";
      return;
    }

    check(!document.querySelector('.topbar input[type="checkbox"]'), "The old Hide Done checkbox is still in the toolbar");
    checkColumns(true, true);
    checkLayout(true, true);

    const firstRoom = document.querySelector('.room-section');
    const expandedHeight = firstRoom.offsetHeight;
    window.scrollTo(document.documentElement.scrollWidth, 0);
    clickToggle(wontWorkToggle);
    checkColumns(true, false);
    checkLayout(true, false);
    await poll();
    checkColumns(true, false);
    checkLayout(true, false);
    clickToggle(wontWorkToggle);
    window.scrollTo(0, 0);
    doneToggle.focus();
    clickToggle(doneToggle);
    check(document.activeElement === doneToggle, "The Done toggle lost focus when it moved to the toolbar");
    checkColumns(false, true);
    checkLayout(false, true);
    clickToggle(wontWorkToggle);
    checkColumns(false, false);
    checkLayout(false, false);
    check(firstRoom.offsetHeight < expandedHeight, "The hidden cards still take up room");
    check(localStorage.getItem(doneKey) === "true" && localStorage.getItem(wontWorkKey) === "true",
      "Hide preferences were not saved independently");
    wontWorkToggle.focus();
    tickets = [...tickets,
      { room_id: 1, column: "done", title: "Another completion" },
      { room_id: 2, column: "wont_work", title: "Another closure" }
    ];
    await poll();
    checkColumns(false, false, 3);
    checkLayout(false, false);
    check(document.activeElement === wontWorkToggle, "The Won't Work toggle lost focus on a changed poll");
    location.reload();
  } catch (error) {
    result.textContent = error.message;
  }
});
</script>"#;
    for window_size in ["1440,900", "1728,900"] {
        assert_headless_board_probe_at_size(fixture, window_size);
    }
}

#[test]
fn headless_page_ticks_ticket_age_without_rebuilding() {
    assert_headless_board_probe(
        r#"<script>
const rooms = [{ id: 1, name: "Requests" }];
let now = 1760000000000;
Date.now = () => now;
const tickets = [
  { room_id: 1, column: "review", title: "Recent", created_at_ms: now - 12.75 * 60000,
    mr_url: "https://github.com/example/platform/pull/41" },
  { room_id: 1, column: "review", title: "Near one hour", created_at_ms: now - 59.75 * 60000 },
  { room_id: 1, column: "review", title: "Older", created_at_ms: now - 1323.75 * 60000 }
];
const intervals = new Map();
window.setInterval = (callback, delay) => { intervals.set(delay, callback); };
window.fetch = async url => ({
  ok: url === "/api/board?view=session",
  json: async () => ({ rooms, tickets, now_ms: now })
});

window.addEventListener("load", async () => {
  const result = document.createElement("pre");
  result.id = "board-probe-result";
  result.hidden = true;
  document.body.appendChild(result);
  try {
    const board = document.getElementById("board-container");
    const section = board.firstElementChild;
    const cards = [...section.querySelectorAll(".ticket-card")];
    const ages = cards.map(card => card.querySelector(".ticket-age"));
    const checkAges = expected => {
      const labels = ages.map(age => age?.textContent);
      if (labels.join() !== expected.join()) throw new Error(`Expected ${expected}, got ${labels}`);
    };
    checkAges(["12 min", "59 min", "22 hr 3 min"]);
    cards.forEach((card, i) => {
      const footer = card.lastElementChild;
      const age = ages[i];
      const link = footer.querySelector(".mr-link");
      if (age.parentElement !== footer || Boolean(link) !== (i === 0)) {
        throw new Error("Expected ages in card footers with and without MR links");
      }
      const cardRect = card.getBoundingClientRect();
      const style = getComputedStyle(card);
      const contentRight = cardRect.right - parseFloat(style.borderRightWidth) - parseFloat(style.paddingRight);
      const ageRect = age.getBoundingClientRect();
      if (Math.abs(ageRect.right - contentRight) > 1) {
        throw new Error(`Ticket ${i + 1}: age right edge ${ageRect.right} does not match ${contentRight}`);
      }
      if (link) {
        const linkRect = link.getBoundingClientRect();
        const contentLeft = cardRect.left + parseFloat(style.borderLeftWidth) + parseFloat(style.paddingLeft);
        if (Math.abs(linkRect.left - contentLeft) > 1 || ageRect.top >= linkRect.bottom) {
          throw new Error("Expected the MR link at the left of the same footer row");
        }
      }
    });
    if (Number(ages[0].dataset.createdAtMs) !== tickets[0].created_at_ms) {
      throw new Error("Age label is missing its creation timestamp");
    }
    const tick = intervals.get(30000);
    if (!tick) throw new Error("Expected a 30000-ms age ticker");
    now += 30000;
    tick();
    checkAges(["13 min", "1 hr 0 min", "22 hr 4 min"]);
    if (board.firstElementChild !== section || cards.some((card, i) => !card.contains(ages[i]))) {
      throw new Error("The age tick rebuilt ticket markup");
    }
    await intervals.get(3000)();
    checkAges(["13 min", "1 hr 0 min", "22 hr 4 min"]);
    if (board.firstElementChild !== section) throw new Error("A clock-only poll rebuilt the board");
    result.textContent = "PASS";
  } catch (error) {
    result.textContent = error.message;
  }
});
</script>"#,
    );
}

#[test]
fn headless_page_freezes_done_ticket_ages() {
    assert_headless_board_probe(
        r#"<script>
const rooms = [{ id: 1, name: "Requests" }];
let now = 1760000000000;
Date.now = () => now;
const minute = 60000;
const active = { room_id: 1, column: "review", title: "Finishing", created_at_ms: now - 15 * minute };
const tickets = [
  active,
  { room_id: 1, column: "review", status: "done", title: "Done status",
    created_at_ms: now - 25 * minute, updated_at_ms: now - 5 * minute },
  { room_id: 1, column: "done", title: "Done column",
    created_at_ms: now - 85 * minute, updated_at_ms: now - 10 * minute },
  { room_id: 1, column: "wont_work", title: "Won't Work column",
    created_at_ms: now - 35 * minute, updated_at_ms: now - 10 * minute },
  { room_id: 1, column: "wont-work", title: "Won't Work alias",
    created_at_ms: now - 40 * minute, updated_at_ms: now - 10 * minute },
  { room_id: 1, column: "done", title: "Negative duration",
    created_at_ms: now - 5 * minute, updated_at_ms: now - 6 * minute }
];
const intervals = new Map();
window.setInterval = (callback, delay) => { intervals.set(delay, callback); };
window.fetch = async url => ({
  ok: url === "/api/board?view=session",
  json: async () => ({ rooms, tickets, now_ms: now })
});

window.addEventListener("load", async () => {
  const result = document.createElement("pre");
  result.id = "board-probe-result";
  result.hidden = true;
  document.body.appendChild(result);
  try {
    const board = document.getElementById("board-container");
    const checkAges = expected => {
      for (const card of board.querySelectorAll(".ticket-card")) {
        const title = card.querySelector(".ticket-title").textContent;
        const label = card.querySelector(".ticket-age")?.textContent;
        if (label !== expected[title]) throw new Error(`${title}: expected ${expected[title]}, got ${label}`);
      }
      if (board.querySelectorAll(".ticket-age").length !== tickets.length) {
        throw new Error("Expected one age label per ticket");
      }
    };
    const expected = { "Finishing": "15 min", "Done status": "20 min", "Done column": "1 hr 15 min",
      "Won't Work column": "25 min", "Won't Work alias": "30 min", "Negative duration": "0 min" };
    checkAges(expected);
    const tick = intervals.get(30000);
    for (let i = 0; i < 2; i++) {
      now += 30000;
      tick();
    }
    expected.Finishing = "16 min";
    checkAges(expected);
    active.column = "done";
    active.updated_at_ms = now;
    await intervals.get(3000)();
    checkAges(expected);
    for (let i = 0; i < 2; i++) {
      now += 30000;
      tick();
    }
    checkAges(expected);
    result.textContent = "PASS";
  } catch (error) {
    result.textContent = error.message;
  }
});
</script>"#,
    );
}

#[test]
fn ticket_model_metadata_round_trips_without_rendering() {
    let tmp = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(bin())
            .arg("--data-dir")
            .arg(tmp.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    run(&["room", "add", "--id", "1", "--name", "Models", "--json"]);
    let created = run(&[
        "ticket",
        "add",
        "--room",
        "Models",
        "--title",
        "Visible ticket",
        "--author-model",
        " private-author/vendor-model.v2 ",
        "--reviewer-model",
        "private-reviewer/initial",
        "--json",
    ]);
    let created = &created["ticket"];
    assert_eq!(created["author_model"], "private-author/vendor-model.v2");
    assert_eq!(created["reviewer_model"], "private-reviewer/initial");

    run(&[
        "ticket",
        "update",
        "T-1",
        "--reviewer-model",
        "private-reviewer/vendor-model.v3",
        "--json",
    ]);
    let models = run(&["ticket", "list", "--json"]);
    let listed = &models["tickets"][0];
    assert_eq!(listed["author_model"], "private-author/vendor-model.v2");
    assert_eq!(listed["reviewer_model"], "private-reviewer/vendor-model.v3");
    assert_eq!(listed["created_at_ms"], created["created_at_ms"]);
    assert_eq!(listed["history"][1]["at_ms"], listed["updated_at_ms"]);
    assert_eq!(
        listed["history"][1]["summary"],
        "Updated T-1 (reviewer_model)"
    );

    let payload = server::board_api_payload(tmp.path(), ViewFilter::Session, 1).unwrap();
    assert_eq!(&payload["tickets"][0], listed);
    assert_eq!(&payload["all_tickets"][0], listed);
    let fixture = r#"<script>
const payload = BOARD_PAYLOAD;
window.fetch = async () => ({ ok: true, json: async () => payload });
window.addEventListener("load", () => {
  const board = document.getElementById("board-container");
  const result = document.createElement("pre");
  result.id = "board-probe-result";
  result.hidden = true;
  document.body.appendChild(result);
  if (board.querySelector(".ticket-title")?.textContent !== "Visible ticket") {
    result.textContent = "Ticket did not render";
    return;
  }
  const ticket = payload.tickets[0];
  const rendered = document.body.cloneNode(true);
  rendered.querySelectorAll("script").forEach(script => script.remove());
  const html = rendered.outerHTML;
  const leaks = [ticket.author_model, ticket.reviewer_model, "author_model", "reviewer_model"]
    .some(model => html.includes(model));
  result.textContent = leaks ? "Internal model fields appeared in the rendered DOM" : "PASS";
});
</script>"#;
    assert_headless_board_probe(&fixture.replace("BOARD_PAYLOAD", &payload.to_string()));

    let updated = run(&[
        "ticket",
        "update",
        "T-1",
        "--author-model",
        "",
        "--reviewer-model",
        "private-reviewer/final",
        "--json",
    ]);
    assert!(updated["ticket"]["author_model"].is_null());
    assert_eq!(
        updated["ticket"]["reviewer_model"],
        "private-reviewer/final"
    );
    assert_eq!(
        updated["ticket"]["history"][2]["summary"],
        "Updated T-1 (author_model, reviewer_model)"
    );

    let mut legacy = listed.clone();
    let object = legacy.as_object_mut().unwrap();
    object.remove("author_model");
    object.remove("reviewer_model");
    let legacy: agile_board::model::Ticket = serde_json::from_value(legacy).unwrap();
    let legacy = serde_json::to_value(legacy).unwrap();
    assert!(legacy
        .get("author_model")
        .is_some_and(serde_json::Value::is_null));
    assert!(legacy
        .get("reviewer_model")
        .is_some_and(serde_json::Value::is_null));
}

fn assert_headless_board_probe(fixture: &str) {
    assert_headless_board_probe_at_size(fixture, "600,500");
}

fn assert_headless_board_probe_at_size(fixture: &str, window_size: &str) {
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
            &format!("--window-size={window_size}"),
            // Allow the probe to finish through a page reload before dumping the DOM.
            "--virtual-time-budget=1000",
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
