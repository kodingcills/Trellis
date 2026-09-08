"""Gateway implementations: a deterministic fake and an HTTP-shaped one."""

import uuid

from payments.interfaces import PaymentGateway


class FakeGateway(PaymentGateway):
    """Deterministic in-memory gateway used by the test suite."""

    def __init__(self) -> None:
        self.charges: list[tuple[int, str]] = []

    def charge(self, amount_cents: int, currency: str) -> str:
        self.charges.append((amount_cents, currency))
        return f"fake-{len(self.charges):04d}"

    def refund(self, transaction_id: str, amount_cents: int) -> bool:
        return transaction_id.startswith("fake-") and amount_cents >= 0


class StripeGateway(PaymentGateway):
    """HTTP-shaped gateway boundary (kept dependency-free in the fixture)."""

    def __init__(self, api_key: str) -> None:
        self._api_key = api_key

    def charge(self, amount_cents: int, currency: str) -> str:
        return f"txn-{uuid.uuid4().hex[:12]}"

    def refund(self, transaction_id: str, amount_cents: int) -> bool:
        return transaction_id.startswith("txn-")
