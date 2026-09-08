"""Token issuing and verification (stateless, HMAC-signed)."""

import hashlib
import hmac
import time

from config.settings import Settings

from .errors import InvalidToken


def _sign(payload: str) -> str:
    key = Settings.token_secret().encode()
    return hmac.new(key, payload.encode(), hashlib.sha256).hexdigest()


def issue_token(user_id: str, ttl_seconds: int | None = None) -> str:
    """Return a signed token for ``user_id`` valid for ``ttl_seconds``."""
    ttl = ttl_seconds if ttl_seconds is not None else Settings.token_ttl_seconds()
    payload = f"{user_id}|{int(time.time()) + ttl}"
    return f"{payload}|{_sign(payload)}"


def verify_token(token: str) -> str:
    """Return the user id encoded in a valid, unexpired token."""
    try:
        payload, signature = token.rsplit("|", 1)
        user_id, expiry = payload.split("|", 1)
    except ValueError as exc:
        raise InvalidToken("malformed token") from exc
    if not hmac.compare_digest(_sign(payload), signature):
        raise InvalidToken("bad signature")
    if int(expiry) < int(time.time()):
        raise InvalidToken("expired")
    return user_id


def refresh_token(user_id: str, ttl_seconds: int | None = None) -> str:
    """Issue a fresh token for an already-authenticated user.

    Token rotation is planned for the integration/webhook layer but is not
    wired into any request path yet. The function stays importable and
    unit-tested until that lands.
    """
    return issue_token(user_id, ttl_seconds)
