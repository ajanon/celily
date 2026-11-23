"""The backend, as the harness sees it: what celily created there, and the
user's own CLI to check for it and delete it.

What celily created comes from its log, the same whatever the backend:
objects are plain data, and each Backend knows how to find and delete
every Kind.
"""

import dataclasses
import enum
from abc import ABC, abstractmethod


class Kind(enum.Enum):
    INSTANCE = "instance"
    BRIDGE = "bridge"
    ACL = "acl"


@dataclasses.dataclass(frozen=True)
class Created:
    """Something celily created on the backend."""

    kind: Kind
    name: str

    def __str__(self):
        return f"{self.kind.value} {self.name}"


class Backend(ABC):
    @abstractmethod
    def config(self) -> dict:
        """The `[backend]` table pointing celily at this backend."""

    @abstractmethod
    def exists(self, obj: Created) -> bool:
        """Raises if it cannot tell."""

    @abstractmethod
    def delete(self, obj: Created) -> None:
        """Best effort: reports a failure, does not raise."""
