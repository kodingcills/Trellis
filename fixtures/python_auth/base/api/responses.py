"""HTTP-ish response helpers (framework-free fixture)."""


def json_response(status: int, body: dict[str, object]) -> dict[str, object]:
    """Wrap a body with a status code."""
    return {"status": status, "body": body}


def error_response(status: int, message: str) -> dict[str, object]:
    """Wrap an error message with a status code."""
    return json_response(status, {"error": message})
