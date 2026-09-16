from __future__ import annotations

import importlib.util
import json
import re
import struct
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path
from unittest import mock

from faktory_design.v1 import Design, Output
from renderer.worker import (
    MAX_BUNDLE_BYTES,
    MAX_GLB_BYTES,
    PROJECTIONS,
    _bundle_within_limit,
    _regular_file_within_limit,
    _validate_facts,
    _validate_glb,
    _validate_svg,
    render,
)

SVG_PATH_POINT = re.compile(r"[ML]([^, ]+),([^ ]+)")


def make_glb(json_payload: bytes, bin_payload: bytes | None = None) -> bytes:
    chunks = struct.pack("<II", len(json_payload), 0x4E4F534A) + json_payload
    if bin_payload is not None:
        chunks += struct.pack("<II", len(bin_payload), 0x004E4942) + bin_payload
    return struct.pack("<4sII", b"glTF", 2, 12 + len(chunks)) + chunks


def set_declared_glb_size(content: bytes) -> bytes:
    return content[:8] + struct.pack("<I", len(content)) + content[12:]


def svg_path_points(content: str) -> list[tuple[float, float]]:
    return [
        (float(x), float(y)) for x, y in SVG_PATH_POINT.findall(content)
    ]


class RendererTests(unittest.TestCase):
    def bundle_root(self, root: Path) -> Path:
        return root / "bundle"

    def render_paths(self, root: Path) -> tuple[Path, Path, Path]:
        output = self.bundle_root(root) / "outputs" / "primary"
        return output / "model.glb", output / "preview.svg", output / "facts.json"

    def projection_paths(self, root: Path) -> tuple[Path, ...]:
        projection_root = self.bundle_root(root) / "outputs" / "primary" / "projections"
        return tuple(projection_root / f"{name}.svg" for name, _, _ in PROJECTIONS)

    def render_model(
        self, source: Path, glb: Path, svg: Path, facts: Path
    ) -> str | None:
        dependency_root = source.parent / "dependencies"
        dependency_root.mkdir(exist_ok=True)
        self.write_dependency_root(dependency_root)
        return render(
            source.parent,
            Path(source.name),
            dependency_root,
            self.bundle_root(source.parent),
        )

    def render_project(
        self,
        project_root: Path,
        entrypoint: str,
        dependency_root: Path,
        output_root: Path,
    ) -> str | None:
        if not any(dependency_root.iterdir()):
            self.write_dependency_root(dependency_root)
        return render(
            project_root,
            Path(entrypoint),
            dependency_root,
            self.bundle_root(output_root),
        )

    @staticmethod
    def write_dependency_root(
        dependency_root: Path,
        nodes: list[dict[str, object]] | None = None,
        package_files: dict[str, dict[str, str]] | None = None,
        root_model_id: str = "fixture",
    ) -> None:
        if nodes is None:
            nodes = [
                {
                    "model_id": root_model_id,
                    "package": f"faktory_models.m_{root_model_id.replace('-', '_')}",
                    "project_revision": "0" * 64,
                    "dependencies": [],
                }
            ]
        namespace = dependency_root / "faktory_models"
        namespace.mkdir(parents=True, exist_ok=True)
        (namespace / "__init__.py").write_text("", encoding="utf-8")
        for node in nodes:
            model_id = str(node["model_id"])
            package = namespace / f"m_{model_id.replace('-', '_')}"
            package.mkdir()
            files = (package_files or {}).get(model_id, {"__init__.py": ""})
            for relative, content in files.items():
                path = package / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
        manifest = {
            "format": "faktory-model-dependencies-v1",
            "root_model_id": root_model_id,
            "nodes": nodes,
        }
        (dependency_root / "dependencies.json").write_text(
            json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8"
        )

    @staticmethod
    def write_mock_output(
        output: Output, output_root: Path
    ) -> tuple[None, dict[str, object]]:
        facts: dict[str, object] = {
            "volume_cubic_millimeters": 1.0,
            "size_millimeters": {"x": 1.0, "y": 1.0, "z": 1.0},
        }
        projection_root = output_root / "projections"
        projection_root.mkdir(parents=True)
        (output_root / "model.glb").write_bytes(b"glTF")
        (output_root / "preview.svg").write_text("<svg></svg>", encoding="utf-8")
        (output_root / "facts.json").write_text("{}\n", encoding="utf-8")
        for name, _, _ in PROJECTIONS:
            (projection_root / f"{name}.svg").write_text(
                "<svg></svg>", encoding="utf-8"
            )
        return None, facts

    def test_supported_result_types_produce_all_outputs(self) -> None:
        sources = {
            "workplane": (
                "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 2, 3)\n",
                "part",
            ),
            "shape": (
                "import cadquery as cq\nresult = cq.Workplane('XY').sphere(2).val()\n",
                "part",
            ),
            "assembly": (
                "import cadquery as cq\n"
                "result = cq.Assembly(cq.Workplane('XY').cylinder(2, 1))\n",
                "assembly",
            ),
        }
        for name, (source_text, expected_role) in sources.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                glb, svg, facts = self.render_paths(root)
                source.write_text(source_text, encoding="utf-8")

                self.assertIsNone(self.render_model(source, glb, svg, facts))
                self.assertTrue(_validate_glb(glb))
                self.assertTrue(_validate_svg(svg))
                self.assertTrue(_validate_facts(facts))
                self.assertTrue(
                    all(_validate_svg(path) for path in self.projection_paths(root))
                )
                self.assertEqual(glb.read_bytes()[:4], b"glTF")
                manifest = json.loads(
                    (self.bundle_root(root) / "outputs.json").read_text(encoding="utf-8")
                )
                self.assertEqual(
                    manifest["outputs"][0] | {"facts": None},
                    {
                        "output_id": "primary",
                        "role": expected_role,
                        "primary": True,
                        "facts": None,
                    },
                )

    def test_design_api_validates_metadata_without_inspecting_geometry(self) -> None:
        geometry = object()
        output = Output("fixture-tool", "tool", geometry, primary=True)
        design = Design(outputs=(output,))
        self.assertIs(output.geometry, geometry)
        self.assertEqual(design.outputs, (output,))

        invalid_outputs = (
            lambda: Output("Uppercase", "part", geometry, primary=True),
            lambda: Output("has--gap", "part", geometry, primary=True),
            lambda: Output("a" * 65, "part", geometry, primary=True),
            lambda: Output("part", "fixture", geometry, primary=True),
            lambda: Output("part", "part", geometry, primary=1),
            lambda: Output("part", "part", geometry, unknown=True),
        )
        for make_output in invalid_outputs:
            with self.subTest(make_output=make_output):
                with self.assertRaises((TypeError, ValueError)):
                    make_output()

        with self.assertRaises(ValueError):
            Design(())
        with self.assertRaises(ValueError):
            Design((output, Output("second", "part", geometry, primary=True)))
        with self.assertRaises(ValueError):
            Design((output, Output("fixture-tool", "part", geometry)))
        with self.assertRaises(ValueError):
            Design(
                tuple(
                    Output(f"part-{index}", "part", geometry, primary=index == 0)
                    for index in range(65)
                )
            )
        with self.assertRaises(TypeError):
            Design((object(),))
        with self.assertRaises(TypeError):
            Design(outputs=(output,), unknown=True)

    def test_multipart_design_produces_ordered_complete_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\n"
                "from faktory_design.v1 import Design, Output\n"
                "result = Design(outputs=(\n"
                "    Output('assembly', 'assembly', cq.Assembly(cq.Workplane('XY').box(3, 2, 1)), primary=True),\n"
                "    Output('bracket', 'part', cq.Workplane('XY').box(1, 2, 3)),\n"
                "    Output('drill-guide', 'tool', cq.Workplane('XY').cylinder(2, 1)),\n"
                "))\n",
                encoding="utf-8",
            )

            self.assertIsNone(self.render_model(source, *self.render_paths(root)))
            bundle = self.bundle_root(root)
            manifest_content = (bundle / "outputs.json").read_text(encoding="utf-8")
            self.assertTrue(manifest_content.endswith("\n"))
            self.assertNotIn(" ", manifest_content)
            manifest = json.loads(manifest_content)
            self.assertEqual(manifest["format"], "faktory-outputs-v1")
            self.assertEqual(
                [item["output_id"] for item in manifest["outputs"]],
                ["assembly", "bracket", "drill-guide"],
            )
            self.assertEqual(
                [item["role"] for item in manifest["outputs"]],
                ["assembly", "part", "tool"],
            )
            self.assertEqual(
                [item["primary"] for item in manifest["outputs"]],
                [True, False, False],
            )
            for summary in manifest["outputs"]:
                output_root = bundle / "outputs" / summary["output_id"]
                self.assertTrue(_validate_glb(output_root / "model.glb"))
                self.assertTrue(_validate_svg(output_root / "preview.svg"))
                self.assertTrue(_validate_facts(output_root / "facts.json"))
                self.assertEqual(
                    summary["facts"],
                    json.loads((output_root / "facts.json").read_text(encoding="utf-8")),
                )
                self.assertTrue(
                    all(
                        _validate_svg(output_root / "projections" / f"{name}.svg")
                        for name, _, _ in PROJECTIONS
                    )
                )

    def test_sixty_four_output_boundary_is_ordered_and_complete(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "from faktory_design.v1 import Design, Output\n"
                "result = Design(tuple(\n"
                "    Output(f'part-{index}', 'part', object(), primary=index == 0)\n"
                "    for index in range(64)\n"
                "))\n",
                encoding="utf-8",
            )

            with mock.patch(
                "renderer.worker._render_output", side_effect=self.write_mock_output
            ):
                self.assertIsNone(self.render_model(source, *self.render_paths(root)))

            manifest = json.loads(
                (self.bundle_root(root) / "outputs.json").read_text(encoding="utf-8")
            )
            self.assertEqual(
                [output["output_id"] for output in manifest["outputs"]],
                [f"part-{index}" for index in range(64)],
            )
            self.assertEqual(
                [output["primary"] for output in manifest["outputs"]],
                [True, *([False] * 63)],
            )
            self.assertEqual(
                len(list((self.bundle_root(root) / "outputs").iterdir())), 64
            )

    def test_constraint_bench_derives_parts_placements_and_tool(self) -> None:
        project_root = Path(__file__).parents[1] / "examples" / "constraint_bench"
        module_name = "test_constraint_bench_interface"
        module_spec = importlib.util.spec_from_file_location(
            module_name, project_root / "interface_spec.py"
        )
        self.assertIsNotNone(module_spec)
        assert module_spec is not None
        self.assertIsNotNone(module_spec.loader)
        interface_module = importlib.util.module_from_spec(module_spec)
        sys.modules[module_name] = interface_module
        try:
            assert module_spec.loader is not None
            module_spec.loader.exec_module(interface_module)
        finally:
            del sys.modules[module_name]

        interface = interface_module.INTERFACE
        self.assertAlmostEqual(
            interface.slot_width, interface.leg_width + interface.fit_clearance
        )
        self.assertAlmostEqual(
            interface.slot_depth, interface.leg_depth + interface.fit_clearance
        )
        self.assertAlmostEqual(
            interface.template_opening_width,
            interface.slot_width + 2 * interface.guide_bushing_offset,
        )
        self.assertAlmostEqual(
            interface.template_opening_depth,
            interface.slot_depth + 2 * interface.guide_bushing_offset,
        )
        with self.assertRaises(ValueError):
            interface_module.BenchInterface(fit_clearance=float("nan"))
        with self.assertRaises(ValueError):
            interface_module.BenchInterface(fit_clearance=interface.leg_width)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dependencies = root / "dependencies"
            bundle = root / "bundle"
            dependencies.mkdir()
            self.write_dependency_root(dependencies)

            self.assertIsNone(render(project_root, Path("main.py"), dependencies, bundle))
            manifest = json.loads((bundle / "outputs.json").read_text(encoding="utf-8"))
            self.assertEqual(
                [
                    (output["output_id"], output["role"], output["primary"])
                    for output in manifest["outputs"]
                ],
                [
                    ("bench", "assembly", True),
                    ("base", "part", False),
                    ("leg", "part", False),
                    ("router-template", "tool", False),
                ],
            )
            facts = {
                output["output_id"]: output["facts"] for output in manifest["outputs"]
            }
            leg_volume = (
                interface.leg_width * interface.leg_depth * interface.leg_height
            )
            base_volume = (
                interface.bench_length
                * interface.bench_depth
                * interface.base_thickness
                - len(interface.leg_centers)
                * interface.slot_width
                * interface.slot_depth
                * interface.base_thickness
            )
            template_width = (
                interface.template_opening_width + 2 * interface.template_border
            )
            template_depth = (
                interface.template_opening_depth + 2 * interface.template_border
            )
            template_volume = (
                template_width * template_depth
                - interface.template_opening_width * interface.template_opening_depth
            ) * interface.template_thickness
            self.assertAlmostEqual(
                facts["leg"]["volume_cubic_millimeters"], leg_volume
            )
            self.assertAlmostEqual(
                facts["base"]["volume_cubic_millimeters"], base_volume
            )
            self.assertAlmostEqual(
                facts["router-template"]["volume_cubic_millimeters"], template_volume
            )
            self.assertAlmostEqual(
                facts["bench"]["volume_cubic_millimeters"],
                base_volume + len(interface.leg_centers) * leg_volume,
            )
            self.assertEqual(
                facts["bench"]["size_millimeters"],
                {
                    "x": interface.bench_length,
                    "y": interface.bench_depth,
                    "z": interface.leg_height
                    + interface.base_thickness
                    - interface.leg_insertion,
                },
            )
            for output in manifest["outputs"]:
                output_root = bundle / "outputs" / output["output_id"]
                self.assertTrue(_validate_glb(output_root / "model.glb"))
                self.assertTrue(_validate_svg(output_root / "preview.svg"))
                self.assertTrue(_validate_facts(output_root / "facts.json"))
                self.assertTrue(
                    all(
                        _validate_svg(output_root / "projections" / f"{name}.svg")
                        for name, _, _ in PROJECTIONS
                    )
                )

    def test_model_package_can_import_design_api(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            dependencies = root / "dependencies"
            outputs = root / "outputs"
            project.mkdir()
            dependencies.mkdir()
            outputs.mkdir()
            (project / "faktory_model").mkdir()
            (project / "faktory_model" / "__init__.py").write_text(
                "from .helper import VALUE\n", encoding="utf-8"
            )
            (project / "faktory_model" / "helper.py").write_text(
                "VALUE = 1\n", encoding="utf-8"
            )
            self.write_dependency_root(
                dependencies,
                package_files={
                    "fixture": {
                        "__init__.py": (
                            "import cadquery as cq\n"
                            "from faktory_design.v1 import Design, Output\n"
                            "def make():\n"
                            "    return Design((Output('model-part', 'part', cq.Workplane('XY').box(1, 2, 3), primary=True),))\n"
                        )
                    }
                },
            )
            (project / "main.py").write_text(
                "from faktory_models.m_fixture import make\nresult = make()\n",
                encoding="utf-8",
            )

            self.assertIsNone(self.render_project(project, "main.py", dependencies, outputs))
            manifest = json.loads(
                (self.bundle_root(outputs) / "outputs.json").read_text(encoding="utf-8")
            )
            self.assertEqual(manifest["outputs"][0]["output_id"], "model-part")

    def test_project_cannot_shadow_design_api(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            libraries = root / "libraries"
            outputs = root / "outputs"
            (project / "faktory_design").mkdir(parents=True)
            libraries.mkdir()
            outputs.mkdir()
            (project / "faktory_design" / "__init__.py").write_text(
                "raise RuntimeError('shadowed API')\n", encoding="utf-8"
            )
            (project / "main.py").write_text(
                "import cadquery as cq\n"
                "from faktory_design.v1 import Design, Output\n"
                "result = Design((Output('part', 'part', cq.Workplane('XY').box(1, 1, 1), primary=True),))\n",
                encoding="utf-8",
            )

            self.assertIsNone(self.render_project(project, "main.py", libraries, outputs))

    def test_multipart_failure_removes_the_whole_temporary_bundle(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\n"
                "from faktory_design.v1 import Design, Output\n"
                "result = Design((\n"
                "    Output('good', 'part', cq.Workplane('XY').box(1, 1, 1), primary=True),\n"
                "    Output('bad', 'part', object()),\n"
                "))\n",
                encoding="utf-8",
            )

            self.assertEqual(
                self.render_model(source, *self.render_paths(root)), "unsupported_geometry"
            )
            self.assertFalse(self.bundle_root(root).exists())
            self.assertEqual(
                [path for path in root.iterdir() if path.name.startswith(".bundle.")], []
            )

    def test_design_like_and_malformed_design_values_are_rejected(self) -> None:
        cases = {
            "unsupported_result": "result = type('DesignLike', (), {'outputs': ()})()\n",
            "invalid_design": (
                "from faktory_design.v1 import Design\n"
                "result = object.__new__(Design)\n"
                "result.outputs = ()\n"
            ),
        }
        for expected, source_text in cases.items():
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(source_text, encoding="utf-8")
                self.assertEqual(
                    self.render_model(source, *self.render_paths(root)), expected
                )
                self.assertFalse(self.bundle_root(root).exists())

    def test_glb_size_limit_accepts_exact_boundary_and_rejects_adjacent_size(self) -> None:
        path = Path("model.glb")
        with mock.patch("renderer.worker._regular_file_size", return_value=MAX_GLB_BYTES):
            self.assertTrue(_regular_file_within_limit(path, MAX_GLB_BYTES))
        with mock.patch(
            "renderer.worker._regular_file_size", return_value=MAX_GLB_BYTES + 1
        ):
            self.assertFalse(_regular_file_within_limit(path, MAX_GLB_BYTES))
        self.assertEqual(MAX_GLB_BYTES, 64 * 1024 * 1024)

    def test_glb_exact_limit_publishes_and_adjacent_size_rejects_bundle(self) -> None:
        for reported_glb_size, expected_error in (
            (MAX_GLB_BYTES, None),
            (MAX_GLB_BYTES + 1, "glb_too_large"),
        ):
            with self.subTest(
                reported_glb_size=reported_glb_size
            ), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(
                    "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 1, 1)\n",
                    encoding="utf-8",
                )

                def reported_size(path: Path) -> int | None:
                    if path.name == "model.glb":
                        return reported_glb_size
                    return path.stat(follow_symlinks=False).st_size

                with mock.patch(
                    "renderer.worker._regular_file_size", side_effect=reported_size
                ):
                    self.assertEqual(
                        self.render_model(source, *self.render_paths(root)), expected_error
                    )
                self.assertEqual(self.bundle_root(root).exists(), expected_error is None)
        self.assertEqual(MAX_GLB_BYTES, 64 * 1024 * 1024)

    def test_bundle_size_limit_accepts_exact_boundary_and_rejects_adjacent_size(
        self,
    ) -> None:
        output = Output("primary", "part", object(), primary=True)
        expected_files = 1 + 10
        exact_sizes = iter([MAX_BUNDLE_BYTES, *([0] * (expected_files - 1))])
        with mock.patch(
            "renderer.worker._regular_file_size", side_effect=lambda _: next(exact_sizes)
        ):
            self.assertTrue(_bundle_within_limit(Path("bundle"), (output,)))

        adjacent_sizes = iter(
            [MAX_BUNDLE_BYTES + 1, *([0] * (expected_files - 1))]
        )
        with mock.patch(
            "renderer.worker._regular_file_size",
            side_effect=lambda _: next(adjacent_sizes),
        ):
            self.assertFalse(_bundle_within_limit(Path("bundle"), (output,)))
        self.assertEqual(MAX_BUNDLE_BYTES, 192 * 1024 * 1024)

    def test_bundle_exact_limit_publishes_and_adjacent_size_cleans_temporary(self) -> None:
        for reported_manifest_size, expected_error in (
            (MAX_BUNDLE_BYTES, None),
            (MAX_BUNDLE_BYTES + 1, "bundle_too_large"),
        ):
            with self.subTest(
                reported_manifest_size=reported_manifest_size
            ), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(
                    "from faktory_design.v1 import Design, Output\n"
                    "result = Design((Output('primary', 'part', object(), primary=True),))\n",
                    encoding="utf-8",
                )

                def reported_size(path: Path) -> int | None:
                    return reported_manifest_size if path.name == "outputs.json" else 0

                with mock.patch(
                    "renderer.worker._render_output", side_effect=self.write_mock_output
                ), mock.patch(
                    "renderer.worker._regular_file_size", side_effect=reported_size
                ):
                    self.assertEqual(
                        self.render_model(source, *self.render_paths(root)),
                        expected_error,
                    )

                self.assertEqual(self.bundle_root(root).exists(), expected_error is None)
                self.assertEqual(
                    [path for path in root.iterdir() if path.name.startswith(".bundle.")],
                    [],
                )

    def test_box_facts_are_exact(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\nresult = cq.Workplane('XY').box(2, 3, 5)\n",
                encoding="utf-8",
            )
            glb, svg, facts_path = self.render_paths(root)

            self.assertIsNone(self.render_model(source, glb, svg, facts_path))
            self.assertEqual(
                json.loads(facts_path.read_text(encoding="utf-8")),
                {
                    "volume_cubic_millimeters": 30.0,
                    "size_millimeters": {"x": 2.0, "y": 3.0, "z": 5.0},
                },
            )

    def test_translated_nested_assembly_facts_preserve_placements(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\n"
                "child = cq.Assembly(cq.Workplane('XY').box(2, 2, 2))\n"
                "child.add(cq.Workplane('XY').box(1, 1, 1), loc=cq.Location((4, 0, 0)))\n"
                "result = cq.Assembly()\n"
                "result.add(child, loc=cq.Location((0, 3, 5)))\n",
                encoding="utf-8",
            )
            glb, svg, facts_path = self.render_paths(root)

            self.assertIsNone(self.render_model(source, glb, svg, facts_path))
            facts = json.loads(facts_path.read_text(encoding="utf-8"))
            self.assertAlmostEqual(facts["volume_cubic_millimeters"], 9.0)
            self.assertEqual(facts["size_millimeters"], {"x": 5.5, "y": 2.0, "z": 2.0})

    def test_overlapping_assembly_components_contribute_independent_volume(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\n"
                "result = cq.Assembly(cq.Workplane('XY').box(2, 2, 2))\n"
                "result.add(cq.Workplane('XY').box(2, 2, 2), "
                "loc=cq.Location((1, 0, 0)))\n",
                encoding="utf-8",
            )
            glb, svg, facts_path = self.render_paths(root)

            self.assertIsNone(self.render_model(source, glb, svg, facts_path))
            facts = json.loads(facts_path.read_text(encoding="utf-8"))
            self.assertAlmostEqual(facts["volume_cubic_millimeters"], 16.0)
            self.assertEqual(facts["size_millimeters"], {"x": 3.0, "y": 2.0, "z": 2.0})

    def test_svg_is_deterministic_fixed_size_and_parseable(self) -> None:
        rendered = []
        for _ in range(2):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(
                    "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 2, 3)\n",
                    encoding="utf-8",
                )
                glb, svg, facts = self.render_paths(root)
                self.assertIsNone(self.render_model(source, glb, svg, facts))
                rendered.append(
                    (
                        svg.read_text(encoding="utf-8"),
                        tuple(
                            path.read_text(encoding="utf-8")
                            for path in self.projection_paths(root)
                        ),
                    )
                )

        self.assertEqual(rendered[0], rendered[1])
        preview, projections = rendered[0]
        self.assertEqual(preview, projections[0])
        for projection in projections:
            root = ET.fromstring(projection)
            self.assertEqual(root.tag.rsplit("}", 1)[-1], "svg")
            self.assertEqual(root.attrib["width"], "640.0")
            self.assertEqual(root.attrib["height"], "480.0")
            self.assertNotIn("x-axis", projection)
            self.assertNotIn("hidden-lines", projection)

    def test_projection_names_and_directions_are_fixed_and_complete(self) -> None:
        expected = (
            ("isometric", (1, -1, 1), (1, 1, 0)),
            ("front", (0, -1, 0), (1, 0, 0)),
            ("back", (0, 1, 0), (-1, 0, 0)),
            ("left", (-1, 0, 0), (0, -1, 0)),
            ("right", (1, 0, 0), (0, 1, 0)),
            ("top", (0, 0, 1), (1, 0, 0)),
            ("bottom", (0, 0, -1), (1, 0, 0)),
        )
        self.assertEqual(PROJECTIONS, expected)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 2, 3)\n",
                encoding="utf-8",
            )
            glb, svg, facts = self.render_paths(root)
            generated = '<svg width="640.0" height="480.0"></svg>'
            with mock.patch("renderer.worker._get_svg", return_value=generated) as get_svg:
                self.assertIsNone(self.render_model(source, glb, svg, facts))

            self.assertEqual(get_svg.call_count, 7)
            self.assertEqual(
                [call.args[1:] for call in get_svg.call_args_list],
                [(direction, screen_right) for _, direction, screen_right in expected],
            )
            self.assertEqual(svg.read_text(encoding="utf-8"), generated)

    def test_principal_views_have_conventional_roll_and_handedness(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text(
                "import cadquery as cq\n"
                "result = cq.Assembly(\n"
                "    cq.Workplane('XY').box(12, 8, 6, centered=(True, True, True))\n"
                ")\n"
                "result.add(cq.Workplane('XY').box(3, 2, 2, "
                "centered=(True, True, True)).translate((7.5, 0, 4)))\n"
                "result.add(cq.Workplane('XY').box(2, 3, 2, "
                "centered=(True, True, True)).translate((0, 5.5, 4)))\n"
                "result.add(cq.Workplane('XY').box(3, 3, 2, "
                "centered=(True, True, True)).translate((7.5, 5.5, 0)))\n",
                encoding="utf-8",
            )
            glb, svg, facts = self.render_paths(root)

            self.assertIsNone(self.render_model(source, glb, svg, facts))
            views = {
                path.stem: svg_path_points(path.read_text(encoding="utf-8"))
                for path in self.projection_paths(root)
            }

            expected_marker_corners = {
                "front": (9.0, 5.0),
                "back": (-9.0, 5.0),
                "left": (-7.0, 5.0),
                "right": (7.0, 5.0),
                "top": (9.0, 7.0),
                "bottom": (9.0, -7.0),
            }
            for name, expected_corner in expected_marker_corners.items():
                with self.subTest(view=name):
                    points = views[name]
                    xs = [point[0] for point in points]
                    ys = [point[1] for point in points]
                    self.assertGreater(max(xs) - min(xs), 1.2 * (max(ys) - min(ys)))
                    self.assertTrue(
                        any(
                            abs(x - expected_corner[0]) < 1e-9
                            and abs(y - expected_corner[1]) < 1e-9
                            for x, y in points
                        )
                    )

    def test_cli_suppresses_source_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            glb, svg, facts = self.render_paths(root)
            source.write_text(
                "import cadquery as cq\nprint('do not leak')\n"
                "result = cq.Workplane('XY').box(1, 1, 1)\n",
                encoding="utf-8",
            )
            dependencies = root / "dependencies"
            dependencies.mkdir()
            self.write_dependency_root(dependencies)

            completed = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "renderer",
                    str(root),
                    source.name,
                    str(dependencies),
                    str(self.bundle_root(root)),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(completed.stdout, "")
            self.assertEqual(completed.stderr, "")
            self.assertTrue(
                all(
                    path.exists()
                    for path in (glb, svg, facts, *self.projection_paths(root))
                )
            )

    def test_cli_reports_only_a_stable_error_category(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            outputs = self.render_paths(root)
            source.write_text(
                "print('source text')\nraise RuntimeError('secret details')\n",
                encoding="utf-8",
            )
            dependencies = root / "dependencies"
            dependencies.mkdir()
            self.write_dependency_root(dependencies)

            completed = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "renderer",
                    str(root),
                    source.name,
                    str(dependencies),
                    str(self.bundle_root(root)),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(completed.returncode, 1)
            self.assertEqual(completed.stdout, "")
            self.assertEqual(
                completed.stderr,
                "renderer_error=source_execution_failed\n",
            )
            self.assertFalse(
                self.bundle_root(root).exists()
            )

    def test_cli_rejects_wrong_argument_count(self) -> None:
        completed = subprocess.run(
            [sys.executable, "-m", "renderer", "project", "main.py"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 2)
        self.assertEqual(completed.stdout, "")
        self.assertEqual(completed.stderr, "renderer_error=invalid_arguments\n")

    def test_source_failures_have_stable_categories(self) -> None:
        cases = {
            "missing_result": "value = 1\n",
            "unsupported_result": "result = object()\n",
            "source_execution_failed": "raise RuntimeError('secret details')\n",
        }
        for expected, source_text in cases.items():
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(source_text, encoding="utf-8")
                self.assertEqual(
                    self.render_model(source, *self.render_paths(root)), expected
                )

    def test_project_sibling_and_nested_package_imports(self) -> None:
        cases = {
            "main.py": {
                "main.py": "from shape import make\nresult = make()\n",
                "shape.py": (
                    "import cadquery as cq\n"
                    "def make():\n    return cq.Workplane('XY').box(2, 3, 4)\n"
                ),
            },
            "parts/main.py": {
                "parts/__init__.py": "",
                "parts/main.py": "from .shape import make\nresult = make()\n",
                "parts/shape.py": (
                    "import cadquery as cq\n"
                    "def make():\n    return cq.Workplane('XY').box(2, 3, 4)\n"
                ),
            },
        }
        for entrypoint, files in cases.items():
            with self.subTest(entrypoint=entrypoint), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                libraries = root / "libraries"
                outputs = root / "outputs"
                project.mkdir()
                libraries.mkdir()
                outputs.mkdir()
                for relative, content in files.items():
                    path = project / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(content, encoding="utf-8")

                self.assertIsNone(
                    self.render_project(project, entrypoint, libraries, outputs)
                )
                facts = json.loads(self.render_paths(outputs)[2].read_text(encoding="utf-8"))
                self.assertEqual(facts["size_millimeters"], {"x": 2.0, "y": 3.0, "z": 4.0})

    def test_dependency_root_precedes_project_namespace_collision(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            dependencies = root / "dependencies"
            outputs = root / "outputs"
            (project / "faktory_models" / "m_fixture").mkdir(parents=True)
            dependencies.mkdir()
            outputs.mkdir()
            (project / "faktory_models" / "m_fixture" / "__init__.py").write_text(
                "raise RuntimeError('project collision executed')\n", encoding="utf-8"
            )
            self.write_dependency_root(
                dependencies,
                package_files={
                    "fixture": {
                        "__init__.py": (
                            "import cadquery as cq\n"
                            "def make():\n    return cq.Workplane('XY').box(2, 3, 4)\n"
                        )
                    }
                },
            )
            (project / "main.py").write_text(
                "from faktory_models.m_fixture import make\nresult = make()\n",
                encoding="utf-8",
            )

            self.assertIsNone(
                self.render_project(project, "main.py", dependencies, outputs)
            )
            facts = json.loads(self.render_paths(outputs)[2].read_text(encoding="utf-8"))
            self.assertEqual(facts["size_millimeters"], {"x": 2.0, "y": 3.0, "z": 4.0})

    def test_all_project_python_files_are_parsed_before_execution(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            dependencies = root / "dependencies"
            outputs = root / "outputs"
            project.mkdir()
            dependencies.mkdir()
            outputs.mkdir()
            self.write_dependency_root(dependencies)
            (project / "main.py").write_text("result = None\n", encoding="utf-8")
            (project / "unused.py").write_text("PRIVATE_SOURCE = (\n", encoding="utf-8")

            self.assertEqual(
                self.render_project(project, "main.py", dependencies, outputs),
                "invalid_project_source",
            )
            self.assertFalse(any(outputs.iterdir()))

    def test_packages_allow_own_and_direct_dependency_imports(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            dependencies = root / "dependencies"
            outputs = root / "outputs"
            project.mkdir()
            dependencies.mkdir()
            outputs.mkdir()
            nodes = [
                {"model_id": "fixture", "package": "faktory_models.m_fixture", "project_revision": "0" * 64, "dependencies": ["gears"]},
                {"model_id": "gears", "package": "faktory_models.m_gears", "project_revision": "1" * 64, "dependencies": []},
            ]
            self.write_dependency_root(
                dependencies,
                nodes=nodes,
                package_files={
                    "fixture": {
                        "__init__.py": (
                            "from .helper import make as relative_make\n"
                            "from faktory_models.m_fixture.helper import make as absolute_make\n"
                            "assert relative_make is absolute_make\n"
                            "make = relative_make\n"
                        ),
                        "helper.py": "from faktory_models.m_gears import make\n",
                        "entrypoint.py": "raise RuntimeError('must not execute')\n",
                    },
                    "gears": {
                        "__init__.py": "import cadquery as cq\ndef make():\n    return cq.Workplane('XY').box(2, 3, 4)\n"
                    },
                },
            )
            (project / "main.py").write_text(
                "from faktory_models import m_fixture\nresult = m_fixture.make()\n",
                encoding="utf-8",
            )

            self.assertIsNone(
                self.render_project(project, "main.py", dependencies, outputs)
            )

    def test_project_and_packages_reject_undeclared_or_legacy_imports(self) -> None:
        cases = {
            "project_transitive": ("from faktory_models.m_wheels import make\nresult = make()\n", "", "invalid_project_source"),
            "package_transitive": ("result = None\n", "from faktory_models.m_wheels import make\n", "invalid_dependency_source"),
            "project_source_alias": ("from faktory_model import make\nresult = make()\n", "", "invalid_project_source"),
            "project_legacy": ("import faktory_shared.anything\nresult = None\n", "", "invalid_project_source"),
        }
        for name, (project_source, package_source, expected) in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                dependencies = root / "dependencies"
                outputs = root / "outputs"
                project.mkdir()
                dependencies.mkdir()
                outputs.mkdir()
                nodes = [
                    {"model_id": "fixture", "package": "faktory_models.m_fixture", "project_revision": "0" * 64, "dependencies": ["gears"]},
                    {"model_id": "gears", "package": "faktory_models.m_gears", "project_revision": "1" * 64, "dependencies": ["wheels"]},
                    {"model_id": "wheels", "package": "faktory_models.m_wheels", "project_revision": "2" * 64, "dependencies": []},
                ]
                self.write_dependency_root(
                    dependencies,
                    nodes=nodes,
                    package_files={
                        "fixture": {"__init__.py": package_source},
                        "gears": {"__init__.py": ""},
                        "wheels": {"__init__.py": ""},
                    },
                )
                (project / "main.py").write_text(project_source, encoding="utf-8")

                self.assertEqual(
                    self.render_project(project, "main.py", dependencies, outputs),
                    expected,
                )
                self.assertFalse(any(outputs.iterdir()))

    def test_invalid_dependency_syntax_has_safe_cli_error(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            dependencies = root / "dependencies"
            outputs = root / "outputs"
            project.mkdir()
            dependencies.mkdir()
            outputs.mkdir()
            self.write_dependency_root(
                dependencies,
                package_files={"fixture": {"__init__.py": "TOP_SECRET = (\n"}},
            )
            (project / "main.py").write_text("result = None\n", encoding="utf-8")

            completed = subprocess.run(
                [sys.executable, "-m", "renderer", str(project), "main.py", str(dependencies), str(self.bundle_root(outputs))],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(completed.returncode, 1)
            self.assertEqual(completed.stdout, "")
            self.assertEqual(completed.stderr, "renderer_error=invalid_dependency_source\n")
            self.assertNotIn("TOP_SECRET", completed.stderr)
            self.assertFalse(any(outputs.iterdir()))

    def test_manifest_rejects_noncanonical_and_invalid_identity_fields(self) -> None:
        mutations = {
            "extra_key": lambda value: value.update({"extra": True}),
            "bad_root": lambda value: value.update({"root_model_id": "Missing"}),
            "bad_package": lambda value: value["nodes"][0].update({"package": "faktory_models.fixture"}),
            "bad_revision": lambda value: value["nodes"][0].update({"project_revision": "A" * 64}),
            "duplicate_dependency": lambda value: value["nodes"][0].update({"dependencies": ["fixture", "fixture"]}),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                dependencies = root / "dependencies"
                outputs = root / "outputs"
                project.mkdir()
                dependencies.mkdir()
                outputs.mkdir()
                self.write_dependency_root(dependencies)
                manifest_path = dependencies / "dependencies.json"
                manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                mutate(manifest)
                manifest_path.write_text(
                    json.dumps(manifest, separators=(",", ":")) + "\n",
                    encoding="utf-8",
                )
                (project / "main.py").write_text("result = None\n", encoding="utf-8")
                self.assertEqual(
                    self.render_project(project, "main.py", dependencies, outputs),
                    "invalid_dependency_manifest",
                )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            dependencies = root / "dependencies"
            outputs = root / "outputs"
            project.mkdir()
            dependencies.mkdir()
            outputs.mkdir()
            self.write_dependency_root(dependencies)
            manifest_path = dependencies / "dependencies.json"
            manifest_path.write_text(
                json.dumps(json.loads(manifest_path.read_text(encoding="utf-8")), indent=2) + "\n",
                encoding="utf-8",
            )
            (project / "main.py").write_text("result = None\n", encoding="utf-8")
            self.assertEqual(
                self.render_project(project, "main.py", dependencies, outputs),
                "invalid_dependency_manifest",
            )

    def test_model_id_package_normalization_handles_digits_and_keywords(self) -> None:
        for model_id, package in (
            ("3-way-clamp", "faktory_models.m_3_way_clamp"),
            ("class", "faktory_models.m_class"),
        ):
            with self.subTest(model_id=model_id), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                dependencies = root / "dependencies"
                outputs = root / "outputs"
                project.mkdir()
                dependencies.mkdir()
                outputs.mkdir()
                nodes = [
                    {
                        "model_id": model_id,
                        "package": package,
                        "project_revision": "0" * 64,
                        "dependencies": [],
                    }
                ]
                self.write_dependency_root(
                    dependencies, nodes=nodes, root_model_id=model_id
                )
                (project / "main.py").write_text(
                    f"import {package}\nresult = None\n", encoding="utf-8"
                )
                self.assertEqual(
                    self.render_project(project, "main.py", dependencies, outputs),
                    "unsupported_result",
                )

    def test_dependency_count_exact_and_adjacent_limits(self) -> None:
        for dependency_count, valid in ((64, True), (65, False)):
            with self.subTest(dependency_count=dependency_count), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                dependencies = root / "dependencies"
                outputs = root / "outputs"
                project.mkdir()
                dependencies.mkdir()
                outputs.mkdir()
                dependency_ids = [f"model-{index:02d}" for index in range(dependency_count)]
                nodes = [
                    {"model_id": "fixture", "package": "faktory_models.m_fixture", "project_revision": "0" * 64, "dependencies": dependency_ids},
                    *[
                        {"model_id": model_id, "package": f"faktory_models.m_{model_id.replace('-', '_')}", "project_revision": f"{index + 1:064x}", "dependencies": []}
                        for index, model_id in enumerate(dependency_ids)
                    ],
                ]
                nodes.sort(key=lambda node: str(node["model_id"]))
                self.write_dependency_root(dependencies, nodes=nodes)
                (project / "main.py").write_text("result = None\n", encoding="utf-8")
                error = self.render_project(project, "main.py", dependencies, outputs)
                self.assertEqual(error, "unsupported_result" if valid else "invalid_dependency_manifest")

    def test_dependency_depth_exact_and_adjacent_limits(self) -> None:
        for depth, valid in ((8, True), (9, False)):
            with self.subTest(depth=depth), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                dependencies = root / "dependencies"
                outputs = root / "outputs"
                project.mkdir()
                dependencies.mkdir()
                outputs.mkdir()
                model_ids = [f"model-{index:02d}" for index in range(depth + 1)]
                nodes = [
                    {
                        "model_id": model_id,
                        "package": f"faktory_models.m_{model_id.replace('-', '_')}",
                        "project_revision": f"{index:064x}",
                        "dependencies": [model_ids[index + 1]] if index < depth else [],
                    }
                    for index, model_id in enumerate(model_ids)
                ]
                self.write_dependency_root(
                    dependencies, nodes=nodes, root_model_id=model_ids[0]
                )
                (project / "main.py").write_text("result = None\n", encoding="utf-8")
                error = self.render_project(project, "main.py", dependencies, outputs)
                self.assertEqual(error, "unsupported_result" if valid else "invalid_dependency_manifest")

    def test_dependency_graph_rejects_cycles_and_disconnected_nodes(self) -> None:
        cases = {
            "self_dependency": [
                {"model_id": "fixture", "package": "faktory_models.m_fixture", "project_revision": "0" * 64, "dependencies": ["fixture"]},
            ],
            "cycle": [
                {"model_id": "fixture", "package": "faktory_models.m_fixture", "project_revision": "0" * 64, "dependencies": ["other"]},
                {"model_id": "other", "package": "faktory_models.m_other", "project_revision": "1" * 64, "dependencies": ["fixture"]},
            ],
            "disconnected": [
                {"model_id": "fixture", "package": "faktory_models.m_fixture", "project_revision": "0" * 64, "dependencies": []},
                {"model_id": "other", "package": "faktory_models.m_other", "project_revision": "1" * 64, "dependencies": []},
            ],
        }
        for name, nodes in cases.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                dependencies = root / "dependencies"
                outputs = root / "outputs"
                project.mkdir()
                dependencies.mkdir()
                outputs.mkdir()
                self.write_dependency_root(dependencies, nodes=nodes)
                (project / "main.py").write_text("result = None\n", encoding="utf-8")
                self.assertEqual(
                    self.render_project(project, "main.py", dependencies, outputs),
                    "invalid_dependency_manifest",
                )

    def test_dependency_staging_and_source_size_are_strict(self) -> None:
        for name, stage in {
            "unexpected_namespace_file": lambda root: (root / "faktory_models" / "extra.py").write_text("", encoding="utf-8"),
            "unrepresented_package": lambda root: (root / "faktory_models" / "m_extra").mkdir(),
            "unexpected_root_path": lambda root: (root / "extra").mkdir(),
            "missing_init": lambda root: (root / "faktory_models" / "m_fixture" / "__init__.py").unlink(),
            "symlink": lambda root: (root / "faktory_models" / "m_fixture" / "link.py").symlink_to("__init__.py"),
        }.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                dependencies = root / "dependencies"
                outputs = root / "outputs"
                project.mkdir()
                dependencies.mkdir()
                outputs.mkdir()
                self.write_dependency_root(dependencies)
                stage(dependencies)
                (project / "main.py").write_text("result = None\n", encoding="utf-8")
                self.assertEqual(
                    self.render_project(project, "main.py", dependencies, outputs),
                    "invalid_dependency_source",
                )

        for limit, expected in ((1, "unsupported_result"), (0, "invalid_dependency_source")):
            with self.subTest(source_limit=limit), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                dependencies = root / "dependencies"
                outputs = root / "outputs"
                project.mkdir()
                dependencies.mkdir()
                outputs.mkdir()
                nodes = [
                    {"model_id": "fixture", "package": "faktory_models.m_fixture", "project_revision": "0" * 64, "dependencies": ["other"]},
                    {"model_id": "other", "package": "faktory_models.m_other", "project_revision": "1" * 64, "dependencies": []},
                ]
                self.write_dependency_root(
                    dependencies,
                    nodes=nodes,
                    package_files={"fixture": {"__init__.py": "# large root excluded\n"}, "other": {"__init__.py": "x"}},
                )
                (project / "main.py").write_text("result = None\n", encoding="utf-8")
                with mock.patch("renderer.worker.MAX_PACKAGE_SOURCE_BYTES", limit):
                    self.assertEqual(
                        self.render_project(project, "main.py", dependencies, outputs),
                        expected,
                    )

    def test_absolute_and_traversing_entrypoints_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            libraries = root / "libraries"
            outputs = root / "outputs"
            project.mkdir()
            libraries.mkdir()
            outputs.mkdir()
            source = project / "main.py"
            source.write_text("result = None\n", encoding="utf-8")
            for entrypoint in (str(source), "../project/main.py"):
                with self.subTest(entrypoint=entrypoint):
                    self.assertEqual(
                        self.render_project(project, entrypoint, libraries, outputs),
                        "invalid_entrypoint",
                    )
                    self.assertFalse(any(outputs.iterdir()))

    def test_repeated_project_execution_restores_globals_and_outputs_identically(self) -> None:
        rendered: list[tuple[bytes, ...]] = []
        original_cwd = Path.cwd()
        original_path = sys.path.copy()
        for _ in range(2):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                project = root / "project"
                libraries = root / "libraries"
                outputs = root / "outputs"
                project.mkdir()
                libraries.mkdir()
                outputs.mkdir()
                (project / "main.py").write_text(
                    "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 2, 3)\n",
                    encoding="utf-8",
                )
                self.assertIsNone(
                    self.render_project(project, "main.py", libraries, outputs)
                )
                rendered.append(
                    tuple(
                        path.read_bytes()
                        for path in (
                            *self.render_paths(outputs),
                            *self.projection_paths(outputs),
                        )
                    )
                )
                self.assertEqual(Path.cwd(), original_cwd)
                self.assertEqual(sys.path, original_path)
        self.assertEqual(rendered[0], rendered[1])

    def test_repeated_package_execution_clears_cache_and_restores_import_state(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "project"
            dependencies = root / "dependencies"
            first_outputs = root / "first"
            second_outputs = root / "second"
            project.mkdir()
            dependencies.mkdir()
            first_outputs.mkdir()
            second_outputs.mkdir()
            self.write_dependency_root(
                dependencies,
                package_files={
                    "fixture": {
                        "__init__.py": (
                            "import cadquery as cq\n"
                            "def make():\n    return cq.Workplane('XY').box(1, 2, 3)\n"
                        )
                    }
                },
            )
            (project / "main.py").write_text(
                "from faktory_models.m_fixture import make\nresult = make()\n",
                encoding="utf-8",
            )
            sentinel = mock.Mock()
            prior = sys.modules.get("faktory_models")
            sys.modules["faktory_models"] = sentinel
            original_path = sys.path.copy()
            original_importer_cache = sys.path_importer_cache.copy()
            original_dont_write_bytecode = sys.dont_write_bytecode
            try:
                self.assertIsNone(
                    self.render_project(project, "main.py", dependencies, first_outputs)
                )
                package = dependencies / "faktory_models" / "m_fixture" / "__init__.py"
                package.write_text(
                    "import cadquery as cq\n"
                    "def make():\n    return cq.Workplane('XY').box(4, 5, 6)\n",
                    encoding="utf-8",
                )
                self.assertIsNone(
                    self.render_project(project, "main.py", dependencies, second_outputs)
                )
                first = json.loads(self.render_paths(first_outputs)[2].read_text(encoding="utf-8"))
                second = json.loads(self.render_paths(second_outputs)[2].read_text(encoding="utf-8"))
                self.assertEqual(first["size_millimeters"], {"x": 1.0, "y": 2.0, "z": 3.0})
                self.assertEqual(second["size_millimeters"], {"x": 4.0, "y": 5.0, "z": 6.0})
                self.assertIs(sys.modules["faktory_models"], sentinel)
                self.assertEqual(sys.path, original_path)
                self.assertEqual(sys.path_importer_cache, original_importer_cache)
                self.assertEqual(sys.dont_write_bytecode, original_dont_write_bytecode)
            finally:
                if prior is None:
                    sys.modules.pop("faktory_models", None)
                else:
                    sys.modules["faktory_models"] = prior

    def test_invalid_utf8_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_bytes(b"result = '\xff'\n")
            self.assertEqual(
                self.render_model(source, *self.render_paths(root)), "invalid_utf8"
            )

    def test_invalid_paths_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "model.py"
            source.write_text("result = None\n", encoding="utf-8")
            libraries = root / "libraries"
            libraries.mkdir()
            existing_output = root / "existing"
            existing_output.mkdir()
            (existing_output / "stale").write_bytes(b"stale")
            dangling_output = root / "dangling"
            dangling_output.symlink_to(root / "missing")

            self.assertEqual(
                render(root, Path("missing.py"), libraries, root / "missing-output"),
                "invalid_entrypoint",
            )
            self.assertEqual(
                render(root, Path(source.name), libraries, source), "invalid_output_path"
            )
            self.assertEqual(
                render(root, Path(source.name), libraries, existing_output),
                "invalid_output_path",
            )
            self.assertEqual(
                render(root, Path(source.name), libraries, root / "missing" / "output"),
                "invalid_output_path",
            )
            self.assertEqual(
                render(root, Path(source.name), libraries, dangling_output),
                "invalid_output_path",
            )
            self.assertEqual((existing_output / "stale").read_bytes(), b"stale")

    def test_malformed_generated_outputs_are_removed(self) -> None:
        cases = {
            "invalid_facts": mock.patch("renderer.worker._validate_facts", return_value=False),
            "invalid_svg": mock.patch("renderer.worker._validate_svg", return_value=False),
            "invalid_glb": mock.patch("renderer.worker._validate_glb", return_value=False),
        }
        for expected, patcher in cases.items():
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "model.py"
                source.write_text(
                    "import cadquery as cq\nresult = cq.Workplane('XY').box(1, 1, 1)\n",
                    encoding="utf-8",
                )
                outputs = self.render_paths(root)
                with patcher:
                    self.assertEqual(self.render_model(source, *outputs), expected)
                self.assertFalse(
                    any(path.exists() for path in (*outputs, *self.projection_paths(root)))
                )

    def test_facts_validator_rejects_nonfinite_and_extra_values(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            facts_path = Path(directory) / "model.json"
            invalid_facts = (
                '{"volume_cubic_millimeters":NaN,"size_millimeters":{"x":1,"y":2,"z":3}}',
                '{"volume_cubic_millimeters":1,"size_millimeters":{"x":1,"y":2,"z":-1}}',
                '{"volume_cubic_millimeters":1,"size_millimeters":{"x":1,"y":2,"z":3},"extra":0}',
            )
            for content in invalid_facts:
                facts_path.write_text(content, encoding="utf-8")
                self.assertFalse(_validate_facts(facts_path))

    def test_glb_validator_accepts_json_only_and_json_with_bin(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "model.glb"
            for content in (
                make_glb(b'{"asset":{"version":"2.0"}} '),
                make_glb(b'{"asset":{"version":"2.0"}} ', b"bin\x00"),
            ):
                output.write_bytes(content)
                self.assertTrue(_validate_glb(output))

    def test_glb_validator_rejects_malformed_containers(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "model.glb"
            valid_json = b'{"asset":{"version":"2.0"}} '
            cases = {
                "short header": b"not glb",
                "wrong declared size": struct.pack(
                    "<4sIIII", b"glTF", 2, 999, 0, 0x4E4F534A
                ),
                "zero-length JSON": make_glb(b""),
                "invalid JSON": make_glb(b"not-json"),
                "non-object JSON": make_glb(b"[1] "),
                "nonstandard JSON constant": make_glb(b"NaN "),
                "NUL-padded JSON": make_glb(b"{} \x00"),
                "truncated JSON": set_declared_glb_size(make_glb(valid_json)[:-1]),
                "wrong first chunk type": make_glb(valid_json).replace(
                    struct.pack("<I", 0x4E4F534A),
                    struct.pack("<I", 0x004E4942),
                    1,
                ),
                "truncated BIN": set_declared_glb_size(
                    make_glb(valid_json, b"mesh")[:-1]
                ),
                "wrong second chunk type": make_glb(valid_json, b"mesh").replace(
                    struct.pack("<I", 0x004E4942),
                    struct.pack("<I", 0x4E4F534A),
                    1,
                ),
                "trailing bytes": set_declared_glb_size(
                    make_glb(valid_json) + b"bad!"
                ),
            }
            for name, content in cases.items():
                with self.subTest(name=name):
                    output.write_bytes(content)
                    self.assertFalse(_validate_glb(output))


if __name__ == "__main__":
    unittest.main()
