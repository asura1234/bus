"""No-model checks for the live integration driver's evidence assertions."""
import unittest
import bus_live_integration as driver
from bus_live_integration import LiveBus, SIDEBAR, assert_reply_cards


class EvidenceTests(unittest.TestCase):
    def test_cursor_uses_cursor_agent_and_third_form_option(self):
        # A generic `agent` command can point to an unrelated installed CLI.
        providers = getattr(driver, "PROVIDERS", {})
        self.assertEqual(providers.get("cursor"), ("cursor-agent", 2))

    def frame(self, *lines):
        return "\n".join(" " * SIDEBAR + line for line in lines)

    def test_prompt_token_is_not_a_reply(self):
        with self.assertRaises(AssertionError):
            assert_reply_cards(self.frame("You → claude", "Reply with exactly NONCE"), ["claude"], "NONCE")

    def test_every_selected_agents_reply_must_be_visible(self):
        frame = self.frame("claude  Claude Code  12:00", "NONCE", "Quote")
        assert_reply_cards(frame, ["claude"], "NONCE")
        with self.assertRaises(AssertionError):
            assert_reply_cards(frame, ["claude", "codex"], "NONCE")
        with self.assertRaises(AssertionError):
            assert_reply_cards(frame, ["claude"], "DIFFERENT")

    def test_picker_click_targets_checkbox_not_existing_reply(self):
        # No PTY/provider process: exercise the actual selection method with a
        # narrow test surface that records the exact mouse target labels.
        class Surface:
            cases = {}
            selected = {1}
            clicked = []

            def send(self, _text): pass
            def save(self, _name): pass
            def state(self):
                return {"agents": {"1": {"id": 1, "name": "claude"}, "2": {"id": 2, "name": "codex"}}}
            def room(self):
                return {"draft": {"recipient_ids": list(self.selected)}}
            def click(self, label):
                self.clicked.append(label)
                self.selected.symmetric_difference_update({1 if "claude" in label else 2})

        surface = Surface()
        LiveBus.select_agents(surface, ["codex"])
        self.assertEqual(surface.clicked, ["[x] claude", "[ ] codex"])


if __name__ == "__main__":
    unittest.main()
