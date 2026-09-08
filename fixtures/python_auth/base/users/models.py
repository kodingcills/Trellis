"""User domain model."""

from dataclasses import dataclass, field
from enum import Enum


class Status(Enum):
    ACTIVE = "active"
    SUSPENDED = "suspended"


@dataclass
class User:
    """An account in the local user directory."""

    user_id: str
    email: str
    status: Status = Status.ACTIVE
    failed_logins: int = 0
    roles: list[str] = field(default_factory=list)

    def is_active(self) -> bool:
        return self.status is Status.ACTIVE
