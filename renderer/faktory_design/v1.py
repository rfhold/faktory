from __future__ import annotations

import re
from collections.abc import Iterable
from typing import Final

_OUTPUT_ID: Final = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
_ROLES: Final = frozenset(("assembly", "part", "tool"))


class Output:
    __slots__ = ("geometry", "output_id", "primary", "role")

    def __init__(
        self,
        output_id: str,
        role: str,
        geometry: object,
        primary: bool = False,
    ) -> None:
        if not isinstance(output_id, str):
            raise TypeError("output_id must be a string")
        if len(output_id.encode("ascii", errors="ignore")) != len(output_id):
            raise ValueError("output_id must contain only ASCII characters")
        if len(output_id) > 64 or _OUTPUT_ID.fullmatch(output_id) is None:
            raise ValueError("invalid output_id")
        if not isinstance(role, str):
            raise TypeError("role must be a string")
        if role not in _ROLES:
            raise ValueError("invalid role")
        if not isinstance(primary, bool):
            raise TypeError("primary must be a boolean")
        self.output_id = output_id
        self.role = role
        self.geometry = geometry
        self.primary = primary


class Design:
    __slots__ = ("outputs",)

    def __init__(self, outputs: Iterable[Output]) -> None:
        if isinstance(outputs, (str, bytes)):
            raise TypeError("outputs must be an iterable of Output values")
        try:
            values = tuple(outputs)
        except TypeError as error:
            raise TypeError("outputs must be an iterable of Output values") from error
        if not 1 <= len(values) <= 64:
            raise ValueError("a design must contain 1 through 64 outputs")
        if any(type(output) is not Output for output in values):
            raise TypeError("outputs must contain only Output values")
        if len({output.output_id for output in values}) != len(values):
            raise ValueError("output_id values must be unique")
        if sum(output.primary for output in values) != 1:
            raise ValueError("a design must have exactly one primary output")
        self.outputs = values


__all__ = ["Design", "Output"]
