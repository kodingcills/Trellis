"""Stateless session behavior."""

import unittest

from auth.errors import InvalidToken
from auth.session import open_session, resume_session


class SessionTests(unittest.TestCase):
    def test_stateless_round_trip(self) -> None:
        token, session = open_session("u7")
        self.assertFalse(session.expired())
        resumed = resume_session(token)
        self.assertEqual(resumed.user_id, "u7")

    def test_resume_rejects_garbage(self) -> None:
        with self.assertRaises(InvalidToken):
            resume_session("not-a-token")


if __name__ == "__main__":
    unittest.main()
