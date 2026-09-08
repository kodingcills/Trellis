"""Display-profile helpers."""

from users.models import User


def display_name(user: User) -> str:
    """Derive a human-readable name from the email local-part."""
    local_part = user.email.split("@", 1)[0]
    return local_part.replace(".", " ").title()


def initials(user: User) -> str:
    """Return the uppercase initials of the display name."""
    parts = display_name(user).split()
    return "".join(p[0] for p in parts).upper() or "?"
