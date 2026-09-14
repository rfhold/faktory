import cadquery as cq

result = cq.Workplane("XY").box(20, 20, 10).edges("|Z").fillet(2)
