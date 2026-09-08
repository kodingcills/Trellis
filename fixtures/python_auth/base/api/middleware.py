"""Request authentication middleware."""

from api.responses import error_response
from auth.errors import AuthError
from auth.service import AuthService


class AuthMiddleware:
    """Guard request handlers behind token verification."""

    def __init__(self, auth: AuthService) -> None:
        self._auth = auth

    def handle(self, request: dict[str, object]) -> dict[str, object]:
        """Verify the request token and attach the user id, or reject it."""
        token = str(request.get("token", ""))
        try:
            session = self._auth.resume(token)
        except AuthError:
            return error_response(401, "invalid token")
        request["user_id"] = session.user_id
        return request
