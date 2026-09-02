#!/usr/bin/env python3
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import smoke_test


class SmokeTestHelpers(unittest.TestCase):
    def test_extracts_responses_output_text(self):
        data = {
            "output": [
                {"type": "reasoning", "summary": "trace"},
                {"type": "message", "content": [
                    {"type": "output_text", "text": "result"}
                ]},
            ]
        }
        self.assertEqual(smoke_test.extract_response(data), ("result", "trace"))

    def test_continue_payload_is_valid_responses_shape(self):
        payload = smoke_test.response_payload("继续")
        self.assertEqual(payload["input"][0]["role"], "user")
        self.assertEqual(payload["input"][0]["content"][0]["text"], "继续")
        self.assertFalse(payload["stream"])


if __name__ == "__main__":
    unittest.main()
