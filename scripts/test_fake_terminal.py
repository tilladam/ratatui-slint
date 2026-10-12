import unittest
from types import SimpleNamespace

from fake_terminal import last_screen, validate_checks


class SmokeAssertions(unittest.TestCase):
    def args(self, **overrides):
        values = dict(expect_exit=0, expect=[], require=[], protocol="kitty", expect_idle=[])
        values.update(overrides)
        return SimpleNamespace(**values)

    def test_missing_graphics_and_restore_fail(self):
        checks = {"exit status": 0, "kitty transmit": False,
                  "entered alt screen": True, "left alt screen": False}
        self.assertEqual(validate_checks(checks, "", {}, self.args(require=["graphics", "restore"])),
                         ["kitty transmit", "left alt screen"])

    def test_idle_requires_both_marks_and_zero_bytes(self):
        for marks, expected in [({}, ["idle a->b"]), ({"a": 10, "b": 11}, ["idle a->b"]),
                                ({"a": 10, "b": 10}, [])]:
            self.assertEqual(validate_checks({"exit status": 0}, "", marks,
                                            self.args(expect_idle=["a:b"])), expected)

    def test_assertions_use_final_screen_not_historical_text(self):
        data = b"\x1b[1;1Htarget 22.0\x1b[1;9H5\x1b[?1049l"
        screen = last_screen(data, 1, 20)
        self.assertIn("target 25.0", screen)
        self.assertEqual(validate_checks({"exit status": 0}, screen, {},
                                        self.args(expect=["target 22.0"])), ["expect 'target 22.0'"])

    def test_expected_panic_exit_must_match(self):
        self.assertEqual(validate_checks({"exit status": 101}, "", {}, self.args(expect_exit=101)), [])
        self.assertEqual(validate_checks({"exit status": 0}, "", {}, self.args(expect_exit=101)), ["exit status"])


if __name__ == "__main__":
    unittest.main()
