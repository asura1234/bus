"""Render the latency page against a private fixture; no Bus or model is started."""
import argparse
import http.server
import json
from pathlib import Path
import subprocess
import tempfile
import threading
from html.parser import HTMLParser


class Rendered(HTMLParser):
    def __init__(self):
        super().__init__()
        self.buttons = self.details = 0
        self.in_groups = self.in_slowest = False
        self.slow_text = []

    def handle_starttag(self, tag, attrs):
        if tag == "tbody":
            self.in_groups = dict(attrs).get("id") == "groups"
            self.in_slowest = dict(attrs).get("id") == "slowest" or self.in_slowest
        self.buttons += int(self.in_groups and tag == "button")
        self.details += int(self.in_slowest and tag == "details")

    def handle_endtag(self, tag):
        if tag == "tbody":
            self.in_groups = False
            self.in_slowest = False

    def handle_data(self, data):
        if self.in_slowest:
            self.slow_text.append(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chrome", required=True, type=Path)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    args = parser.parse_args()
    args.evidence_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    page = Path(__file__).resolve().parents[2] / "src/messaging/dashboard/latency_page.html"
    phases = ["accepted_to_box", "enqueue", "ready_queue", "dispatch", "native_transport",
              "writer_queue", "paste_prepare", "text_write", "paste_pacing", "ownership_wait",
              "enter_dispatch", "enter_write", "post_enter_confirmation", "native_total", "reply_delivery"]
    series = {"phases": {name: {"n": 1, "p50": 12, "p90": 12, "p99": 12, "max": 12}
                         for name in phases}, "hours": []}
    slow = {"request_id": 42, "recipient": "fixture-recipient", "recipient_id": 2,
            "provider": "claude", "room": "fixture-room", "room_id": 1,
            "sent_at_ms": 1793683600000, "accepted_to_box_ms": 12,
            "phases": {"accepted_to_box": 12, "text_write": 3}}
    fixture = {"timezone": "UTC+08:00", "generated_at_ms": 1793683600000,
               "metrics": {"all": series, "providers": {"claude": series},
                           "recipients": {"2": {"name": "fixture-recipient", "series": series}},
                           "slowest": [slow], "recipient_slowest": {"2": [slow]}}}
    click = b"""<script>const probe=setInterval(()=>{const b=[...document.querySelectorAll('#groups button')].find(b=>b.textContent.includes('fixture-recipient'));if(!b)return;if(!b.closest('tr').classList.contains('selected'))b.click();const d=document.querySelector('#slowest details');if(d){d.open=true;clearInterval(probe)}},50)</script>"""

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            routes = {"/latency": (page.read_bytes() + click, "text/html"),
                      "/latency/data": (json.dumps(fixture).encode(), "application/json")}
            if self.path not in routes:
                self.send_error(404)
                return
            payload, mime = routes[self.path]
            self.send_response(200)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, *_args):
            pass

    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory(prefix="bus317-browser-") as profile:
                cmd = [str(args.chrome), "--headless=new", "--disable-gpu", "--no-sandbox",
                       "--no-first-run", "--disable-background-networking", "--disable-sync",
                       "--user-data-dir=" + profile, "--window-size=1440,1000", "--virtual-time-budget=1500",
                       "--screenshot=" + str(args.evidence_dir / "page.png"), "--dump-dom",
                       f"http://127.0.0.1:{server.server_port}/latency"]
                result = subprocess.run(cmd, capture_output=True, text=True, timeout=45)
        finally:
            server.shutdown()
            thread.join()
    (args.evidence_dir / "dom.html").write_text(result.stdout)
    (args.evidence_dir / "chrome.log").write_text(result.stderr)
    assert result.returncode == 0, result.stderr[-500:]
    rendered = Rendered()
    rendered.feed(result.stdout)
    observed = {"buttons": rendered.buttons, "details": rendered.details, "slow_text": rendered.slow_text}
    (args.evidence_dir / "result.json").write_text(json.dumps(observed, indent=2))
    assert rendered.buttons == 2, f"Expected selectable group buttons: {rendered.buttons}"
    assert "fixture-recipient (#2) · hourly" in result.stdout, "group selection did not change the series"
    assert rendered.details == 1, f"Missing real phase disclosure: {rendered.slow_text}"
    assert "3 ms" in rendered.slow_text, f"Missing text-write phase: {rendered.slow_text}"


if __name__ == "__main__":
    main()
