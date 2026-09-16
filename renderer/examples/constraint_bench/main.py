from faktory_design.v1 import Design, Output

from geometry import make_base, make_bench, make_leg, make_router_template
from interface_spec import INTERFACE

leg = make_leg(INTERFACE)
base = make_base(INTERFACE)
router_template = make_router_template(INTERFACE)
bench = make_bench(INTERFACE, base, leg)

result = Design(
    outputs=(
        Output("bench", "assembly", bench, primary=True),
        Output("base", "part", base),
        Output("leg", "part", leg),
        Output("router-template", "tool", router_template),
    )
)
