"""Request routing for the auth/user flows."""

from api.middleware import AuthMiddleware
from api.responses import error_response, json_response
from auth.errors import AuthError
from auth.service import AuthService
from users.service import UserService


def post_login(users: UserService, body: dict[str, str]) -> dict[str, object]:
    """Handle login requests (email + password)."""
    try:
        token = users.login(body["email"], body["password"])
    except (LookupError, AuthError, ValueError) as exc:
        return error_response(401, str(exc))
    return json_response(200, {"token": token})


def post_register(users: UserService, body: dict[str, str]) -> dict[str, object]:
    """Handle registration requests."""
    try:
        user = users.register(body["user_id"], body["email"], body["password"])
    except ValueError as exc:
        return error_response(409, str(exc))
    return json_response(201, {"user_id": user.user_id})


def guarded(middleware: AuthMiddleware, request: dict[str, object]) -> dict[str, object]:
    """A protected endpoint delegating to the auth middleware."""
    return middleware.handle(request)
