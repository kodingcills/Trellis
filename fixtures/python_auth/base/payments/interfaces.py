"""Payment gateway interface."""

from abc import ABC, abstractmethod


class PaymentGateway(ABC):
    """A strategy for charging and refunding orders."""

    @abstractmethod
    def charge(self, amount_cents: int, currency: str) -> str:
        """Execute a charge; return a provider transaction id."""

    @abstractmethod
    def refund(self, transaction_id: str, amount_cents: int) -> bool:
        """Refund part or all of a charge."""
