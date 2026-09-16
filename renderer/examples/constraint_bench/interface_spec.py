from __future__ import annotations

import math
from dataclasses import dataclass, fields


@dataclass(frozen=True)
class BenchInterface:
    bench_length: float = 60.0
    bench_depth: float = 28.0
    base_thickness: float = 4.0
    leg_width: float = 5.0
    leg_depth: float = 5.0
    leg_height: float = 20.0
    leg_insertion: float = 2.0
    slot_edge_inset_x: float = 7.0
    slot_edge_inset_y: float = 7.0
    fit_clearance: float = 0.4
    guide_bushing_offset: float = 1.5
    template_border: float = 5.0
    template_thickness: float = 3.0

    def __post_init__(self) -> None:
        for field in fields(self):
            value = getattr(self, field.name)
            if type(value) not in (int, float) or not math.isfinite(value) or value <= 0:
                raise ValueError(f"{field.name} must be finite and positive")
        if self.slot_width <= self.leg_width or self.slot_depth <= self.leg_depth:
            raise ValueError("base slots must include positive fit clearance")
        if self.fit_clearance >= min(self.leg_width, self.leg_depth):
            raise ValueError("fit_clearance must be smaller than the leg cross-section")
        if (
            self.template_opening_width <= self.slot_width
            or self.template_opening_depth <= self.slot_depth
        ):
            raise ValueError("template opening must include the guide-bushing offset")
        if self.leg_insertion >= min(self.base_thickness, self.leg_height):
            raise ValueError("leg_insertion must be smaller than the base and leg heights")
        if self.slot_edge_inset_x >= self.bench_length / 2:
            raise ValueError("x edge inset must stay within half the base length")
        if self.slot_edge_inset_y >= self.bench_depth / 2:
            raise ValueError("y edge inset must stay within half the base depth")
        if self.slot_width / 2 >= self.slot_edge_inset_x:
            raise ValueError("slot width does not fit within the x edge inset")
        if self.slot_depth / 2 >= self.slot_edge_inset_y:
            raise ValueError("slot depth does not fit within the y edge inset")
        if self.bench_length / 2 - self.slot_edge_inset_x <= self.slot_width / 2:
            raise ValueError("opposing slots overlap along the base length")
        if self.bench_depth / 2 - self.slot_edge_inset_y <= self.slot_depth / 2:
            raise ValueError("opposing slots overlap along the base depth")

    @property
    def slot_width(self) -> float:
        return self.leg_width + self.fit_clearance

    @property
    def slot_depth(self) -> float:
        return self.leg_depth + self.fit_clearance

    @property
    def template_opening_width(self) -> float:
        return self.slot_width + 2 * self.guide_bushing_offset

    @property
    def template_opening_depth(self) -> float:
        return self.slot_depth + 2 * self.guide_bushing_offset

    @property
    def leg_centers(self) -> tuple[tuple[float, float], ...]:
        x = self.bench_length / 2 - self.slot_edge_inset_x
        y = self.bench_depth / 2 - self.slot_edge_inset_y
        return ((-x, -y), (-x, y), (x, -y), (x, y))


INTERFACE = BenchInterface()
