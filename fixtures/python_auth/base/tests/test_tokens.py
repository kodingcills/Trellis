"""Token issue/verify behavior (the refresh path is not exercised here)."""

import unittest

from auth.errors import InvalidToken
from auth.tokens import issue_token, verify_token


class TokenTests(unittest.TestCase):
    def test_round_trip(self) -> None:
        token = issue_token("u1", ttl_seconds=60)
        self.assertEqual(verify_token(token), "u1")

    def test_tampered_token_rejected(self) -> None:
        token = issue_token("u1", ttl_seconds=60)
        tampered = token[:-1] + ("0" if token[-1] != "0" else "1")
        with self.assertRaises(InvalidToken):
            verify_token(tampered)

    def test_expired_token_rejected(self) -> None:
        token = issue_token("u1", ttl_seconds=-1)
        with self.assertRaises(InvalidToken):
            verify_token(token)


if __name__ == "__main__":
    unittest.main()
