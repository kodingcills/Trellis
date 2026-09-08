"""Stateless sessions: everything needed travels inside the signed token.

There is deliberately no server-side session storage in this service; a
session is a signed token plus its configured lifetime.
"""

import time

from auth.tokens import issue_token, verify_token
from config.settings import Settings


class Session:
    """A stateless session bound to a user id."""

    def __init__(self, user_id: str, expires_at: int) -> None:
        self.user_id = user_id
        self.expires_at = expires_at

    def expired(self, now: int | None = None) -> bool:
        now = int(time.time()) if now is None else now
        return now >= self.expires_at


def open_session(user_id: str) -> tuple[str, Session]:
    """Issue a token and the matching stateless session window."""
    ttl = Settings.session_ttl_seconds()
    token = issue_token(user_id, ttl)
    return token, Session(user_id, int(time.time()) + ttl)


def resume_session(token: str) -> Session:
    """Rebuild a session from a token, rejecting invalid or expired tokens."""
    user_id = verify_token(token)  # raises InvalidToken on bad signature/expiry
    return Session(user_id, int(time.time()) + Settings.session_ttl_seconds())
