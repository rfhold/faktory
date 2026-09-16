from __future__ import annotations

import cadquery as cq

from interface_spec import BenchInterface


def make_leg(spec: BenchInterface) -> cq.Workplane:
    return cq.Workplane("XY").box(
        spec.leg_width,
        spec.leg_depth,
        spec.leg_height,
        centered=(True, True, False),
    )


def make_base(spec: BenchInterface) -> cq.Workplane:
    base = cq.Workplane("XY").box(
        spec.bench_length,
        spec.bench_depth,
        spec.base_thickness,
        centered=(True, True, False),
    )
    for x, y in spec.leg_centers:
        slot = (
            cq.Workplane("XY")
            .box(
                spec.slot_width,
                spec.slot_depth,
                spec.base_thickness,
                centered=(True, True, False),
            )
            .translate((x, y, 0))
        )
        base = base.cut(slot)
    return base


def make_router_template(spec: BenchInterface) -> cq.Workplane:
    width = spec.template_opening_width + 2 * spec.template_border
    depth = spec.template_opening_depth + 2 * spec.template_border
    plate = cq.Workplane("XY").box(
        width,
        depth,
        spec.template_thickness,
        centered=(True, True, False),
    )
    opening = cq.Workplane("XY").box(
        spec.template_opening_width,
        spec.template_opening_depth,
        spec.template_thickness,
        centered=(True, True, False),
    )
    return plate.cut(opening)


def make_bench(
    spec: BenchInterface, base: cq.Workplane, leg: cq.Workplane
) -> cq.Assembly:
    assembly = cq.Assembly()
    base_height = spec.leg_height - spec.leg_insertion
    assembly.add(base, name="base", loc=cq.Location((0, 0, base_height)))
    for index, (x, y) in enumerate(spec.leg_centers, start=1):
        assembly.add(leg, name=f"leg-{index}", loc=cq.Location((x, y, 0)))
    return assembly
