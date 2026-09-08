"""Settlement ledger for completed charges."""

import time


class Ledger:
    """Append-only record of settled transactions."""

    def __init__(self) -> None:
        self.entries: list[dict[str, object]] = []

    def settle(self, transaction_id: str, amount_cents: int) -> dict[str, object]:
        """Record a settled transaction and return the entry."""
        entry = {
            "transaction_id": transaction_id,
            "amount_cents": amount_cents,
            "settled_at": int(time.time()),
        }
        self.entries.append(entry)
        return entry

    def balance_cents(self) -> int:
        """Sum of all settled amounts."""
        return sum(int(entry["amount_cents"]) for entry in self.entries)
